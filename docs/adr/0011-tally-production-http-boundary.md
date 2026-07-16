# ADR 0011: Tally production HTTP boundary

- Status: accepted
- Date: 2026-07-15

## Context

Bridge previously had two bounded response readers: one inside the native app
and one inside the repository-synthetic qualification worker. Portable tests
could therefore pass without proving the code used by the app. Runtime session
identity also collapsed every accepted loopback address at a port to one
`localhost` key, even though `127.0.0.1`, other `127/8` addresses, and `::1`
can identify different local services.

## Decision

`bridge-tally-transport` is the production XML-over-HTTP boundary. The native
Tally client delegates status and XML requests to it. The crate:

- accepts only `localhost` or literal loopback addresses;
- normalizes `localhost` to `127.0.0.1` but preserves every other actual IPv4
  loopback address and `::1` as distinct endpoint identities;
- disables inherited proxies and redirects;
- applies a bounded whole-request deadline;
- caps outbound XML and encoded response bytes at closed limits;
- rejects non-identity `Content-Encoding` before interpreting the body;
- decodes UTF-8/BOM, UTF-16LE/BOM, and UTF-16BE/BOM through the portable
  protocol crate; and
- exposes closed, privacy-safe transport errors without URLs, headers, bodies,
  company identifiers, or accounting values.

The deterministic simulator separately characterizes Content-Length,
close-delimited, and chunked bodies, declared and streamed cap overflow,
truncation, slow headers, redirects, non-2xx statuses, and content encoding.
These are Bridge resilience requirements, not statements about framing that
Tally guarantees.

The export application-status parser rejects duplicate or misplaced critical
header fields and active XML constructs. The import parser accepts the current
documented direct `ENVELOPE/BODY/DATA` counter profile without combining it
with Tally's documented wrapped `IMPORTRESULT` profile.

The unauthenticated `GET /status` response is only a best-effort local
diagnostic heuristic; it is not part of the documented third-party XML
contract and cannot identify the product or gate the XML POST probe. A
successful, strictly parsed XML export can establish reachability of that
observed endpoint profile, but not responder authenticity.

Runtime sessions use the normalized actual origin as their client, cache, and
snapshot identity. Cancellation retains the endpoint pacing quarantine, and a
half-open circuit reserves one probe. A protocol/application rejection does not
reset prior transport failures.

## Evidence authority

Portable tests establish repository-synthetic behavior of the transport,
decoder, parser, generator, and simulator. They do not establish:

- that the unauthenticated local responder was Tally;
- behavior of a Tally release or Education/licensed profile;
- that Tally emits every tested HTTP framing or encoding;
- source completeness, accounting correctness, or snapshot atomicity;
- stable performance or bounded total process memory; or
- write/import capability.

The existing parser-qualification receipt continues to keep
`runtime_cap_binding=false`; it was produced by a separate worker and is not
retroactively upgraded by this ADR.

## Consequences

Portable CI can now test the exact response reader used by the app. Native
runtime/cancellation tests pass on the validated Windows toolchain, and CI is
configured to repeat the native workspace on Windows and macOS. Process-isolated
HTTP evidence, large-master traversal through the runtime, phase timing, and a
read-only live compatibility receipt remain follow-on work.
