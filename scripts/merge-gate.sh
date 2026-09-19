#!/usr/bin/env bash
# Validate a pull request's compatibility-surface reseal, run the privacy/PII
# scan, and confirm review evidence names the current head SHA.
#
# Usage: scripts/merge-gate.sh <pr-number> [--repo OWNER/NAME]
#        [--independent-review-sha FULL_SHA] (manual attestation naming the head)
#        [--binary-review-sha FULL_SHA] (manual binary-byte, ownership, license, and NOTICE review)
# Exit:  0 may merge, 1 must not, 2 could not determine.
#
# This script intentionally does not re-derive PR/head/base identity binding,
# branch-protection required-check contexts, or a checks rollup: those are
# GitHub's own job, enforced natively by required status checks on master
# (see docs/proposed-merge-gate-ci.md). Every check here is bound to one
# server-observed head SHA; unknown or incomplete evidence is never
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
      sed -n '2,15p' "$0"
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
: >"$errfile"
if ! meta=$(gh pr view "$PR" --repo "$REPO" \
        --json headRefOid,baseRefName,title,body,changedFiles 2>"$errfile"); then
  die "could not read PR #$PR in $REPO"
fi
if ! jq -e '
  type == "object" and
  (.headRefOid | type == "string" and test("^[0-9a-fA-F]{40}$")) and
  (.baseRefName | type == "string" and length > 0) and
  (.title | type == "string") and
  (.changedFiles | type == "number" and floor == . and . >= 0)
' <<<"$meta" >/dev/null 2>&1; then
  die "PR metadata was not a valid complete JSON object"
fi
head=$(jq -r '.headRefOid' <<<"$meta")
base=$(jq -r '.baseRefName' <<<"$meta")
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
echo "PR #$PR ($REPO)  head=$short  base=$base"

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
      (.name | type == "string") and (.email | type == "string" and test("^[A-Za-z0-9.!#$%&*+/=?^_`{|}~-]+@[A-Za-z0-9]([A-Za-z0-9.-]*[A-Za-z0-9])?$"))) and
    (.commit.committer | type == "object" and
      (.name | type == "string") and (.email | type == "string" and test("^[A-Za-z0-9.!#$%&*+/=?^_`{|}~-]+@[A-Za-z0-9]([A-Za-z0-9.-]*[A-Za-z0-9])?$"))) and
    ((.author == null) or (.author | type == "object" and (.login | type == "string"))) and
    ((.committer == null) or (.committer | type == "object" and (.login | type == "string")))))
' <<<"$metadata_commits" >/dev/null 2>&1; then
  unknown "could not prove complete head-bound PR commit metadata for the privacy scan"
  privacy_metadata=""
else
  # Author and committer emails were structurally validated above and are an
  # explicit identity-only source class. Do not mix them into payload/path/
  # metadata scan input, where an identical address would be customer data.
  # Commit messages are their own source class. Before scanning them, redact
  # only the one known public-agent address when it occupies an exact
  # terminal Co-Authored-By Git trailer. The helper leaves every contributor
  # name, other trailer, PR field, destination, and payload untouched.
  commit_message_json=$(jq -c '(if all(.[]; type == "array") then flatten else . end) | map(.commit.message)' <<<"$metadata_commits")
  sanitized_messages_status=0
  sanitized_message_json=$(printf '%s' "$commit_message_json" | python3 "$script_dir/merge_gate_privacy.py" --redact-public-agent-attribution-messages) || sanitized_messages_status=$?
  if [ "$sanitized_messages_status" -ne 0 ] || ! jq -e 'type == "array" and all(.[]; type == "string")' <<<"$sanitized_message_json" >/dev/null 2>&1; then
    unknown "could not classify public-agent attribution trailers in complete head-bound commit metadata"
    privacy_metadata=""
  else
    commit_messages=$(jq -r '.[]' <<<"$sanitized_message_json")
    commit_identity_names=$(jq -r '(if all(.[]; type == "array") then flatten else . end)[] |
      [.commit.author.name, .commit.committer.name, (.author.login? // null), (.committer.login? // null)] |
      map(select(. != null))[]' <<<"$metadata_commits")
    privacy_metadata="$title
$raw_prbody
$commit_messages
$commit_identity_names"
  fi
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
  parsed_diff=$(python3 "$script_dir/merge_gate_diff.py" <<<"$diff") || parsed_diff_status=$?
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
    # The privacy classifier's own identifier-shape detection also runs on
    # the (already home-dir-redacted) filename: a coverage/metadata-only
    # example is echoed verbatim into gate output, and a filename can itself
    # carry an identifier, digest, or email shape.
    item=$(printf '%s' "$item" | python3 "$script_dir/merge_gate_privacy.py" --redact-shapes 2>/dev/null) || item="$item"
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
      if [ "$status" = "removed" ]; then
        # A removed record has no new content, so it is excluded from the
        # metadata-only/textual-destination checks below (its +++ destination
        # is legitimately /dev/null). It must still be reconciled first: a
        # removed record absent from the diff was already caught by the
        # match_count check above, and its line totals must still agree with
        # REST before it is excluded from further scanning.
        if [ "$diff_added" -ne "$rest_added" ] || [ "$diff_deleted" -ne "$rest_deleted" ]; then
          record_coverage_issue "line totals for '$filename' differ from REST metadata"
        fi
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
    # Reconciliation above is REST-to-diff only: it proves every REST record
    # has exactly one matching diff section, but says nothing about a diff
    # destination that has no REST record at all. Check the reverse direction
    # too, so a diff section smuggled in outside the REST changed-file set
    # cannot silently skip the privacy scan's REST-driven bookkeeping.
    extra_diff_destinations=$(comm -13 <(cut -f1 "$changed_records" | sort -u) <(cut -f1 "$diff_stats" | sort -u))
    if [ -n "$extra_diff_destinations" ]; then
      while IFS= read -r extra; do
        record_coverage_issue "diff destination '$extra' has no corresponding REST record"
      done <<<"$extra_diff_destinations"
    fi
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
  scan_input_write_status=0
  {
    printf '%s\n%s\n' "$privacy_metadata" "$path_text"
    cat "$added_payload"
  } >"$scan_input_file" || scan_input_write_status=$?
  if [ "$scan_input_write_status" -ne 0 ]; then
    unknown "could not assemble complete privacy scan input"
  else
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
fi
# A review naming the exact current head SHA. Closes #317: PRs were merged
# 2-8 minutes after opening, where "zero unresolved review threads" meant
# "the reviewer had not started", not "reviewed clean". A clean review can
# leave no review object at all -- only a reaction plus a summary comment --
# so the absence of any review artifact naming this exact head is a FAIL,
# never a silent pass.
: >"$errfile"
review_evidence_status=0
head_reviews=$(gh api --paginate --slurp "repos/$REPO/pulls/$PR/reviews" 2>"$errfile") || review_evidence_status=$?
review_names_head=false
if [ "$review_evidence_status" -eq 0 ] && jq -e '
  type == "array" and (all(.[]; type == "array") or all(.[]; type == "object"))
' <<<"$head_reviews" >/dev/null 2>&1; then
  review_names_head=$(jq -r --arg head "$head" '
    (if all(.[]; type == "array") then flatten else . end) |
    any(.[]; .commit_id == $head)
  ' <<<"$head_reviews")
fi
comment_names_head=false
if [ "$review_names_head" != "true" ]; then
  : >"$errfile"
  head_comments_status=0
  head_comments=$(gh api --paginate --slurp "repos/$REPO/issues/$PR/comments" 2>"$errfile") || head_comments_status=$?
  if [ "$head_comments_status" -eq 0 ] && jq -e '
    type == "array" and (all(.[]; type == "array") or all(.[]; type == "object"))
  ' <<<"$head_comments" >/dev/null 2>&1; then
    short_marker="\`${short}\`"
    comment_names_head=$(jq -r --arg head "$head" --arg marker "$short_marker" '
      (if all(.[]; type == "array") then flatten else . end) |
      any(.[]; ((.body // "") | contains($head)) or ((.body // "") | contains($marker)))
    ' <<<"$head_comments")
  fi
fi
if [ "$review_names_head" = "true" ] || [ "$comment_names_head" = "true" ]; then
  say "ok" "review evidence names the current head $short"
else
  bad "no review evidence (review or comment) names current head $short — see #317"
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

echo "MAY MERGE — compatibility-surface, privacy, and head-SHA review-evidence checks passed for $short."
echo "Branch protection on $base enforces the remaining required status checks separately."
printf '  [ "$(gh pr view %s --repo %s --json baseRefName -q .baseRefName)" = "%s" ] \\\n    && gh pr merge %s --repo %s --squash --match-head-commit %s\n' \
  "$PR" "$REPO" "$base" "$PR" "$REPO" "$head"
exit 0
