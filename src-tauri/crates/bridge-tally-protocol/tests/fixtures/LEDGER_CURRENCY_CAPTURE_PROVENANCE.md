# `ledgers_currency_*_live`: provenance

A ledger's own currency, `CURRENCYNAME`, captured on the production `List of Ledgers` snapshot
(bridge#551). A foreign-currency ledger's bills arrive from the Bills reports as plain amounts, and a
zero-balance foreign ledger closes as a plain `0.00`. The shape of an amount therefore cannot tell a
foreign ledger from a base one; its currency can.

## Provenance

- **Host / gateway:** TallyPrime **Silver (licensed)** 7.1, `education_mode` false. `/status` was
  healthy before and after each request.
- **Date:** 2026-09-22 (FOREX) and 2026-09-23 (single-currency book).
- **Request:** the production `List of Ledgers` snapshot (`render_native_ledger_snapshot_request`)
  with `CURRENCYNAME` appended to its `FETCH`:
  `FETCH NAME, PARENT, CLOSINGBALANCE, OPENINGBALANCE, ISBILLWISEON, CURRENCYNAME`, plus the
  `BRIDGECOMPANYGUID` compute.
- **Encoding:** BOM-less UTF-16LE, the undecoded wire bytes.

| file | bytes | sha256 |
|---|---|---|
| `ledgers_currency_forex_live.utf16le.xml` | 15,608 | `79c30834ca545015c9bcde9d3bc61035f5d33cc29dfbe840332084f8c47f3195` |
| `ledgers_currency_single_live.utf16le.xml` | 19,320 | `9da6db208e08d3f23be380f587cf16b5f7941fb54a32615bd34424aebc451d91` |
| `bills_receivable_forex_live.utf16le.xml` | 8,176 | `578b3337c295d70e7c643414e575ed05b19d012afd88733c972294e5eb8dc568` |

### `ledgers_currency_forex_live`

- **Company:** `BRIDGE CORPUS FOREX` (synthetic). Base INR (currency master NAME `I₹`), plus a `$`
  master. Period `20250401`–`20250930`.
- **Rows:** 10 ledgers, `STATUS 1`. Every row carries `CURRENCYNAME` with text:
  - `$` on `BRIDGE FX DEBTOR A`, `FX USD Debtor 01` and `FX USD Debtor 02`;
  - `I₹` on the other seven, including `Cash`, `FX Sales` and `Profit & Loss A/c`.
- **The field changes nothing else.** With the `CURRENCYNAME` elements removed, the response is
  byte-identical to the same request without the field.
- **Three balances are composite:** the closings `-$ 1100.00 @ I₹ 86/$  = -I₹ 94600.00` and
  `-$ 2000.00 @ I₹ 86/$  = -I₹ 172000.00`, and `BRIDGE FX DEBTOR A`'s opening
  `-$ 500.00 @ I₹ 84/$  = -I₹ 42000.00`.
- **`FX USD Debtor 01` closes as a plain `0.00`.** It is a `$` ledger whose balance shape alone would
  pass for rupees.

### `bills_receivable_forex_live`

- **Request:** the production Bills Receivable request (`render_native_bills_request`), same book and
  period as `ledgers_currency_forex_live`, read the same session (2026-09-22).
- **Rows:** 18 open bills, every `BILLCL` a plain decimal:
  - 4 on the three `$` ledgers: `FX-OPEN-1` `-43000.00` and `FX-INV-1` `-51600.00` on
    `BRIDGE FX DEBTOR A`, `FXU-INV-001` `-83500.00`, `FXU-INV-002` `-172000.00`;
  - 14 on the rupee ledgers.
- **Opening bills:** `FX-OPEN-1` and the rupee `INR-OPEN-1` are dated and due `31-Mar-25`, the day
  before the book's `BOOKSFROM` (20250401, as reported by the capturing session; `BOOKSFROM` is not
  in these bytes), so the date parser must admit bills dated before
  `BOOKSFROM` (bridge#612; `TALLY_PROTOCOL_REFERENCE` §12a.10).
- **Why it is committed:** none of the four dollar bills carries a currency marker, so only the
  ledger snapshot's `CURRENCYNAME` identifies them. With the snapshot above, this is the pair a
  classified outstandings read is tested on. It is also the captured case for bridge#612.

### `ledgers_currency_single_live`

- **Company:** `Bridge Billwise Lab` (synthetic; see `docs/tally/TEST_CORPUS.md`). One Currency
  master: NAME `Rs.`, ORIGINALNAME `Rs.`, MAILINGNAME `Indian Rupees`, as that book's own currency
  read reported them in the same session. That read is not committed here. Period
  `20240401`–`20240930`.
- **Rows:** 13 ledgers, `STATUS 1`. Every row, bill-wise debtors, `Cash` and non-party ledgers alike,
  carries `CURRENCYNAME` `Rs.`, the master's NAME. Every amount is a plain decimal.

## `groups_forex_live`, `bills_payable_forex_live`: the rest of FOREX's outstandings read (bridge#551)

Captured so that a classified outstandings read can be tested end to end on FOREX from captures
only, with `ledgers_currency_forex_live` and `bills_receivable_forex_live` above.

- **Host / gateway:** TallyPrime **Silver (licensed)** 7.1, `education_mode` false; `/status`
  byte-identical before and after.
- **Date:** 2026-09-23, 10:57–10:59 +0530, one request at a time, read-only. Book `BRIDGE CORPUS
  FOREX` (synthetic), period `20250401`–`20250930`.
- **Requests:** the production renderers of master `2db6a9c5`: the group snapshot
  (`render_native_group_snapshot_request`) and Bills Payable (`render_native_bills_request`).
- **Encoding:** BOM-less UTF-16LE, the undecoded wire bytes.
- **Same session:** that session's Bills Receivable and ledger-snapshot responses are byte-identical
  to `bills_receivable_forex_live` and `ledgers_currency_forex_live` (captured 2026-09-22).

| file | bytes | sha256 |
|---|---|---|
| `groups_forex_live.utf16le.xml` | 54,282 | `9beb3431445a7edca3bb2dcceccb110d69ff40dc447a9667d64ea4f3632d2bc4` |
| `bills_payable_forex_live.utf16le.xml` | 46 | `8d37111f1de57f9c4d5ea3e984d10db165c5a8b28a0c0ec6b2688ffbc61d5ad3` |

- **`groups_forex_live`:** `STATUS 1`, 28 groups. Every GUID in it has FOREX's company-GUID prefix
  `b14e9b2d`: the collection is scoped to the one company named.
- **`bills_payable_forex_live`:** the bare `<ENVELOPE></ENVELOPE>` that a `Data` report returns when
  the book has no open payable bill. A second send in the same session returned the same 46 bytes.

## Known limits

- One release (7.1), one machine, two books.
- Whether older releases return `CURRENCYNAME` in this collection is not measured.
- The base master's NAME differs by book (`I₹`, `Rs.`), so a comparison must use the NAME read from
  the same book, never a constant.
