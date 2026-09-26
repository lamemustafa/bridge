# Proposed: an order-independent compatibility seal

Status: **proposal, not adopted.** It changes what the compatibility seal
stores, so adopting it is the owner's decision (bridge#740, question 1). This
document is the implementable design behind option A of bridge#740. The
measurements and the merge-queue follow-up (option B) are in that issue.

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
   - So every existing and future receipt and attestation keeps its meaning,
     and nothing is re-signed.
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
   - `tools/bridge-tally-live-read` reads `surface.manifest_sha256`: once to
     stamp a receipt, and once to check an expected surface.
   - Both use `surface.digest()` instead, so a receipt collected after
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

Git treats changes on adjacent lines as conflicting, so two PRs pinning
neighbouring files may still conflict. That errs on the safe side.

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
| `tools/bridge-tally-live-read/src/lib.rs` | `surface.digest()` at the two call sites |
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

- Revert the PR. Because `digest()` equals the old stored value, the old tool
  reproduces the same `manifest_sha256` and `compatibility_surface_sha256` from
  the reverted files via `seal-surface` and `repoint-matrix`.
- Receipts and attestations stay valid in both directions.
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
