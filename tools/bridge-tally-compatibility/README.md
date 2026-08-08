# Bridge Tally compatibility evidence

This crate defines a separate authority boundary for live, read-only Tally
qualification. It does not perform network requests and can never emit a
support claim. Raw receipts are privacy-reduced observations with checksum-only
integrity; positive release claims additionally require an exact-scope,
maintainer-reviewed Ed25519 attestation from a configured non-revoked key.

The contract deliberately excludes raw requests/responses, endpoint ports,
company or record identifiers, accounting values, local paths, and arbitrary
labels. It permanently states that responder authenticity, accounting
correctness, source completeness/atomicity, performance support, and writes are
not established by this evidence. Portable receipts also permanently record
that the Tauri runtime was not observed.

The companion CLI validates receipts, seals deterministic source-surface
manifests, and enforces structured release claims. The later live controller
must depend only on sealed read templates and produce this DTO; it must not
expose generic XML dispatch or any write surface.
