# ADR 0009: Bound Tally observability before runtime instrumentation

Status: accepted for the portable aggregation contract and PR15 read-runtime
collection; persistence, support-bundle integration, and performance budgets
are not yet enabled.

## Decision

Bridge starts PR13 with a local-only `bridge-tally-observability` crate. Its
input surface contains only one closed terminal-attempt enum, elapsed
`Duration` values intended to come from a monotonic clock, and either response
bytes actually consumed or an explicit unavailable measurement. The crate
aggregates these caller-supplied values but cannot
authenticate how they were measured. It accepts no strings, dynamic
labels, identifiers, endpoints, ports, dates, hashes, payloads, errors, paths,
company or record metadata, amounts, GSTIN/PAN, or arbitrary attributes.

The schema-v2 collector aggregates into 1,592 fixed bucket-frequency cells in coherent,
fixed-memory snapshots. It stores no event history. Latency and response-byte
boundaries are versioned, inclusive, and carry overflow buckets. Counts
saturate rather than wrap, and lost cell increments from saturation are
disclosed. Queue wait is defined to exclude Bridge's deliberate post-request
pacing. Response latency is
defined as a custom Bridge pipeline observation that excludes queue wait and
pacing; the portable runtime measures from request-future start through its
terminal decoded result. Response bytes mean bytes actually read, including
a partial body later classified as a size, decode, transport, timeout, or
cancellation failure. A caller that cannot observe that value must use
`unavailable`, never a fabricated zero. Neither measurement is a standard OpenTelemetry HTTP
instrument.

Runtime classification must use structured state, never error-string matching.
Explicit caller cancellation is `cancelled`; a configured deadline is
`timeout`; crossing the body cap is `size_limit`; a completed body that cannot
decode is `decode`; an HTTP failure status is `http_status`; other connection,
reset, or body-read failures are `transport`; and `success` requires the
bounded body and decode stages to finish. Queue deadline and cancellation are
terminal attempt variants and therefore cannot claim a response in the same
observation.

The preview always emits the same eight request-class rows in a reviewed
schema. Exact cell counts are reduced to coarse buckets, there is no timestamp,
and the collection scope is `unstamped_collector_instance_lifetime`.
Serialization is bounded to 64 KiB and the payload receives a domain-separated
checksum. The preview states that observations are caller-supplied and
unauthenticated, collection completeness and duplicate-call detection are not
established, taxonomy rows are not capability evidence, the crate has no
network exporter, authenticity is none, integrity is checksum-only, and no
performance support is established.

The preview is a custom, lossy, coarsened bucket-frequency summary. It has no
arithmetic sums and cannot be merged, used for exact rates or percentile
regression gates, or losslessly converted into an OpenTelemetry Histogram.

## Authority and privacy boundary

The collector contract cannot change capability, verification, checkpoint,
retry, circuit-breaker, write, or accounting outcomes. The separate portable
read runtime owns retry and circuit decisions before reporting their bounded
outcomes to the collector. The preview is not Proof of Sync, a
source-completeness signal, a Tally-authentic measurement, or evidence of GST
or accounting correctness. It has no network exporter, database,
logging/tracing backend, system-metrics sampler, or reset API.
Always-emitted rows, including `Import`, are taxonomy only. Imports remain
disabled and must not be wired until write-specific `OutcomeUnknown` semantics
are represented separately.

The design follows data minimisation and bounded-cardinality principles. In
particular, company metadata is omitted rather than hashed because hashing a
small or predictable identifier space is not reliable anonymisation:

- https://opentelemetry.io/docs/security/handling-sensitive-data/
- https://opentelemetry.io/docs/specs/otel/metrics/sdk/
- https://opentelemetry.io/docs/specs/semconv/general/attribute-requirement-level/
- https://doc.rust-lang.org/std/time/struct.Instant.html

## Consequences

Portable tests can prove fixed-memory cell cardinality, saturation, bucket
boundaries, coherent snapshots, privacy-reduced serialization without direct
identifiers, explicit unavailable-byte and circuit-rejection cells,
deterministic checksums, schema-v2 golden bytes, and a hard preview-size
ceiling. The preview still
reveals coarse operational behaviour and must remain local unless the user
previews and explicitly chooses to share it. These tests do not
prove native execution, timing accuracy, collection completeness, or that any
workload meets a budget.

PR15 wires queue, retry, circuit, cancellation, and response outcomes through a
closed read-only runtime classifier. Single-response native reads report
encoded bytes after a complete transport body, including a later
application-validation or parse failure. Compound probes, pre-response
failures, and partial transport reads mark them unavailable because the native
transport does not yet expose a partial-byte count. Native Windows validation
passes with the documented SQLCipher Perl/libclang prerequisites, while macOS
evidence remains a configured CI responsibility. Phase, reconciliation,
checkpoint-age, memory, database, resume, and UI measurements remain separate work. A later
synthetic harness must record build/platform/generator/sample methodology and
keep its results distinct from live Tally Education or production performance.
