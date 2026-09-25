# Batch E2a fixture provenance: `bank_reconciliation`

Lane E, 2026-09-25. Every book and every statement row here is invented: no fixture is a Tally read or
a bank statement of any real assessee, and every account reference is a masked placeholder.

## What these fixtures establish, and what they do not

- `synthetic-bank-statement.json` is an invented statement for 2025-04-01..2025-05-31, in the shape
  `parity/python_golden.py --emit-bank-statement` writes. With the two `[roles]` keys this batch adds
  to `synthetic-engagement.toml` (`bank_reconciliation_ledger` and a lower-case
  `bank_charge_narration_terms`), `golden/synthetic.bank_reconciliation.json` is the reference's dump
  on the synthetic read: one match, a payment settled as two statement rows, a bank-only charge, an
  unexplained credit, an opening that ties and a closing that does not.
- `edge-books/bankrec_paths.json` reaches: an equal-gap tie decided by the lower statement index; a
  split each way; charge terms matched upper-cased (a lower-case term, and `Straße` upper-casing to
  `STRASSE`); both timing reasons; a voucher with two bank lines summed to one row; a zero-net and an
  optional voucher left out; a numberless voucher labelled by its GUID's last 12 characters; a
  narration over 60 characters, cut by code points; a balance break (BANK-1 on both sides) and a
  missing balance; and a window ending on the FY end (the Trial Balance closing is reported).
- `edge-books/bankrec_big_pool.json` reaches the split search's bound: 41 candidate rows are not
  searched, and a window not ending on the FY end reports no Trial Balance closing.
- A repeated books GUID (two matched vouchers sharing one) is refused on both sides: the reference
  raises `duplicate figure id`, and `tests/edge_books.rs` checks that the port returns an error.
- Nothing here reads a real statement. A real statement is client data; it is emitted locally with
  `--emit-bank-statement` for local parity and never committed.

## How they were produced

At brain engine commit `1038dc05` (brain main), under Python 3.13:

    uv run -q --python 3.13 --with openpyxl --with xlrd --with python-docx --with jsonschema \
        --with striprtf --with pdfplumber python parity/python_golden.py ENGINE \
        tests/fixtures/synthetic-engagement.toml tests/fixtures/golden/synthetic.bank_reconciliation.json \
        --test bank_reconciliation --bank-statement tests/fixtures/synthetic-bank-statement.json
    uv run -q --python 3.13 --with openpyxl --with xlrd --with python-docx --with jsonschema \
        --with striprtf --with pdfplumber python parity/edge_golden.py ENGINE \
        tests/fixtures/edge-books/bankrec_NAME.json tests/fixtures/golden

## Bytes

| File | Bytes | SHA-256 | Path |
| --- | ---: | --- | --- |
| `bankrec_big_pool.json` | 11,130 | `cfbc7323113363b0e7e81c193a4197be634812c5b261ed974110c9f28c73b324` | `edge-books/bankrec_big_pool.json` |
| `bankrec_paths.json` | 6,743 | `5a908ed18b930b73b4bbcc9df4843c824f7ac0ebb3059be85f5e6da79086591d` | `edge-books/bankrec_paths.json` |
| `edge.bankrec_big_pool.bank_reconciliation.json` | 19,246 | `cc39084a5a38758c8ec7250b08d91c4998e91bb9192cfcbecbbd7d3c94803bcf` | `golden/edge.bankrec_big_pool.bank_reconciliation.json` |
| `edge.bankrec_paths.bank_reconciliation.json` | 20,596 | `d0461c9e8e751beb91502acc1855d84bae480fc378a9957dd34ed35678a1c329` | `golden/edge.bankrec_paths.bank_reconciliation.json` |
| `synthetic.bank_reconciliation.json` | 18,098 | `31b35497683d80416273300844d0e18606a5deaa44ad30e22be55f7fcf4fb787` | `golden/synthetic.bank_reconciliation.json` |
| `synthetic-bank-statement.json` | 1,368 | `8ba3bf7d8609e7b578feb90f43832c4ce4a2319f1f1d6b9f47b779357db62c07` | `synthetic-bank-statement.json` |
