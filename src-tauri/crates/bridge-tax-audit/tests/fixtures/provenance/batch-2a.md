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

Every golden here was written by the reference implementation at `105b6c37` (after the engine
re-sync, #597), under `uv run --python 3.13`. They were first written at `57f2619b`; regenerating at
`105b6c37` added `"POP-5"` to `book_invariants_evaluated` in all nine, and a POP-5 violation in the
two edge books that deliberately share a voucher GUID (`creditor_ageing_short`,
`statutory_dues_more`). Nothing else changed. The four reference modules these tests read
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
  - MSMED interest;
  - a negative next-year payment line, which is never walked.
- `creditor_ageing_plain`: no lag and no classification; an opening debit taken as an advance.
- `creditor_ageing_short`: a one-month period and a 20-day lag, so the opening lot is inside its
  window while a bill dated before the period start is past it; two vouchers sharing one GUID.
- `statutory_dues`, which reaches:
  - the PF/ESI due-date walk: opening lot on time, late and unpaid; a lot unpaid past its due
    date; a lot due after the year end; an unmatched advance;
  - TDS payable as figures only;
  - RCM GST, and output GST with an opening partly paid;
  - a nature over two ledgers, one with no TB row, whose TB does not tie (S43B-1);
  - a nature outside the reference's list, over two ledgers that both have TB rows.
- `statutory_dues_calendar`: a January to December period, and rules without `[s43b]` or
  `[s36_1_va]`, so the reference's own defaults apply.
- `statutory_dues_coverage`: salary expense with no PF or ESI nature mapped.
- `statutory_dues_more`: an employee-contribution nature with a debit opening (no opening lot); a
  deduction on the 1st of a month; two vouchers sharing one GUID.

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
| `synthetic.creditor_ageing_43bh.json` | 41,684 | `bc136de44d4d9d1f1e7863d48faa947f4a256068d3c8a46f73e06ae300ecba22` | `golden/synthetic.creditor_ageing_43bh.json` |
| `synthetic.statutory_dues_43b.json` | 19,498 | `982c8798ffc3c09bea95392d3042b8ea165abe4a156619d5bafaafb62bf5ae5d` | `golden/synthetic.statutory_dues_43b.json` |
| `creditor_ageing.json` | 11,878 | `44d4f8f8765095f1dcf25d3400eea68291dee2c90327c266c92a9b7eb2bfdd5a` | `edge-books/creditor_ageing.json` |
| `creditor_ageing_plain.json` | 1,931 | `84d0c85bfccdbab58615e01da082267aaf260669c277a85449e383fe62a87aad` | `edge-books/creditor_ageing_plain.json` |
| `statutory_dues.json` | 7,666 | `82911e6f7b84c2b2fedb49aa90f31792e228ab5d51930984308d721c5ad3f316` | `edge-books/statutory_dues.json` |
| `statutory_dues_calendar.json` | 2,716 | `99fa812846c3acfb63d7e36e2d3f50ff52bcebe50d34e15dc3cbd5e211e6c47d` | `edge-books/statutory_dues_calendar.json` |
| `statutory_dues_coverage.json` | 1,088 | `71ae90b48e4ffbf5872009ab6f35508aa647f7e2e8fe3edb695fe0f054f894c0` | `edge-books/statutory_dues_coverage.json` |
| `edge.creditor_ageing.creditor_ageing_43bh.json` | 67,392 | `d89955293ef55f38835c5d77c40a172ea4b5874ea87a016305909e7937f5937c` | `golden/edge.creditor_ageing.creditor_ageing_43bh.json` |
| `edge.creditor_ageing_plain.creditor_ageing_43bh.json` | 17,855 | `12eb33ca1310d7dd3dcfd167865dfadd994b09ebdaff79530a50aedb22b5f326` | `golden/edge.creditor_ageing_plain.creditor_ageing_43bh.json` |
| `edge.statutory_dues.statutory_dues_43b.json` | 63,411 | `c984bc0d7e77f64922fee6c98e91dc501b49af0aff061431425ff5edc46a7073` | `golden/edge.statutory_dues.statutory_dues_43b.json` |
| `edge.statutory_dues_calendar.statutory_dues_43b.json` | 20,032 | `477bfc8b2a36eb8508c7ee12ba7546e516f588661092346f089c28b8e4e96698` | `golden/edge.statutory_dues_calendar.statutory_dues_43b.json` |
| `edge.statutory_dues_coverage.statutory_dues_43b.json` | 4,530 | `82c15550b563f282091f39ce2aaa48f16fb86bb8cfff813741e641613a8c0e4f` | `golden/edge.statutory_dues_coverage.statutory_dues_43b.json` |
| `creditor_ageing_short.json` | 1,776 | `439f508c4d09ea3147b991f31293ee566b8e109e3a6178f3830ad48ba9deb902` | `edge-books/creditor_ageing_short.json` |
| `edge.creditor_ageing_short.creditor_ageing_43bh.json` | 20,896 | `417520ad35135cb2fe40c829422a65daa93067cab8a851d4de2389968cf4687f` | `golden/edge.creditor_ageing_short.creditor_ageing_43bh.json` |
| `statutory_dues_more.json` | 2,660 | `f8b70c074525f01ee9f21d050a4df3312403e88dd3db07687d822c2785555f47` | `edge-books/statutory_dues_more.json` |
| `edge.statutory_dues_more.statutory_dues_43b.json` | 13,931 | `15e5fb6842c5f34cd6127366a0caebd3657605d5881fa28e4e818e2080a0f16d` | `golden/edge.statutory_dues_more.statutory_dues_43b.json` |
