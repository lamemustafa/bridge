# `currency_inr_modern_live`, `currency_inr_legacy_live`, `currency_multi_live` — provenance

Three captures of the base-currency collection, taken to replace the hand-authored `const LIVE` in
`native_outstandings/wire.rs` and `const CURRENCY` in `tally/runtime.rs`. Those constants were
written by hand — including the `MAILINGNAME` spelling the rule tests for — and so could only ever
prove that Bridge agrees with itself. See issue #172.

## Provenance

- **Host / gateway:** TallyPrime **Silver (licensed)**, `http://localhost:9001`, TallyPrime 7.1.
- **Date:** 2026-08-23
- **Encoding:** **BOM-less UTF-16LE**, exactly as received — undecoded wire bytes, not decoded
  text. `.gitattributes` marks this tree `-text`, so they are safe from newline rewriting.
- **Request shape:** the production `render_company_currency_request` output, verbatim.
- **`/status`:** healthy before and after each request.

| file | bytes | sha256 | company | symbol | `MAILINGNAME` | count |
|---|---|---|---|---|---|---|
| `currency_inr_modern_live.utf16le.xml` | 3,404 | `0dc84aa287cab1e1922db7e99a01f9f2b0bacd0d777fdd0b080adedc6622ed22` | `Bridge Validation Lab` | `I₹` | `INR` | 1 |
| `currency_inr_legacy_live.utf16le.xml` | 3,428 | `dcc3539205080c4272b42d333b693e6c90e1cdd6b9e9e080d4ea6b8ae2abb06e` | `Bridge Billwise Lab` | `Rs.` | `Indian Rupees` | 1 |
| `currency_multi_live.utf16le.xml` | 3,800 | `b64c0d5feb528fa02f81de576de5c766a95e1da1000975b1e2932868ae34118b` | `BRIDGE CORPUS FOREX` | `$` | `USD` | 2 |

## What each capture establishes

### `currency_inr_modern_live` — the form the rule currently rejects

An ordinary single-currency Indian company as **TallyPrime 7.1 creates it**: symbol `I₹`
(`U+0049` then `U+20B9`), mailing name `INR`. `is_inr` evaluates to `false` against this, which is
the defect in #172. This is Bridge's own long-standing reference company, not a book built for
corpus work — it predates the 2026-08 corpus generators entirely.

### `currency_inr_legacy_live` — the form the rule was written for

The older spelling, symbol `Rs.` and mailing name `Indian Rupees`. This capture is why the fix is
to *widen* the accepted set rather than replace one literal with another: both forms are live on
the same machine, so a customer base spanning Tally versions will present both.

### `currency_multi_live` — the case that must keep failing

Two currency masters defined, so which one is BASE is not determinable from this read.
`currency_count == 1` must continue to gate, and `is_inr` must stay `false` here. Guessing would
put a rupee symbol in front of a dollar balance.

## Known limits

- One product tier (licensed Silver), one release (7.1), one machine.
- Six of the fourteen books on this machine were sampled for the table in #172; the three captured
  here are the distinct shapes among them.
- No company on this machine defines a single **non-Indian** currency, so that case
  (`count == 1`, mailing name `US Dollars`) remains covered only by a constructed variant.
