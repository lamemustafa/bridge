# `wa1-payment-*` — provenance

One `verify_import` of a single Payment that Bridge posted through its own post path, captured
byte for byte at the wire, with the journal lines and saved import file that post left behind. It
is the fixture for comparing the single-voucher review dialog (`acknowledge_post_review`) with the
post dialog for the same voucher (#730), on a readback whose debit carries trailing zeros.

## Provenance

- **Host / gateway:** TallyPrime **Silver (licensed)**, `http://127.0.0.1:9001`, TallyPrime 7.1,
  reached through a byte-recording pass-through proxy on `127.0.0.1:9102` that forwards each
  request to completion, never retries and does not alter bytes. Only synthetic companies were
  loaded (the extent and high-water responses list them); no client or client-derived book.
- **Date:** 2026-09-26, 15:21 IST.
- **The post:** a debug `bridge_mcp` built from #721's head with only the batch cap raised (the post
  path unchanged) posted one saved batch of ONE Payment, dated 2026-04-03, Dr `Test Expense B` 1.00
  / Cr `Cash` 1.00, to the synthetic company `BRIDGE AMEND LAB` after one native approval. Tally
  answered `CREATED 1`; it read back `posted_verified`. The post dialog showed `Dr 1.00` and
  `Cr 1.00`.
- **The capture:** one `verify_import` of that batch through the proxy, 30 exchanges in the order
  `SEESESEHSHSEECSCSEEVSVSEEVSVSE` (status probe, company extent, company high water, voucher
  census, import verification). Every repeat of a request received the same response. The
  extent and high-water requests equal those of the `d3-batch-*` capture; the census and
  import-verification requests are this batch's own.
- **The readback:** the one voucher reads back with `-1.00` on the debit (ISDEEMEDPOSITIVE Yes)
  and `1.00` on the credit: Tally's own digits, with trailing zeros.
- **Local state, not wire bytes:** `wa1-payment-journal.jsonl` holds the post's six journal lines
  (build, the pre-post read, dispatch intent, response, two verified readbacks) as written, and
  `wa1-payment-import.xml` the saved import file. Later that day the voucher was marked Optional
  in Tally; the capture and the journal lines here predate that. The test rewrites the journal's
  `endpoint_origin` (the proxy's) to the simulator's address. The masters doubt the test reviews
  is written by the test itself; the live post had none.
- **Encoding:** the four responses are **BOM-less UTF-16LE**, exactly as received.

| file (in `agent/`) | bytes | sha256 |
|---|---|---|
| `wa1-payment-company-extent.utf16le.xml` | 7234 | `85b04c5cd61dffdf0506466074f9b77a4ceb3aa109ecbf4ed9204ceff8b6ceb7` |
| `wa1-payment-company-high-water.utf16le.xml` | 4826 | `b01141df3ee02fcbbf6a43e667ca6b1c3bafbde0d4063432737a5de410a128ee` |
| `wa1-payment-voucher-census.utf16le.xml` | 5468 | `d92b4aff960009b5f42d4ba77512894989efee1a77ff50afe50b8362b3dd7be7` |
| `wa1-payment-import-verification.utf16le.xml` | 8064 | `98ab2bef4b3d7e49bb5b3af755ccc5ca5b0119296032d09a383d5d8954e612cd` |
| `wa1-payment-journal.jsonl` | 2742 | `2f979b1fa4335ab97744d609eb79a57a0de69a68e5795517f04014a3858b3136` |
| `wa1-payment-import.xml` | 1037 | `0c2ee2981538ea3725583b197e8c2bea007bec264d1e96d61916b08ddf65f1f3` |

## What it establishes

- Tally returns a single Payment's amounts in the import-verification read as `-1.00` / `1.00`:
  two decimals, trailing zeros kept, the debit signed negative.
- The order of one single-voucher `verify_import`: the same 30 exchanges as a batch's.

## What it does not establish

- Any amount without two decimals, or a currency with other precision.
- Any voucher type but Payment, or any company but this synthetic one.
