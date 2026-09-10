# Protocol-reference section register

`TALLY_PROTOCOL_REFERENCE.md` numbers its sections sequentially and several branches extend it at
once. A number is not visible to another branch until it merges, so two branches can claim the same
one and neither notices until the second rebases.

**This is not hypothetical.** A branch cut from an older master added a `9.11c` while master gained
a different `9.11c` underneath it. Separately, `1.2` is *already* used twice on master today.

**Claim a number by adding `section-claims/<number>.md` in the same PR that uses it.** One file per
number, named for the number. Nothing else in this directory changes.

```bash
ls docs/tally/section-claims/                 # every number claimed on this branch
printf '%s\n' "PR #NNN — proposed, unmerged" > docs/tally/section-claims/9.14.md
```

## Why a file per number, and what it does not do

The mechanism is chosen for how Git merges it, not for tidiness.

*Two PRs claiming different numbers add different paths.* They never touch the same file, so they
never conflict. An earlier draft of this register kept a list and asked authors to append at the
end; that does not work, because two branches appending different last lines to one file produce a
content conflict at that line — distinct claims collided anyway.

*Two PRs claiming the **same** number add the **same** path.* Git reports an add/add conflict and
neither can merge without a human looking at it. **This is the point.** A representation that
merged both silently — a claims directory keyed on the PR number, say — would be conflict-free and
useless, because the duplicate would reach master unnoticed, which is the failure this register
exists to prevent.

**What it cannot do.** It is not a reservation system. Two open PRs can both choose `9.14`, and
until one of them merges neither branch can see the other's file: rebasing shows only merged
claims. What is guaranteed is narrower and still worth having — **the duplicate cannot reach master
silently.** The second PR hits the conflict before it merges, at which point rule 3 decides who
moves. If duplicate numbers keep costing rebases despite this, the real fix is a CI check that
reads the claim files across *open* PRs; that is not built.

Before claiming, and again when you rebase, it is worth one command:

```bash
gh pr list --state open --json number,files --jq \
  '.[] | select(.files[].path | startswith("docs/tally/section-claims/")) | .number'
```

## Rules

1. **Claim before you write.** Add the claim file in the PR that adds the section.
2. **A letter suffix means "belongs with its parent"** (`9.12a` elaborates `9.12`), not "came
   later". Do not use a suffix to dodge claiming a number.
3. **Never renumber a merged section.** Other documents and commit messages cite these numbers; if
   two land on the same one, the *later* arrival moves.
4. **Re-read the directory when you rebase.** A number free when you branched may not be free now.
5. **A claim is not a claim about Tally.** These files allocate numbers. Whether the behaviour a
   section describes is verified, and to what scope, is settled in the reference itself and in its
   evidence blocks — never here. A claim file records the number, the PR, and whether it has
   merged; it must not describe gateway behaviour, because an unqualified claim read here would
   carry none of the reference's VERIFIED / PARTIAL / UNVERIFIED marking.

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
