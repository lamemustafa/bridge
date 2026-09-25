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

## The compliance source's other reads, at the same moment (2026-09-25, 22:49-22:50 IST)

The same host, taken the same way, with only synthetic books loaded. The company-extent collection
was read before and after. Both responses are byte-identical and give the values above (`ALTVCHID` 18,
`ALTMSTID` 216), so the book did not change between the compliance master above (22:01) and these
reads. Together they are one moment of one book.

- `company_extents_forex_live`: the extent collection (`CompanyBookExtentV2`), the "before" read. It
  lists every loaded company, all of them synthetic lab books.
- `balance_snapshot_forex_live`: the compliance source's balance snapshot
  (`render_native_ledger_snapshot_request`), over `20250401`–`20261001`, the runtime's closing
  boundary after `LASTVOUCHERDATE`. 10 ledgers, each with `CURRENCYNAME`.
- `group_snapshot_forex_live`: the compliance source's group snapshot
  (`render_native_group_snapshot_request`). 28 groups. Its `PARENTSTRUCTURE` values hold raw U+0003
  separators, which a strict XML parser refuses.

| file | bytes | sha256 |
|---|---|---|
| `company_extents_forex_live.utf16le.xml` | 6,586 | `4e6eeaccb3f2d6e149285151942a5d088b6894248c5e86bb2df47917266eaa83` |
| `balance_snapshot_forex_live.utf16le.xml` | 15,768 | `55cfe08de04f5eec29eba85fcf7363e840af29c3992f1496c32538399404ab93` |
| `group_snapshot_forex_live.utf16le.xml` | 54,280 | `35d78956890fc78f7b019d50d1a148d0c8c40da8bf339904e0aa94aad1bda0c2` |
