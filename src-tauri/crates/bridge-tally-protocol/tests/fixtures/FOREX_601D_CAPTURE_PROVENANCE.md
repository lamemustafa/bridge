# `compliance_master_forex_live` and `trial_balance_*_forex_live`: provenance

The compliance ledger master and the native Trial Balance of a book with several Currency masters
(bridge#551), captured so the compliance and Trial Balance reads can be tested on the wire bytes
Tally returns, not on strings written to match a parser.

## Provenance

- **Host / gateway:** TallyPrime **Silver (licensed)** 7.1, `education_mode` false. `/status` was
  healthy before and after each request. Only synthetic lab books were loaded.
- **Date:** 2026-09-25, between 22:01 and 22:02 IST. Each request was sent once, and each answered
  in under 0.1 s.
- **Company:** `BRIDGE CORPUS FOREX` (synthetic). Base INR (currency master NAME `I₹`), plus a `$`
  master. The day's extent read, taken first, gave `BOOKSFROM` 20250401, `LASTVOUCHERDATE` 20260915,
  `ALTVCHID` 18 and `ALTMSTID` 216.
- **Requests:** the bytes Bridge's own builders emit, not written by hand, over `20250401`–`20260915`:
  - `compliance_master_forex_live`: the compliance master request (`render_party_ledger_master_request`);
  - `trial_balance_forex_live`: the native Trial Balance (`render_native_trial_balance_request`);
  - `trial_balance_currency_forex_live`: the same Trial Balance with `CURRENCYNAME` appended to its
    `FETCH` (`render_native_trial_balance_request_with_currency`). That is its only difference.
- **Encoding:** BOM-less UTF-16LE, the undecoded wire bytes.

| file | bytes | sha256 |
|---|---|---|
| `compliance_master_forex_live.utf16le.xml` | 21,714 | `36fe16f1c5f65752c4c75d84b35988f8472e45f62b0641bbfbb1098d8478817f` |
| `trial_balance_forex_live.utf16le.xml` | 15,476 | `2d797a9baf9aa7f66076ba7486db85d60a03edc0052d438c1399278aeeb32131` |
| `trial_balance_currency_forex_live.utf16le.xml` | 16,510 | `a14577d2606a0ec43e73048caca809089fe9bd4a568fb6a35ef8c7407c8c99b7` |

## What the bytes show (one book, one run: PARTIAL)

- **Compliance master:** 10 ledgers. `BRIDGE FX DEBTOR A`'s `OPENINGBALANCE` is the composite
  `-$ 500.00 @ I₹ 84/$  = -I₹ 42000.00`. The request fetches no `CURRENCYNAME`, and one row's
  `PARENT` holds the reserved `&#4;` value.
- **Trial Balance with `CURRENCYNAME`:** every one of the 10 rows carries it: `$` on 3 and `I₹` on 7.
- **A ledger's currency does not predict whether its values are composite.** Three `I₹` rows carry
  dollar composites:
  - `FX Party 01`, in `DEBITTOTALS` and `TBALCLOSING`: `-$ 100.00 @ I₹ 201/$  = -I₹ 20100.00`;
  - `FX Sales`, in `CREDITTOTALS` and `TBALCLOSING`: `$ 100.00 @ I₹ 3786/$  = I₹ 378600.00`;
  - `Profit & Loss A/c`, in `TBALCLOSING`: `$ 0.00 @ I₹ /$  = I₹ 0.00`. Its rate slot is empty.
  The rates are derived (base total divided by the dollar component), not rates any voucher used.
- **The `=` parts do not tie:** summed over all ten rows, they come to 34,500.
