# Protocol-reference section register

`TALLY_PROTOCOL_REFERENCE.md` numbers its sections sequentially, and several branches add to it
at once. A number is not visible to another branch until it merges, so two branches can claim the
same one and neither notices until the second rebases — **which has already happened**: a branch
cut from an older master added a `9.11c` while master gained a different `9.11c` underneath it.

**Claim a number here, in the same PR that uses it.** This file is append-only, so two branches
appending different lines merge without a conflict; the reference itself does not.

## In use on `master`

| Range | Subject |
| --- | --- |
| 1–2 | transport, negotiation, endpoint |
| 3–4 | company identity, request shapes |
| 5.1–5.6 | reads: collections, scoping, cost |
| 6.1–6.2 | dates and periods |
| 7 | masters |
| 8.1–8.2 | groups and ancestry |
| 9.1–9.11c | writes (import): response shapes, traps, company binding |
| 10–14 | change detection, observability, limits |

Exact headings are authoritative in the file itself; this table is a map, not a copy.

## Claimed by an open branch, not yet merged

| Number | PR | Subject |
| --- | --- | --- |
| 8.2a | #289 | reserved group identity via `RESERVEDNAME` |
| 9.11d | #280 | `SVCURRENTCOMPANY` is not a write guard |
| 9.12, 9.12a, 9.12b | #283 | item invoices; `ALLLEDGERENTRIES` silently discarded; delete by `REMOTEID` |
| 9.13 | #289 | Payment / Receipt / Contra on the import writer |

**Next free in the write range: `9.14`.** Next free suffix under 9.11: `9.11e`.

## Rules

1. **Claim before you write.** Add the row here first; if two PRs race, the second sees the first
   in this file even before it merges to master.
2. **A letter suffix means "belongs with its parent"** (`9.12a` elaborates `9.12`), not "came
   later". Do not use a suffix to avoid claiming a new number.
3. **Never renumber a merged section.** Other documents, commit messages and the brain cite these
   numbers. If two land on the same number, the *later* one moves.
4. **Rebase before you finalise.** A number free when you branched may not be free now. Re-read
   this file, not just the reference.

## The reference is a pinned source — editing it needs a reseal

`TALLY_PROTOCOL_REFERENCE.md` is pinned in `compatibility-surface.json`, so **even a
documentation-only edit stales its digest** and fails the `Tally portable core` job
(`real_tree_has_complete_migration_and_report_surface_coverage`). This is not obvious: nothing in
a docs diff suggests a compatibility gate is involved, and both PRs that added this register and
the offline importer failed CI for exactly this reason.

From `tools`, with the pinned toolchain (a Homebrew `rustc` on `PATH` will shadow rustup and the
project pins 1.96 — check `rustc --version` first):

```bash
S=../docs/tally/compatibility/compatibility-surface.json
M=../docs/tally/compatibility/compatibility-matrix.json
cargo run --locked -p bridge-tally-compatibility -- rehash-surface "$S" .. --output "$S"
cargo run --locked -p bridge-tally-compatibility -- seal-surface   "$S"    --output "$S"
cargo run --locked -p bridge-tally-compatibility -- repoint-matrix "$M" "$S" --output "$M"
```

`--output` is required; without it the tool prints to stdout and changes nothing. Order matters —
see `docs/release-process.md#compatibility-surface-reseal`. `rehash-surface` reports its changed
count, so `rehash_surface_changed:1` after a single-file doc edit is the expected confirmation.
