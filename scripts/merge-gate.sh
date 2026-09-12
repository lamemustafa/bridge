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
blocked_seen=0
errfile=$(mktemp); trap 'rm -f "$errfile"' EXIT
say()  { printf '  %-6s %s\n' "$1" "$2"; }
bad()  { say "BLOCK" "$1"; fail=1; }
good() { say "ok"    "$1"; }
die()  { echo "$1" >&2; exit 2; }

meta=$(gh pr view "$PR" --repo "$REPO" \
        --json headRefOid,baseRefName,mergeable,mergeStateStatus,isDraft,state,body 2>/dev/null) \
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
  BLOCKED)
    # GitHub blocks for reasons this script may not model — a missing required
    # approval, for instance. The checks below usually explain it, but printing
    # `ok` for BLOCKED reads as approval for a state GitHub is refusing, so say
    # what it is and let the specific checks account for it.
    say "note" "merge state BLOCKED — GitHub is refusing; the checks below should say why"
    blocked_seen=1 ;;
  CLEAN|HAS_HOOKS|UNSTABLE) good "merge state $mstate is not stale" ;;
  *) bad "unrecognised merge state '$mstate' — refusing rather than guessing" ;;
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
# `gh pr checks` exits nonzero both when a check is failing and when the query
# itself fails, so the status alone cannot be read as a verdict — but empty
# stdout from a broken query must never be reported as "no checks", which is a
# statement about the PR rather than about the request.
buckets=$(gh pr checks "$PR" --repo "$REPO" --json bucket,name 2>"$errfile")
if [ -z "$buckets" ]; then
  if [ -s "$errfile" ]; then
    bad "could not query checks: $(tr '\n' ' ' <"$errfile" | cut -c1-120)"
  else
    bad "no checks reported for this PR"
  fi
elif [ "$buckets" = "[]" ]; then
  bad "no checks reported for this PR"
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
#    Filter on the AUTHOR as well as the marker. The marker is just text in a
#    comment body, so any PR participant could post one carrying a `Completed`
#    row for the current SHA and the gate would accept it as a review. The
#    summary is posted by the Codex app; require that login and a Bot type.
body=$(gh api --paginate "repos/$REPO/issues/$PR/comments" \
        --jq '.[] | select(.user.login=="chatgpt-codex-connector[bot]" and .user.type=="Bot")
                  | select(.body|contains("codex-pull-request-review-summary")) | .body' 2>/dev/null)
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
#    Actually paginate. Blocking whenever a second page exists made every busy
#    PR permanently unmergeable — and a PR accumulates threads precisely by
#    being reviewed carefully, so the rule punished the PRs it should trust.
cursor=null; open=0; total=0; ok_threads=1
while : ; do
  page=$(gh api graphql -f owner="$OWNER" -f name="$NAME" -F pr="$PR" \
    -f cursor="$([ "$cursor" = "null" ] && echo "" || echo "$cursor")" -f query='
    query($owner:String!,$name:String!,$pr:Int!,$cursor:String){
      repository(owner:$owner,name:$name){
        pullRequest(number:$pr){
          reviewThreads(first:100,after:$cursor){
            totalCount pageInfo{hasNextPage endCursor} nodes{isResolved} }}}}' 2>/dev/null)
  if [ -z "$page" ] || [ "$(jq -r '.data.repository.pullRequest' <<<"$page")" = "null" ]; then
    bad "could not read review threads for $REPO#$PR"; ok_threads=0; break
  fi
  total=$(jq -r '.data.repository.pullRequest.reviewThreads.totalCount' <<<"$page")
  open=$(( open + $(jq '[.data.repository.pullRequest.reviewThreads.nodes[]|select(.isResolved==false)]|length' <<<"$page") ))
  [ "$(jq -r '.data.repository.pullRequest.reviewThreads.pageInfo.hasNextPage' <<<"$page")" = "true" ] || break
  cursor=$(jq -r '.data.repository.pullRequest.reviewThreads.pageInfo.endCursor' <<<"$page")
done
if [ "$ok_threads" -eq 1 ]; then
  if [ "$open" -eq 0 ]; then good "0 of $total threads unresolved"; else bad "$open of $total threads unresolved"; fi
fi

# 6. Nothing shaped like client data in what this PR ADDS. Scan the whole merge
#    diff rather than the author's own commits — a real transaction reference
#    sat in a comment on public master through two PRs because each author
#    scanned only what they wrote — but scan ADDED lines only, or the gate
#    blocks the very PR that deletes a leak.
# AGENTS.md: "Each PR must link to one line in review-checklist.md as completed
# before merge." A gate that checks everything except the repository's own
# stated pre-merge rule is not the gate it claims to be.
prbody=$(jq -r '.body // ""' <<<"$meta")
if grep -qiE 'review-checklist' <<<"$prbody"; then
  good "description links review-checklist.md"
else
  bad "description does not link review-checklist.md (AGENTS.md requires one completed line per PR)"
fi

diff=$(gh pr diff "$PR" --repo "$REPO" 2>/dev/null)
if [ -z "$diff" ]; then
  bad "could not read diff for the privacy scan"
else
  # A UUID's last group is twelve hex characters and often all digits; the
  # canonical RFC example UUID tripped this. Strip UUIDs rather than
  # widening the placeholder list, which would start excusing real values.
  # Hex digests are the other machine-generated shape that trips this: a
  # sha256 in a lockfile or a sealed surface manifest contains long digit runs
  # by chance, and dropping \b (see below) made them visible. The canonical RFC
  # example UUID tripped it the same way. Strip digests and UUIDs — both are
  # generated, neither can carry a client identifier — rather than loosening the
  # placeholder list, which would start excusing real values. (Deliberately no
  # example digits in this comment: a literal here is a literal in the diff,
  # and this scan reads its own file like any other.)
  # A binary file is a hole in this scan, not an absence of findings: the patch
  # carries a marker instead of content, so a screenshot or PDF of a client
  # statement reads exactly like a clean diff. Refuse rather than pass.
  binaries=$(grep -cE '^(Binary files .* differ|GIT binary patch)' <<<"$diff")
  if [ "$binaries" -gt 0 ]; then
    bad "$binaries binary change(s) the privacy scan cannot read — inspect by hand before merging: $(grep -E '^\+\+\+ b/' <<<"$diff" | sed 's|^+++ b/||' | tr '\n' ' ' | cut -c1-160)"
  fi
  raw_added=$(grep '^+' <<<"$diff" | grep -v '^+++')
  added=$(sed -E 's/[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}/<uuid>/g' <<<"$raw_added" \
          | sed -E 's/[0-9a-fA-F]{32,}/<digest>/g')
  # Exemptions are REPORTED, never silent. Stripping generated-looking values
  # keeps the false-positive rate low enough that the gate is read at all, but a
  # blanket exemption that nobody can see is how a real value gets erased — so
  # say how many were dropped and let the operator judge.
  exempt=$(( $(grep -cE '[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}|[0-9a-fA-F]{32,}' <<<"$raw_added") ))
  [ "$exempt" -eq 0 ] || say "note" "$exempt added line(s) carried a UUID or hex digest, exempted from the scan — check by eye if this PR touches client data"
  # Placeholders match these shapes too — XXXXX1234X is a fabricated PAN and X
  # is an uppercase letter. A gate that cries wolf gets ignored, so obvious
  # placeholders are excluded by an EXPLICIT list; widening the shape itself
  # would start excusing real values. No backreferences: this must be plain
  # ERE, and a grep that errors returns nothing, which reads as a clean scan.
  # `^0{6,}` is the padded-fixture shape: six or more leading zeros then a small
  # number, as constructed test pages use for MICR and postcode fields. Kept
  # narrow on purpose — a real account number can begin with a zero or two, so
  # only a run long enough to be plainly synthetic is excused.
  placeholder='^(X+|Z+|A+)[0-9]+(X|Z|A)?$|^[0-9]{2}(X+|Z+|A+)[0-9]+[0-9A-Z]*$|^(0+|1+|2+|3+|4+|5+|6+|7+|8+|9+)$|^(0?1234567890|1234567890[0-9]*)$|^0{6,}[0-9]{1,5}$'
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
