# ADR 0010: Synthetic Tally qualification is parser-only evidence

## Status

Accepted for the portable PR13B1 correctness slice.

## Decision

Bridge uses a repository-owned deterministic generator and a separate fresh
worker process to qualify protocol decoding and voucher parsing. The generator
streams bounded windows to temporary files before measurement. Every window is
limited to 32 MiB of encoded body bytes, has exact record and entry counts, and
has a domain-separated SHA-256 digest. The worker rechecks bytes and digest,
parses each window, and must produce the exact counts and independently
generator-derived semantic digest in every retained sample. The semantic
projection binds identities, dates, voucher fields, cancelled/optional flags,
entry association, names, exact amount text, sign evidence, and fragment hashes.

A qualification-only body reader defines the encoded-body boundary precisely:
exactly 33,554,432 bytes is accepted; a declared larger length is rejected
before reading; and an unknown-length source is read through at most byte
33,554,433 to detect overflow. This reader is not yet the production HTTP
component, so passing it cannot establish the runtime cap.

The JSON receipt schema is versioned, rejects unknown fields when deserialized,
and caps both output and validated input at 256 KiB. It retains every accepted
raw timing sample, omits p95 below 20 samples, and uses nearest-rank p95 at 20
or more. It binds the executable and Cargo.lock digests, Rust compiler version,
target triple, actual Cargo profile, the crate's structurally empty feature set,
generator schema/seed/shape, independently regenerated per-window
payload/semantic hashes, and manifest hash. CI may embed `GITHUB_SHA` at build
time; a missing local build-embedded commit remains null. Its checksum detects
corruption only; it provides no authenticity.

The receipt authority is `repository_synthetic_bridge_parser_qualification`.
It permanently records that live Tally was not observed and that the evidence
does not establish Tally support, a Tally capability, accounting correctness,
a production response-cap binding, or a performance budget. The 50k and 500k
scenarios are windowed corpora and do not claim one-response capacity or Tally
snapshot completeness.

## Consequences

- Pull-request CI may gate generator/parser correctness and receipt integrity.
- Hosted-runner durations are diagnostic and cannot create release budgets.
- Windows process-lifetime peak working set and Unix/macOS process-lifetime
  `getrusage` maximum resident size are normalized to bytes and recorded with
  distinct method identifiers; values are not compared across platforms. The
  difference between baseline and final lifetime maxima is not an allocation
  measurement.
- HTTP delivery, cap-boundary transport behavior, native persistence,
  reconciliation, resume, UI responsiveness, and live Education/licensed Tally
  compatibility remain separate qualification slices.
- Deep voucher evidence is characterization only because the production parser
  does not yet own a maximum XML-depth contract.
