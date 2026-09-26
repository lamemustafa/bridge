# `d3-cancelled-*` — provenance

One `verify_import` of the same 50-voucher batch as `d3-batch-*` (see
`D3_BATCH_CAPTURE_PROVENANCE.md`), captured byte for byte at the wire after a person cancelled one
of its vouchers in Tally's own screen. It is the fixture for how a cancelled voucher reads back
(bridge#758).

## Provenance

- **Host / gateway:** TallyPrime **Silver (licensed)**, `http://127.0.0.1:9001`, TallyPrime 7.1,
  education mode off. Only synthetic companies were loaded; no client or client-derived book.
- **Date:** 2026-09-26, about 16:02 IST.
- **What changed before the read:** in a supervised sitting, the owner opened D3-003 (voucher
  number 3) in alteration and cancelled it (Alt+X). The company's voucher mark moved from 1,684 to
  1,685 and the voucher's ALTERID from 1,422 to 1,685. Earlier in the same sitting, D3-001 and
  D3-002 were each altered at the screen (a cost-centre allocation, and a save with no change),
  which moved their ALTERIDs to 1,680 and 1,677; the verification read does not fetch the fields
  those edits touched.
- **The capture:** the same debug `bridge_mcp` as the `d3-batch-*` capture (built from the
  batch-posting branch at `c4cd4e08`, byte-identical by SHA-256) ran one `verify_import` of the batch through a pass-through proxy that wrote every request and response
  verbatim and forwarded each to completion: 30 exchanges, all status 200. The four distinct
  request bodies are byte-identical to the four in the `d3-batch-*` capture (same SHA-256), and
  every repeat of a request received the same response.
- **Local state:** the test reuses `d3-batch-journal.jsonl` and `d3-batch-import.xml`, the post's
  journal and saved file, unchanged by the cancel.
- **Encoding:** the four responses are **BOM-less UTF-16LE**, exactly as received. Nothing is
  derived or edited.

| file | bytes | sha256 |
|---|---|---|
| `agent/d3-cancelled-company-extent.utf16le.xml` | 7254 | `b2574fe3ed303e9ce7052232b331177e0a8432073820527768c4440429aabbbd` |
| `agent/d3-cancelled-company-high-water.utf16le.xml` | 4826 | `95fcf6ddbb823effba3ca152c9deeb5a5668b39719bd9f74284556eeb6cd4a89` |
| `agent/d3-cancelled-voucher-census.utf16le.xml` | 124410 | `743dcd652b3d8470975056c07b3a4643a025866404bf902bc0a13ea6d1d541e5` |
| `agent/d3-cancelled-import-verification.utf16le.xml` | 251982 | `95462269afc4fce07a7b746eb474e6ad6014c4034b72b523ac978c58dfbde975` |

## What it establishes

- A cancelled voucher stays in the import-verification read with `ISCANCELLED` Yes, its voucher
  number (3), its narration marker, GUID and MASTERID, and its new ALTERID (1,685). Its
  `ALLLEDGERENTRIES.LIST` is present but empty, and it has no voucher-level `ISDEEMEDPOSITIVE`.
- So a cancelled voucher's entries can never match its build, and its content comparison cannot
  tell a cancel from an edit.

## Limits

One run, one book, one release (TallyPrime 7.1 Silver). A voucher cancelled by any other route
(for example, through the gateway) is not covered.
