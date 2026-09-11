# Protocol-reference section numbers

`TALLY_PROTOCOL_REFERENCE.md` numbers its sections sequentially and several branches extend it at
once. A number is not visible to another branch until it merges, so two branches can claim the same
one and neither notices until the second rebases.

**This is not hypothetical.** A branch cut from an older master added a `9.11c` while master gained
a different `9.11c` underneath it. Separately, `1.2` is *already* used twice on master today.

**The check is `scripts/check-protocol-section-numbers.mjs`, and it runs in CI.** It reads the
numbered headings out of the reference and fails on a duplicate. There is nothing to remember and
nothing to keep in step.

```bash
node scripts/check-protocol-section-numbers.mjs   # before you push, if you like
```

## Why the numbers are read from the document

An earlier version of this file asked authors to record a claim — first as a line appended to a
list, then as a file per number. Both were worse than they looked, and the review that took them
apart is worth summarising, because the reasoning generalises:

1. **Appending at the end of one file conflicts even for *distinct* claims.** Git merges by line;
   "the end of the file" is one line, and both branches wrote it.
2. **A file per number fixes that** — different numbers are different paths — **and the same number
   is the same path**, which is an add/add conflict a human has to resolve. That collision is the
   enforcement, not a defect; a scheme that merged both silently would be conflict-free and useless.
3. **But it only covered the numbers people remembered to claim.** Six files against seventy
   headings already in the document. An author could take an existing number, add the
   previously-absent claim file for it, hit no conflict at all, and merge a duplicate heading — the
   exact failure the mechanism existed to prevent, now reached *through* it.

The headings **are** the allocation. Anything that keeps a second copy of them drifts from them, and
a copy that is only sometimes updated drifts silently. So the gate reads the document.

## What the gate does and does not guarantee

On a `pull_request` event GitHub checks out `refs/pull/N/merge`, so the gate runs against **the PR
merged into the base as of that run**, not against the branch alone. Two PRs choosing the same
number are therefore caught as soon as one of them merges and the other re-runs — the second is
stopped before it lands, and rule 3 below says who moves.

Two residuals, both worth knowing:

- **A green check can go stale.** If the base moves after the run and the repository permits
  merging a branch that is not up to date, the old result stands. Closing that is a repository
  setting — *require branches to be up to date*, or a merge queue — not something a script can do.
  The `push: master` run is the backstop: it fails loudly on master rather than quietly.
- **Nothing catches two open PRs *before* either merges.** No file in the repository can see across
  unmerged branches. That needs a job with repository access reading open PRs, and it is not built.

## Rules

1. **A letter suffix means "belongs with its parent"** (`9.12a` elaborates `9.12`), not "came
   later". Do not use a suffix to dodge a number.
2. **Never renumber a merged section.** Other documents and commit messages cite these numbers; if
   two land on the same one, the *later* arrival moves. `KNOWN_DUPLICATES` in the gate exists for
   collisions that predate it, and nothing may be added there to get a new one through.

   **Retitling is not renumbering, and is allowed.** Citations point at the number, so changing a
   heading's words breaks nothing — and it is sometimes the point of a change, as when a title
   states a narrow case in general-sounding words. That includes retitling a section to words
   another section already uses: two headings may share a title, and the reference has such a pair
   today.

   **The gate tests whether a merged *number* is still there**, not whether a heading kept its
   words. Identifying sections by title needs a section to have an identity apart from its number,
   and the only candidate — the title — fails three ways: a Setext heading carries its number
   inside its text, retitling and renumbering together makes the old title vanish, and two sections
   sharing a title make retitling either one look like a move.

3. **A merged number may not be removed either.** `see §9.7` breaks the same way whether 9.7 was
   renumbered, retitled into a different number, or deleted outright, so the gate treats all three
   alike. In practice this costs nothing: no number has ever disappeared from the reference.

   If a section genuinely must go, **leave its number in place with a line saying where the content
   went** — a redirect heading. The citation still lands, and the reader gets to the replacement.
   Deleting the heading leaves them at a number that no longer exists, with nothing to follow.

4. **Do not exchange two numbers. Nothing enforces this.** Swap them and both are still present,
   so the gate sees nothing, while every citation to either lands on the other's content.

   Catching it needs a section to be recognisable apart from its number, and the only candidate is
   its title — which is mutable, and that is fatal rather than awkward: **a title moving from one
   number to another is exactly what a swap and a legitimate retitle chain both look like.** Rename
   `10 Alpha` to `10 Beta` and `20 Beta` to `20 Gamma` and `Beta` has vacated 20 and occupied 10,
   with nothing moved.

   Three attempts were made and each produced a false positive on a legitimate edit — firing on any
   retitle; then blind to a swap of two sections whose titles each appear twice; then firing on the
   retitle chain above. A gate that blocks legitimate documentation edits gets bypassed or switched
   off, which costs more than the gap. So this is a **third residual**, alongside the stale green
   check and the two unmerged PRs: known, written down, and not pretended away.
5. **Claiming a number says nothing about Tally.** Whether the behaviour a section describes is
   verified, and to what scope, is settled in the reference itself and in its evidence blocks. The
   confidence markers are the reference's job and cannot be summarised anywhere else without
   losing them.

This file deliberately does **not** summarise what each existing section says. The reference is the
authority for that, and a copy here would drift; an earlier draft of this register already
described several ranges incorrectly.

## Editing the reference needs a compatibility-surface reseal

`TALLY_PROTOCOL_REFERENCE.md` is pinned in `compatibility-surface.json`, so **even a
documentation-only edit stales its digest** and fails the `Tally portable core` job
(`real_tree_has_complete_migration_and_report_surface_coverage`). Nothing in a docs diff suggests a
compatibility gate is involved; two PRs failed CI for exactly this before it was written down.

The procedure — the commands, their order, the required `--output`, and the toolchain to run them
with — is in **[`docs/release-process.md`](../release-process.md#compatibility-surface-reseal)**.
It is not repeated here: a second copy drifts, and a section author following a stale one leaves
the surface improperly resealed.
