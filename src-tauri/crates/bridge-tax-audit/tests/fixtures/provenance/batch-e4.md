# Batch E4 fixture provenance: `party_monthly`

Lane E, 2026-09-25. Every book here is invented: no fixture is a Tally read of any real assessee.

## What these fixtures establish, and what they do not

- `golden/synthetic.party_monthly.json` is the reference's dump on the synthetic read (52 figures; its
  Trial Balance ties in every block, so no finding).
- `edge-books/pm_paths.json` (`top_n` 2) reaches: named parties, "Others" with its plural and singular
  labels and an equal-total tie decided by name, a sub-group party, the cash-or-bank, no-party and
  several-parties rows (an expense with a cash line stays no party), credit and debit notes, a month
  that nets to nil (no figure), a voucher dated after the period, two vouchers sharing one GUID counted as
  two; and the Trial Balance tied within a rupee, matched by one optional voucher, matched only by the
  post-dated and cancelled vouchers together, and matched by two sets at once (so naming none), with an
  opening balance on a P&L ledger.
- `edge-books/pm_not_fy.json`: a calendar-year period (no month figures), a missing Direct Expenses
  group, and a Trial Balance difference no excluded voucher matches.
- `edge-books/pm_empty.json`: all four groups and no voucher. Its 16 figures are the registry's
  `min_figures`.
- A party whose tag equals a fixed row's repeats a figure id: refused on both sides
  (`tests/edge_books.rs`). PWM-1 and PWM-2 are shown to fire on tampered results.
- Regression fixtures only: the evidence for real books is local parity on the real reads, never
  committed.

## How they were produced

At brain engine commit `1038dc05`, under Python 3.13:

    uv run -q --python 3.13 --with openpyxl --with xlrd --with python-docx --with jsonschema \
        --with striprtf --with pdfplumber python parity/python_golden.py ENGINE \
        tests/fixtures/synthetic-engagement.toml tests/fixtures/golden/synthetic.party_monthly.json \
        --test party_monthly
    uv run -q --python 3.13 --with openpyxl --with xlrd --with python-docx --with jsonschema \
        --with striprtf --with pdfplumber python parity/edge_golden.py ENGINE \
        tests/fixtures/edge-books/pm_NAME.json tests/fixtures/golden

## Bytes

| File | Bytes | SHA-256 | Path |
| --- | ---: | --- | --- |
| `pm_empty.json` | 771 | `969cbcf93f17ef083503e5084c5f8666a618b27b315103bec5ea76e69d69670c` | `edge-books/pm_empty.json` |
| `pm_not_fy.json` | 1,990 | `effb4bf2ebfe8b437ddd582bc3f86176e4d00afd847dddf3f88b3cc2ed321276` | `edge-books/pm_not_fy.json` |
| `pm_paths.json` | 8,957 | `6f355798bf5b7209611b0f5e90f7c7757d870fb017ac53001a2f7f29f40e42ae` | `edge-books/pm_paths.json` |
| `edge.pm_empty.party_monthly.json` | 6,715 | `406d20f5a48315ea0b10d0d3515186ec83a1ba63a9efb21a6871ef8f42410369` | `golden/edge.pm_empty.party_monthly.json` |
| `edge.pm_not_fy.party_monthly.json` | 9,433 | `5a7ade3a79af8ba2b74b66cd36aac37920cf5976ee3f067aeaf6c0783213936f` | `golden/edge.pm_not_fy.party_monthly.json` |
| `edge.pm_paths.party_monthly.json` | 42,575 | `c7573d78bd67f31b3c358ee9c59bb5bc2c9814fe4191ac6a89ebea017824bbd0` | `golden/edge.pm_paths.party_monthly.json` |
| `synthetic.party_monthly.json` | 23,356 | `63f59247cd8b90552807db3a08f5b1ba4db19140904dd2f1a0f684e2f1b373df` | `golden/synthetic.party_monthly.json` |
