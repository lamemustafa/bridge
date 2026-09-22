# Batch 2a: creditor_ageing_43bh and statutory_dues_43b

Both tests are eligible under the freeze bar: each does real work on two real books. The local
measurement, counts only:
- creditor_ageing_43bh: 3 findings on one client and 50 on another.
- statutory_dues_43b: 4 findings and 6 non-zero figures on one client, and 2 findings (RCM GST)
  on another.

## Held, and why

The same bar holds these tests, measured by the reference's own `pack._compute` on the three
clients (counts only):
- `counter_cheques_40a3` and `narration_payees`: findings on one client only.
- `entity_269st_gap`: no findings on any client; no person took cash on one day through two
  ledgers.
- `specified_persons_40a2b`: only its `applicable` figure on every client.
- `related_parties_cl23`: substantive figures on one client only; the other two carry only its
  `applicable` figure and one finding.

## Reference commit

Every golden here was written by the reference implementation at `57f2619b`, the same commit as
batch 1's goldens, under `uv run --python 3.13`. The four reference modules these tests read
(`creditor_ageing_43bh`, `statutory_dues_43b`, and the `config` and `binding` loaders they use)
are unchanged between that commit and the engine branch's current tip.

## What the fixtures establish

**Synthetic engagement.** `synthetic-engagement.toml` gains three things, all invented:
- `[roles].creditor_groups` and a `groups` trade-creditor source;
- a `[creditor_ageing_43bh]` table: a 3-day acceptance lag, and Udyam classes for four of the
  read's creditors (micro, small, medium, trader);
- a `[statutory_dues.nature_by_ledger]` table mapping three existing ledgers to natures.

Every other synthetic golden regenerates byte-identical under the new engagement. The synthetic
read itself is unchanged.

**Edge books.** Hand-written books reach what the synthetic read does not; each book's `comment`
names its cases.
- `creditor_ageing` (1-day lag), which reaches:
  - each bucket edge, and each side of the 15- and 45-day windows;
  - every classification;
  - an opening lot beside an in-year bill on the period start;
  - next-year payments settling a lot exactly on its window, one after it, one partly, and one
    lot with none;
  - AGE-1's three violations;
  - a creditor with no TB row, and a configured name that is no ledger;
  - MSMED interest.
- `creditor_ageing_plain`: no lag and no classification; an opening debit taken as an advance.
- `creditor_ageing_short`: a one-month period and a 20-day lag, so the opening lot is inside its
  window while a bill dated before the period start is past it.
- `statutory_dues`, which reaches:
  - the PF/ESI due-date walk: opening lot on time, late and unpaid; a lot unpaid past its due
    date; a lot due after the year end; an unmatched advance;
  - TDS payable as figures only;
  - RCM GST, and output GST with an opening partly paid;
  - a nature over two ledgers, one with no TB row, whose TB does not tie (S43B-1);
  - a nature outside the reference's list.
- `statutory_dues_calendar`: a January to December period, and rules without `[s43b]` or
  `[s36_1_va]`, so the reference's own defaults apply.
- `statutory_dues_coverage`: salary expense with no PF or ESI nature mapped.

**Next-year payments.** The reference's pack never passes next-year payment data to
creditor_ageing_43bh, so the synthetic golden and local parity run without it. Only the edge
books pass it, through `parity/edge_golden.py`, as the reference's `run()` accepts it.

## Invocations

```text
uv run -q --python 3.13 --with openpyxl --with xlrd --with python-docx --with jsonschema \
    --with striprtf --with pdfplumber python parity/python_golden.py ENGINE \
    tests/fixtures/synthetic-engagement.toml tests/fixtures/golden/synthetic.TEST.json --test TEST
uv run -q --python 3.13 --with openpyxl --with xlrd --with python-docx --with jsonschema \
    --with striprtf --with pdfplumber python parity/edge_golden.py ENGINE \
    tests/fixtures/edge-books/NAME.json tests/fixtures/golden
```

## Bytes

| Fixture | Bytes | SHA-256 | Path |
| --- | ---: | --- | --- |
| `synthetic.creditor_ageing_43bh.json` | 41,671 | `03c3d982caa0418f72950ece3900c86c8646865c5f8f77493df4a6faffa8c981` | `golden/synthetic.creditor_ageing_43bh.json` |
| `synthetic.statutory_dues_43b.json` | 19,485 | `c4f8c0cbbfe4e0ecc6d872bf6fb868b72a57e6377438217a621761edfe83f4c0` | `golden/synthetic.statutory_dues_43b.json` |
| `creditor_ageing.json` | 11,805 | `f71a3561a568c926c24cddf6dc98b5ce3facac824ec3892a0406a672262e1ab3` | `edge-books/creditor_ageing.json` |
| `creditor_ageing_plain.json` | 1,931 | `84d0c85bfccdbab58615e01da082267aaf260669c277a85449e383fe62a87aad` | `edge-books/creditor_ageing_plain.json` |
| `statutory_dues.json` | 7,167 | `bd6ba845edc9b81183b327abb0d15b3fff9ae9a0565e48ef971a9a4701a12cb3` | `edge-books/statutory_dues.json` |
| `statutory_dues_calendar.json` | 2,716 | `99fa812846c3acfb63d7e36e2d3f50ff52bcebe50d34e15dc3cbd5e211e6c47d` | `edge-books/statutory_dues_calendar.json` |
| `statutory_dues_coverage.json` | 1,088 | `71ae90b48e4ffbf5872009ab6f35508aa647f7e2e8fe3edb695fe0f054f894c0` | `edge-books/statutory_dues_coverage.json` |
| `edge.creditor_ageing.creditor_ageing_43bh.json` | 67,379 | `3fc38ac84e8ac0eb761253f2440cae486b10d2a75b7ef0883684369d996adf2c` | `golden/edge.creditor_ageing.creditor_ageing_43bh.json` |
| `edge.creditor_ageing_plain.creditor_ageing_43bh.json` | 17,842 | `8ad24d8469e746196ec5e8976dffb338d1ef95538e8c95cf2f5f2bb88db93ab3` | `golden/edge.creditor_ageing_plain.creditor_ageing_43bh.json` |
| `edge.statutory_dues.statutory_dues_43b.json` | 59,346 | `6095b001eee7197e5d63159418b5c70d8919bd1d39d8c81f12332379745dea44` | `golden/edge.statutory_dues.statutory_dues_43b.json` |
| `edge.statutory_dues_calendar.statutory_dues_43b.json` | 20,019 | `0333484a6172bf07a833e173a753a1d2b1dde569aeb3817b0fedeccd8b630cdf` | `golden/edge.statutory_dues_calendar.statutory_dues_43b.json` |
| `edge.statutory_dues_coverage.statutory_dues_43b.json` | 4,517 | `f03a64cd832f55239eaba7ee24c40077c59ddf59d268e49ba7f056b41977bd46` | `golden/edge.statutory_dues_coverage.statutory_dues_43b.json` |
| `creditor_ageing_short.json` | 1,318 | `617469f01e763663317aa21a7468e223711a31aeff31b66c753e2091a449e140` | `edge-books/creditor_ageing_short.json` |
| `edge.creditor_ageing_short.creditor_ageing_43bh.json` | 19,940 | `02d64b6dce813af738a809f011a2f55e06fc7fdf8be33b3c97f962841422e60a` | `golden/edge.creditor_ageing_short.creditor_ageing_43bh.json` |
