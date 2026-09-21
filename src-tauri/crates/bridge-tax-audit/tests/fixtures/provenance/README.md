# Per-batch fixture provenance

Each port batch from batch 2 on writes one file here, `<batch>.md` (for example `batch-2.md`),
recording what its fixtures establish and do not, the reference-implementation commit its goldens
were produced at, the exact invocations, and a byte table in the same shape as `../PROVENANCE.md`'s
("| `file` | bytes | `sha256` | `path` |"). `scripts/check-fixture-provenance.mjs` reads every
Markdown file under `tests/fixtures`, so rows here are machine-checked like the main table. Keeping
one file per batch means two lanes porting in parallel never edit the same file.
