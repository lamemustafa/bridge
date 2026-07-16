# Bridge Tally protocol

Portable production parsing for Bridge's Tally integration. This crate owns:

- bounded byte decoding for UTF-8, UTF-8 BOM, UTF-16LE, and UTF-16BE;
- strict export-envelope `STATUS` handling;
- company, ledger, and voucher parsing;
- exact import counters without retaining raw `LINEERROR` text;
- optional company-context and duplicate-identity evidence.

It intentionally has no HTTP client, Tauri, database, SQLCipher, or native
dependency. `tally-protocol-simulator` is a development dependency and supplies
the synthetic compatibility corpus.

Bridge retains its existing `tally::xml_parser` API as a thin re-export of this
crate. New callers that need verification evidence can use
`parse_*_with_evidence`; legacy `parse_*` functions still return the original
record vectors.

Duplicate source identities are exposed only as occurrence counts and SHA-256
digests. Raw duplicate identifiers and raw line-error messages are not retained
in evidence or error strings.

## Native JSONEX evidence boundary

The optional `jsonex-parser` feature contains a portable, bounded parser for
the exact Ledger and Voucher collection-envelope shapes documented for
TallyPrime 7.0+. Its results are deliberately named `Unbound`: those official
examples do not echo enough evidence to bind the response to a requested
company, date range, Bridge query profile, or complete Core Accounting source
scope.

The separate optional `jsonex-request-builder` feature produces deterministic
bytes for only two versioned official-example logical profiles: the Ledger
collection payload with exact `fetch_List` spelling, and the unbounded
`TSPLVoucherColl` payload. It fixes every request header/value and TDL literal,
requires an exact validated company name, binds BOM bytes to the declared
charset, and marks every result ineligible for dispatch, company verification,
or date-range claims. Company input also shares Bridge's 255-byte local safety
cap; this is an application resource bound, not a documented Tally limit.

Every wire serialization remains live-unverified. In particular, the BOM modes
combine the plain DOCX logical body with Tally's documented charset/BOM rules;
they are not byte-for-byte copies of Tally's separate multilingual examples,
which currently use different export-format/fetch spellings. Those alternatives
require separate versioned profiles and evidence rather than silent aliases.

The Bridge application enables neither feature. They add no HTTP client,
runtime transport selection, canonical adapter, mirror write, checkpoint, or
Tally write path. Both are exercised separately in CI so the evidence code
cannot silently decay while production JSONEX remains unavailable.
