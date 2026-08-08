# Bridge Tally live-read controller

This standalone controller is the only PR14 component that performs live
network reads. Invoking it without the exact `run` command performs no network
operation. A run requires a local ignored config, a reviewed tracked synthetic
fixture manifest, the literal `--consent read-only-synthetic` option, a
single-use run-bound interactive challenge before network access, and a second
receipt-and-output-bound confirmation after preview and before save. The run
challenge uses fresh operating-system randomness, expires after five minutes,
and commits to the full config, endpoint, fixture, source/build surface, and
executable evidence. Exact About/profile values and an affirmative no-customer-
data attestation are mandatory; unknown or false values fail before consent or
network access.

The dependency graph contains only the compatibility DTO/gate, portable
protocol, a typed read-only adapter over the production loopback transport,
serialization, hashing, and async runtime. It has no direct generic transport,
Tauri, database, sync, import, or write dependency. The controller can dispatch
only the closed `ReadOnlyProfile` variants shared with production; it accepts
no XML, report name, TDL, payload, or company identifier on the command line.

Run from `src-tauri` after creating `.bridge-live/profile.json` from the
tracked example:

```sh
cargo run --locked -p bridge-tally-live-read -- run \
  ../.bridge-live/profile.json ../.bridge-live/receipt.json \
  --consent read-only-synthetic
```

Raw responses remain bounded and memory-only. The previewed/saved receipt
excludes endpoint details and source identifiers and permanently disclaims
responder authenticity, accounting correctness, source completeness/atomicity,
performance support, writes, and automatic support authority.

Both config and output must be direct children of the repository's canonical
ignored `.bridge-live` directory. The controller issues a non-cloneable output
target, revalidates it at save time, accepts JSON only, and never overwrites an
existing receipt.

The non-default native outstandings qualification feature remains a separate
observation-only binary. Candidate application/HTTP/transport failures are
recorded as bounded attempt facts and receive trailing identity brackets with
zero retry while identity remains unchanged. Its receipt save likewise consumes
a repository-issued, exact-output-bound JSON target; it adds no parser,
accounting, runtime, mirror, write, or support authority.
