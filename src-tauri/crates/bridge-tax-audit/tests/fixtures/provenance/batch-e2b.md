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

## Bytes

| File | Bytes | SHA-256 | Path |
| --- | ---: | --- | --- |
| `hvr_bare.json` | 1,611 | `7767924cf40a464b3fc25fdcbe870921db918f637536049537b707a4e72e2523` | `edge-books/hvr_bare.json` |
| `hvr_paths.json` | 10,843 | `f36437bb51a2fc8c18fa8151be175d5a03fcd7d6e1fd8f5ab088636fb1582b93` | `edge-books/hvr_paths.json` |
| `edge.hvr_bare.high_value_register.json` | 18,818 | `d28a4249f788f9e54ac3a0308c42becf03f2f6f9dadf0c5c6394340644aa441f` | `golden/edge.hvr_bare.high_value_register.json` |
| `edge.hvr_paths.high_value_register.json` | 64,831 | `210099c2f040dfa929ee9ca6ab5fa58c9ff38b9cc59a433cffeeba3458093b57` | `golden/edge.hvr_paths.high_value_register.json` |
| `synthetic.high_value_register.json` | 49,979 | `df8704602affda1e5af667309ea600a3ff229b2ec3c19e8297a3aa0a1ef81602` | `golden/synthetic.high_value_register.json` |
