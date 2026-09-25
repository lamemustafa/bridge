# Per-batch fixture provenance

Each port batch from batch 2 on writes one file here, `<batch>.md` (for example `batch-2.md`),
recording what its fixtures establish and do not, the reference-implementation commit its goldens
were produced at, the exact invocations, and a byte table in the same shape as `../PROVENANCE.md`'s
("| `file` | bytes | `sha256` | `path` |"). `scripts/check-fixture-provenance.mjs` reads every
Markdown file under `tests/fixtures` and checks the hash of every row it can parse; the crate's own
`tests/provenance_rows.rs` requires exactly one row per golden and edge book and checks its file
name, byte count and SHA-256 itself, so a missing, mistyped or stale row fails. Keeping
one file per batch means two lanes porting in parallel never edit the same file.
