# Batch E3a fixture provenance: `stock`

Lane E, 2026-09-25. Every book, stock item and Stock Summary row here is invented: no fixture is a
Tally read of any real assessee.

## What these fixtures establish, and what they do not

- `synthetic-read/` gains two invented parts written by `parity/generate_fixture.py`: `stock_items.xml`
  (four masters, one of them value-only, with Tally's reserved `&#4; Not Applicable` unit) and
  `stock_summary_close.xml` (a closing Stock Summary as of the period end: a goods item negative at
  close, a value-only item with a negative value, an item the masters do not carry). Every other part
  is byte-identical, and every other synthetic golden regenerates byte-identical with them and with
  the `[stock]` table added to `synthetic-engagement.toml` (opening `from_masters`, closing `from_read`).
- `golden/synthetic.stock.json` is the reference's dump on that read. The synthetic vouchers carry no
  inventory, so STK-1 fires on every closing item whose quantity differs from its opening: the check
  is reached, on an invented book.
- `edge-books/stock_paths.json` reaches: an item negative only when same-day stock-out goes first; one
  negative at opening that recovers (definition (a), not (b)); a stock journal's IN/OUT tags;
  value-only goods lines counted and valued without sign; lines with no computable movement or no
  item; a value-only item's line; a voucher on a Stock-in-Hand ledger and a ledger with no Trial
  Balance row; an optional voucher left out; 0.1 + 0.2 against a closing 0.3 and -5e-7, both within
  tolerance; openings seeded from the summary and from a master; a summary item with no master; a
  summary quantity differing from its master's; an opening gap of nil beside a closing gap that is
  not (so the two-figures finding needs only one); STK-1 firing on two items.
- `edge-books/stock_quiet.json` has nothing to find. Its 30 figures are the registry's `min_figures`.
- A goods line with no quantity field refuses on both sides (`tests/edge_books.rs`).
- These are regression fixtures only. The reader's evidence is the real books' stock parts, compared
  locally with `examples/local_parity.rs` and never committed.

## How they were produced

At brain engine commit `1038dc05`, under Python 3.13:

    python3 parity/generate_fixture.py tests/fixtures
    uv run -q --python 3.13 --with openpyxl --with xlrd --with python-docx --with jsonschema \
        --with striprtf --with pdfplumber python parity/python_golden.py ENGINE \
        tests/fixtures/synthetic-engagement.toml tests/fixtures/golden/synthetic.stock.json --test stock
    uv run -q --python 3.13 --with openpyxl --with xlrd --with python-docx --with jsonschema \
        --with striprtf --with pdfplumber python parity/edge_golden.py ENGINE \
        tests/fixtures/edge-books/stock_NAME.json tests/fixtures/golden

## Bytes

| File | Bytes | SHA-256 | Path |
| --- | ---: | --- | --- |
| `stock_paths.json` | 7,161 | `46d11c242cc918cc019e345d1f3f2e3c6b8ad56fa50fc6fd0e3ce2890ad588bc` | `edge-books/stock_paths.json` |
| `stock_quiet.json` | 1,074 | `981835c6ff5eaac34d93bc59851429aa47cf3e99fd8ac437700883f40173c39f` | `edge-books/stock_quiet.json` |
| `edge.stock_paths.stock.json` | 20,586 | `b5131077e1b9e4e849e32fc0cfe840c0afd09dd816cc0542429b3b83f7b182de` | `golden/edge.stock_paths.stock.json` |
| `edge.stock_quiet.stock.json` | 12,008 | `03cb9d43267f85ff1e38a4a7bf9202c1223cc8cb6516effe599e185000439695` | `golden/edge.stock_quiet.stock.json` |
| `synthetic.stock.json` | 18,542 | `7c5b6b674d11ef26e8dce4fe2bcf023275551a943cdf05b238d373ec024decdd` | `golden/synthetic.stock.json` |
