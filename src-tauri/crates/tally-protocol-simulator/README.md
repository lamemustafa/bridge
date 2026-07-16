# Deterministic Tally protocol simulator

This crate supplies loopback-only one-request and bounded sequential HTTP
simulators plus a synthetic fixture corpus for Bridge's Tally protocol tests. It has no dependency on Tally,
SQLCipher, OpenSSL, Tauri, a network service, or customer books.

## Scenario recipe

A scenario is composed from six explicit dimensions:

1. `Fixture` selects the application payload and its expected meaning.
2. `WireEncoding` selects UTF-8, UTF-8 with BOM, UTF-16LE, or UTF-16BE.
3. `Delivery` selects immediate delivery, slow headers, slow body, reset before
   response, or delayed reset after the request is considered processed.
4. `http_status` keeps HTTP transport status separate from Tally's application
   `STATUS`.
5. `ResponseFraming` selects Content-Length, close-delimited, chunked, or a
   deliberately mismatched declared length.
6. `ResponseContentEncoding` selects no header, `identity`, or a synthetic
   unsupported `gzip` claim.

The simulators always bind an ephemeral `127.0.0.1` port. Production code does
not depend on this crate. `Simulator` handles exactly one request;
`SequenceSimulator` handles an explicit ordered plan of 1–64 requests.

The versioned voucher generator is a separate scale-test input. It writes a
bounded corpus one window at a time, rejects a window above the 32 MiB encoded
body limit before touching the caller's writer, and returns only counts, byte
length, and a domain-separated SHA-256 digest. It never turns simulator output
into live-Tally evidence. The qualification controller pre-generates these
windows before starting a fresh parser worker so generator memory and time are
outside the measured process.

The master generator produces up to 50,000 deterministic ledger masters with
strict company/schema/count evidence. It preflights the selected UTF-8 or
BOM-qualified UTF-16 wire representation against the same 32 MiB encoded-body
ceiling before returning bytes. Its counts and hashes are repository-synthetic
evidence, not a claim that a real Tally profile can export that size.

```rust
use std::time::Duration;
use tally_protocol_simulator::{Delivery, Fixture, ScenarioPlan, Simulator};

let plan = ScenarioPlan::new(Fixture::ExportStatusZero)
    .with_delivery(Delivery::SlowBody {
        chunk_bytes: 8,
        delay: Duration::from_millis(20),
    });
let simulator = Simulator::spawn(plan)?;
// Point a test client at simulator.address().
# Ok::<(), std::io::Error>(())
```

## Fixture rules

- Use only names prefixed with `BRIDGE SYNTHETIC` and UUIDs in the reserved
  synthetic range used by this directory.
- Do not copy output from a real company or Tally installation.
- Do not add emails, phone numbers, GST registrations, home-directory paths, or
  developer usernames.
- Keep transport failure separate from application failure. HTTP 200 plus
  `STATUS=0` is an application rejection, not a successful empty collection.
- Add a deterministic assertion whenever a fixture is added.
- Use `Fixture::InconsistentDateFilter` when a test needs a declared July
  window whose synthetic response improperly contains a June voucher; accepting
  that row as canonical state is always a defect.

UTF-16 fixtures are derived byte-for-byte from their UTF-8 source by
`ScenarioPlan::response_bytes`; binary duplicates are intentionally not checked
in. This keeps the canonical fixture reviewable while testing both endian BOMs.
