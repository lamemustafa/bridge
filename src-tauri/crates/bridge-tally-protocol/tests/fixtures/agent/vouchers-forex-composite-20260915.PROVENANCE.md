# `vouchers-forex-composite-20260915`: provenance

A live `vouchers` window read of the synthetic several-currency lab book, holding one voucher whose amounts Tally stored as a foreign-currency composite (#674).

## Capture

- **Host / gateway:** TallyPrime **Silver (licensed)**, 7.1, `education_mode=false`, `http://127.0.0.1:9001` (a lab instance).
- **Date:** 2026-09-25, 14:16 IST.
- **Company:** `BRIDGE CORPUS FOREX`, a synthetic lab book. Its voucher mark (ALTVCHID) was 18 at the time; a later receipt moved it to 19, and this file predates that receipt.
- **Window:** 2026-09-15 to 2026-09-15, one request, one part.
- **Request:** the text is in `vouchers-forex-composite-20260915.request.xml`. It is byte for byte what `render_agent_vouchers` renders today for this company and window, unnarrowed, with the `AGENT_VOUCHER_FETCH` list, and a test asserts it.
- **Response:** BOM-less UTF-16LE, exactly as received.

| file | bytes | sha256 |
|---|---|---|
| `vouchers-forex-composite-20260915.utf16le.xml` | 37190 | `2995928746502093128777901fa34cd9cdb70cdbeab4112593702786d066bdc9` |
| `vouchers-forex-composite-20260915.request.xml` | 858 | `dd2352aff9aa5b0b4bed50870d8d4bd27886b1067aa107e7b428e73fde95b324` |

## What it establishes

- **One voucher.** It is a Sales voucher (ALTERID 18) on a rupee party ledger, entered as `$100 @ 86`, with a bill-wise New Ref.
- **The composite.** Tally stores `-$ 100.00 @ I₹ 86/$  = -I₹ 8600.00` as the AMOUNT of the party entry and of its bill allocation, and `$ 100.00 @ I₹ 86/$  = I₹ 8600.00` as the AMOUNT of the sales entry.
- **Elsewhere in the voucher.** Two `VATEXPAMOUNT` elements, one on each entry, carry the entry's composite too; the voucher parser does not read that element.
- **The failure it reproduces.** Before #674, `vouchers` refused this whole window with `bill_allocation_amount_invalid`.

## Screening

- Every name in the response is a synthetic lab name.
- The response's `PARTYGSTIN` element is present and empty; it carries no PAN, contact or address value.
- The company name appears only in the request.

## Limits

- One release (7.1), Silver, one book, one run, one currency (`$`) at one rate.
- The book's second composite voucher, a receipt with a foreign bill allocation dated the same day, came after this capture.
