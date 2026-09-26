# Batch E2b fixture provenance: `high_value_register`

Lane E, 2026-09-25. Every book, statement row and AIS row here is invented: no fixture is a Tally
read, a bank statement or an AIS of any real assessee, and every account reference is a masked
placeholder.

## What these fixtures establish, and what they do not

- `golden/synthetic.high_value_register.json` is the reference's dump on the synthetic read, with
  the synthetic bank statement and the synthetic AIS rows as caller data, and the two `[roles]` keys
  this batch adds to `synthetic-engagement.toml`: `counterparty_type_by_ledger` (one debtor typed a
  Government company, so its cash receipt leaves the register) and
  `s194n_withdrawal_narration_terms` (a lower-case term matching two cheque withdrawals). Every
  other synthetic golden regenerates byte-identical with those keys added.
- `edge-books/hvr_paths.json` reaches every row-walk branch: a (party, day) row at exactly the
  s.269ST limit; the unidentified-party bucket for a walk-in sale and a walk-in purchase; tax and
  round-off folded into a single party; the Loans (Liability) exclusion for a cash receipt only
  (a repayment and a loan received by bank stay in); counterparty types excluded in cash mode and
  kept in bank mode, and a type that is not excluded; two parties on one voucher; Contra and an
  optional voucher left out; a cash-and-bank voucher and a withdrawal booked as a Receipt, whose
  other money ledger is never a party; limb (b) groups by reference, and the blank-reference count;
  journal transfers over and under the threshold and the three kinds skipped; a numberless voucher;
  s.194N terms matched upper-cased, a zero-debit row and an unmatched debit left out, a narration
  cut at 60 code points; one AIS row; a co-operative recipient.
- `edge-books/hvr_bare.json` has nothing optional: no statement, no AIS rows, an unknown recipient
  type. Its 38 figures are the registry's `min_figures` for this test.
- A two-line journal with both lines on one party ledger repeats a figure id. The reference raises
  `duplicate figure id`; `tests/edge_books.rs` checks that the port returns an error on the same
  book, not a panic.
- Nothing here reads a real statement or AIS. Those are client data, emitted locally with
  `--emit-bank-statement` / `--emit-traces-documents` for local parity and never committed.

## How they were produced

At brain engine commit `1038dc05`, under Python 3.13 (the edge books themselves are written by
hand in a small generator, as data, then read by both sides):

    uv run -q --python 3.13 --with openpyxl --with xlrd --with python-docx --with jsonschema \
        --with striprtf --with pdfplumber python parity/python_golden.py ENGINE \
        tests/fixtures/synthetic-engagement.toml tests/fixtures/golden/synthetic.high_value_register.json \
        --test high_value_register --bank-statement tests/fixtures/synthetic-bank-statement.json \
        --traces-documents tests/fixtures/synthetic-traces-documents.json
    uv run -q --python 3.13 --with openpyxl --with xlrd --with python-docx --with jsonschema \
        --with striprtf --with pdfplumber python parity/edge_golden.py ENGINE \
        tests/fixtures/edge-books/hvr_NAME.json tests/fixtures/golden

**Regenerated 2026-09-26** at reference commit `140bc7d3`, the same commands, for the reference's change in how a
(party, day) row's party side is compared with its vouchers' own money line (`parity/PORT-NOTE-HVR.md`):
`synthetic.high_value_register.json` and `edge.hvr_paths.high_value_register.json` changed as that note lists;
`edge.hvr_bare.high_value_register.json` regenerates byte-identical. A control run of the same commands at
`1038dc05` reproduced all three previous goldens byte-identical.

**Regenerated again 2026-09-26** at reference commit `e41d9010`, the same commands, for the reference's handling of
a row proven under the threshold (`parity/PORT-NOTE-HVR.md`, its own section): all three goldens change as that note
lists. A control run at `140bc7d3` reproduced the three goldens it replaces byte-identical.

## Bytes

| File | Bytes | SHA-256 | Path |
| --- | ---: | --- | --- |
| `hvr_bare.json` | 1,611 | `7767924cf40a464b3fc25fdcbe870921db918f637536049537b707a4e72e2523` | `edge-books/hvr_bare.json` |
| `hvr_paths.json` | 10,843 | `f36437bb51a2fc8c18fa8151be175d5a03fcd7d6e1fd8f5ab088636fb1582b93` | `edge-books/hvr_paths.json` |
| `edge.hvr_bare.high_value_register.json` | 22,750 | `ffe3d4ad46240ce769ed6d7a747fc04bc42129e57ab08f1fd5be291859eb9764` | `golden/edge.hvr_bare.high_value_register.json` |
| `edge.hvr_paths.high_value_register.json` | 72,530 | `9d9a0f59ab69a461a48ddf310e6c6428796c5b1c563a00bd4daa99ebec3fc8f1` | `golden/edge.hvr_paths.high_value_register.json` |
| `synthetic.high_value_register.json` | 55,298 | `954fa976447f3e2c78aff4f093955119f355aa2a41a569455cba8b8bfe1967cf` | `golden/synthetic.high_value_register.json` |
