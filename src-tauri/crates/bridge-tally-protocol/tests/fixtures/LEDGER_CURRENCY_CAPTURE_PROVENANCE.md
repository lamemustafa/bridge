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

### `ledgers_currency_forex_live`

- **Company:** `BRIDGE CORPUS FOREX` (synthetic). Base INR (currency master NAME `I₹`), plus a `$`
  master. Period `20250401`–`20250930`.
- **Rows:** 10 ledgers, `STATUS 1`. Every row carries `CURRENCYNAME` with text:
  - `$` on `BRIDGE FX DEBTOR A`, `FX USD Debtor 01` and `FX USD Debtor 02`;
  - `I₹` on the other seven, including `Cash`, `FX Sales` and `Profit & Loss A/c`.
- **The field changes nothing else.** With the `CURRENCYNAME` elements removed, the response is
  byte-identical to the same request without the field.
- **Two `$` ledgers close composite:** `-$ 1100.00 @ I₹ 86/$  = -I₹ 94600.00` and
  `-$ 2000.00 @ I₹ 86/$  = -I₹ 172000.00`.
- **`FX USD Debtor 01` closes as a plain `0.00`.** It is a `$` ledger whose balance shape alone would
  pass for rupees.

### `ledgers_currency_single_live`

- **Company:** `Bridge Billwise Lab` (synthetic; see `docs/tally/TEST_CORPUS.md`). One Currency
  master: NAME `Rs.`, ORIGINALNAME `Rs.`, MAILINGNAME `Indian Rupees`. Period
  `20240401`–`20240930`.
- **Rows:** 13 ledgers, `STATUS 1`. Every row, bill-wise debtors, `Cash` and non-party ledgers alike,
  carries `CURRENCYNAME` `Rs.`, the master's NAME. Every amount is a plain decimal.

## Known limits

- One release (7.1), one machine, two books.
- Whether older releases return `CURRENCYNAME` in this collection is not measured.
- The base master's NAME differs by book (`I₹`, `Rs.`), so a comparison must use the NAME read from
  the same book, never a constant.
