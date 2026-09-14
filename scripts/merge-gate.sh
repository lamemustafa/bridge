#!/usr/bin/env bash
# Decide whether a pull request may be merged, and say why not when it may not.
#
# Usage: scripts/merge-gate.sh <pr-number> [--repo OWNER/NAME]
#        [--independent-review-sha FULL_SHA] (explicit manual review attestation)
#        [--binary-review-sha FULL_SHA] (manual binary-byte, ownership, license, and NOTICE review)
# DSC/credential review record format (review or PR comment by another reviewer):
#   Security review: FULL_SHA
#   Result: accepted
# Include the reviewed scope and reasoning in that record.
# Exit:  0 may merge, 1 must not, 2 could not determine.
#
# Every positive result is bound to one server-observed head, base tip, complete
# check set, provider review commit, review-thread set, changed-file set, and
# readable compatibility surface. Unknown or incomplete evidence is never
# converted into an empty successful set.

set -uo pipefail

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd) || {
  echo "could not resolve merge-gate script directory" >&2
  exit 2
}

PR=""
REPO=""
INDEPENDENT_REVIEW_SHA=""
BINARY_REVIEW_SHA=""
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
    --independent-review-sha)
      INDEPENDENT_REVIEW_SHA="${2:-}"
      [ -n "$INDEPENDENT_REVIEW_SHA" ] || { echo "--independent-review-sha needs a full commit SHA" >&2; exit 2; }
      shift 2
      ;;
    --independent-review-sha=*)
      INDEPENDENT_REVIEW_SHA="${1#*=}"
      [ -n "$INDEPENDENT_REVIEW_SHA" ] || { echo "--independent-review-sha= needs a full commit SHA" >&2; exit 2; }
      shift
      ;;
    --binary-review-sha)
      BINARY_REVIEW_SHA="${2:-}"
      [ -n "$BINARY_REVIEW_SHA" ] || { echo "--binary-review-sha needs a full commit SHA" >&2; exit 2; }
      shift 2
      ;;
    --binary-review-sha=*)
      BINARY_REVIEW_SHA="${1#*=}"
      [ -n "$BINARY_REVIEW_SHA" ] || { echo "--binary-review-sha= needs a full commit SHA" >&2; exit 2; }
      shift
      ;;
    -h|--help)
      sed -n '2,17p' "$0"
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
[[ "$PR" =~ ^[0-9]+$ ]] || { echo "PR selector must be numeric" >&2; exit 2; }

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
if [ -z "$OWNER" ] || [ -z "$NAME" ] || [ "$OWNER" = "$REPO" ] || [[ "$REPO" == */*/* ]] \
  || ! [[ "$OWNER" =~ ^[A-Za-z0-9][A-Za-z0-9._-]*$ ]] \
  || ! [[ "$NAME" =~ ^[A-Za-z0-9][A-Za-z0-9._-]*$ ]]; then
  echo "--repo must be OWNER/NAME, got '$REPO'" >&2
  exit 2
fi

# This command encodes Bridge-specific master workflow and surface policy.
# An arbitrary repository's passing checks cannot qualify that contract.
if [ "$REPO" != "lamemustafa/bridge" ]; then
  echo "unsupported repository: this gate implements lamemustafa/bridge policy" >&2
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
# Independent acceptance decides whether a change is a production regression,
# whether its branch is dedicated, and whether it carries the required type:rectify label.
: >"$errfile"
if ! meta=$(gh pr view "$PR" --repo "$REPO" \
        --json headRefOid,baseRefOid,baseRefName,mergeable,mergeStateStatus,isDraft,state,title,body,changedFiles 2>"$errfile"); then
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
  (.title | type == "string") and
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
title=$(jq -r '.title' <<<"$meta")
raw_prbody=$(jq -r '.body // ""' <<<"$meta")
prbody="$raw_prbody"
visible_body_status=0
prbody=$(python3 -c 'import re, sys
text = sys.stdin.read()
print(re.sub(r"<!--.*?(?:-->|\Z)", "", text, flags=re.S), end="")' <<<"$prbody") || visible_body_status=$?
if [ "$visible_body_status" -ne 0 ]; then
  unknown "could not extract visible PR description content"
  prbody=""
fi

if [ -n "$INDEPENDENT_REVIEW_SHA" ] && ! [[ "$INDEPENDENT_REVIEW_SHA" =~ ^[0-9a-fA-F]{40}$ ]]; then
  bad "independent review attestation must be a full 40-hex commit SHA"
elif [ -n "$INDEPENDENT_REVIEW_SHA" ] && [ "$INDEPENDENT_REVIEW_SHA" != "$head" ]; then
  bad "independent review attestation names a different commit than the PR head"
fi
if [ -n "$BINARY_REVIEW_SHA" ] && ! [[ "$BINARY_REVIEW_SHA" =~ ^[0-9a-fA-F]{40}$ ]]; then
  bad "binary review attestation must be a full 40-hex commit SHA"
elif [ -n "$BINARY_REVIEW_SHA" ] && [ "$BINARY_REVIEW_SHA" != "$head" ]; then
  bad "binary review attestation names a different commit than the PR head"
fi

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
  DRAFT) bad "merge state DRAFT — the PR is not ready for merge" ;;
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
required_contexts=""
protection=$(gh api "repos/$REPO/branches/$base/protection/required_status_checks" 2>"$errfile") || protection_status=$?
if [ "$protection_status" -ne 0 ]; then
  unknown "could not read required status-check contexts for $base"
  required_contexts=""
elif ! jq -e '
  type == "object" and
  (.strict | type == "boolean" and . == true) and
  ((.contexts // []) | type == "array" and all(.[]; type == "string" and length > 0)) and
  ((.checks // []) | type == "array" and all(.[]; type == "object" and (.context | type == "string" and length > 0)))
' <<<"$protection" >/dev/null 2>&1; then
  unknown "required status-check response was malformed"
  required_contexts=""
else
  required_contexts=$(jq -r '((.contexts // []) + ([.checks // [] | .[]? | .context] | map(select(type == "string" and length > 0))) | unique)[]' <<<"$protection")
  documented_contexts=$'Dependency security\nFrontend build\nGitGuardian Security Checks\nRequired checks\nRust format'
  if [ -z "$required_contexts" ]; then
    unknown "branch protection returned no required status-check contexts"
  elif missing_documented=$(comm -23 <(sort -u <<<"$documented_contexts") <(sort -u <<<"$required_contexts")) && [ -n "$missing_documented" ]; then
    bad "branch protection omits $(wc -l <<<"$missing_documented" | tr -d ' ') documented required check context(s)"
  elif unexpected_documented=$(comm -13 <(sort -u <<<"$documented_contexts") <(sort -u <<<"$required_contexts")) && [ -n "$unexpected_documented" ]; then
    bad "branch protection includes $(wc -l <<<"$unexpected_documented" | tr -d ' ') undocumented required check context(s)"
  else
    say "ok" "loaded $(wc -l <<<"$required_contexts" | tr -d ' ') required check context(s)"
  fi
fi
printf '%s\n' "$required_contexts" >"$tmpdir/required-contexts"

# gh uses pass/fail/pending/skipping/cancel buckets. Preserve command status,
# then parse JSON and require each protected context individually.
: >"$errfile"
check_status=0
context_report_valid=false
check_bad=1
all_bad=1
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
    printf '%s' "$buckets" >"$tmpdir/check-buckets.json"
    context_report_status=0
    context_report_valid=false
    context_report=$(jq --rawfile contexts "$tmpdir/required-contexts" '
      . as $buckets | ($contexts | split("\n") | map(select(length > 0))) |
      map(. as $context | [$buckets[] | select(.name == $context)] |
        {name: $context, state: (if length == 0 then "missing"
          elif all(.[]; .bucket == "pass") then "pass"
          else map(.bucket) | unique | join(",") end)}) |
      {total: length, failed: ([.[] | select(.state != "pass")] | length),
       examples: ([.[] | select(.state != "pass")][0:8] | map(
         "required check \u0027" + (.name | gsub("[[:cntrl:]]"; "?") | .[0:80]) + "\u0027 " +
         (if .state == "missing" then "was not reported" else "is not passing (" + .state + ")" end)))}
    ' <"$tmpdir/check-buckets.json") || context_report_status=$?
    if [ "$context_report_status" -ne 0 ] || ! jq -e '. as $report | type == "object" and ($report.total | type == "number" and floor == . and . >= 0) and ($report.failed | type == "number" and floor == . and . >= 0 and . <= $report.total) and ($report.examples | type == "array" and length <= 8 and all(.[]; type == "string"))' <<<"$context_report" >/dev/null 2>&1; then
      unknown "could not compute bounded required-check diagnostics"
    else
      context_report_valid=true
      check_bad=$(jq -r '.failed' <<<"$context_report")
      context_count=$(jq -r '.total' <<<"$context_report")
      if [ "$check_bad" -gt 0 ]; then
        bad "$check_bad of $context_count required check contexts are not passing; up to 8 bounded examples: $(jq -r '.examples | join("; ")' <<<"$context_report")"
      else
        say "ok" "all $context_count required check contexts passed"
      fi
    fi
    all_bad=$(jq '[.[] | select(.bucket == "fail" or .bucket == "cancel" or .bucket == "pending")] | length' <<<"$buckets")
    skipped=$(jq '[.[] | select(.bucket == "skipping")] | length' <<<"$buckets")
    [ "$all_bad" -eq 0 ] || bad "$all_bad reported check(s) are failing, cancelled, or pending"
    [ "$skipped" -eq 0 ] || say "note" "$skipped skipped check(s) await explicit workflow allowlist and path-condition validation"
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
      (.id | type == "number" and floor == . and . >= 0) and
      (.name | type == "string" and length > 0) and
      (.status == "completed") and
      (.conclusion | type == "string" and (. == "success" or . == "skipped" or . == "neutral" or . == "failure" or . == "cancelled" or . == "timed_out" or . == "action_required" or . == "stale")) and
      (.head_sha | type == "string" and test("^[0-9a-fA-F]{40}$") and . == $head)
    ))) and
  ((map(.total_count) | unique | length) == 1) and
  ((map(.check_runs | length) | add) == .[0].total_count) and
  ((map(.check_runs) | add | map(.id) | unique | length) == .[0].total_count)
' <<<"$check_runs" >/dev/null 2>&1; then
  unknown "could not validate complete head-bound check-run evidence"
else
  printf '%s' "$check_runs" >"$tmpdir/check-runs.json"
  failed_runs_status=0
  failed_runs=$(jq --rawfile contexts "$tmpdir/required-contexts" '
    ($contexts | split("\n")) as $required |
    [.[] | .check_runs[] | . as $run |
      select((.conclusion != "success" and .conclusion != "neutral" and .conclusion != "skipped") or
             (.conclusion != "success" and ($required | index($run.name)) != null))] | length
  ' <(printf '%s' "$check_runs")) || failed_runs_status=$?
  if [ "$failed_runs_status" -ne 0 ] || ! [[ "$failed_runs" =~ ^[0-9]+$ ]]; then
    unknown "could not evaluate refreshed check-run conclusions"
  elif [ "$failed_runs" -gt 0 ]; then
    bad "$failed_runs refreshed check run(s) are failed or required-but-not-successful"
  else
    say "ok" "completed check-run pages are successful and bound to head $short"
  fi
fi
: >"$errfile"
statuses_status=0
statuses=$(gh api --paginate --slurp "repos/$REPO/commits/$head/status?per_page=100" 2>"$errfile") || statuses_status=$?
printf '%s' "$statuses" >"$tmpdir/combined-status-pages.json"
if [ "$statuses_status" -ne 0 ] || ! jq -e --arg head "$head" '
  type == "array" and length > 0 and
  all(.[]; type == "object" and
    (.sha | type == "string" and test("^[0-9a-fA-F]{40}$") and . == $head) and
    (.total_count | type == "number" and floor == . and . >= 0) and
    (.state | type == "string" and (. == "success" or . == "pending" or . == "failure" or . == "error")) and
    (.statuses | type == "array" and all(.[];
      type == "object" and
      (.id | type == "number" and floor == . and . >= 0) and
      (.context | type == "string" and length > 0) and
      (.state == "success" or .state == "failure" or .state == "error")
    ))
  ) and
  ((map(.total_count) | unique | length) == 1) and
  ((map(.statuses | length) | add) == .[0].total_count) and
  ((map(.statuses) | add | map(.id) | unique | length) == .[0].total_count) and
  ((map(.statuses) | add | map(.context) | unique | length) == .[0].total_count) and
  (if .[0].state == "pending" then .[0].total_count == 0 and (map(.statuses | length) | add) == 0
   elif .[0].state == "failure" or .[0].state == "error" then .[0].state as $aggregate | all(.[]; .state == $aggregate)
   else all(.[]; .state == "success") end)
' <"$tmpdir/combined-status-pages.json" >/dev/null 2>&1; then
  unknown "could not validate complete head-bound commit-status evidence"
elif jq -e 'any(.[]; (.state == "failure" or .state == "error") or any(.statuses[]; .state == "failure" or .state == "error"))' <"$tmpdir/combined-status-pages.json" >/dev/null; then
  bad "combined commit-status evidence reports a failure"
else
  say "ok" "complete commit-status pages are bound to head $short"
fi

# Scan all published PR metadata. Commit messages are paginated because they
# can become squash subjects or release evidence independently of the patch.
: >"$errfile"
metadata_pr_status=0
metadata_pr=$(gh api "repos/$REPO/pulls/$PR" 2>"$errfile") || metadata_pr_status=$?
if [ "$metadata_pr_status" -ne 0 ] || ! jq -e --arg head "$head" '
  type == "object" and
  (.commits | type == "number" and floor == . and . > 0 and . <= 250) and
  (.head | type == "object" and
   (.sha | type == "string" and test("^[0-9a-fA-F]{40}$") and . == $head))
' <<<"$metadata_pr" >/dev/null 2>&1; then
  unknown "could not prove complete head-bound PR commit metadata for the privacy scan"
  privacy_metadata=""
else
  metadata_commit_total=$(jq -r '.commits' <<<"$metadata_pr")
fi

: >"$errfile"
metadata_status=0
metadata_commits=$(gh api --paginate --slurp "repos/$REPO/pulls/$PR/commits?per_page=100" 2>"$errfile") || metadata_status=$?
if [ -z "${metadata_commit_total:-}" ] || [ "$metadata_status" -ne 0 ] || ! jq -e --argjson expected "$metadata_commit_total" --arg head "$head" '
  type == "array" and (all(.[]; type == "array") or all(.[]; type == "object")) and
  ((if all(.[]; type == "array") then flatten else . end) |
   length == $expected and
   ([.[].sha] | unique | length) == $expected and
   any(.[]; .sha == $head) and
   all(.[]; type == "object" and
    (.sha | type == "string" and test("^[0-9a-fA-F]{40}$")) and
    (.commit | type == "object") and
    (.commit.message | type == "string") and
    (.commit.author | type == "object" and
      (.name | type == "string") and (.email | type == "string" and test("^[A-Za-z0-9.!#$%&*+/=?^_`{|}~-]+@[A-Za-z0-9][A-Za-z0-9.-]*\\.[A-Za-z]{2,}$"))) and
    (.commit.committer | type == "object" and
      (.name | type == "string") and (.email | type == "string" and test("^[A-Za-z0-9.!#$%&*+/=?^_`{|}~-]+@[A-Za-z0-9][A-Za-z0-9.-]*\\.[A-Za-z]{2,}$"))) and
    ((.author == null) or (.author | type == "object" and (.login | type == "string"))) and
    ((.committer == null) or (.committer | type == "object" and (.login | type == "string")))))
' <<<"$metadata_commits" >/dev/null 2>&1; then
  unknown "could not prove complete head-bound PR commit metadata for the privacy scan"
  privacy_metadata=""
else
  # Author and committer emails were structurally validated above and are an
  # explicit identity-only source class. Do not mix them into payload/path/
  # metadata scan input, where an identical address would be customer data.
  commit_messages=$(jq -r '(if all(.[]; type == "array") then flatten else . end)[] |
    [.commit.message, .commit.author.name, .commit.committer.name,
     (.author.login? // null), (.committer.login? // null)] |
    map(select(. != null))[]' <<<"$metadata_commits")
  privacy_metadata="$title
$raw_prbody
$commit_messages"
fi

# Require a completed checklist item and a same-repository line permalink.
# The repository template puts the permalink on the item's indented
# continuation, so accept it there as well as in an inline Markdown link.
# A filename in prose, a foreign link, or an anchor outside the current
# checklist file is not completion evidence.
read_review_checklist() {
  local response content decoded decode_status
  : >"$errfile"
  response=$(gh api "repos/$REPO/contents/review-checklist.md?ref=$head" 2>"$errfile") || return 1
  content=$(jq -er 'select(.encoding == "base64") | .content | strings' <<<"$response") || return 1
  decode_status=0
  decoded=$(printf '%s' "${content//$'\n'/}" | base64 --decode 2>"$errfile") || decode_status=$?
  if [ "$decode_status" -ne 0 ]; then
    decode_status=0
    decoded=$(printf '%s' "${content//$'\n'/}" | base64 -D 2>"$errfile") || decode_status=$?
  fi
  [ "$decode_status" -eq 0 ] && [ -n "$decoded" ] || return 1
  review_checklist="$decoded"
}
checklist_link_ok() {
  local body="$1" checklist="$2" line awaiting_permalink=0 link anchor
  local checked='^[[:space:]]*-[[:space:]]*\[[xX]\][[:space:]]+'
  local permalink="https://github\.com/${OWNER}/${NAME}/blob/${head}/review-checklist\.md#L[0-9]+"
  local permalink_boundary="${permalink}"'([[:space:]]|\)|$)'
  while IFS= read -r line; do
    if printf '%s\n' "$line" | grep -Eq "$checked"; then
      if printf '%s\n' "$line" | grep -Eiq "$permalink_boundary"; then
        awaiting_permalink=2
      elif printf '%s\n' "$line" | grep -Eiq 'review-checklist\.md'; then
        awaiting_permalink=1
      else
        awaiting_permalink=0
      fi
    elif [ "$awaiting_permalink" -eq 1 ] && printf '%s\n' "$line" | grep -Eq '^[[:space:]]+'; then
      if printf '%s\n' "$line" | grep -Eiq "$permalink_boundary"; then
        awaiting_permalink=2
      fi
    elif [ "$awaiting_permalink" -ne 2 ]; then
      awaiting_permalink=0
    fi
    if [ "$awaiting_permalink" -eq 2 ]; then
      while IFS= read -r link; do
        anchor=${link##*#L}
        anchor=${anchor%%[^0-9]*}
        if sed -n "${anchor}p" <<<"$checklist" | grep -Eq '^[[:space:]]*-[[:space:]]*\[[ xX]\][[:space:]]+[^[:space:]]'; then
          return 0
        fi
      done < <(printf '%s\n' "$line" | grep -Eio "$permalink_boundary")
      awaiting_permalink=0
    fi
  done <<<"$body"
  return 1
}
review_checklist=""
checklist_status=0
read_review_checklist || checklist_status=$?
if [ "$checklist_status" -ne 0 ]; then
  unknown "could not read review-checklist content for link validation"
elif ! checklist_link_ok "$prbody" "$review_checklist"; then
  bad "description lacks a completed same-repository review-checklist link to an existing line"
else
  say "ok" "description links a completed review-checklist item"
fi

body_section_has_content() {
  local body="$1" labels="$2" allow_placeholders="${3:-false}"
  awk -v labels="$labels" -v allow_placeholders="$allow_placeholders" '
    function heading(line, lower) {
      lower = tolower(line)
      sub(/^[[:space:]]*#+[[:space:]]*/, "", lower)
      return lower ~ ("^(" labels ")[[:space:]]*:[[:space:]]*[^[:space:]]") ||
             lower ~ ("^(" labels ")[[:space:]]*:?[[:space:]]*$")
    }
    function template_prompt(line, lower) {
      lower = tolower(line)
      return lower == "what concrete user or maintainer workflow changes, and why now?" ||
             lower ~ /^-[[:space:]]*\[[xX]\][[:space:]]/ ||
             lower ~ /^-[[:space:]]*exact candidate sha:/ ||
             lower ~ /^-[[:space:]]*commands and results[[:space:]]*\(.*\):[[:space:]]*$/ ||
             lower ~ /^-[[:space:]]*captured\/fixture\/live scope and known limitations:[[:space:]]*$/ ||
             lower ~ /^-[[:space:]]*manual\/ui evidence[[:space:]]*\(.*\):[[:space:]]*$/
    }
    function after_colon(line, value) {
      value = line
      sub(/^[^:]*:[[:space:]]*/, "", value)
      return value
    }
    function meaningful(value, lower) {
      gsub(/<!--[[:print:][:space:]]*-->/, "", value)
      lower = tolower(value)
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", lower)
      # A terminal dot or similar punctuation does not make an otherwise empty
      # template answer substantive (for example, `N/A.` or `TBD.`).
      gsub(/[,.;:!?]+$/, "", lower)
      gsub(/[[:space:]]+$/, "", lower)
      if (lower == "") return 0
      # Fence delimiters and thematic breaks are structure, not policy content.
      if (lower ~ /^(```|~~~)/) return 0
      if (lower ~ /^([-][[:space:]]*){3,}$/ || lower ~ /^([*][[:space:]]*){3,}$/ || lower ~ /^(_[[:space:]]*){3,}$/) return 0
      if (allow_placeholders == "true" && lower ~ /^(n\/a|none|no impact)$/) return 1
      return lower !~ /^(n\/a|none|pending|todo|tbd|not applicable|unaffected|not affected|not impacted|no impact)$/
    }
    {
      # The canonical PR template uses labelled list fields as well as
      # headings.  Accept a filled field for the requested policy label, but
      # never the untouched label or a checkbox by itself.
      lower = tolower($0)
      gsub(/<!--[[:print:][:space:]]*-->/, "", lower)
      if (pending_list) {
        if (lower ~ /^[[:space:]]*$/) {
          pending_list = 0
        } else if (lower ~ /^[[:space:]]*[-#]/) {
          pending_list = 0
        } else if (lower !~ /^[[:space:]]+/) {
          pending_list = 0
        } else if (lower ~ /:[[:space:]]*[^[:space:]]/ && meaningful(after_colon(lower))) {
          found = 1
          exit
        } else if (lower ~ /:[[:space:]]*$/) {
          pending_list = 2
        } else if (pending_list && lower ~ /[^[:space:]]/) {
          if (!template_prompt($0) && lower !~ /^[[:space:]]*<!--/ && meaningful(lower)) {
            found = 1
            exit
          }
          pending_list = 0
        }
      }
      if (lower ~ "^[[:space:]]*-[[:space:]]*(" labels ")[^:]*:[[:space:]]*[^[:space:]]" && meaningful(after_colon(lower)) &&
          !template_prompt($0) && lower !~ /^[[:space:]]*-[[:space:]]*\[[ xX]\][[:space:]]/) {
        found = 1
        exit
      }
      if (lower ~ "^[[:space:]]*-[[:space:]]*(" labels ")[^:]*:?[[:space:]]*$") {
        pending_list = 1
        next
      }
      if (heading($0)) {
        lower = tolower($0)
        sub(/^[[:space:]]*#+[[:space:]]*/, "", lower)
        if (lower ~ ("^(" labels ")[[:space:]]*:[[:space:]]*[^[:space:]]") && meaningful(after_colon(lower))) { found = 1; exit }
        waiting = 1
        next
      }
      if ($0 ~ /^[[:space:]]*#/) {
        waiting = 0
        next
      }
      if (waiting && $0 ~ /[^[:space:]]/) {
        # A labelled sibling list field belongs to the surrounding template,
        # not to this heading.  In particular, Migration compatibility must
        # not double as Rollback notes merely because it follows that heading.
        if (lower ~ /^[[:space:]]*[-*][[:space:]]*[^:[:space:]][^:]*:[[:space:]]*/) {
          waiting = 0
          next
        }
        if ($0 ~ /^[[:space:]]*<!--/) next
        if (template_prompt($0)) next
        if (meaningful(lower)) { found = 1; exit }
      }
    }
    END { exit(found ? 0 : 1) }
  ' <<<"$body"
}
if ! body_section_has_content "$prbody" 'functional summary|outcome and reason'; then
  bad "description lacks a non-empty functional summary"
else
  say "ok" "description includes a functional summary"
fi
if ! body_section_has_content "$prbody" 'test or reproduction command|commands and results|validation and evidence'; then
  bad "description lacks a non-empty test or reproduction command"
else
  say "ok" "description includes test or reproduction evidence"
fi
body_has_validation_command() {
  # Require a concrete inline/fenced command inside the named validation section.
  # A tool name mentioned in prose or an unrelated section is not a command.
  python3 -c '
import re, shlex, sys
text = re.sub(r"<!--.*?-->", "", sys.stdin.read(), flags=re.S)
active = fenced = False
for line in text.splitlines():
    heading = re.match(r"^\s*#{1,6}\s+(.+?)\s*$", line)
    if heading:
        active = bool(re.fullmatch(r"(?:test or reproduction command|commands and results|validation and evidence):?", heading[1], re.I))
        fenced = False
        continue
    if not active:
        continue
    if re.match(r"^\s*(?:```|~~~)", line):
        fenced = not fenced
        continue
    candidates = [(line.strip()[2:] if line.strip().startswith("$ ") else line.strip())] if fenced else re.findall(r"`([^`]+)`", line)
    for command in candidates:
        if re.search(r"\.\.\.|…|<[^>]+>", command):
            continue
        try:
            words = shlex.split(command)
        except ValueError:
            continue
        if words and words[0] == "corepack":
            words = words[1:]
        if len(words) >= 2 and (words[0] in {"python", "python3", "pytest", "pnpm", "npm", "node", "cargo", "make", "bash", "sh", "gh"} or words[0].startswith("scripts/")):
            sys.exit(0)
sys.exit(1)
' <<<"$1"
}
if ! body_has_validation_command "$prbody"; then
  bad "description lacks an actual test or reproduction command"
fi

# P4 is a three-part design record, not a generic scope paragraph.  When a
# patch adds implementation code, each question must have an answer that is
# more than the untouched template label.
body_has_p4_answers() {
  local body="$1"
  body_section_has_content "$body" 'existing component reused' &&
    body_section_has_content "$body" 'what is deleted' &&
    body_section_has_content "$body" 'what breaks if this is not built'
}

body_has_platform_evidence() {
  local body="$1" host="$2"
  # A named host field must include evidence on that line or in a following
  # content line. A bare list label is only a template prompt, not evidence.
  python3 -c '
import re, sys
host = sys.argv[1].lower()
placeholders = {
    "", "none", "n/a", "not applicable", "unaffected", "not affected",
    "not impacted", "no impact", "pending", "todo", "tbd",
}
marker = r"(validation|evidence|test|check|unaffected|not applicable|not affected|not impact)"
def normalize(value):
    return value.strip().lower().rstrip(".,;:!?").strip()
def continuation_evidence(value):
    value = normalize(value)
    if not value or value in placeholders or re.match(r"^(?:[-*]\s+)?(?:note|notes|status|tracking|todo)\b", value):
        return False
    command = re.search(r"(?:^|[`$\s])(?:python(?:3)?|pytest|pnpm|npm|node|cargo|make|bash|sh|gh)(?:[\s`]|$)", value)
    affirmative = re.search(r"\b(?:passed|succeeded|validated|completed)\b", value)
    negated = re.search(r"\b(?:not|never|failed|failure|without|no)\b(?:\W+\w+){0,4}\W+\b(?:pass(?:ed)?|succeed(?:ed)?|validat(?:ed|ion)|complet(?:ed|ion))\b", value)
    outcome = affirmative and not negated and re.search(r"\b(?:ci|test|check|validation|windows|macos)\b", value)
    unaffected = re.search(r"\b(?:unaffected|not affected|not impacted)\b", value) and re.search(r"\b(?:because|as|this)\b", value)
    return bool(outcome or unaffected)
waiting = False
waiting_list_indent = None
for raw in sys.stdin.read().splitlines():
    visible = re.sub(r"<!--.*?(?:-->|$)", "", raw)
    indent = len(visible) - len(visible.lstrip(" \t"))
    lower = visible.strip().lower()
    if not lower:
        continue
    is_fence = bool(re.match(r"^(?:```|~~~)", lower))
    is_list_item = bool(re.match(r"^[-*]\s+", lower))
    is_heading = bool(re.match(r"^#{1,6}\s+", lower))
    is_field = host in lower and bool(re.search(marker, lower))
    if waiting:
        # Empty fence delimiters carry no validation result. Keep looking so a
        # real fenced command can still establish the named host evidence.
        if is_fence:
            continue
        # A new evidence field or heading ends the preceding empty field; it
        # cannot be treated as the earlier host evidence.
        if ((waiting_list_indent is not None and is_list_item and indent <= waiting_list_indent) or
                is_heading or (is_list_item and re.search(marker, lower))):
            waiting = False
            waiting_list_indent = None
        elif continuation_evidence(lower):
            raise SystemExit(0)
        else:
            waiting = False
            waiting_list_indent = None
    if not is_field:
        continue
    if ":" in lower:
        value = normalize(re.sub(r"^#{1,6}\s+", "", lower.split(":", 1)[1]))
        # Inline evidence is subject to the same quality threshold as a
        # continuation. Otherwise an arbitrary sentence after the host label
        # turns a template field into acceptance evidence.
        if continuation_evidence(value):
            raise SystemExit(0)
    # Headings and list labels without an inline answer may be completed by a
    # following substantive validation result. All other bare forms fail.
    waiting = True
    waiting_list_indent = indent if is_list_item else None
raise SystemExit(1)
' "$host" <<<"$body"
}

# Paginate changed files through the REST endpoint; gh pr view hard-codes a
# first:100 GraphQL fragment in some versions. Retain the line counts as well:
# the privacy scan can only be complete when the textual diff describes every
# non-removed destination with the byte count GitHub reported.
: >"$errfile"
files_status=0
files=$(gh api --paginate --slurp "repos/$REPO/pulls/$PR/files?per_page=100" 2>"$errfile") || files_status=$?
changed_records="$tmpdir/changed-files.tsv"
changed_inventory_complete=false
if [ "$files_status" -ne 0 ] || ! jq -e '
  type == "array" and
  (all(.[]; type == "array" and all(.[];
      type == "object" and
      ((.filename | type) == "string") and (.filename | length > 0) and (.filename | test("[\u0000-\u001F\u007F]") | not) and
      ((.status | type) == "string") and (.status | length > 0) and
      (((.previous_filename? == null) or (((.previous_filename | type) == "string") and ((.previous_filename | length) > 0) and ((.previous_filename | test("[\u0000-\u001F\u007F]")) | not))) and ((.status != "renamed") or (((.previous_filename | type) == "string") and ((.previous_filename | length) > 0) and ((.previous_filename | test("[\u0000-\u001F\u007F]")) | not)))) and
      ((.additions | type) == "number") and (.additions | floor == . and . >= 0) and
      ((.deletions | type) == "number") and (.deletions | floor == . and . >= 0))) or
   all(.[]; type == "object" and
      ((.filename | type) == "string") and (.filename | length > 0) and (.filename | test("[\u0000-\u001F\u007F]") | not) and
      ((.status | type) == "string") and (.status | length > 0) and
      (((.previous_filename? == null) or (((.previous_filename | type) == "string") and ((.previous_filename | length) > 0) and ((.previous_filename | test("[\u0000-\u001F\u007F]")) | not))) and ((.status != "renamed") or (((.previous_filename | type) == "string") and ((.previous_filename | length) > 0) and ((.previous_filename | test("[\u0000-\u001F\u007F]")) | not)))) and
      ((.additions | type) == "number") and (.additions | floor == . and . >= 0) and
      ((.deletions | type) == "number") and (.deletions | floor == . and . >= 0)))
' <<<"$files" >/dev/null 2>&1; then
  unknown "could not read the complete changed-file set"
  changed=""
else
  jq -r '(if all(.[]; type == "array") then flatten else . end)[] | [.filename, .status, .additions, .deletions, (.previous_filename? // "")] | @tsv' <<<"$files" >"$changed_records"
  changed=$(cut -f1 "$changed_records")
  changed_count=$(jq '(if all(.[]; type == "array") then flatten else . end) | length' <<<"$files")
  unique_changed_count=$(jq '(if all(.[]; type == "array") then flatten else . end) | map(.filename) | unique | length' <<<"$files")
  if [ "$changed_count" -eq 0 ]; then
    unknown "changed-file response contained no filenames"
  elif [ "$changed_files_expected" -gt 3000 ]; then
    unknown "PR reports $changed_files_expected changed files beyond the REST files API cap"
  elif [ "$changed_count" -ne "$changed_files_expected" ] || [ "$unique_changed_count" -ne "$changed_count" ]; then
    unknown "changed-file response has $changed_count unique records; PR metadata reports $changed_files_expected"
  else
    changed_inventory_complete=true
  fi
fi

# The PR workflow has exactly three job-level skip paths. Native and bundle
# matrix jobs may skip only when the same changed-file patterns that ci.yml
# uses evaluate false; compiler-cache retention skips on every PR because its
# job condition is master-only. Every other skipped check is evidence of an
# unreviewed workflow condition and blocks rather than becoming "optional".
validate_skipped_ci_jobs() {
  local source="$1" names_file="$2" policy_ok_var="${3:-skipped_ci_policy_ok}" encoded name paths_file
  local native_pattern bundle_pattern
  native_pattern='^(\.github/workflows/ci\.yml|\.github/actions/setup-windows-native/|rust-toolchain\.toml|src-tauri/|tools/)'
  bundle_pattern='^(\.github/workflows/ci\.yml|\.github/actions/setup-windows-native/|packaging/mcpb/|package\.json|pnpm-lock\.yaml|\.node-version|vite\.config\.ts|tsconfig\.json|postcss\.config\.js|index\.html|src/|src-tauri/(src/|crates/|Cargo\.lock|Cargo\.toml|.*/Cargo\.toml|tauri\.conf\.json|build\.rs|icons/)|LICENSE$|NOTICE$|THIRD_PARTY_LICENSES\.txt$|THIRD_PARTY_LICENSES_RUST\.txt$|scripts/(capture-package-log(\.test)?\.py|check-mcpb-bundle(\.test)?\.py|package-mcpb\.mjs|check-license-metadata\.mjs|check-dependency-inventory\.mjs|check-windows-bundle-resources\.ps1|check-macos-bundle-resources(\.mutation)?\.mjs)$)'

  [ -s "$names_file" ] || return
  if [ "$changed_inventory_complete" != true ]; then
    printf -v "$policy_ok_var" '%s' false
    unknown "$source reports skipped CI job(s), but their path conditions could not be validated from the complete changed-file set"
    return
  fi
  paths_file="$tmpdir/skipped-ci-paths"
  awk -F '\t' '{ print $1; if ($2 == "renamed") print $5 }' "$changed_records" >"$paths_file"
  while IFS= read -r encoded; do
    name=$(printf '%s' "$encoded" | base64 --decode 2>/dev/null) || name=$(printf '%s' "$encoded" | base64 -D 2>/dev/null) || {
      printf -v "$policy_ok_var" '%s' false
      unknown "$source returned an unreadable skipped CI job name"
      continue
    }
    case "$name" in
      "Native checks (windows-latest)"|"Native checks (macos-latest)")
        if grep -Eq "$native_pattern" "$paths_file"; then
          printf -v "$policy_ok_var" '%s' false
          bad "$source reports a skipped native CI matrix job despite native-scope changed files"
        else
          say "note" "$source reports an allowlisted skipped native CI matrix job for a non-native change"
        fi
        ;;
      "Bundle smoke (windows-latest)"|"Bundle smoke (macos-latest)")
        if grep -Eq "$bundle_pattern" "$paths_file"; then
          printf -v "$policy_ok_var" '%s' false
          bad "$source reports a skipped bundle CI matrix job despite bundle-scope changed files"
        else
          say "note" "$source reports an allowlisted skipped bundle CI matrix job for a non-bundle change"
        fi
        ;;
      "Retain two compiler-cache snapshots per OS")
        say "note" "$source reports the allowlisted master-only compiler-cache retention job skipped on this PR"
        ;;
      *)
        printf -v "$policy_ok_var" '%s' false
        bad "$source reports a skipped CI job outside the explicit workflow allowlist"
        ;;
    esac
  done <"$names_file"
}

check_bucket_skips="$tmpdir/check-bucket-skips.b64"
check_run_skips="$tmpdir/check-run-skips.b64"
skipped_ci_policy_ok=true
if [ ! -f "$tmpdir/check-buckets.json" ]; then
  skipped_ci_policy_ok=false
elif ! jq -r '.[] | select(.bucket == "skipping") | .name | @base64' "$tmpdir/check-buckets.json" >"$check_bucket_skips"; then
  skipped_ci_policy_ok=false
  unknown "could not read skipped check names from the checks rollup"
elif [ -f "$tmpdir/check-runs.json" ] && ! jq -r '.[] | .check_runs[] | select(.conclusion == "skipped") | .name | @base64' "$tmpdir/check-runs.json" >"$check_run_skips"; then
  skipped_ci_policy_ok=false
  unknown "could not read skipped check names from refreshed check-run evidence"
else
  validate_skipped_ci_jobs "checks rollup" "$check_bucket_skips"
  [ -f "$tmpdir/check-runs.json" ] && validate_skipped_ci_jobs "refreshed check-run evidence" "$check_run_skips"
fi
[ "$context_report_valid" = true ] && [ "$check_bad" -eq 0 ] && [ "$all_bad" -eq 0 ] && [ "$skipped_ci_policy_ok" = true ] && say "ok" "all reported checks concluded successfully"

# Existing workflow changes require the rollback and migration-compatibility
# notes mandated by the project review flow. The complete REST file set, rather
# than the rendered diff, is the authority for this conditional requirement.
workflow_change=0
if [ -n "$files_status" ] && [ "$files_status" -eq 0 ]; then
  workflow_change=$(jq -r '
    (if all(.[]; type == "array") then flatten else . end) |
    any(.[]; (.filename | startswith(".github/workflows/")) or
              ((.previous_filename? // "") | startswith(".github/workflows/")))
  ' <<<"$files")
fi
native_frontend_change=0
if [ "$files_status" -eq 0 ]; then
  native_frontend_change=$(jq -r '
    (if all(.[]; type == "array") then flatten else . end) |
    any(.[]; [ .filename, (.previous_filename? // "") ][] |
      test("^(src-tauri/|src/|scripts/.*\\.(ts|tsx|js|mjs)$)"))
  ' <<<"$files")
fi

# Treat source additions as implementation work only when the REST line totals
# prove that bytes were added.  Rename paths are considered for every path
# policy below, so moving platform, migration, or sensitive code cannot evade
# the relevant review record.
implementation_code_added=false
dependency_manifest_added=false
platform_sensitive_change=false
migration_change=false
if [ "$files_status" -eq 0 ]; then
  implementation_code_added=$(jq -r '
    (if all(.[]; type == "array") then flatten else . end) |
    any(.[]; (.additions > 0) and (.filename | test("\\.(rs|ts|tsx|js|mjs|py|go|java|kt|swift|c|cc|cpp|h|hpp|sh|bash|ps1|psm1|sql)$")))
  ' <<<"$files")
  dependency_manifest_added=$(jq -r '
    (if all(.[]; type == "array") then flatten else . end) |
    any(.[]; (.additions > 0) and (.filename | test("(^|/)(package\\.json|Cargo\\.toml)$")))
  ' <<<"$files")
  platform_sensitive_change=$(jq -r '
    (if all(.[]; type == "array") then flatten else . end) |
    any(.[]; [.filename, (.previous_filename? // "")][] |
      test("^(src-tauri/|src/.*\\.(rs|ts|tsx|js|mjs)$)|\\.(ps1|psm1)$|^\\.github/workflows/(ci\\.yml|release-mcpb-preview\\.yml)$|^\\.github/actions/setup-windows-native/|(^|/)(windows|macos|darwin|win32|local_files|paths)(/|[._-])"; "i"))
  ' <<<"$files")
  migration_change=$(jq -r '
    (if all(.[]; type == "array") then flatten else . end) |
    any(.[]; [.filename, (.previous_filename? // "")][] |
      test("(^|/)(migrations?|schema|database|db)(/|[._-])"; "i"))
  ' <<<"$files")
fi

# The review contract requires an explicit security-impact statement whenever
# either spelling of a renamed path touches a DSC, credential, or Tally surface.
# Treat native and frontend source conservatively: their generic filenames can
# carry DSC/Tally behavior. Use the complete REST inventory, including removals
# and prior rename paths; named tooling and documentation surfaces are included.
security_sensitive_change=0
if [ "$files_status" -eq 0 ]; then
  security_sensitive_change=$(jq -r '
    (if all(.[]; type == "array") then flatten else . end) |
    any(.[]; [ .filename, (.previous_filename? // "") ][] |
      ascii_downcase | test("^src-tauri/(crates|src)/|^src/|^docs/(tally|agent)/|^scripts/(bank_statement_import|sanitise-bbox-capture|prune-package-compiler-cache)|^\\.github/workflows/(ci\\.yml|release-mcpb-preview\\.yml|deploy-install-page\\.yml)$|(^|/)[^/]*(dsc|credential|tally)[^/]*(/|$)"))
  ' <<<"$files")
fi
if [ "$security_sensitive_change" = "true" ]; then
  if ! body_section_has_content "$prbody" 'security impact|security implications' true; then
    bad "credential-sensitive path change lacks non-empty security-impact notes"
  else
    say "ok" "credential-sensitive path change includes security-impact notes"
  fi

fi
# DSC/credential changes require a separate security-focused reviewer comment.
# A general approval or the PR author’s impact notes are not that record.
security_reviewer_change=false
if [ "$files_status" -eq 0 ]; then
  security_reviewer_change=$(jq '
    (if all(.[]; type == "array") then flatten else . end) |
    any(.[]; [.filename, (.previous_filename? // "")][] |
      (test("(^|[/_.-])(dsc|credential[s]?|certificate[s]?|keystore|secret[s]?)(?=[/_.-]|$|[A-Z])"; "i") or
       test("^scripts/(merge-gate\\.sh|merge_gate_privacy\\.py|merge_gate_diff\\.py|bank_statement_import\\.py)$"; "i") or
       test("^scripts/prune-package-compiler-cache\\.mjs$"; "i") or
       test("^\\.github/workflows/(ci\\.yml|release-mcpb-preview\\.yml|deploy-install-page\\.yml)$"; "i") or
       test("^src-tauri/Cargo\\.(toml|lock)$|^src-tauri/src/lib\\.rs$|^src/(AxalScreen|DocumentsScreen)\\.tsx$|^src-tauri/src/axal\\.rs$|^src-tauri/src/db/encrypted\\.rs$|^src-tauri/src/documents\\.rs$|^src-tauri/src/commands\\.rs$"; "i")))
  ' <<<"$files")
fi
validate_security_reviewer() {
if [ "$security_reviewer_change" = "true" ]; then
  pr_author=$(jq -er '.user.login | strings | select(length > 0)' <<<"$metadata_pr" 2>/dev/null) || pr_author=""
  security_review=false
  if [ -n "$pr_author" ] && [ "$review_status" -eq 0 ] && [ "$comment_status" -eq 0 ]; then
    printf '%s' "$reviews" >"$tmpdir/reviews.json"
    printf '%s' "$comments" >"$tmpdir/comments.json"
    security_review=$(jq -n --arg head "$head" --arg author "$pr_author" --slurpfile reviews "$tmpdir/reviews.json" --slurpfile comments "$tmpdir/comments.json" '
      def records: if all(.[]; type == "array") then flatten else . end;
      def source_records: if length == 1 then .[0] else . end;
      def reviewer: (.user.login | type == "string" and length > 0) and
        .user.login != $author and (.user.type == "User" or .user.type == "Bot") and
        ((.author_association == "OWNER" or .author_association == "MEMBER" or .author_association == "COLLABORATOR") or
         (.user.login == "chatgpt-codex-connector[bot]" and .user.type == "Bot"));
      def visible_body: .body | gsub("(?s:<!--.*?(?:-->|$))"; "") | gsub("(?ms)^[ ]{0,3}(```|~~~)[^\\n]*\\n.*?(^[ ]{0,3}\\1[ \\t]*$|\\z)"; "");
      def substantive:
        gsub("^[[:space:]]+|[[:space:]]+$"; "") as $text |
        ($text | length >= 12) and
        ($text | test("^(n/?a|none|tbd|todo|pending|unknown|not applicable)[[:space:].,:;!?-]*$"; "i") | not);
      def has_substantive_line($label):
        [try (capture("(?im)^#{0,6} *(?:" + $label + ")[ ]*:[ ]*(?<value>.+)$").value | select(substantive)) catch empty] | length > 0;
      def focused: (.body | type == "string") and
        (visible_body | test("(?im)^#{0,6} *security review: *" + $head + " *$")) and
        (visible_body | test("(?im)^result: *accepted *$")) and
        (visible_body | has_substantive_line("reviewed +(dsc|credential|certificate|keystore|secret)([ ]+[A-Za-z][A-Za-z-]*)?")) and
        (visible_body | has_substantive_line("security rationale|security reasoning"));
      ([($reviews | source_records | records)[] | select(reviewer and focused and .commit_id == $head and
         (.state == "APPROVED" or .state == "COMMENTED"))] +
       [($comments | source_records | records)[] | select(reviewer and focused)]) | length > 0
    ' 2>/dev/null) || security_review=false
  fi
  if [ "$security_review" != "true" ]; then
    unknown "credential-sensitive change lacks a separate current-head security-focused reviewer comment"
  else
    say "ok" "separate security-focused reviewer comment names full current head $short"
  fi
fi
}
if [ "$workflow_change" = "true" ]; then
  if ! body_section_has_content "$prbody" 'rollback notes|rollback procedure|migration/sync compatibility and rollback procedure'; then
    bad "workflow change lacks non-empty rollback notes"
  fi
  if ! body_section_has_content "$prbody" 'migration compatibility|migration impact|migration/sync compatibility'; then
    bad "workflow change lacks non-empty migration compatibility notes"
  fi
fi
if [ "$native_frontend_change" = "true" ] && ! body_section_has_content "$prbody" 'migration compatibility|migration impact|migration/sync compatibility'; then
  bad "native or frontend change lacks non-empty migration compatibility notes"
fi
if [ "$migration_change" = "true" ] && ! body_section_has_content "$prbody" 'rollback notes|rollback procedure|migration/sync compatibility and rollback procedure'; then
  bad "database migration path lacks non-empty rollback notes"
fi
if [ "$implementation_code_added" = "true" ] && ! body_has_p4_answers "$prbody"; then
  bad "implementation code addition lacks all three substantive P4 reuse, deletion, and omission answers"
fi
if [ "$dependency_manifest_added" = "true" ] && ! body_section_has_content "$prbody" 'dependency justification|dependency rationale|new dependency justification'; then
  bad "dependency manifest addition lacks a substantive dependency justification"
fi
if [ "$platform_sensitive_change" = "true" ]; then
  if ! body_has_platform_evidence "$prbody" windows; then
    bad "platform-sensitive change lacks substantive Windows validation evidence or unaffected-host rationale"
  fi
  if ! body_has_platform_evidence "$prbody" macos; then
    bad "platform-sensitive change lacks substantive macOS validation evidence or unaffected-host rationale"
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
  # Parse Git's C-style quoted paths with the existing Python runtime. The
  # shell receives only JSON/TSV data after the parser has matched every diff
  # section, so non-ASCII destinations retain their REST filename identity.
  diff_stats="$tmpdir/diff-stats.tsv"
  added_payload="$tmpdir/added-payload"
  parsed_diff_status=0
  parsed_diff=$(python3 scripts/merge_gate_diff.py <<<"$diff") || parsed_diff_status=$?
  if [ "$parsed_diff_status" -ne 0 ] || ! jq -e '
    type == "object" and
    (.records | type == "array" and all(.[]; type == "object" and
      (.destination | type == "string" and length > 0) and
      ((.textual_destination == null) or (.textual_destination | type == "string" and length > 0)) and
      (.added | type == "number" and floor == . and . >= 0) and
      (.deleted | type == "number" and floor == . and . >= 0) and
      (.binary | type == "boolean") and (.gitlink | type == "boolean"))) and
    (.added_payload | type == "array" and all(.[]; type == "string"))
  ' <<<"$parsed_diff" >/dev/null 2>&1; then
    unknown "could not parse diff sections for the privacy scan"
    : >"$diff_stats"
    : >"$added_payload"
  else
    jq -r '.records[] | [.destination, .added, .deleted, (if .textual_destination == null then 0 else 1 end), (if .binary then 1 else 0 end), (if .gitlink then 1 else 0 end)] | @tsv' <<<"$parsed_diff" >"$diff_stats"
    jq -r '.added_payload[]' <<<"$parsed_diff" >"$added_payload"


  coverage_count=0
  coverage_examples=""
  metadata_only_count=0
  metadata_only_examples=""
  binary_count=0
  gitlink_count=0
  bounded_coverage_name() {
    local item
    item=$(sed -E 's#/(Users|home)/[^/[:space:]]+#<home>#g; s#[A-Za-z]:[\\/]+Users[\\/]+[^\\/[:space:]]+#<home>#g' <<<"$1")
    printf '%s' "${item:0:160}"
  }
  record_coverage_issue() {
    coverage_count=$((coverage_count + 1))
    if [ "$coverage_count" -le 8 ]; then
      local item
      item=$(bounded_coverage_name "$1")
      coverage_examples="${coverage_examples}${coverage_examples:+; }$item"
    fi
  }
  record_metadata_only() {
    metadata_only_count=$((metadata_only_count + 1))
    if [ "$metadata_only_count" -le 8 ]; then
      metadata_only_examples="${metadata_only_examples}${metadata_only_examples:+; }$(bounded_coverage_name "$1")"
    fi
  }
  path_text=""
  if [ -s "$changed_records" ]; then
    while IFS=$'\t' read -r filename status rest_added rest_deleted previous_filename; do
      [ "$status" = "removed" ] && continue
      match_count=$(awk -F '\t' -v filename="$filename" '$1 == filename { count++ } END { print count+0 }' "$diff_stats")
      if [ "$match_count" -ne 1 ]; then
        record_coverage_issue "omits or duplicates '$filename'"
        continue
      fi
      diff_record=$(awk -F '\t' -v filename="$filename" '$1 == filename { print; exit }' "$diff_stats")
      IFS=$'\t' read -r _ diff_added diff_deleted textual binary gitlink <<<"$diff_record"
      if [ "$gitlink" -eq 1 ]; then
        gitlink_count=$((gitlink_count + 1))
      fi
      if [ "$binary" -eq 1 ]; then
        # Its bytes cannot be reconciled through textual hunks.
        binary_count=$((binary_count + 1))
        continue
      fi
      if [ "$textual" -ne 1 ] && [ "$diff_added" -eq 0 ] && [ "$diff_deleted" -eq 0 ]; then
        if [ "$rest_added" -eq 0 ] && [ "$rest_deleted" -eq 0 ]; then
          record_metadata_only "$filename"
        else
          record_coverage_issue "metadata-only '$filename' conflicts with REST line totals"
        fi
      elif [ "$textual" -ne 1 ]; then
        record_coverage_issue "lacks a textual destination for '$filename'"
      elif [ "$diff_added" -ne "$rest_added" ] || [ "$diff_deleted" -ne "$rest_deleted" ]; then
        record_coverage_issue "line totals for '$filename' differ from REST metadata"
      fi
    done <"$changed_records"
    # This option is an operator statement, never evidence inferred from the
    # PR body: it attests that every current binary addition/change byte and
    # its ownership, license, and NOTICE obligations were independently read.
    if [ "$binary_count" -gt 0 ]; then
      if [ "$BINARY_REVIEW_SHA" = "$head" ] && [ "$INDEPENDENT_REVIEW_SHA" = "$head" ]; then
        say "ok" "$binary_count binary addition/change(s) have explicit current-head binary and independent review attestations"
      else
        bad "$binary_count binary addition/change(s) require matching --binary-review-sha and --independent-review-sha human attestations"
      fi
    fi
    [ "$gitlink_count" -eq 0 ] || unknown "$gitlink_count gitlink change(s) require explicit provenance, license, and NOTICE review"
    if [ "$coverage_count" -gt 0 ]; then
      unknown "privacy diff coverage failed for $coverage_count non-removed REST file(s): $coverage_examples"
    fi
    if [ "$metadata_only_count" -gt 0 ]; then
      say "note" "$metadata_only_count metadata-only diff section(s) have REST 0/0 totals: $metadata_only_examples"
    fi
    # Destination paths are scan input from the complete REST set, not only
    # from whatever textual patch GitHub happened to render. Removed paths
    # carry no newly added material and are deliberately excluded.
    path_text=$(awk -F '\t' '$2 != "removed" { print $1 }' "$changed_records")
  fi
  scan_input_file="$tmpdir/privacy-scan-input"
  {
    printf '%s\n%s\n' "$privacy_metadata" "$path_text"
    cat "$added_payload"
  } >"$scan_input_file"
  privacy_result_status=0
  privacy_result=$(python3 "$script_dir/merge_gate_privacy.py" --head "$head" <"$scan_input_file") || privacy_result_status=$?
  if [ "$privacy_result_status" -ne 0 ] || ! jq -e '
    type == "object" and
    (.blockers | type == "array" and all(.[]; type == "string" and length > 0)) and
    (.indeterminate | type == "array" and all(.[]; type == "string" and length > 0)) and
    (.notes | type == "array" and all(.[]; type == "string" and length > 0))
  ' <<<"$privacy_result" >/dev/null 2>&1; then
    unknown "privacy classifier failed or returned malformed output"
  else
    while IFS= read -r message; do bad "$message"; done < <(jq -r '.blockers[]' <<<"$privacy_result")
    while IFS= read -r message; do unknown "$message"; done < <(jq -r '.indeterminate[]' <<<"$privacy_result")
    while IFS= read -r message; do say "note" "$message"; done < <(jq -r '.notes[]' <<<"$privacy_result")
  fi
  fi
fi

# Fetch mutable review evidence only in this late phase, immediately before the
# final PR/base identity fence. A prior snapshot can be invalidated by a later
# provider summary, security comment, or review thread without moving the head.
: >"$errfile"
review_status=0
provider_review=""
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
fi

: >"$errfile"
comment_status=0
comments=$(gh api --paginate --slurp "repos/$REPO/issues/$PR/comments" 2>"$errfile") || comment_status=$?
summary_good=0
if [ "$comment_status" -ne 0 ]; then
  unknown "could not read provider review summaries"
elif ! jq -e 'type == "array" and (all(.[]; type == "array") or all(.[]; type == "object"))' <<<"$comments" >/dev/null 2>&1; then
  unknown "provider review-summary response was malformed"
else
  summary_rows=$(jq -r '
    (if all(.[]; type == "array") then flatten else . end)[] |
    select(.user.login == "chatgpt-codex-connector[bot]" and .user.type == "Bot") |
    select((.body // "") | contains("codex-pull-request-review-summary")) |
    .body
  ' <<<"$comments" | grep -E '^\| (📝|🔍)' | tail -1)
  if [ -z "$summary_rows" ]; then
    bad "no provider review summary row"
  elif ! grep -Fq "\`$short\`" <<<"$summary_rows"; then
    bad "latest provider summary names a different head than $short"
  elif ! grep -Fq 'Completed' <<<"$summary_rows"; then
    bad "provider review run for $short is not completed"
  else
    summary_good=1
    say "ok" "provider review summary is completed on $short"
  fi
fi

if [ "$provider_review" = "matched" ] && [ "$summary_good" -eq 1 ]; then
  say "ok" "provider review records the full current head $short"
elif [ "$provider_review" != "matched" ] && [ "$summary_good" -eq 1 ]; then
  if [ -n "$INDEPENDENT_REVIEW_SHA" ] && [ "$INDEPENDENT_REVIEW_SHA" = "$head" ]; then
    say "ok" "manual independent review attestation names full current head $short"
  elif [ -n "$INDEPENDENT_REVIEW_SHA" ]; then
    bad "manual independent review attestation does not match the current head"
  else
    unknown "provider summary exposes only an abbreviated head; pass --independent-review-sha with an exact manual review attestation"
  fi
fi

# Paginate the final thread set; partial, changed, duplicate, or unresolved
# results remain indeterminate/blocking rather than inheriting an earlier read.
cursor=""
open_threads=0
total_threads=-1
fetched_threads=0
thread_ids="$tmpdir/final-review-thread-ids"
: >"$thread_ids"
thread_ok=1
while :; do
  : >"$errfile"
  page_status=0
  if [ -n "$cursor" ]; then
    page=$(gh api graphql -f owner="$OWNER" -f name="$NAME" -F pr="$PR" -f cursor="$cursor" -f query='
    query($owner:String!,$name:String!,$pr:Int!,$cursor:String){
      repository(owner:$owner,name:$name){
        pullRequest(number:$pr){ reviewThreads(first:100,after:$cursor){
          totalCount pageInfo{hasNextPage endCursor} nodes{id isResolved}
        }}
      }
    }' 2>"$errfile") || page_status=$?
  else
    # GraphQL treats an omitted nullable variable as null. Sending an empty
    # string is not the first page and is rejected by the connection API.
    page=$(gh api graphql -f owner="$OWNER" -f name="$NAME" -F pr="$PR" -f query='
    query($owner:String!,$name:String!,$pr:Int!,$cursor:String){
      repository(owner:$owner,name:$name){
        pullRequest(number:$pr){ reviewThreads(first:100,after:$cursor){
          totalCount pageInfo{hasNextPage endCursor} nodes{id isResolved}
        }}
      }
    }' 2>"$errfile") || page_status=$?
  fi
  if [ "$page_status" -ne 0 ] || ! jq -e '.data.repository.pullRequest.reviewThreads | type == "object" and (.totalCount | type == "number" and floor == . and . >= 0) and (.pageInfo.hasNextPage | type == "boolean") and (.nodes | type == "array" and all(.[]; (.id | type == "string" and length > 0) and (.isResolved | type == "boolean")))' <<<"$page" >/dev/null 2>&1; then
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
  jq -r '.data.repository.pullRequest.reviewThreads.nodes[].id' <<<"$page" >>"$thread_ids"
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
  unique_thread_count=$(sort -u "$thread_ids" | wc -l | tr -d ' ')
  if [ "$unique_thread_count" -ne "$fetched_threads" ]; then
    unknown "review-thread pagination repeated thread IDs"
  elif [ "$fetched_threads" -ne "$total_threads" ]; then
    unknown "review-thread pagination returned $fetched_threads of $total_threads nodes"
  elif [ "$open_threads" -eq 0 ]; then
    say "ok" "0 of $total_threads review threads unresolved"
  else
    bad "$open_threads of $total_threads review threads unresolved"
  fi
fi
validate_security_reviewer

# Re-read all moving PR and base identities immediately before emitting a merge command.
: >"$errfile"
final_meta_status=0
final_meta=$(gh pr view "$PR" --repo "$REPO" \
  --json headRefOid,baseRefOid,baseRefName,mergeable,mergeStateStatus,isDraft,state,title,body,changedFiles 2>"$errfile") || final_meta_status=$?
if [ "$final_meta_status" -ne 0 ] || ! jq -e 'type == "object" and (.headRefOid | type == "string" and test("^[0-9a-fA-F]{40}$")) and (.baseRefOid | type == "string" and test("^[0-9a-fA-F]{40}$")) and (.baseRefName | type == "string") and (.mergeable | type == "string") and (.mergeStateStatus | type == "string") and (.isDraft | type == "boolean") and (.state | type == "string") and (.title | type == "string") and (.changedFiles | type == "number" and floor == . and . >= 0)' <<<"$final_meta" >/dev/null 2>&1; then
  unknown "could not revalidate PR head and base before merge"
else
  final_head=$(jq -r '.headRefOid' <<<"$final_meta")
  final_base_ref_oid=$(jq -r '.baseRefOid' <<<"$final_meta")
  final_base=$(jq -r '.baseRefName' <<<"$final_meta")
  final_mergeable=$(jq -r '.mergeable' <<<"$final_meta")
  final_state=$(jq -r '.mergeStateStatus' <<<"$final_meta")
  final_draft=$(jq -r '.isDraft' <<<"$final_meta")
  final_pstate=$(jq -r '.state' <<<"$final_meta")
  final_title=$(jq -r '.title' <<<"$final_meta")
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
    BEHIND|DIRTY|UNKNOWN|BLOCKED|UNSTABLE|DRAFT) bad "PR merge state changed to $final_state during preflight" ;;
    *) unknown "PR merge state changed to unrecognised value '$final_state' during preflight" ;;
  esac
  raw_final_body=$(jq -r '.body // ""' <<<"$final_meta")
  final_body="$raw_final_body"
  final_visible_status=0
  final_body=$(python3 -c 'import re, sys
text = sys.stdin.read()
print(re.sub(r"<!--.*?(?:-->|\Z)", "", text, flags=re.S), end="")' <<<"$final_body") || final_visible_status=$?
  [ "$final_visible_status" -eq 0 ] || unknown "could not extract final visible PR description content"
  [ "$final_title" = "$title" ] || bad "PR title changed during preflight"
  if [ "$checklist_status" -eq 0 ] && ! checklist_link_ok "$final_body" "$review_checklist"; then
    bad "PR description changed and no longer carries a completed same-repository line-specific checklist link"
  fi
  if [ "$raw_final_body" != "$raw_prbody" ]; then
    bad "PR description changed during preflight; re-run metadata privacy scan"
    if ! body_section_has_content "$final_body" 'functional summary|outcome and reason'; then
      bad "PR description changed and no longer carries a non-empty functional summary"
    fi
    if ! body_section_has_content "$final_body" 'test or reproduction command|commands and results|validation and evidence'; then
      bad "PR description changed and no longer carries test or reproduction evidence"
    fi
    if ! body_has_validation_command "$final_body"; then
      bad "PR description changed and no longer carries an actual test or reproduction command"
    fi
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

# Check runs and legacy statuses can change while the late review/thread phase
# is in progress without moving the PR head. Re-read both complete, head-bound
# feeds immediately before the final identity fence; a rerun that is pending or
# failed must not inherit the earlier successful snapshot.
: >"$errfile"
final_check_runs_status=0
final_check_runs=$(gh api --paginate --slurp "repos/$REPO/commits/$head/check-runs?per_page=100" 2>"$errfile") || final_check_runs_status=$?
if [ "$final_check_runs_status" -ne 0 ] || ! jq -e --arg head "$head" '
  type == "array" and length > 0 and
  all(.[]; type == "object" and
    (.total_count | type == "number" and floor == . and . >= 0) and
    (.check_runs | type == "array" and all(.[];
      type == "object" and
      (.id | type == "number" and floor == . and . >= 0) and
      (.name | type == "string" and length > 0) and
      (.status == "completed") and
      (.conclusion | type == "string" and (. == "success" or . == "skipped" or . == "neutral" or . == "failure" or . == "cancelled" or . == "timed_out" or . == "action_required" or . == "stale")) and
      (.head_sha | type == "string" and test("^[0-9a-fA-F]{40}$") and . == $head)
    ))) and
  ((map(.total_count) | unique | length) == 1) and
  ((map(.check_runs | length) | add) == .[0].total_count) and
  ((map(.check_runs) | add | map(.id) | unique | length) == .[0].total_count)
' <<<"$final_check_runs" >/dev/null 2>&1; then
  unknown "could not revalidate final head-bound check-run evidence"
else
  final_check_run_skips="$tmpdir/final-check-run-skips.b64"
  final_skipped_ci_policy_ok=true
  if ! jq -r '.[] | .check_runs[] | select(.conclusion == "skipped") | .name | @base64' <<<"$final_check_runs" >"$final_check_run_skips"; then
    final_skipped_ci_policy_ok=false
    unknown "could not read skipped check names from final refreshed check-run evidence"
  else
    validate_skipped_ci_jobs "final refreshed check-run evidence" "$final_check_run_skips" final_skipped_ci_policy_ok
  fi
  final_failed_runs_status=0
  final_failed_runs=$(jq --rawfile contexts "$tmpdir/required-contexts" '
    ($contexts | split("\n")) as $required |
    [.[] | .check_runs[] | . as $run |
      select((.conclusion != "success" and .conclusion != "neutral" and .conclusion != "skipped") or
             (.conclusion != "success" and ($required | index($run.name)) != null))] | length
  ' <(printf '%s' "$final_check_runs")) || final_failed_runs_status=$?
  if [ "$final_failed_runs_status" -ne 0 ] || ! [[ "$final_failed_runs" =~ ^[0-9]+$ ]]; then
    unknown "could not evaluate final check-run conclusions"
  elif [ "$final_failed_runs" -gt 0 ]; then
    bad "$final_failed_runs final check run(s) are failed or required-but-not-successful"
  elif [ "$final_skipped_ci_policy_ok" != true ]; then
    :
  else
    say "ok" "final check-run pages are successful and bound to head $short"
  fi
fi

: >"$errfile"
final_statuses_status=0
final_statuses=$(gh api --paginate --slurp "repos/$REPO/commits/$head/status?per_page=100" 2>"$errfile") || final_statuses_status=$?
if [ "$final_statuses_status" -ne 0 ] || ! jq -e --arg head "$head" '
  type == "array" and length > 0 and
  all(.[]; type == "object" and
    (.sha | type == "string" and test("^[0-9a-fA-F]{40}$") and . == $head) and
    (.total_count | type == "number" and floor == . and . >= 0) and
    (.state | type == "string" and (. == "success" or . == "pending" or . == "failure" or . == "error")) and
    (.statuses | type == "array" and all(.[];
      type == "object" and
      (.id | type == "number" and floor == . and . >= 0) and
      (.context | type == "string" and length > 0) and
      (.state == "success" or .state == "failure" or .state == "error")
    ))
  ) and
  ((map(.total_count) | unique | length) == 1) and
  ((map(.statuses | length) | add) == .[0].total_count) and
  ((map(.statuses) | add | map(.id) | unique | length) == .[0].total_count) and
  ((map(.statuses) | add | map(.context) | unique | length) == .[0].total_count) and
  (if .[0].state == "pending" then .[0].total_count == 0 and (map(.statuses | length) | add) == 0
   elif .[0].state == "failure" or .[0].state == "error" then .[0].state as $aggregate | all(.[]; .state == $aggregate)
   else all(.[]; .state == "success") end)
' <<<"$final_statuses" >/dev/null 2>&1; then
  unknown "could not revalidate final head-bound commit-status evidence"
elif jq -e 'any(.[]; (.state == "failure" or .state == "error") or any(.statuses[]; .state == "failure" or .state == "error"))' <<<"$final_statuses" >/dev/null; then
  bad "final combined commit-status evidence reports a failure"
else
  say "ok" "final commit-status pages are bound to head $short"
fi

# The last check/status snapshot is useful only while it still names this PR
# and base. A head/base move after it invalidates the entire preflight.
: >"$errfile"
late_meta_status=0
late_meta=$(gh pr view "$PR" --repo "$REPO" --json headRefOid,baseRefOid,baseRefName 2>"$errfile") || late_meta_status=$?
if [ "$late_meta_status" -ne 0 ] || ! jq -e 'type == "object" and (.headRefOid | type == "string" and test("^[0-9a-fA-F]{40}$")) and (.baseRefOid | type == "string" and test("^[0-9a-fA-F]{40}$")) and (.baseRefName | type == "string")' <<<"$late_meta" >/dev/null 2>&1; then
  unknown "could not revalidate PR identity after final CI evidence"
else
  [ "$(jq -r '.headRefOid' <<<"$late_meta")" = "$head" ] || bad "PR head moved after final CI evidence"
  [ "$(jq -r '.baseRefOid' <<<"$late_meta")" = "$base_ref_oid" ] || bad "PR base OID moved after final CI evidence"
  [ "$(jq -r '.baseRefName' <<<"$late_meta")" = "$base" ] || bad "PR base moved after final CI evidence"
fi

# The branch endpoint is independent of the PR metadata endpoint.  Re-read it
# after the final check/status snapshot and identity fence so a base advance in
# that last interval cannot produce a merge command for an unreviewed tree.
if [ -n "$base_tip" ]; then
  : >"$errfile"
  final_fenced_base_tip_status=0
  final_fenced_base_tip=$(gh api "repos/$REPO/branches/$base" --jq '.commit.sha' 2>"$errfile") || final_fenced_base_tip_status=$?
  if [ "$final_fenced_base_tip_status" -ne 0 ] || ! [[ "$final_fenced_base_tip" =~ ^[0-9a-fA-F]{40}$ ]]; then
    unknown "could not revalidate base tip after final CI evidence"
  elif [ "$final_fenced_base_tip" != "$base_tip" ]; then
    bad "base tip moved after final CI evidence"
  fi
fi

echo
if [ "$fail" -ne 0 ]; then
  echo "MUST NOT MERGE"
  exit 1
fi
if [ "$uncertain" -ne 0 ]; then
  echo "INDETERMINATE — do not merge until the missing evidence is obtained"
  exit 2
fi

echo "MAY MERGE — bind the merge to the reviewed head and validated base:"
printf '  [ "$(gh pr view %s --repo %s --json baseRefName -q .baseRefName)" = "%s" ] \\\n    && gh pr merge %s --repo %s --squash --match-head-commit %s\n' \
  "$PR" "$REPO" "$base" "$PR" "$REPO" "$head"
exit 0
