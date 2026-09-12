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
    --repo=*) REPO="${1#--repo=}"
              [ -n "$REPO" ] || { echo "--repo= needs OWNER/NAME" >&2; exit 2; }; shift ;;
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
                  | select(.body|contains("codex-pull-request-review-summary")) | .body' 2>"$errfile")
comment_query_failed=0
[ -s "$errfile" ] && [ -z "$body" ] && comment_query_failed=1
row=$(grep -E '^\| (📝|🔍)' <<<"$body" | tail -1)
if [ "$comment_query_failed" -eq 1 ]; then
  # "I could not ask" is not "there is no review". Saying the second when the
  # first is true is the failure this whole script is about.
  bad "could not read PR comments: $(tr '\n' ' ' <"$errfile" | cut -c1-110)"
elif [ -z "$row" ]; then
  bad "no Codex review summary at all"
elif ! grep -qE "\`$short\`" <<<"$row"; then
  # Anchored to the backtick cell: an unanchored substring also matches the
  # row's timestamp and URL, which are not claims about a commit.
  bad "latest review names a different commit than $short — it has not seen this push"
elif grep -q 'Completed' <<<"$row"; then
  # A seven-hex prefix is 28 bits and a matching commit can be ground
  # deliberately, after which a stale `Completed` row would vouch for code
  # nobody read. Codex publishes only seven characters, so the prefix cannot be
  # strengthened — but a ground commit has to be created AFTER the review it is
  # impersonating, and that is checkable. Require the reviewed row to postdate
  # the head commit.
  # An earlier revision compared the review's timestamp against the head
  # commit's committer date. That defence is VOID: `GIT_COMMITTER_DATE` is set
  # by whoever creates the commit, so an author grinding a prefix can also
  # backdate it. A check an attacker can satisfy is worse than no check,
  # because it reads as coverage.
  #
  # What IS sound is uniqueness. If a colliding commit was ground and pushed,
  # both commits are in the PR, so more than one of its commits shares the
  # prefix. Count them.
  sharing=$(gh api --paginate "repos/$REPO/pulls/$PR/commits" --jq '.[].sha' 2>/dev/null \
            | { grep -c "^$short" || true; })
  if [ "$sharing" -gt 1 ]; then
    bad "$sharing commits in this PR share the prefix $short — the review row cannot say which it read"
  else
    good "review completed on $short (prefix unique among this PR's commits)"
    # Stated, not hidden: a seven-hex prefix is 28 bits. Uniqueness within the
    # PR does not exclude a collision created elsewhere and force-pushed as the
    # sole commit. Closing that needs Codex to publish a full SHA; until then
    # this rule is a deterrent, not a proof.
  fi
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

# 7. Pin freshness. The compatibility surface pins the raw bytes of 211 files,
#    and ANY operation that can change those bytes — an edit, a formatter, a
#    merge, a rebase taking the base's manifest — invalidates the seal. Nothing
#    in the local loop re-reads pins before a commit, so CI's gate is the only
#    thing that notices and every instance reaches a reviewer instead of its
#    author. Three sessions hit this independently in one day, which makes it a
#    class rather than a set of mistakes.
#
#    Checked without a checkout: if this PR touches a pinned file, it must also
#    touch the manifest. That does not prove the hashes are right — only CI's
#    gate does — but it catches the whole observed failure, which is a reseal
#    that never ran.
SURFACE=docs/tally/compatibility/compatibility-surface.json
# `gh pr view --json files` hard-codes `files(first: 100)`, so a PR touching more
# than 100 files silently returns a partial list — and a pinned file outside that
# page reads as untouched. Use the paginated REST endpoint.
changed=$(gh api --paginate "repos/$REPO/pulls/$PR/files" --jq '.[].filename' 2>/dev/null)
if [ -z "$changed" ]; then
  bad "could not list changed files for the pin-freshness check"
else
  pinned=$(gh api "repos/$REPO/contents/$SURFACE?ref=$head" --jq '.content' 2>/dev/null \
           | tr -d '\n' | base64 --decode 2>/dev/null \
           | jq -r '[.. | objects | select(has("path")) | .path] | .[]' 2>/dev/null)
  if [ -z "$pinned" ]; then
    # "I could not read the manifest" and "there is no manifest" are different
    # statements, and only one of them is about the repository. Distinguish by
    # asking whether the path exists at all; anything else fails closed.
    if gh api "repos/$REPO/contents/$SURFACE?ref=$head" --jq '.sha' >/dev/null 2>&1; then
      bad "the compatibility surface exists at this head but could not be read or parsed — refusing rather than skipping"
    else
      say "note" "no compatibility surface at this head — pin-freshness check does not apply"
    fi
  else
    touched=$(comm -12 <(sort -u <<<"$pinned") <(sort -u <<<"$changed") | grep -v "^$SURFACE$" | head -20)
    if [ -z "$touched" ]; then
      good "touches no pinned file (nothing to reseal)"
    elif grep -qx "$SURFACE" <<<"$changed"; then
      good "touches $(wc -l <<<"$touched" | tr -d ' ') pinned file(s) and the manifest moved with them"
    else
      bad "touches pinned file(s) without updating $SURFACE — the reseal did not run: $(tr '\n' ' ' <<<"$touched" | cut -c1-150)"
    fi
  fi
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
  # A binary DELETION removes a file rather than adding unreadable content, so
  # it is a cleanup, not a hole — counting it blocked the PR that deletes a
  # leaked screenshot, the same inversion as scanning removed lines.
  binaries=$(grep -E '^(Binary files .* differ|GIT binary patch)' <<<"$diff" \
             | grep -cv 'and /dev/null differ')
  if [ "$binaries" -gt 0 ]; then
    bad "$binaries binary change(s) the privacy scan cannot read — inspect by hand before merging: $(grep -E '^\+\+\+ b/' <<<"$diff" | sed 's|^+++ b/||' | tr '\n' ' ' | cut -c1-160)"
  fi
  # The unified-diff file header is `+++ ` WITH A SPACE. Filtering `^+++`
  # discarded any added line whose own content starts with `++`, so
  # an added line whose content began `++` produced no scannable text at all —
  # a place to hide a value from the scan, in the scan's own input. (No example
  # identifier in this comment: the scan reads its own file, and a literal that
  # illustrates a leak pattern IS the pattern. This is the third time a comment
  # here has flagged itself, which is the check working rather than failing.)
  # Identify the header STRUCTURALLY. `+++ ` alone is not enough: an added line
  # whose content begins with `++` produces exactly that prefix. Git's header
  # is always `+++ b/<path>` or `+++ /dev/null`, so match those and nothing
  # else — a payload line that merely starts with `++` is content, and must
  # reach the scan rather than being mistaken for a header.
  # A filename is content too. The header lines are dropped from the scan, so a
  # file whose BASENAME carries an identifier passed with safe contents. Add the
  # destination paths back as their own scannable text.
  added_paths=$(grep -E '^\+\+\+ b/' <<<"$diff" | sed 's|^+++ b/||')
  raw_added=$(printf '%s\n%s\n' "$(grep '^+' <<<"$diff" | grep -vE '^\+\+\+ (b/|/dev/null)')" "$added_paths")
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
  # Case-insensitively: a GSTIN or PAN written in lower or mixed case is the
  # same identifier, and prose is exactly where it would be written that way.
  # The placeholder list is applied to the UPPERCASED form for the same reason.
  # A phone number is written `+91 98765 43210`, `(98765) 43210`, `98765-43210`.
  # Scanning contiguous digits only, every segment falls under both thresholds
  # and the line reads clean.
  #
  # Stripping ALL separators to make a projection was the first fix and it was
  # wrong: it joined two adjacent dates into a sixteen-digit run and `CE_ADR_
  # 0016_E` into a PAN shape, inventing eight findings on a diff that had none.
  # A gate that cries wolf gets ignored, which costs more than this catches.
  #
  # So: permit separators only INSIDE a phone-shaped run, then re-check the
  # result really is a ten-digit Indian mobile. Nothing outside that shape is
  # joined, so no unrelated numbers are fused.
  squashed=$(grep -Eo '[6-9][0-9]{4}[][()+. _-]{0,3}[0-9]{5}' <<<"$added" \
             | sed -E 's/[][()+. _-]//g' | grep -E '^[6-9][0-9]{9}$' || true)
  hits=$(grep -Eio '[0-9]{2}[A-Z]{5}[0-9]{4}[A-Z][0-9A-Z]{3}|[A-Z]{5}[0-9]{4}[A-Z]|[6-9][0-9]{9}' <<<"$added
$squashed" | tr '[:lower:]' '[:upper:]' | sort -u | { grep -cvE "$placeholder" || true; })
  runs=$(grep -Eo '[0-9]{11,18}' <<<"$added
$squashed" | sort -u | { grep -cvE "$placeholder" || true; })
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
echo "MAY MERGE — bind the merge to the reviewed commit AND the validated base:"
echo "  [ \"\$(gh pr view $PR --repo $REPO --json baseRefName -q .baseRefName)\" = \"$base\" ] \\"
echo "    && gh pr merge $PR --repo $REPO --squash --match-head-commit $head"
echo
echo "  (--match-head-commit validates only the head; the base can be changed"
echo "   after this check without moving the head, so re-read it too.)"
exit 0
