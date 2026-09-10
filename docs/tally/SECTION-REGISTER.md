# Protocol-reference section register

`TALLY_PROTOCOL_REFERENCE.md` numbers its sections sequentially and several branches extend it at
once. A number is not visible to another branch until it merges, so two branches can claim the same
one and neither notices until the second rebases.

**This is not hypothetical.** A branch cut from an older master added a `9.11c` while master gained
a different `9.11c` underneath it. Separately, `1.2` is *already* used twice on master today.

**Claim your number by appending to the list at the end of this file, in the same PR that uses it.**
Appending at the end means two branches claiming different numbers do not touch the same lines.

## Rules

1. **Claim before you write**, and append — never insert into the middle of the list.
2. **A letter suffix means "belongs with its parent"** (`9.12a` elaborates `9.12`), not "came
   later". Do not use a suffix to dodge claiming a number.
3. **Never renumber a merged section.** Other documents and commit messages cite these numbers; if
   two land on the same one, the *later* arrival moves.
4. **Re-read this file when you rebase.** A number free when you branched may not be free now.
5. **A claim here is not a claim about Tally.** This file allocates numbers. Whether the behaviour
   a section describes is verified, and to what scope, is settled in the reference itself and in
   its evidence blocks — never here.

This file deliberately does **not** summarise what each existing section says. The reference is the
authority for that, and a copy here would drift; an earlier draft of this register already
described several ranges incorrectly.

## The reference is a pinned source — editing it needs a reseal

`TALLY_PROTOCOL_REFERENCE.md` is pinned in `compatibility-surface.json`, so **even a
documentation-only edit stales its digest** and fails the `Tally portable core` job
(`real_tree_has_complete_migration_and_report_surface_coverage`). Nothing in a docs diff suggests a
compatibility gate is involved; two PRs failed CI for exactly this before it was written down.

From `tools`, with the pinned toolchain (a Homebrew `rustc` on `PATH` shadows rustup and the project
pins 1.96 — check `rustc --version` first):

```bash
S=../docs/tally/compatibility/compatibility-surface.json
M=../docs/tally/compatibility/compatibility-matrix.json
cargo run --locked -p bridge-tally-compatibility -- rehash-surface "$S" .. --output "$S"
cargo run --locked -p bridge-tally-compatibility -- seal-surface   "$S"    --output "$S"
cargo run --locked -p bridge-tally-compatibility -- repoint-matrix "$M" "$S" --output "$M"
```

`--output` is required; without it the tool prints to stdout and changes nothing. Order matters —
see `docs/release-process.md#compatibility-surface-reseal`. `rehash-surface` reports a changed
count, so `rehash_surface_changed:1` after a single-file doc edit is the expected confirmation; a
surprising count means your resolution is wrong.

## Claims

Append below. One line per number. A claim records only that a PR intends to use the number.

- `8.2a` — PR #289 — proposed, unmerged
- `9.11d` — PR #280 — proposed, unmerged
- `9.12`, `9.12a`, `9.12b` — PR #283 — proposed, unmerged
- `9.13` — PR #289 — proposed, unmerged
