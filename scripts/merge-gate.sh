#!/usr/bin/env bash
# Decide whether a pull request may be merged, and say why not when it may not.
#
# Every condition here exists because it failed. On 2026-09-12 four PRs were
# merged before their reviews arrived — #312 by three seconds — leaving eleven
# findings, four of them P1, on code already in master. The gate added to stop
# that then deadlocked a clean PR, because a Codex pass with no findings
# submits no review object at all. Later the same day two separate sessions
# reported a PR "clean, zero open threads" from a query issued seconds before
# the review posted.
#
# The single rule underneath all of it: a review is only evidence about the
# commit it names. Freshness is not implied by the absence of findings, and
# "no findings yet" is indistinguishable from "not looked yet" unless you
# check which commit was looked at.
#
# Usage:  scripts/merge-gate.sh <pr-number> [--repo owner/name]
# Exit:   0 = may merge, 1 = must not, 2 = could not determine.

set -uo pipefail

PR="${1:-}"
REPO="${2:-}"
[ -n "$PR" ] || { echo "usage: $0 <pr-number> [owner/name]" >&2; exit 2; }
if [ -n "$REPO" ]; then R=(--repo "$REPO"); else R=(); fi

fail=0
say()  { printf '  %-6s %s\n' "$1" "$2"; }
bad()  { say "BLOCK" "$1"; fail=1; }
good() { say "ok"    "$1"; }

meta=$(gh pr view "$PR" "${R[@]}" --json headRefOid,baseRefName,mergeable,mergeStateStatus,isDraft 2>/dev/null) || {
  echo "could not read PR #$PR" >&2; exit 2; }
head=$(jq -r .headRefOid <<<"$meta")
base=$(jq -r .baseRefName <<<"$meta")
mergeable=$(jq -r .mergeable <<<"$meta")
state=$(jq -r .mergeStateStatus <<<"$meta")
draft=$(jq -r .isDraft <<<"$meta")
short=${head:0:7}

echo "PR #$PR  head=$short  base=$base  $mergeable/$state"

[ "$draft" = "false" ] || bad "draft"

# 1. Mergeable against its base.
case "$mergeable" in
  MERGEABLE) good "no conflicts" ;;
  CONFLICTING) bad "conflicts with $base — rebase first" ;;
  *) bad "mergeability still UNKNOWN; re-run in a moment" ;;
esac

# 2. Based on master. ci.yml fires on pull_request into master ONLY, so a
#    stacked PR runs no CI at all and a green tick on it measures nothing.
if [ "$base" = "master" ]; then
  good "based on master (CI actually runs)"
else
  bad "based on '$base', not master — ci.yml does not fire, so checks here prove nothing"
fi

# 3. Every check concluded, none failed. SKIPPED is fine; PENDING is not.
checks=$(gh pr checks "$PR" "${R[@]}" 2>/dev/null)
if [ -z "$checks" ]; then
  bad "no checks reported"
else
  pend=$(grep -cE '[[:space:]](pending|queued|in_progress)[[:space:]]' <<<"$checks")
  bust=$(grep -cE '[[:space:]](fail|failure|cancelled|timed_out)[[:space:]]' <<<"$checks")
  [ "$pend" -eq 0 ] || bad "$pend check(s) still running"
  [ "$bust" -eq 0 ] || bad "$bust check(s) failing"
  [ "$pend" -eq 0 ] && [ "$bust" -eq 0 ] && good "all checks concluded, none failing"
fi

# 4. A Codex review that names THIS head. The summary comment is edited in
#    place, so its created_at is meaningless — read the commit in its table.
#    A clean pass emits no review object, only this row plus a thumbs-up, which
#    is why the row and not the review list is the thing to read.
body=$(gh api "repos/${REPO:-lamemustafa/bridge}/issues/$PR/comments" \
        --jq '.[] | select(.body|contains("codex-pull-request-review-summary")) | .body' 2>/dev/null)
row=$(grep -E '^\| (📝|🔍)' <<<"$body" | tail -1)
if [ -z "$row" ]; then
  bad "no Codex review summary at all"
elif ! grep -q "$short" <<<"$row"; then
  bad "latest review names a different commit than $short — it has not seen this push"
elif grep -q "Running" <<<"$row"; then
  bad "review still running on $short — 'no findings yet' is not 'no findings'"
elif grep -qE "Completed|Failed" <<<"$row"; then
  good "review completed on $short"
else
  bad "could not read review state from: $row"
fi

# 5. No unresolved threads. required_conversation_resolution is on, so this is
#    the gate, not a courtesy. Paginate: a first:100 page once hid 19 threads.
threads=$(gh api graphql -f query="{repository(owner:\"${REPO%%/*}\",name:\"${REPO##*/}\"){pullRequest(number:$PR){reviewThreads(first:100){totalCount pageInfo{hasNextPage} nodes{isResolved}}}}}" 2>/dev/null \
  || gh api graphql -f query="{repository(owner:\"lamemustafa\",name:\"bridge\"){pullRequest(number:$PR){reviewThreads(first:100){totalCount pageInfo{hasNextPage} nodes{isResolved}}}}}" 2>/dev/null)
if [ -z "$threads" ]; then
  bad "could not read review threads"
else
  total=$(jq -r '.data.repository.pullRequest.reviewThreads.totalCount' <<<"$threads")
  more=$(jq -r '.data.repository.pullRequest.reviewThreads.pageInfo.hasNextPage' <<<"$threads")
  open=$(jq '[.data.repository.pullRequest.reviewThreads.nodes[]|select(.isResolved==false)]|length' <<<"$threads")
  [ "$more" = "false" ] || bad "more than 100 threads — paginate before trusting this count"
  if [ "$open" -eq 0 ]; then good "0 of $total threads unresolved"; else bad "$open of $total threads unresolved"; fi
fi

# 6. Nothing shaped like client data in the diff. Scan the WHOLE merge diff,
#    not your own commits: a real transaction reference sat in a comment on
#    public master through two PRs because each author scanned only what they
#    wrote. An identifier is findable by shape; no name list catches it.
diff=$(gh pr diff "$PR" "${R[@]}" 2>/dev/null)
# A UUID's last group is twelve hex characters and is all digits often enough
# to look like an account number — the canonical `550e8400-…-446655440000`
# tripped this on its first run. Remove UUIDs before scanning rather than
# widening the placeholder list, which would start excusing real values.
diff=$(sed -E 's/[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}/<uuid>/g' <<<"$diff")
if [ -z "$diff" ]; then
  bad "could not read diff for the privacy scan"
else
  # Placeholders match these shapes too — `XXXXX1234X` is a fabricated PAN and
  # `X` is an uppercase letter. A gate that cries wolf gets ignored, which is
  # worse than no gate, so drop anything whose letters are one repeated
  # character or whose digits are a repeat or a straight run.
  # No backreferences: this must be plain ERE or grep errors, and a grep that
  # errors returns no matches — which reads exactly like a clean scan. That is
  # the same shape as a control reporting zero because its branch never fired,
  # so the placeholder list is spelled out instead of `([0-9])\\1+`.
  placeholder='^(X+|Z+|A+)[0-9]+(X|Z|A)?$|^[0-9]{2}(X+|Z+|A+)[0-9]+[0-9A-Z]*$|^(0+|1+|2+|3+|4+|5+|6+|7+|8+|9+)$|^(0?1234567890|1234567890[0-9]*)$'
  hits=$(grep -Eo '[0-9]{2}[A-Z]{5}[0-9]{4}[A-Z][0-9A-Z]{3}|\b[A-Z]{5}[0-9]{4}[A-Z]\b|\b[6-9][0-9]{9}\b' <<<"$diff" \
          | sort -u | { grep -cvE "$placeholder" || true; })
  runs=$(grep -oE '\b[0-9]{11,18}\b' <<<"$diff" | sort -u | { grep -cvE "$placeholder" || true; })
  # Prove the pattern itself compiles; a silent regex error is the failure mode
  # this whole block exists to avoid.
  # The probe string must be one the pattern genuinely matches, or the probe
  # fails on a perfectly good pattern — which is how this check first behaved.
  printf 'XXXXX1234X\n' | grep -qE "$placeholder" \
    || { bad "privacy-scan pattern failed to compile or match its own probe"; hits=-1; }
  if [ "$hits" -eq 0 ] && [ "$runs" -eq 0 ]; then
    good "no GSTIN/PAN/mobile shapes, no unexplained long digit runs"
  else
    bad "privacy scan: $hits identifier shape(s), $runs unexplained long digit run(s) — inspect before merging"
  fi
fi

echo
if [ "$fail" -eq 0 ]; then echo "MAY MERGE"; exit 0; else echo "MUST NOT MERGE"; exit 1; fi
