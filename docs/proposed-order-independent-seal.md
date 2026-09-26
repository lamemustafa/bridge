# Proposed: an order-independent compatibility seal

Status: **adopted and implemented.** Approved by the maintainer on 26 Sep 2026
(the merge-queue decision, and a direct go on this design). This document is the design
behind option A of bridge#740; the measurements and the merge-queue follow-up
(option B) are in that issue. "Implementation notes" at the end records where
the implementing change differs from this design.

## Problem

Every PR that touches a pinned file reseals. The reseal rewrites two
**aggregate** digests:

- `manifest_sha256` in `docs/tally/compatibility/compatibility-surface.json`;
- `compatibility_surface_sha256` in `docs/tally/compatibility/compatibility-matrix.json`.

Two PRs that touch *different* pinned files therefore always conflict on
exactly those two lines, although their per-file `sha256` lines merge cleanly.
GitHub never runs the local merge driver (`docs/release-process.md`, "Merge
driver (local only)"). Every pinned PR must therefore be re-merged and
resealed by hand after every other pinned merge. `strict: true` also makes it
re-run CI and re-bind review evidence each time.

How often this happens:
- 80% of merges touch the surface, and 63% of consecutive pinned merges touch
  disjoint pinned files.
- On 26 Sep, two PRs that changed nothing were forced through that loop four
  times in three hours.

## Design

**Store only the per-file hashes. The gate computes the aggregate.**

1. **The surface digest stays the same function, but is not stored.**
   - Add `CompatibilitySurfaceManifest::digest()`. It returns
     `checksum(b"bridge.tally.compatibility-surface/1\0", V1View { schema_version: 1, files, manifest_sha256: "" })`.
   - That is byte-for-byte the value `seal()` stores today, because `seal()`
     checksums the manifest with `manifest_sha256` cleared.
   - So the same pins give the same digest: the schema change alone does not
     move it. Any change to a pinned file still moves it and invalidates
     evidence bound to the old digest, as before. (The implementing commit
     itself changes pinned files, so the live digest does move with it; no
     evidence exists today to invalidate.)
2. **Surface schema 2** is `{ "schema_version": 2, "files": [...] }`.
   - `validate()` keeps every structural check: the file count against
     `MAX_SURFACE_FILES`, relative paths, sorted and unique paths, and each
     `sha256` well-formed.
   - It drops only the self-checksum.
   - The parser accepts only schema 2 and fails closed on 1, so the migration
     cannot be half-applied.
   - `validate_files` (the byte check of every pin) and the REQUIRED
     file/directory coverage are unchanged.
3. **Support manifest schema 2** drops `compatibility_surface_sha256`.
   - `enforce_support_gate` compares each attestation's and receipt's
     `compatibility_surface_sha256` with `surface.digest()` instead of with a
     stored copy (`attestation_scope_mismatch`, and the receipt check in
     `validate_receipt_for_claim`).
   - `support_surface_mismatch` retires; it guarded only the stored copy.
4. **Collectors bind to the computed digest.**
   - `tools/bridge-tally-live-read` reads `surface.manifest_sha256` at three
     sites: stamping a receipt, the post-consent check of the expected
     surface, and stamping the native-outstandings probe receipt.
   - All three use `surface.digest()` instead, so a receipt collected after
     migration carries the same value the gate computes.
5. **CLI:**
   - `rehash-surface` is unchanged. It is now the whole reseal, including for
     a pin-list change, because there is no self-checksum to refuse a hand-added
     entry.
   - `seal-surface` and `repoint-matrix` are removed.
   - `scripts/reseal.sh` runs `rehash-surface` only, and `--pins-changed`
     becomes an alias.
   - `--verify` keeps its meaning: a byte comparison against a scratch rehash.
6. **Local merge driver:** a disjoint-pin merge no longer conflicts, so the
   driver has nothing to resolve in the common case. Delete
   `scripts/reseal-merge-driver.mjs` and its harness in a second PR, once A has
   run in anger. Keeping the two changes separate keeps each reviewable.

## Why it is safe

The seal's security property is the **per-file** check. `validate_files`
re-hashes all 280 pins, so a Tally-path change without a matching hash line
cannot go green, and the PR diff names every pinned file that changed. That is
untouched.

**Merging cannot attest content nobody reviewed.** A pinned file's merged bytes
can differ from both reviewed versions only if both sides changed that file.
Then both changed its one `sha256` line to different values, and git reports a
conflict, the same as today. When only one side changed it, the merged bytes
are that side's bytes, and its hash line is that side's.

Neighbouring pins merge cleanly too: each entry spans four lines (`{`, `"path"`,
`"sha256"`, `}`), so two hash lines are never adjacent. Changes that still
conflict: two pin-list insertions at the same sorted position; appending a pin
after the last entry while the other side edits the last pinned file (the
closing `}` becomes `},` next to its hash line); and renaming a pin on one side
while the other edits that file (the path and hash lines are adjacent).

One case merges cleanly and is still caught: side A **adds a pin** for a file
(its hash from A's bytes) while side B edits that file, unpinned on B. The
merged hash line and the merged bytes then disagree, and `validate_files` fails
on the merged tree. That is loud, not silent, but only because CI runs on the
merge result (`strict: true`, or the merge queue of option B). The property
rests on that.

**What is lost: the self-checksum against a hand-edited pin list.**
- It never stopped a deliberate edit, since `seal-surface` re-attests any list.
- A dropped *judgement* pin is already invisible to the gate
  (`docs/release-process.md`, "A dropped entry slips through silently"), so
  this adds no blind spot.
- Pins that must never be dropped belong in `REQUIRED_SURFACE_FILES` or
  `REQUIRED_SURFACE_DIRECTORIES`, which are presence-checked.

**Unchanged:**
- Receipts still bind to one exact surface: the same digest function over the
  same pin list and hashes.
- Every pinned change still invalidates a positive claim's evidence, by design.
- All 11 claims are `level: unknown` today, so no evidence is affected by the
  migration.

## Change list, for the implementing PR

| Area | Change |
| --- | --- |
| `tools/bridge-tally-compatibility/src/lib.rs` (pinned, REQUIRED) | `digest()`; surface and support schema 2; drop `seal()` on the surface and `repoint_surface`; gate compares against `digest()` |
| `…/src/main.rs` | remove `seal-surface` and `repoint-matrix`; usage string |
| `…/src/lib_tests.rs`, `main_tests.rs`, `bills_native_outstandings_probe_receipt*.rs` | update the fixtures that build sealed surfaces and matrices; add three tests: `digest()` equals the old stored `manifest_sha256` on the committed v1 file; a v1 surface is refused; a disjoint-pin three-way merge of two schema-2 surfaces is conflict-free and passes `validate_files` |
| `tools/bridge-tally-live-read/src/lib.rs`, `native_outstandings_qualification.rs` | `surface.digest()` at the three call sites (receipt stamp, post-consent check, native-probe receipt) |
| `docs/tally/compatibility/*.json` | regenerate once as schema 2 (the migration commit) |
| `scripts/reseal.sh`, `reseal.test.mjs` | rehash only |
| `scripts/check-protocol-section-numbers.mjs` (+ test), `scripts/merge_gate_test_gh.py`, `scripts/merge-gate.sh` | stop requiring `manifest_sha256`; keep the pin-list and hash checks |
| `docs/release-process.md` | replace the three-command sequence and the "surface conflicts" section; keep the pin-list reconciliation guidance for same-file conflicts |

**Migration order**, because the tool pins its own source:
1. change the tool and its tests;
2. build it;
3. regenerate both JSON files as schema 2 with the new `rehash-surface`;
4. run `--verify`;
5. make it one commit, so no intermediate commit is half-migrated.

## Rollback

- Revert the PR. Because `digest()` computes the old stored value for the same
  pins, the old tool's `seal-surface` and `repoint-matrix` reproduce a
  consistent `manifest_sha256` and `compatibility_surface_sha256` from the
  reverted files.
- Evidence is bound to a digest, and the revert itself changes pinned files, so
  evidence made on either side of it would need re-collecting, exactly as after
  any pinned change. None exists today: all 11 claims are `level: unknown`.
- No data migration is involved.

## Test plan

- `cargo test -p bridge-tally-compatibility` and `-p bridge-tally-live-read`.
- `node --test scripts/reseal.test.mjs scripts/check-protocol-section-numbers.test.mjs`
  and `python3 scripts/merge-gate.test.py`.
- **Continuity:** on the pre-migration commit, `digest()` of the committed
  surface equals its stored `manifest_sha256`.
- **Live conflict check**, in throwaway branches:
  - two PRs touching different pinned files (for example two ADRs) each
    rehash, then merge: no conflict, and `gate` passes on the merge;
  - two PRs touching the same pinned file conflict on its hash line.
- **Inverse mutations:**
  - `validate_files` skipped: the stale-byte test fails;
  - `digest()` domain changed: the continuity test fails;
  - a v1 file accepted: the schema test fails.

## Not in scope

- The merge queue (bridge#740 option B). It needs this change first.
- Narrowing the surface or raising `MAX_SURFACE_FILES`.
- Deleting the merge driver, which is a follow-up PR.
- The per-merge update that `strict: true` forces. After this change a PR
  behind master still has to be updated and re-run after every pinned merge;
  it becomes a clean "Update branch", with no hand re-merge or reseal, but it
  is not free. The merge queue (option B) is what removes it.

## Implementation notes

Where the implementing change (bridge#760) differs from the design above:

- **The merge driver changes in the same PR.** It called `seal-surface` and
  `repoint-matrix`, which are removed, and CI runs its tests. So it now writes
  the reconciled pin list and claims directly, with no tool run. Deleting it
  stays a follow-up.
- **Two more readers needed schema 2:** `scripts/merge-gate.sh` (which also
  reads the base tip, so it accepts schema 1 there) and
  `scripts/check-protocol-section-numbers.mjs`.
- **The migration is a mechanical edit, not a tool run.** The new tool refuses
  schema 1, so both JSON files were converted by dropping the stored digest and
  setting `schema_version: 2`, checked to round-trip byte for byte, and then
  resealed with the new `rehash-surface`.
- **The disjoint-pin merge is tested end to end, not in the Rust tests.**
  `scripts/reseal-merge-driver.test.mjs` merges two branches in a disposable
  clone with git's built-in text merge (as GitHub merges), then runs the gate
  and `reseal.sh --verify` on the result, instead of a three-way merge of two
  in-memory surfaces in `lib_tests.rs`.
- **Continuity is tested against the committed schema-1 file itself**, kept as
  a byte-exact fixture under `tools/bridge-tally-compatibility/tests/fixtures`,
  not against a typed digest.
