# Synthetic Tally parser qualification

This portable crate produces versioned evidence for one narrow claim: a fresh
Bridge protocol worker decoded and parsed a deterministic repository-generated
voucher corpus with exact counts and hashes.

It does **not** contact Tally, exercise HTTP, the Tauri runtime, SQLCipher,
canonical persistence, reconciliation, resume, or the UI. A receipt is
hard-coded and revalidated with all of these claims false:

- live Tally was observed;
- Tally support or capability was established;
- accounting correctness was established;
- a performance budget was established;
- the qualification response cap is bound to the production runtime.

The controller generates UTF-8 windows into a temporary directory, verifies
each window is at most 32 MiB, then starts a fresh worker for every sample. The
worker verifies the domain-separated payload digest before parsing and reports
only exact counts, an independently expected semantic-output digest, monotonic
non-steady elapsed nanoseconds for the whole file-read/digest/decode/parse/hash
pipeline, and method-labelled process-lifetime peak resident memory. The delta
between two lifetime maxima is diagnostic, not an allocation measure. Paths
and raw payloads are never written to the receipt. Generator files are deleted
with the temporary directory.

Run a correctness smoke receipt locally:

```text
cargo run --locked --release -p bridge-tally-qualification -- run ci-smoke target/tally-qualification-smoke.json 7 3
```

Supported scenario names are `ci-smoke`, `small-1k`, `medium-50k`,
`large-500k`, and `large-voucher`. The deprecated `deep-voucher` spelling is
accepted as an alias for `large-voucher`; no parser-depth claim is made. The
50k and 500k cases are explicitly windowed corpora, never one enormous Tally
response. Debug or hosted-runner measurements are diagnostic only. No baseline
comparator or performance budget exists in schema v1.
