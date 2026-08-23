# `vouchers_ageing_corpus_live` and `vouchers_gst_credit_periods_live` — provenance

Two captures taken to replace the hand-authored `ageing_vouchers_xml` helper in
`tests/outstandings.rs`, which built the response by hand — including the exact `BILLCREDITPERIOD`
spelling the parser expects — and so could only ever prove that Bridge agrees with itself.

## Provenance

- **Host / gateway:** TallyPrime **Silver (licensed)**, `http://localhost:9001`.
  Response header reports `PRODMAJORREL 7 / PRODMINORREL 1`, i.e. TallyPrime 7.1.
- **Date:** 2026-08-23
- **Encoding:** **BOM-less UTF-16LE**, exactly as received. These are the undecoded wire bytes, not
  decoded text. `.gitattributes` marks this tree `-text`, so they are safe from newline rewriting.
- **Request shape:** the production `BridgeVoucherExport` collection — `SYSTEM TYPE="Formulae"` plus
  `<FILTERS>`, with `ALLLEDGERENTRIES.BILLALLOCATIONS.*` fetched.
- **`/status`:** healthy before and after each request.

## Companies — synthetic, built for this purpose

Both books were created over the XML import path on 2026-08-23. No PII, no real GSTIN, no real
company data.

| file | bytes | sha256 | company |
|---|---|---|---|
| `vouchers_ageing_corpus_live.utf16le.xml` | 44,156 | `497aec1804603b5c79a6ece404554c1f0ee1fb005ce3187b627f3292c51605f6` | `BRIDGE CORPUS AGEING` |
| `vouchers_gst_credit_periods_live.utf16le.xml` | 267,252 | `1e340126eda8e767d2b53cab8bb2086add1ed35f53f7216a16edbc16624b30b8` | `BRIDGE CORPUS GST` |

## What each capture establishes

### `vouchers_ageing_corpus_live` — the bucket straddle

Eight bills whose `(age, credit period)` pairs deliberately straddle the 30/60/90 boundaries in
opposite directions. As of 2026-03-31, **seven of eight fall in different buckets** under bill-date
and due-date ageing; the eighth has a zero credit period and is the control.

Before this book existed, issue #114 was unfalsifiable: every bill in the reference corpus had an
empty credit period, so the two methods coincided and any implementation passed.

### `vouchers_gst_credit_periods_live` — the unit variants and multi-entry vouchers

Carries **five distinct credit-period serialisations as Tally actually writes them**:

```
'15 Days'   '30 Days'   '2 Weeks'   '1 Months'   '2 Months'
```

This is the only artefact in the project containing real **month** and **week** serialisations. A
parser requiring a `" Days"` suffix fails the entire outstandings read on any of them — which is a
defect that shipped into review on 2026-08-23 precisely because every generated corpus emitted only
`" Days"`.

It also carries vouchers with **both 3 and 4 ledger entries** (inter-state IGST, and intra-state
CGST+SGST). Every other book in the project has exactly two entries per voucher, so this is the only
fixture that can expose an assumption about voucher arity.

**No GSTIN appears in the voucher payload**, so no sanitisation was applied or required. The party
GSTINs held on the master records are deliberately non-conforming in any case — PAN block
`ZZZZZ0000Z`, checksum not computed — and cannot collide with a real registration.

## Known limits

- One product tier (licensed Silver), one release (7.1), one machine.
- 2026 is not a leap year, so no 29-February clamp appears in these bytes.
- Recorded but **not** present here: Tally normalises a submitted `"1 Day"` to `"1 Days"` on write.
  That was measured separately; no bill in these captures carries a singular form.
