# `statement_*` — provenance

Tally's built-in Balance Sheet and Profit and Loss, requested by report name (protocol reference §12a.1, §12a.11). These are the fixtures for `native_statement_reports.rs` (#692). Every file is a live capture, never hand-written.

## Provenance

- **Host / gateway:** TallyPrime **Silver (licensed)**, 7.1, `education_mode=false`, `http://127.0.0.1:9001` (a lab instance).
- **Date:** 2026-09-26.
  - The statement captures were taken between 11:14 and 11:17 IST. `tally_status` was healthy before and after that session.
  - The group tree and Trial Balance were taken between 16:53 and 16:55 IST. `tally_status` was healthy before them. In the same minutes, the Balance Sheet and Profit and Loss were requested again and came back byte-identical to the 11:14 captures (the same SHA-256), so the book had not changed.
- **Encoding:** responses are **BOM-less UTF-16LE**, exactly as received. Requests are the exact bytes sent: **UTF-16LE with a BOM**. `.gitattributes` marks this tree `-text`.
- **Request shape:**
  - A statement request is §12a.1's built-in report request. Its text is what `render_native_statement_request` renders (a test asserts this).
  - The group tree and Trial Balance requests are the production `render_native_group_snapshot_request` and `render_native_trial_balance_request` text, taken from those functions' format strings.
- **Companies:** synthetic lab books only.
  - `BRIDGE READS LAB`: 3 vouchers.
  - `BRIDGE CORPUS DENSE`: a generated book of 29,900 vouchers.
  - The responses carry no company identity (§12a.1).

| file | bytes | sha256 | company | report | window |
|---|---|---|---|---|---|
| `statement_balance_sheet_fy_live.utf16le.xml` | 1822 | `6c13df52e220f5be9881af6c2b98bdf5554af2791c79465a6c5535058868bf45` | `BRIDGE READS LAB` | Balance Sheet | 2025-04-01 to 2026-03-31 |
| `statement_profit_and_loss_fy_live.utf16le.xml` | 678 | `b0c50c1676fbc83585ed52dac8942709b05f6a77ef5c5f2b20a51ad68339cbed` | `BRIDGE READS LAB` | Profit and Loss | 2025-04-01 to 2026-03-31 |
| `statement_balance_sheet_empty_month_live.utf16le.xml` | 1792 | `8e4c608d4864bb49a90abe78263846a58c0189d66693077f4f1719a79abb9213` | `BRIDGE READS LAB` | Balance Sheet | 2025-05-01 to 2025-05-31 (no activity) |
| `statement_balance_sheet_dense_month_live.utf16le.xml` | 1842 | `b06b1cef7a83457bed5d85e45c6d2ac6d36421a330764d786e05d5ac0ef40430` | `BRIDGE CORPUS DENSE` | Balance Sheet | 2025-05-01 to 2025-05-31 |
| `statement_balance_sheet_fy_request.utf16le.xml` | 758 | `82e01ff2547407d05c68c03bccdceaa3ea87a3a5b938e606baf0135fbd404daf` | `BRIDGE READS LAB` | request for the first row | as above |
| `statement_profit_and_loss_fy_request.utf16le.xml` | 762 | `0f2c8db6b265d2609d5e426f3fd33ef82df623c1f8b08e30c7d47b5799fd0a9b` | `BRIDGE READS LAB` | request for the second row | as above |
| `statement_groups_fy_live.utf16le.xml` | 54280 | `56c36221867d15e229f1f944d06be6d5f01bf69242151b362b3931740b9c14d8` | `BRIDGE READS LAB` | List of Groups (28 groups) | none (a master collection) |
| `statement_trial_balance_fy_live.utf16le.xml` | 11238 | `c85ddd0d95c4a0863a96e760bfe2eb19cd53227255b771a0b7b8143c0bfd2a2d` | `BRIDGE READS LAB` | native Trial Balance (7 ledgers) | 2025-04-01 to 2026-03-31 |
| `statement_groups_fy_request.utf16le.xml` | 1066 | `1eea1e25ad0a3bca798f3baf233afd7f9226dfaca253174674b7b0f15e8fe9cc` | `BRIDGE READS LAB` | request for the group tree | none |
| `statement_trial_balance_fy_request.utf16le.xml` | 1144 | `2b66b57d39579ec152f6c41ff96ea98185862af7cf55efdc536813bd470bec3d` | `BRIDGE READS LAB` | request for the Trial Balance | as above |

## What each capture establishes

- **Full-year Balance Sheet.** Five `BSNAME`/`BSAMT` pairs. `Current Liabilities` is `4250.00` and the `Profit & Loss A/c` line is `-4250.00`; the other three lines are empty. The name `Profit & Loss A/c` arrives with an `&amp;` entity.
- **Full-year Profit and Loss.** Two `DSPACCNAME`/`PLAMT` pairs: a `Cost of Sales :` heading with its main amount `-4250.00`, and `Purchase Accounts` with its sub-amount `-4250.00`.
- **Empty month.** The same five Balance Sheet lines with every amount element present and empty. This is the case a parser must not read as zero.
- **Heavy book, one month.** Large plain signed amounts (`222962422.38` and `-222962422.38`) with no digit grouping. The response took 0.23 s.
- **Tie to the native trial balance.** For the full year, the statement figures matched `trial_balance` for the same window (§12a.11).
- **One company's four sources.** The group tree, Trial Balance and both full-year statements are all `BRIDGE READS LAB` over one window.
  - `reports::statements` derives both statements from the first two and ties them to the last two.
  - Every compared line matched: Current Liabilities `4250.00`, the carried `Profit & Loss A/c` `-4250.00` and `Purchase Accounts` `-4250.00`.
  - Primary groups with no ledger are empty in Tally's statements and zero in the derivation.

## Known limits

- One release (7.1), Silver, two synthetic books.
  - Each statement request was run twice, hours apart, with identical bytes.
  - Each group tree and Trial Balance request was run once.
- No book here holds inventory, so no closing-stock line was observed.
- The explode flag, Education, Gold and foreign-currency books were not measured.
