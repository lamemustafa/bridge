#!/usr/bin/env bash
# Decide whether a pull request may be merged, and say why not when it may not.
#
# Every rule here exists because it failed. On 2026-09-12 four PRs were merged
# before their reviews arrived — one by three seconds — leaving eleven findings,
# four of them P1, on code already in master. The gate added to stop that then
# deadlocked a clean PR, because a Codex pass with no findings submits no review
# object at all. Later the same day two sessions reported a PR "clean, zero open
# threads" from a query issued seconds before its review posted.
#
# The rule underneath all of it: a review is only evidence about the commit it
# names. "No findings yet" and "not looked yet" are indistinguishable unless you
# check which commit was looked at.
#
# Usage: scripts/merge-gate.sh <pr-number> [--repo OWNER/NAME]
# Exit:  0 may merge, 1 must not, 2 could not determine.

set -uo pipefail

PR=""; REPO=""
while [ $# -gt 0 ]; do
  case "$1" in
    --repo) REPO="${2:-}"; [ -n "$REPO" ] || { echo "--repo needs OWNER/NAME" >&2; exit 2; }; shift 2 ;;
    --repo=*) REPO="${1#--repo=}"; shift ;;
    -h|--help) sed -n '2,20p' "$0"; exit 0 ;;
    -*) echo "unknown option: $1" >&2; exit 2 ;;
    *) [ -z "$PR" ] && PR="$1" || { echo "unexpected argument: $1" >&2; exit 2; }; shift ;;
  esac
done
[ -n "$PR" ] || { echo "usage: $0 <pr-number> [--repo OWNER/NAME]" >&2; exit 2; }

# Resolve the repository ONCE, explicitly. Never fall back to a different
# repository on a transient failure: a same-numbered PR elsewhere with no open
# threads would read as a clean result for the PR actually being gated.
if [ -z "$REPO" ]; then
  REPO=$(gh repo view --json nameWithOwner -q .nameWithOwner 2>/dev/null) \
    || { echo "could not determine repository; pass --repo OWNER/NAME" >&2; exit 2; }
fi
OWNER="${REPO%%/*}"; NAME="${REPO##*/}"
[ -n "$OWNER" ] && [ -n "$NAME" ] && [ "$OWNER" != "$REPO" ] \
  || { echo "--repo must be OWNER/NAME, got '$REPO'" >&2; exit 2; }

fail=0
say()  { printf '  %-6s %s\n' "$1" "$2"; }
bad()  { say "BLOCK" "$1"; fail=1; }
good() { say "ok"    "$1"; }
die()  { echo "$1" >&2; exit 2; }

meta=$(gh pr view "$PR" --repo "$REPO" \
        --json headRefOid,baseRefName,mergeable,mergeStateStatus,isDraft,state 2>/dev/null) \
  || die "could not read PR #$PR in $REPO"
head=$(jq -r .headRefOid <<<"$meta")
base=$(jq -r .baseRefName <<<"$meta")
mergeable=$(jq -r .mergeable <<<"$meta")
mstate=$(jq -r .mergeStateStatus <<<"$meta")
draft=$(jq -r .isDraft <<<"$meta")
pstate=$(jq -r .state <<<"$meta")
short=${head:0:7}

echo "PR #$PR ($REPO)  head=$short  base=$base  $mergeable/$mstate"

[ "$pstate" = "OPEN" ] || bad "PR is $pstate, not OPEN"
[ "$draft" = "false" ] || bad "draft"

# 1. Mergeable, and not merely "no conflicts". BEHIND means the head has not
#    seen the current base, so its CI and its review describe a tree that no
#    longer exists — the branch protection is strict and would refuse anyway.
case "$mergeable" in
  MERGEABLE) good "no conflicts" ;;
  CONFLICTING) bad "conflicts with $base — rebase first" ;;
  *) bad "mergeability still UNKNOWN; re-run in a moment" ;;
esac
case "$mstate" in
  BEHIND) bad "head is BEHIND $base — its checks and review describe a stale tree; rebase" ;;
  DIRTY)  bad "merge state DIRTY — conflicts" ;;
  UNKNOWN) bad "merge state UNKNOWN; re-run in a moment" ;;
  *) good "merge state $mstate is not stale" ;;
esac

# 2. Based on master. ci.yml fires on pull_request into master ONLY, so a
#    stacked PR runs no CI at all and a green tick on it measures nothing.
if [ "$base" = "master" ]; then
  good "based on master (CI actually runs)"
else
  bad "based on '$base', not master — ci.yml does not fire, so checks here prove nothing"
fi

# 3. Every check concluded and none failed. Read the JSON buckets rather than
#    the human columns: a check name contains spaces, so column-splitting the
#    text output misreads the bucket. gh's buckets are pass/fail/pending/
#    skipping/cancel — note `cancel`, not `cancelled`; matching the longer word
#    left a cancelled check counted as neither failing nor pending, which read
#    as success.
buckets=$(gh pr checks "$PR" --repo "$REPO" --json bucket,name 2>/dev/null)
if [ -z "$buckets" ] || [ "$buckets" = "[]" ]; then
  bad "no checks reported"
else
  pend=$(jq '[.[]|select(.bucket=="pending")]|length' <<<"$buckets")
  bust=$(jq '[.[]|select(.bucket=="fail" or .bucket=="cancel")]|length' <<<"$buckets")
  tot=$(jq 'length' <<<"$buckets")
  [ "$pend" -eq 0 ] || bad "$pend of $tot check(s) still running"
  [ "$bust" -eq 0 ] || bad "$bust of $tot check(s) failed or were cancelled: $(jq -r '[.[]|select(.bucket=="fail" or .bucket=="cancel")|.name]|join(", ")' <<<"$buckets")"
  [ "$pend" -eq 0 ] && [ "$bust" -eq 0 ] && good "all $tot checks concluded, none failing or cancelled"
fi

# 4. A Codex review that names THIS head, and actually completed. The summary
#    comment is edited in place, so its created_at is meaningless — read the
#    commit in its table. A clean pass emits no review object, only this row
#    plus a thumbs-up, which is why the row and not the review list is read.
#    Paginate: the summary is an ordinary issue comment and the default page is
#    30, so on a busy PR it is not on the first one.
body=$(gh api --paginate "repos/$REPO/issues/$PR/comments" \
        --jq '.[] | select(.body|contains("codex-pull-request-review-summary")) | .body' 2>/dev/null)
row=$(grep -E '^\| (📝|🔍)' <<<"$body" | tail -1)
if [ -z "$row" ]; then
  bad "no Codex review summary at all"
elif ! grep -q "$short" <<<"$row"; then
  bad "latest review names a different commit than $short — it has not seen this push"
elif grep -q 'Completed' <<<"$row"; then
  good "review completed on $short"
else
  # Running, Failed, Errored — none of these is a review. A failed review run
  # means nothing looked at the code, which is exactly the state this gate
  # exists to catch; treating it as completed was the original bug inverted.
  st=$(grep -oE 'Running|Failed|Errored|Cancelled' <<<"$row" | head -1)
  bad "review state '${st:-unrecognised}' on $short — only Completed counts"
fi

# 5. No unresolved threads. required_conversation_resolution is on, so this is
#    the gate, not a courtesy. Paginate: a first:100 page once hid 19 threads.
threads=$(gh api graphql -f owner="$OWNER" -f name="$NAME" -F pr="$PR" -f query='
  query($owner:String!,$name:String!,$pr:Int!){
    repository(owner:$owner,name:$name){
      pullRequest(number:$pr){
        reviewThreads(first:100){ totalCount pageInfo{hasNextPage} nodes{isResolved} }}}}' 2>/dev/null)
if [ -z "$threads" ] || [ "$(jq -r '.data.repository.pullRequest' <<<"$threads")" = "null" ]; then
  bad "could not read review threads for $REPO#$PR"
else
  total=$(jq -r '.data.repository.pullRequest.reviewThreads.totalCount' <<<"$threads")
  more=$(jq -r '.data.repository.pullRequest.reviewThreads.pageInfo.hasNextPage' <<<"$threads")
  open=$(jq '[.data.repository.pullRequest.reviewThreads.nodes[]|select(.isResolved==false)]|length' <<<"$threads")
  [ "$more" = "false" ] || bad "more than 100 threads — paginate before trusting this count"
  if [ "$open" -eq 0 ]; then good "0 of $total threads unresolved"; else bad "$open of $total threads unresolved"; fi
fi

# 6. Nothing shaped like client data in what this PR ADDS. Scan the whole merge
#    diff rather than the author's own commits — a real transaction reference
#    sat in a comment on public master through two PRs because each author
#    scanned only what they wrote — but scan ADDED lines only, or the gate
#    blocks the very PR that deletes a leak.
diff=$(gh pr diff "$PR" --repo "$REPO" 2>/dev/null)
if [ -z "$diff" ]; then
  bad "could not read diff for the privacy scan"
else
  # A UUID's last group is twelve hex characters and often all digits; the
  # canonical 550e8400-…-446655440000 tripped this. Strip UUIDs rather than
  # widening the placeholder list, which would start excusing real values.
  # Hex digests are the other machine-generated shape that trips this: a
  # sha256 in a lockfile or a sealed surface manifest contains long digit runs
  # by chance, and dropping \b (see below) made them visible. Strip digests and
  # UUIDs — both are generated, neither can carry a client identifier — rather
  # than loosening the placeholder list, which would start excusing real values.
  added=$(grep '^+' <<<"$diff" | grep -v '^+++' \
          | sed -E 's/[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}/<uuid>/g' \
          | sed -E 's/[0-9a-fA-F]{32,}/<digest>/g')
  # Placeholders match these shapes too — XXXXX1234X is a fabricated PAN and X
  # is an uppercase letter. A gate that cries wolf gets ignored, so obvious
  # placeholders are excluded by an EXPLICIT list; widening the shape itself
  # would start excusing real values. No backreferences: this must be plain
  # ERE, and a grep that errors returns nothing, which reads as a clean scan.
  placeholder='^(X+|Z+|A+)[0-9]+(X|Z|A)?$|^[0-9]{2}(X+|Z+|A+)[0-9]+[0-9A-Z]*$|^(0+|1+|2+|3+|4+|5+|6+|7+|8+|9+)$|^(0?1234567890|1234567890[0-9]*)$'
  printf 'XXXXX1234X\n' | grep -qE "$placeholder" \
    || { bad "privacy-scan pattern failed to compile or match its own probe"; fail=1; }
  # NO \b around the digit run. The leak that motivated this gate was written
  # `HDF CH12345678901` — glued to letters — and \b does not match between `H`
  # and `1`, so the scan that was supposed to catch it could not see it at all.
  hits=$(grep -Eo '[0-9]{2}[A-Z]{5}[0-9]{4}[A-Z][0-9A-Z]{3}|[A-Z]{5}[0-9]{4}[A-Z]|[6-9][0-9]{9}' <<<"$added" \
          | sort -u | { grep -cvE "$placeholder" || true; })
  runs=$(grep -Eo '[0-9]{11,18}' <<<"$added" | sort -u | { grep -cvE "$placeholder" || true; })
  if [ "$hits" -eq 0 ] && [ "$runs" -eq 0 ]; then
    good "added lines carry no identifier shapes and no unexplained long digit runs"
  else
    bad "privacy scan: $hits identifier shape(s), $runs unexplained long digit run(s) in ADDED lines — inspect before merging"
  fi
fi

echo
if [ "$fail" -ne 0 ]; then echo "MUST NOT MERGE"; exit 1; fi
# 7. Bind the merge to the commit that was actually reviewed. Between this
#    check and the merge the head can move, and everything above would then
#    describe a commit the PR no longer points at.
echo "MAY MERGE — bind the merge to the reviewed commit:"
echo "  gh pr merge $PR --repo $REPO --squash --match-head-commit $head"
exit 0
