# `d3-batch-*` — provenance

One `verify_import` of a 50-voucher batch that Bridge posted through its own post path, captured
byte for byte at the wire, with the journal and saved import file that post left behind. It is the
fixture for reviewing a doubted batch end to end (`acknowledge_post_review` over 50 vouchers).

## Provenance

- **Host / gateway:** TallyPrime **Silver (licensed)**, `http://127.0.0.1:9001`, TallyPrime 7.1,
  education mode off. Only synthetic companies were loaded; no client or client-derived book.
- **Date:** 2026-09-26.
- **The post (slice D3):** a debug `bridge_mcp` built from the batch-posting branch at `c4cd4e08`, with
  posting and batch posting switched on, posted one saved batch of 50 Journals (1.00 to 50.00,
  Dr `Test Expense B` / Cr `Cash`, dated 2026-04-01) to a synthetic company after one native
  approval. Tally answered `CREATED 50` and nothing else; the company's voucher mark moved from
  1,419 to 1,469; all 50 read back verified.
- **The capture:** the same binary ran one `verify_import` of that batch through a pass-through
  proxy that wrote every request and response verbatim and forwarded each to completion. The four
  distinct request kinds were checked byte for byte against Bridge's own: the company extents and
  import-verification reads against the hashes Bridge recorded, and the company high-water and
  voucher census reads against the same requests emitted from Bridge's builders in a test. Every
  repeat of a request received the same response.
- **Local state, not wire bytes:** `d3-batch-journal.jsonl` is the post's journal and
  `d3-batch-import.xml` the saved import file, both as written. The test rewrites the journal's
  `endpoint_origin` to the simulator's address, since a dispatched batch verifies only on the
  origin it recorded. The step doubt the test reviews is written by the test itself; the live post
  had none.
- **Encoding:** the four responses are **BOM-less UTF-16LE**, exactly as received.

| file | bytes | sha256 |
|---|---|---|
| `agent/d3-batch-company-extent.utf16le.xml` | 7246 | `b9fe8d5febd4ae22eae2d11b408ec311fcec3a21410829d67cd37f9ebace907c` |
| `agent/d3-batch-company-high-water.utf16le.xml` | 4818 | `fda1afb616fed9adf8a3408f63bcd04094bb5bab16c275c882ccfbafda77996c` |
| `agent/d3-batch-voucher-census.utf16le.xml` | 124420 | `eb90841da8d2237f43fee668b5219a3bb287e064d85ff27b3536c4ac003166ae` |
| `agent/d3-batch-import-verification.utf16le.xml` | 253666 | `e2f34ff30b486869aa8560742a1be5ba4abb4251dd0850f9f4eb0e254321ce0f` |
| `agent/d3-batch-journal.jsonl` | 17410 | `922b2c13173ce23e39a13ed067bda5f87098ebc719e6f42dddefce3719289b54` |
| `agent/d3-batch-import.xml` | 31052 | `1b7ff4ddc901fcab949218b78005ad7bd8870ac5cd34f00465f1bb71712dcec5` |

## What it establishes

- A 50-voucher readback of Bridge's own batch, as Tally returns it to the import-verification read:
  AlterIDs 1,420 to 1,469 in batch order, each voucher carrying its batch marker.
- The order of one `verify_import` on a book this size: a status probe and paired company-extent
  reads around the high-water, census and import-verification reads, 30 exchanges in all.
