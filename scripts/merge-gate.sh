#!/usr/bin/env bash
# Decide whether a pull request may be merged, and say why not when it may not.
#
# Usage: scripts/merge-gate.sh <pr-number> [--repo OWNER/NAME]
# Exit:  0 may merge, 1 must not, 2 could not determine.
#
# Every positive result is bound to one server-observed head, base tip, complete
# check set, provider review commit, review-thread set, changed-file set, and
# readable compatibility surface. Unknown or incomplete evidence is never
# converted into an empty successful set.

set -uo pipefail

PR=""
REPO=""
while [ $# -gt 0 ]; do
  case "$1" in
    --repo)
      REPO="${2:-}"
      [ -n "$REPO" ] || { echo "--repo needs OWNER/NAME" >&2; exit 2; }
      shift 2
      ;;
    --repo=*)
      REPO="${1#--repo=}"
      [ -n "$REPO" ] || { echo "--repo= needs OWNER/NAME" >&2; exit 2; }
      shift
      ;;
    -h|--help)
      sed -n '2,13p' "$0"
      exit 0
      ;;
    -*)
      echo "unknown option: $1" >&2
      exit 2
      ;;
    *)
      if [ -z "$PR" ]; then
        PR="$1"
      else
        echo "unexpected argument: $1" >&2
        exit 2
      fi
      shift
      ;;
  esac
done
[ -n "$PR" ] || { echo "usage: $0 <pr-number> [--repo OWNER/NAME]" >&2; exit 2; }

# Resolve the repository once. An explicit target must never fall back to the
# current checkout if one later API call fails.
if [ -z "$REPO" ]; then
  if ! REPO=$(gh repo view --json nameWithOwner -q .nameWithOwner 2>/dev/null); then
    echo "could not determine repository; pass --repo OWNER/NAME" >&2
    exit 2
  fi
fi
OWNER="${REPO%%/*}"
NAME="${REPO##*/}"
if [ -z "$OWNER" ] || [ -z "$NAME" ] || [ "$OWNER" = "$REPO" ] || [[ "$REPO" == */*/* ]]; then
  echo "--repo must be OWNER/NAME, got '$REPO'" >&2
  exit 2
fi

fail=0
uncertain=0
tmpdir=$(mktemp -d)
errfile="$tmpdir/error"
trap 'rm -rf "$tmpdir"' EXIT
say() { printf '  %-13s %s\n' "$1" "$2"; }
bad() { say "BLOCK" "$1"; fail=1; }
unknown() { say "INDETERMINATE" "$1"; uncertain=1; }
die() { echo "$1" >&2; exit 2; }

# A malformed response is different from a valid empty result. Validate the
# outer shape before extracting fields so jq errors cannot become empty values.
: >"$errfile"
if ! meta=$(gh pr view "$PR" --repo "$REPO" \
        --json headRefOid,baseRefOid,baseRefName,mergeable,mergeStateStatus,isDraft,state,body,changedFiles 2>"$errfile"); then
  die "could not read PR #$PR in $REPO"
fi
if ! jq -e '
  type == "object" and
  (.headRefOid | type == "string" and test("^[0-9a-fA-F]{40}$")) and
  (.baseRefOid | type == "string" and test("^[0-9a-fA-F]{40}$")) and
  (.baseRefName | type == "string" and length > 0) and
  (.mergeable | type == "string") and
  (.mergeStateStatus | type == "string") and
  (.isDraft | type == "boolean") and
  (.state | type == "string") and
  (.changedFiles | type == "number" and floor == . and . >= 0)
' <<<"$meta" >/dev/null 2>&1; then
  die "PR metadata was not a valid complete JSON object"
fi
head=$(jq -r '.headRefOid' <<<"$meta")
base_ref_oid=$(jq -r '.baseRefOid' <<<"$meta")
base=$(jq -r '.baseRefName' <<<"$meta")
mergeable=$(jq -r '.mergeable' <<<"$meta")
mstate=$(jq -r '.mergeStateStatus' <<<"$meta")
draft=$(jq -r '.isDraft' <<<"$meta")
pstate=$(jq -r '.state' <<<"$meta")
changed_files_expected=$(jq -r '.changedFiles' <<<"$meta")
short=${head:0:7}
prbody=$(jq -r '.body // ""' <<<"$meta")

echo "PR #$PR ($REPO)  head=$short  base=$base  $mergeable/$mstate"
[ "$pstate" = "OPEN" ] || bad "PR is $pstate, not OPEN"
[ "$draft" = "false" ] || bad "draft"

case "$mergeable" in
  MERGEABLE) say "ok" "no conflicts" ;;
  CONFLICTING) bad "conflicts with $base — rebase first" ;;
  *) unknown "mergeability is '$mergeable'; re-run when GitHub has determined it" ;;
esac
case "$mstate" in
  BEHIND) bad "head is BEHIND $base — its checks and review describe a stale tree; rebase" ;;
  DIRTY) bad "merge state DIRTY — conflicts" ;;
  UNKNOWN) unknown "merge state UNKNOWN; re-run in a moment" ;;
  BLOCKED) bad "merge state BLOCKED — GitHub is refusing this merge" ;;
  CLEAN|HAS_HOOKS) say "ok" "merge state $mstate is not stale" ;;
  UNSTABLE) bad "merge state UNSTABLE — GitHub has not established a mergeable result" ;;
  *) unknown "unrecognised merge state '$mstate'" ;;
esac

if [ "$base" = "master" ]; then
  say "ok" "based on master (CI actually runs)"
else
  bad "based on '$base', not master — ci.yml does not fire, so checks here prove nothing"
fi

# Capture the base tip independently. A PR can retain the same base name while
# the branch advances during this run.
: >"$errfile"
base_tip_status=0
base_tip=$(gh api "repos/$REPO/branches/$base" --jq '.commit.sha' 2>"$errfile") || base_tip_status=$?
if [ "$base_tip_status" -ne 0 ] || ! [[ "$base_tip" =~ ^[0-9a-fA-F]{40}$ ]]; then
  unknown "could not read the full current tip of base '$base'"
  base_tip=""
else
  say "ok" "captured base tip ${base_tip:0:7}"
fi
if [ -n "$base_tip" ] && [ "$base_ref_oid" != "$base_tip" ]; then
  unknown "PR base OID $base_ref_oid differs from the current '$base' tip $base_tip"
fi

# Bind the reviewed head to the actual base lineage returned by GitHub. The
# compare API is queried with the captured OIDs and must say that base is an
# ancestor of head; mergeability alone does not establish that relationship.
if [ -n "$base_tip" ] && [ "$base_ref_oid" = "$base_tip" ]; then
  : >"$errfile"
  compare_status=0
  comparison=$(gh api "repos/$REPO/compare/${base_tip}...${head}" 2>"$errfile") || compare_status=$?
  if [ "$compare_status" -ne 0 ] || ! jq -e --arg base "$base_tip" '
    type == "object" and
    (.status | type == "string" and (. == "ahead" or . == "identical")) and
    (.behind_by | type == "number" and floor == . and . == 0) and
    (.merge_base_commit | type == "object") and
    (.merge_base_commit.sha | type == "string" and test("^[0-9a-fA-F]{40}$") and . == $base)
  ' <<<"$comparison" >/dev/null 2>&1; then
    unknown "base/head compare did not prove that the captured base tip is an ancestor"
  else
    say "ok" "compare API binds base tip ${base_tip:0:7} as head's merge base"
  fi
fi

# Branch protection is the source of required check contexts. A pass list with
# an omitted required context is not a complete check result.
: >"$errfile"
protection_status=0
protection=$(gh api "repos/$REPO/branches/$base/protection/required_status_checks" 2>"$errfile") || protection_status=$?
if [ "$protection_status" -ne 0 ]; then
  unknown "could not read required status-check contexts for $base"
  required_contexts=""
elif ! jq -e '
  type == "object" and
  ((.contexts // []) | type == "array" and all(.[]; type == "string" and length > 0)) and
  ((.checks // []) | type == "array" and all(.[]; type == "object" and (.context | type == "string" and length > 0)))
' <<<"$protection" >/dev/null 2>&1; then
  unknown "required status-check response was malformed"
  required_contexts=""
else
  required_contexts=$(jq -r '((.contexts // []) + ([.checks // [] | .[]? | .context] | map(select(type == "string" and length > 0))) | unique)[]' <<<"$protection")
  if [ -z "$required_contexts" ]; then
    unknown "branch protection returned no required status-check contexts"
  else
    say "ok" "loaded $(wc -l <<<"$required_contexts" | tr -d ' ') required check context(s)"
  fi
fi

# gh uses pass/fail/pending/skipping/cancel buckets. Preserve command status,
# then parse JSON and require each protected context individually.
: >"$errfile"
check_status=0
buckets=$(gh pr checks "$PR" --repo "$REPO" --json bucket,name 2>"$errfile") || check_status=$?
if [ -z "$buckets" ]; then
  unknown "checks query returned no JSON"
elif ! jq -e 'type == "array" and all(.[]; type == "object" and (.name | type == "string" and length > 0) and (.bucket | type == "string"))' <<<"$buckets" >/dev/null 2>&1; then
  unknown "checks query returned malformed JSON"
else
  # The first validation accepts only the documented buckets; keep this second
  # check explicit because jq's precedence is easy to misread in a gate.
  if ! jq -e 'all(.[]; (.bucket == "pass" or .bucket == "fail" or .bucket == "pending" or .bucket == "skipping" or .bucket == "cancel"))' <<<"$buckets" >/dev/null 2>&1; then
    unknown "checks query contained an unknown bucket"
  elif [ "$check_status" -ne 0 ] && [ "$(jq '[.[] | select(.bucket == "fail" or .bucket == "cancel" or .bucket == "pending")] | length' <<<"$buckets")" -eq 0 ]; then
    unknown "checks command failed even though no failing or pending result was returned"
  elif [ "$(jq 'length' <<<"$buckets")" -eq 0 ]; then
    bad "no checks reported for this PR"
  else
    check_bad=0
    while IFS= read -r context; do
      [ -n "$context" ] || continue
      context_state=$(jq -r --arg context "$context" '
        map(select(.name == $context)) |
        if length == 0 then "missing"
        elif all(.[]; .bucket == "pass") then "pass"
        else map(.bucket) | unique | join(",")
        end
      ' <<<"$buckets")
      case "$context_state" in
        pass) say "ok" "required check '$context' passed" ;;
        missing) bad "required check '$context' was not reported"; check_bad=1 ;;
        *) bad "required check '$context' is not passing ($context_state)"; check_bad=1 ;;
      esac
    done <<<"$required_contexts"
    all_bad=$(jq '[.[] | select(.bucket == "fail" or .bucket == "cancel" or .bucket == "pending")] | length' <<<"$buckets")
    skipped=$(jq '[.[] | select(.bucket == "skipping")] | length' <<<"$buckets")
    [ "$all_bad" -eq 0 ] || bad "$all_bad reported check(s) are failing, cancelled, or pending"
    [ "$skipped" -eq 0 ] || say "note" "$skipped optional check(s) are skipped; required skipped contexts remain blocking"
    [ "$check_bad" -eq 0 ] && [ "$all_bad" -eq 0 ] && say "ok" "all reported checks concluded successfully"
  fi
fi

# The checks rollup is head-bound by its PR endpoint, but it does not expose a
# check-run SHA in `gh pr checks`. Verify the complete provider check-run pages
# and commit-status response independently so a malformed or mixed response
# cannot become positive evidence. The required-context decision above remains
# authoritative for branch protection, including status-only contexts.
: >"$errfile"
check_runs_status=0
check_runs=$(gh api --paginate --slurp "repos/$REPO/commits/$head/check-runs?per_page=100" 2>"$errfile") || check_runs_status=$?
if [ "$check_runs_status" -ne 0 ] || ! jq -e --arg head "$head" '
  type == "array" and length > 0 and
  all(.[]; type == "object" and
    (.total_count | type == "number" and floor == . and . >= 0) and
    (.check_runs | type == "array" and all(.[];
      type == "object" and
      (.name | type == "string" and length > 0) and
      (.head_sha | type == "string" and test("^[0-9a-fA-F]{40}$") and . == $head)
    ))) and
  ((map(.total_count) | unique | length) == 1) and
  ((map(.check_runs | length) | add) == .[0].total_count)
' <<<"$check_runs" >/dev/null 2>&1; then
  unknown "could not validate complete head-bound check-run evidence"
else
  say "ok" "check-run pages are complete and bound to head $short"
fi
: >"$errfile"
statuses_status=0
statuses=$(gh api "repos/$REPO/commits/$head/status" 2>"$errfile") || statuses_status=$?
if [ "$statuses_status" -ne 0 ] || ! jq -e --arg head "$head" '
  type == "object" and
  (.total_count | type == "number" and floor == . and . >= 0) and
  (.statuses | type == "array" and all(.[];
    type == "object" and
    (.context | type == "string" and length > 0) and
    (.state | type == "string" and length > 0) and
    ((.sha // $head) | type == "string" and test("^[0-9a-fA-F]{40}$") and . == $head)
  ))
' <<<"$statuses" >/dev/null 2>&1; then
  unknown "could not validate head-bound commit-status evidence"
else
  say "ok" "commit-status response is bound to head $short"
fi

# Provider review objects carry an immutable full commit_id even when the
# human-readable summary is abbreviated. No author-controlled commit timestamp
# is used. A clean summary without a full provider OID is indeterminate and
# points the operator to independent exact-head acceptance.
: >"$errfile"
review_status=0
reviews=$(gh api --paginate --slurp "repos/$REPO/pulls/$PR/reviews" 2>"$errfile") || review_status=$?
if [ "$review_status" -ne 0 ]; then
  unknown "could not read provider review records"
elif ! jq -e 'type == "array" and (all(.[]; type == "array") or all(.[]; type == "object"))' <<<"$reviews" >/dev/null 2>&1; then
  unknown "provider review response was malformed"
else
  provider_review=$(jq -r --arg head "$head" '
    (if all(.[]; type == "array") then flatten else . end) |
    map(select(.user.login == "chatgpt-codex-connector[bot]" and .user.type == "Bot" and
               .state == "COMMENTED" and .commit_id == $head)) |
    if length > 0 then "matched" else "" end
  ' <<<"$reviews")
  if [ "$provider_review" = "matched" ]; then
    say "ok" "provider review records the full current head $short"
  else
    # Read the summary only to distinguish absent evidence from a provider
    # summary that exposes an abbreviated current prefix.
    : >"$errfile"
    comment_status=0
    comments=$(gh api --paginate --slurp "repos/$REPO/issues/$PR/comments" 2>"$errfile") || comment_status=$?
    if [ "$comment_status" -ne 0 ]; then
      unknown "could not read provider review summaries"
    elif ! jq -e 'type == "array" and (all(.[]; type == "array") or all(.[]; type == "object"))' <<<"$comments" >/dev/null 2>&1; then
      unknown "provider review-summary response was malformed"
    else
      summaries=$(jq -r '
        (if all(.[]; type == "array") then flatten else . end)[] |
        select(.user.login == "chatgpt-codex-connector[bot]" and .user.type == "Bot") |
        select((.body // "") | contains("codex-pull-request-review-summary")) |
        .body
      ' <<<"$comments")
      if [ -z "$summaries" ]; then
        bad "no provider review records or summaries"
      else
        current_prefix=0
        if grep -Fq "\`$short\`" <<<"$summaries"; then current_prefix=1; fi
        if [ "$current_prefix" -eq 1 ]; then
          unknown "provider summary exposes only an abbreviated head; obtain full-SHA provider evidence or independently review this exact head"
        else
          bad "provider review evidence names a different head"
        fi
      fi
    fi
  fi
fi

# Paginate review threads and count unresolved nodes over every page.
cursor=""
open_threads=0
total_threads=-1
fetched_threads=0
thread_ok=1
while :; do
  : >"$errfile"
  page_status=0
  if [ -z "$cursor" ]; then
    page=$(gh api graphql -f owner="$OWNER" -f name="$NAME" -F pr="$PR" -f cursor="" -f query='
      query($owner:String!,$name:String!,$pr:Int!,$cursor:String){
        repository(owner:$owner,name:$name){
          pullRequest(number:$pr){ reviewThreads(first:100,after:$cursor){
            totalCount pageInfo{hasNextPage endCursor} nodes{isResolved}
          }}
        }
      }' 2>"$errfile") || page_status=$?
  else
    page=$(gh api graphql -f owner="$OWNER" -f name="$NAME" -F pr="$PR" -f cursor="$cursor" -f query='
      query($owner:String!,$name:String!,$pr:Int!,$cursor:String){
        repository(owner:$owner,name:$name){
          pullRequest(number:$pr){ reviewThreads(first:100,after:$cursor){
            totalCount pageInfo{hasNextPage endCursor} nodes{isResolved}
          }}
        }
      }' 2>"$errfile") || page_status=$?
  fi
  if [ "$page_status" -ne 0 ] || ! jq -e '.data.repository.pullRequest.reviewThreads | type == "object" and (.totalCount | type == "number" and floor == . and . >= 0) and (.pageInfo.hasNextPage | type == "boolean") and (.nodes | type == "array" and all(.[]; .isResolved | type == "boolean"))' <<<"$page" >/dev/null 2>&1; then
    unknown "could not read review threads for $REPO#$PR"
    thread_ok=0
    break
  fi
  page_total=$(jq -r '.data.repository.pullRequest.reviewThreads.totalCount' <<<"$page")
  if [ "$total_threads" -eq -1 ]; then
    total_threads="$page_total"
  elif [ "$page_total" -ne "$total_threads" ]; then
    unknown "review-thread totalCount changed during pagination"
    thread_ok=0
    break
  fi
  page_nodes=$(jq '.data.repository.pullRequest.reviewThreads.nodes | length' <<<"$page")
  fetched_threads=$((fetched_threads + page_nodes))
  if [ "$fetched_threads" -gt "$total_threads" ]; then
    unknown "review-thread pagination exceeded totalCount"
    thread_ok=0
    break
  fi
  page_open=$(jq '[.data.repository.pullRequest.reviewThreads.nodes[] | select(.isResolved == false)] | length' <<<"$page")
  open_threads=$((open_threads + page_open))
  has_next=$(jq -r '.data.repository.pullRequest.reviewThreads.pageInfo.hasNextPage' <<<"$page")
  [ "$has_next" = "true" ] || break
  if [ "$page_nodes" -eq 0 ]; then
    unknown "review-thread pagination returned no nodes while claiming another page"
    thread_ok=0
    break
  fi
  next_cursor=$(jq -r '.data.repository.pullRequest.reviewThreads.pageInfo.endCursor // empty' <<<"$page")
  if [ -z "$next_cursor" ] || [ "$next_cursor" = "$cursor" ]; then
    unknown "review-thread pagination returned no advancing cursor"
    thread_ok=0
    break
  fi
  cursor="$next_cursor"
done
if [ "$thread_ok" -eq 1 ]; then
  if [ "$fetched_threads" -ne "$total_threads" ]; then
    unknown "review-thread pagination returned $fetched_threads of $total_threads nodes"
  elif [ "$open_threads" -eq 0 ]; then
    say "ok" "0 of $total_threads review threads unresolved"
  else
    bad "$open_threads of $total_threads review threads unresolved"
  fi
fi

# Require a completed checklist item and a same-repository line permalink.
# The repository template puts the permalink on the item's indented
# continuation, so accept it there as well as in an inline Markdown link.
# A filename in prose, a foreign link, or an unrelated checked item is not
# completion evidence.
checklist_link_ok() {
  local body="$1" line awaiting_permalink=0
  local checked='^[[:space:]]*-[[:space:]]*\[[xX]\][[:space:]]+'
  local permalink="https://github\\.com/${OWNER}/${NAME}/blob/[^[:space:])]+/review-checklist\\.md#L[0-9]+"
  while IFS= read -r line; do
    if printf '%s\n' "$line" | grep -Eq "$checked"; then
      if printf '%s\n' "$line" | grep -Eiq "$permalink"; then
        return 0
      fi
      if printf '%s\n' "$line" | grep -Eiq 'review-checklist\.md'; then
        awaiting_permalink=1
      else
        awaiting_permalink=0
      fi
    elif [ "$awaiting_permalink" -eq 1 ] && printf '%s\n' "$line" | grep -Eq '^[[:space:]]+'; then
      if printf '%s\n' "$line" | grep -Eiq "$permalink"; then
        return 0
      fi
    else
      awaiting_permalink=0
    fi
  done <<<"$body"
  return 1
}
if ! checklist_link_ok "$prbody"; then
  bad "description lacks a completed same-repository line-specific review-checklist link"
else
  say "ok" "description links a completed review-checklist item"
fi

# Paginate changed files through the REST endpoint; gh pr view hard-codes a
# first:100 GraphQL fragment in some versions. Retain the line counts as well:
# the privacy scan can only be complete when the textual diff describes every
# non-removed destination with the byte count GitHub reported.
: >"$errfile"
files_status=0
files=$(gh api --paginate --slurp "repos/$REPO/pulls/$PR/files?per_page=100" 2>"$errfile") || files_status=$?
changed_records="$tmpdir/changed-files.tsv"
if [ "$files_status" -ne 0 ] || ! jq -e '
  type == "array" and
  (all(.[]; type == "array" and all(.[];
      type == "object" and
      ((.filename | type) == "string") and (.filename | length > 0) and (.filename | test("[\\t\\r\\n]") | not) and
      ((.status | type) == "string") and (.status | length > 0) and
      ((.additions | type) == "number") and (.additions | floor == . and . >= 0) and
      ((.deletions | type) == "number") and (.deletions | floor == . and . >= 0))) or
   all(.[]; type == "object" and
      ((.filename | type) == "string") and (.filename | length > 0) and (.filename | test("[\\t\\r\\n]") | not) and
      ((.status | type) == "string") and (.status | length > 0) and
      ((.additions | type) == "number") and (.additions | floor == . and . >= 0) and
      ((.deletions | type) == "number") and (.deletions | floor == . and . >= 0)))
' <<<"$files" >/dev/null 2>&1; then
  unknown "could not read the complete changed-file set"
  changed=""
else
  jq -r '(if all(.[]; type == "array") then flatten else . end)[] | [.filename, .status, .additions, .deletions] | @tsv' <<<"$files" >"$changed_records"
  changed=$(cut -f1 "$changed_records")
  changed_count=$(jq '(if all(.[]; type == "array") then flatten else . end) | length' <<<"$files")
  unique_changed_count=$(jq '(if all(.[]; type == "array") then flatten else . end) | map(.filename) | unique | length' <<<"$files")
  if [ "$changed_count" -eq 0 ]; then
    unknown "changed-file response contained no filenames"
  elif [ "$changed_files_expected" -gt 3000 ]; then
    unknown "PR reports $changed_files_expected changed files beyond the REST files API cap"
  elif [ "$changed_count" -ne "$changed_files_expected" ] || [ "$unique_changed_count" -ne "$changed_count" ]; then
    unknown "changed-file response has $changed_count unique records; PR metadata reports $changed_files_expected"
  fi
fi

# Read and validate the v1 surface as a required object. Any transport,
# decoding, JSON, or schema failure is indeterminate; an unrelated nested
# `path` must not turn an incomplete manifest into an empty pin set. Both the
# reviewed head and captured base tip are checked: a head surface that silently
# drops a previously pinned path is a human hold, and changed paths are tested
# against the union so an unpinned head cannot hide a reseal obligation.
SURFACE="docs/tally/compatibility/compatibility-surface.json"
read_surface_paths() {
  local ref="$1"
  local response content decoded decode_status
  surface_paths_result=""
  : >"$errfile"
  response=$(gh api "repos/$REPO/contents/$SURFACE?ref=$ref" 2>"$errfile") || return 1
  content=$(jq -er 'select(.encoding == "base64") | .content | strings' <<<"$response") || return 1
  decoded=""
  decode_status=0
  decoded=$(printf '%s' "${content//$'\n'/}" | base64 --decode 2>"$errfile") || decode_status=$?
  if [ "$decode_status" -ne 0 ]; then
    decode_status=0
    decoded=$(printf '%s' "${content//$'\n'/}" | base64 -D 2>"$errfile") || decode_status=$?
  fi
  if [ "$decode_status" -ne 0 ] || ! jq -e '
    type == "object" and
    .schema_version == 1 and
    ((.manifest_sha256 | type) == "string") and (.manifest_sha256 | test("^[0-9a-f]{64}$")) and
    (.files | type == "array" and length > 0 and
      all(.[]; type == "object" and
        ((.path | type) == "string") and (.path | length > 0) and
        ((.sha256 | type) == "string") and (.sha256 | test("^[0-9a-f]{64}$")))) and
    (([.files[].path] | length) == ([.files[].path] | unique | length))
  ' <<<"$decoded" >/dev/null 2>&1; then
    return 1
  fi
  surface_paths_result=$(jq -r '.files[].path' <<<"$decoded")
}

head_surface_status=0
read_surface_paths "$head" || head_surface_status=$?
if [ "$head_surface_status" -ne 0 ]; then
  unknown "could not read and validate compatibility surface at $short"
  pinned=""
else
  pinned="$surface_paths_result"
  say "ok" "validated v1 compatibility surface at head $short"
fi

base_surface_status=0
if [ -n "$base_tip" ]; then
  read_surface_paths "$base_tip" || base_surface_status=$?
fi
if [ -n "$base_tip" ] && [ "$base_surface_status" -ne 0 ]; then
  unknown "could not read and validate compatibility surface at base ${base_tip:0:7}"
  base_pinned=""
elif [ -n "$base_tip" ]; then
  base_pinned="$surface_paths_result"
  say "ok" "validated v1 compatibility surface at base ${base_tip:0:7}"
else
  base_pinned=""
fi

if [ -n "$changed" ] && [ -n "$pinned" ] && [ -n "$base_pinned" ]; then
  removed_pins=$(comm -23 <(sort -u <<<"$base_pinned") <(sort -u <<<"$pinned"))
  if [ -n "$removed_pins" ]; then
    removed_count=$(wc -l <<<"$removed_pins" | tr -d ' ')
    unknown "$removed_count base-pinned path(s) are absent from the head surface; human review is required"
  fi
  union_pinned=$(printf '%s\n%s\n' "$base_pinned" "$pinned" | sort -u)
  touched=$(comm -12 <(sort -u <<<"$union_pinned") <(sort -u <<<"$changed") | awk -v surface="$SURFACE" '$0 != surface')
  if [ -z "$touched" ]; then
    say "ok" "changed files contain no pinned path requiring a reseal"
  elif grep -Fxq "$SURFACE" <<<"$changed"; then
    say "ok" "changed pinned paths include the compatibility surface"
  else
    bad "changed pinned paths omit the compatibility surface reseal"
  fi
fi

# Scan destination paths and added payload lines. Removed/context lines never
# enter the privacy scan. Binary additions are an explicit human-inspection
# hold because their bytes are absent from a textual patch.
: >"$errfile"
diff_status=0
diff=$(gh pr diff "$PR" --repo "$REPO" 2>"$errfile") || diff_status=$?
if [ "$diff_status" -ne 0 ] || [ -z "$diff" ]; then
  unknown "could not read diff for the privacy scan"
else
  # Keep one record per `diff --git` section. A header alone proves only that
  # GitHub named a file; the line counts below prove it supplied the complete
  # textual payload for that destination.
  diff_stats="$tmpdir/diff-stats.tsv"
  awk '
    function emit() {
      if (!in_file) return
      destination = textual_destination != "" ? textual_destination : header_destination
      if (destination != "") {
        printf "%s\t%d\t%d\t%d\t%d\n", destination, added, deleted, textual, binary
      }
    }
    /^diff --git a\// {
      emit()
      in_file = 1
      header_destination = $0
      sub(/^diff --git a\/.* b\//, "", header_destination)
      textual_destination = ""
      added = deleted = textual = binary = 0
      next
    }
    /^\+\+\+ b\// {
      textual_destination = $0
      sub(/^\+\+\+ b\//, "", textual_destination)
      textual = 1
      next
    }
    /^(Binary files .* differ|GIT binary patch)$/ { binary = 1; next }
    /^\+/ && $0 !~ /^\+\+\+ / { added++; next }
    /^-/ && $0 !~ /^--- / { deleted++; next }
    END { emit() }
  ' <<<"$diff" >"$diff_stats"

  binary_count=$(awk '/^(Binary files .* differ|GIT binary patch)/ && $0 !~ /and \/dev\/null differ/ {n++} END {print n+0}' <<<"$diff")
  [ "$binary_count" -eq 0 ] || bad "$binary_count binary addition/change(s) require human privacy inspection"
  path_text=""
  if [ -s "$changed_records" ]; then
    while IFS=$'\t' read -r filename status rest_added rest_deleted; do
      [ "$status" = "removed" ] && continue
      match_count=$(awk -F '\t' -v filename="$filename" '$1 == filename { count++ } END { print count+0 }' "$diff_stats")
      if [ "$match_count" -ne 1 ]; then
        unknown "privacy diff omits or duplicates non-removed REST destination '$filename'"
        continue
      fi
      diff_record=$(awk -F '\t' -v filename="$filename" '$1 == filename { print; exit }' "$diff_stats")
      IFS=$'\t' read -r _ diff_added diff_deleted textual binary <<<"$diff_record"
      if [ "$binary" -eq 1 ]; then
        # The binary hold above requires human inspection. Its bytes cannot be
        # reconciled through textual hunks, but its destination was covered.
        continue
      fi
      if [ "$textual" -ne 1 ]; then
        unknown "privacy diff lacks a textual destination for '$filename'"
      elif [ "$diff_added" -ne "$rest_added" ] || [ "$diff_deleted" -ne "$rest_deleted" ]; then
        unknown "privacy diff line totals for '$filename' differ from REST metadata"
      fi
    done <"$changed_records"
    # Destination paths are scan input from the complete REST set, not only
    # from whatever textual patch GitHub happened to render. Removed paths
    # carry no newly added material and are deliberately excluded.
    path_text=$(awk -F '\t' '$2 != "removed" { print $1 }' "$changed_records")
  fi
  added=$(awk '/^\+/ && $0 !~ /^\+\+\+ (b\/|\/dev\/null)/ { print substr($0, 2) }' <<<"$diff")
  scan_input="$path_text
$added"
  redacted=$(sed -E 's/[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}/<uuid>/g; s/[0-9a-fA-F]{32,}/<digest>/g' <<<"$scan_input")
  exempt=$(grep -Ec '[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}|[0-9a-fA-F]{32,}' <<<"$scan_input")
  [ "$exempt" -eq 0 ] || say "note" "$exempt added/path line(s) carried generated UUID/digest shapes; inspect those lines"
  placeholder='^(X+|Z+|A+)[0-9]+(X|Z|A)?$|^[0-9]{2}(X+|Z+|A+)[0-9]+[0-9A-Z]*$|^(0+|1+|2+|3+|4+|5+|6+|7+|8+|9+)$|^(0?1234567890|1234567890[0-9]*)$|^0{6,}[0-9]{1,5}$'
  if printf '%s\n' 'XXXXX1234X' | grep -qE "$placeholder"; then :; else
    probe_status=$?
    if [ "$probe_status" -eq 1 ]; then
      bad "privacy placeholder control did not match its synthetic probe"
    else
      unknown "privacy placeholder expression failed"
    fi
  fi
  count_nonplaceholder() {
    local pattern="$1" input="$2" matches="" status=0 item count=0
    if matches=$(grep -Eio "$pattern" <<<"$input"); then
      :
    else
      status=$?
      if [ "$status" -eq 1 ]; then matches=""; else return 2; fi
    fi
    while IFS= read -r item; do
      [ -n "$item" ] || continue
      item=$(tr '[:lower:]' '[:upper:]' <<<"$item")
      if printf '%s\n' "$item" | grep -qE "$placeholder"; then
        :
      else
        status=$?
        [ "$status" -eq 1 ] && count=$((count + 1)) || return 2
      fi
    done <<<"$matches"
    printf '%s\n' "$count"
  }
  # Phone numbers are often entered with a country prefix and visual
  # separators. Keep the original text for the general scans, and add one
  # separator-free view for the phone shape so ordinary formatting cannot
  # split a customer number into harmless short fragments.
  normalized_phone=$(tr -d $' ()-.\t' <<<"$redacted")
  scan_shapes="$redacted
$normalized_phone"
  hits_status=0
  hits=$(count_nonplaceholder '[0-9]{2}[A-Z]{5}[0-9]{4}[A-Z][0-9A-Z]{3}|[A-Z]{5}[0-9]{4}[A-Z]|[6-9][0-9]{9}' "$scan_shapes") || hits_status=$?
  runs_status=0
  runs=$(count_nonplaceholder '[0-9]{11,18}' "$scan_shapes") || runs_status=$?
  if [ "$hits_status" -ne 0 ] || [ "$runs_status" -ne 0 ]; then
    unknown "privacy scan expression failed"
  elif [ "$hits" -eq 0 ] && [ "$runs" -eq 0 ]; then
    say "ok" "added destination paths and payload lines carry no identifier shapes"
  else
    bad "privacy scan found $hits identifier shape(s) and $runs unexplained long digit run(s)"
  fi
fi

# Re-read all moving identities immediately before emitting a merge command.
: >"$errfile"
final_meta_status=0
final_meta=$(gh pr view "$PR" --repo "$REPO" \
  --json headRefOid,baseRefOid,baseRefName,mergeable,mergeStateStatus,isDraft,state,body,changedFiles 2>"$errfile") || final_meta_status=$?
if [ "$final_meta_status" -ne 0 ] || ! jq -e 'type == "object" and (.headRefOid | type == "string" and test("^[0-9a-fA-F]{40}$")) and (.baseRefOid | type == "string" and test("^[0-9a-fA-F]{40}$")) and (.baseRefName | type == "string") and (.mergeable | type == "string") and (.mergeStateStatus | type == "string") and (.isDraft | type == "boolean") and (.state | type == "string") and (.changedFiles | type == "number" and floor == . and . >= 0)' <<<"$final_meta" >/dev/null 2>&1; then
  unknown "could not revalidate PR head and base before merge"
else
  final_head=$(jq -r '.headRefOid' <<<"$final_meta")
  final_base_ref_oid=$(jq -r '.baseRefOid' <<<"$final_meta")
  final_base=$(jq -r '.baseRefName' <<<"$final_meta")
  final_mergeable=$(jq -r '.mergeable' <<<"$final_meta")
  final_state=$(jq -r '.mergeStateStatus' <<<"$final_meta")
  final_draft=$(jq -r '.isDraft' <<<"$final_meta")
  final_pstate=$(jq -r '.state' <<<"$final_meta")
  final_changed_files=$(jq -r '.changedFiles' <<<"$final_meta")
  [ "$final_head" = "$head" ] || bad "PR head moved during preflight"
  [ "$final_base_ref_oid" = "$base_ref_oid" ] || bad "PR base OID moved during preflight"
  [ "$final_base" = "$base" ] || bad "PR base moved during preflight"
  [ "$final_mergeable" = "MERGEABLE" ] || bad "PR mergeability changed to $final_mergeable during preflight"
  [ "$final_draft" = "false" ] || bad "PR became draft during preflight"
  [ "$final_pstate" = "OPEN" ] || bad "PR state changed to $final_pstate during preflight"
  [ "$final_changed_files" = "$changed_files_expected" ] || bad "PR changed-file count moved during preflight"
  case "$final_state" in
    CLEAN|HAS_HOOKS) : ;;
    BEHIND|DIRTY|UNKNOWN|BLOCKED|UNSTABLE) bad "PR merge state changed to $final_state during preflight" ;;
    *) unknown "PR merge state changed to unrecognised value '$final_state' during preflight" ;;
  esac
  final_body=$(jq -r '.body // ""' <<<"$final_meta")
  if ! checklist_link_ok "$final_body"; then
    bad "PR description changed and no longer carries a completed same-repository line-specific checklist link"
  fi
fi
if [ -n "$base_tip" ]; then
  : >"$errfile"
  final_base_tip_status=0
  final_base_tip=$(gh api "repos/$REPO/branches/$base" --jq '.commit.sha' 2>"$errfile") || final_base_tip_status=$?
  if [ "$final_base_tip_status" -ne 0 ] || ! [[ "$final_base_tip" =~ ^[0-9a-fA-F]{40}$ ]]; then
    unknown "could not revalidate base tip before merge"
  elif [ "$final_base_tip" != "$base_tip" ]; then
    bad "base tip moved during preflight"
  fi
fi

echo
if [ "$uncertain" -ne 0 ]; then
  echo "INDETERMINATE — do not merge until the missing evidence is obtained"
  exit 2
fi
if [ "$fail" -ne 0 ]; then
  echo "MUST NOT MERGE"
  exit 1
fi

echo "MAY MERGE — bind the merge to the reviewed head and validated base:"
printf '  [ "$(gh pr view %s --repo %s --json baseRefName -q .baseRefName)" = "%s" ] \\\n    && gh pr merge %s --repo %s --squash --match-head-commit %s\n' \
  "$PR" "$REPO" "$base" "$PR" "$REPO" "$head"
exit 0
