# Per-batch fixture provenance

Each port batch from batch 2 on writes one file here, `<batch>.md` (for example `batch-2.md`),
recording what its fixtures establish and do not, the reference-implementation commit its goldens
were produced at, the exact invocations, and a byte table in the same shape as `../PROVENANCE.md`'s
("| `file` | bytes | `sha256` | `path` |"). `scripts/check-fixture-provenance.mjs` reads every
Markdown file under `tests/fixtures` and checks every row's hash; the crate's own
`tests/provenance_rows.rs` makes a row compulsory for every golden and edge book, so naming one in
prose alone fails. Keeping
one file per batch means two lanes porting in parallel never edit the same file.
