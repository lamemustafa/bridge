# `ledgers_forex_composite_live` — provenance

The composite foreign-currency `CLOSINGBALANCE` shape, captured rather than constructed.

Before this file, the only occurrence of that shape in the repository was a hand-authored string in
`native_outstandings/wire.rs`. It **did not match what Tally emits** — measured 2026-08-24:

```
Tally:    '-$ 2000.00 @ I₹ 84/$  = -I₹ 168000.00'      <- two spaces before '='
authored: '-$ 2000.00 @ I₹ 84/$ = -I₹ 168000.00'       <- one
```

The classifier accepts both — it splits on `@` and `=` and trims — so no defect shipped. But the
authored fixture could never have detected a spacing, qualification, or encoding mismatch, because it
*was* the mismatch. That is the reason this capture exists.

## Provenance

- **Host / gateway:** TallyPrime **Silver (licensed)** 7.1, `http://localhost:9001`
- **Date:** 2026-08-24
- **Company:** `BRIDGE CORPUS FOREX` (synthetic; base INR, one `$` currency master)
- **Request:** the production `List of Ledgers` collection —
  `FETCH NAME, PARENT, CLOSINGBALANCE, OPENINGBALANCE, ISBILLWISEON`, `SVFROMDATE 20250401`,
  `SVTODATE 20250730` (the book's own extent)
- **Encoding:** BOM-less UTF-16LE, undecoded wire bytes. `.gitattributes` marks this tree `-text`.
- **`/status`:** healthy before and after, gated (aborts rather than proceeding on an unhealthy check)

| file | bytes | sha256 |
|---|---|---|
| `ledgers_forex_composite_live.utf16le.xml` | 10,580 | `4941f30826ec51da9ab1c834abb1abcd711ffec22464044d5c669b77aaa313f8` |

`STATUS 1`, 8 ledgers, exactly one of which carries a composite balance.

## What it establishes

Fed to the production `parse_native_ledger_snapshot`, these bytes produce:

```
Err(ForeignCurrencyLedgerBalance { ledger_name: "FX USD Debtor 02" })
"Tally reported a foreign-currency closing balance for ledger FX USD Debtor 02"
```

So the typed diagnostic and the ledger name are confirmed against real wire bytes, not against a
string written to match the parser.

**Bridge still fails closed here, deliberately.** No attempt is made to parse the composite string
or to fall back to its trailing base-currency figure. `BILLCL` on the same bill remains a plain
decimal; only the ledger balance changes shape.

## Known limits

- One release (7.1), one machine, one currency pair (`$` against `I₹`).
- Only a *negative* composite balance appears. A positive one, and a ledger whose foreign balance is
  zero, are not represented.
- The seven other ledgers in this book carry ordinary decimal balances, so the capture also serves as
  its own control: the classifier must reject exactly one row, not all eight.
