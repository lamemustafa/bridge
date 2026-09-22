# Batch C2 fixture provenance: `twentysixas_receipts` and `tds_tcs_26as`

Lane C, 2026-09-22. Every book and every document row here is invented: no fixture is a Tally read,
a Form 26AS, an AIS or a TIS of any real assessee, and every TAN string is deliberately not
TAN-shaped (`TAN-INVENTED-A`, `TAN-EDGE-A`, ...).

## What these fixtures establish, and what they do not

- `synthetic-traces-documents.json` holds invented Form 26AS, AIS and TIS rows in the shape
  `parity/python_golden.py --emit-traces-documents` writes. With the `[tds_tcs_26as]` table this
  batch adds to `synthetic-engagement.toml` (empty TDS/TCS/advance-tax ledger lists, since the
  synthetic read has none, and two debtors aliased to invented TANs), `golden/synthetic.*.json` are
  the reference's dumps of both tests on the synthetic read. On it `tds_tcs_26as` reaches no books
  claim at all; the edge books carry the matching.
- `edge-books/tds26as_matching.json` reaches, in `tds_tcs_26as`: a deductor under two sections
  compared once against one claim (folded), matched at exactly Re 1 and `amount_differs` beyond it;
  every 26AS-only reason (no alias, not found in books, different period, a row of another part)
  and both books-only reasons (not found in 26AS, different period); a party-less voucher attributed
  through an alias target and one that is not; a zero role line that is no claim; the TCS booking
  fact on fixed-asset purchases from a matched collector, with and without a TCS line; and AIS/TIS
  figures beside the books. Its comment names each.
- `edge-books/tds26as_receipts.json` reaches, in `twentysixas_receipts`: billing through Sales,
  Debit Note and Credit Note vouchers (a Journal's Sales line is not billing) within Re 1; interest
  under a lower-case, spaced section compared with income credited, one paisa beyond Re 1; another
  section counted, not compared; a party with nothing in the books; an alias to a ledger missing
  from the book, which both sides report as the module invariant TR-4; an unaliased TAN and a
  Part VI row ignored; and a voucher with no number.
- Both books run both tests. Each golden is the reference's dump of that book and test, module
  invariants included.
- They do not establish anything about a real assessee's documents, or about reading a document:
  this crate reads none. Real-client rows are used only in local parity runs and are not here.

## Reference commit and invocations

Produced at the reference engine commit `4632491210c6383d46d9203c61716d191fd7fd6c`, under Python 3.13:

    uv run -q --python 3.13 --with openpyxl --with xlrd --with python-docx --with jsonschema \
        --with striprtf --with pdfplumber python parity/python_golden.py ENGINE \
        tests/fixtures/synthetic-engagement.toml tests/fixtures/golden/synthetic.TEST.json \
        --test TEST --traces-documents tests/fixtures/synthetic-traces-documents.json
    uv run -q --python 3.13 --with openpyxl --with xlrd --with python-docx --with jsonschema \
        --with striprtf --with pdfplumber python parity/edge_golden.py ENGINE \
        tests/fixtures/edge-books/tds26as_NAME.json tests/fixtures/golden

These goldens are regenerated at the owner-pushed commit once the engine re-sync (POP-5) lands.

## Bytes

| File | Bytes | SHA-256 | Path |
| --- | ---: | --- | --- |
| `tds26as_matching.json` | 10,933 | `e079c6887c2aeb83c66acd9478cbf4d7d069f50d4ae9eba2a305812de9430760` | `edge-books/tds26as_matching.json` |
| `tds26as_receipts.json` | 5,971 | `6963fbe424710350596c4dada0ccffb66d3c9dc356ab8bab9bb4ee322bb5b6e0` | `edge-books/tds26as_receipts.json` |
| `edge.tds26as_matching.tds_tcs_26as.json` | 25,117 | `dac51f5bc8150fc0e74b6ce0e9b11028bd8458bea17e2d875105dae5707fea75` | `golden/edge.tds26as_matching.tds_tcs_26as.json` |
| `edge.tds26as_matching.twentysixas_receipts.json` | 16,646 | `e7ff42395eb95f77c3c4b8c3f1a419f64bd50a40bccf17dd39582dca35e1eb18` | `golden/edge.tds26as_matching.twentysixas_receipts.json` |
| `edge.tds26as_receipts.tds_tcs_26as.json` | 15,806 | `b72cdd46844616b759badbd2ad07a9b776448ae073a79b9996eefd2059d6a921` | `golden/edge.tds26as_receipts.tds_tcs_26as.json` |
| `edge.tds26as_receipts.twentysixas_receipts.json` | 13,066 | `98cb0c184d177da150ea5bc90fda8b9f4ac6b59d75b4aa47443b55bdf962cba1` | `golden/edge.tds26as_receipts.twentysixas_receipts.json` |
| `synthetic.tds_tcs_26as.json` | 16,368 | `e5f9ba5c1f5fd6736729666c3d50dc93b3e4400b140a71ba38d2bdf947d3132c` | `golden/synthetic.tds_tcs_26as.json` |
| `synthetic.twentysixas_receipts.json` | 12,118 | `f1e6089ab63c7f1f47ecdb6e4cf46648dd580b6fb31b611a0ee632c8a1274832` | `golden/synthetic.twentysixas_receipts.json` |
| `synthetic-traces-documents.json` | 2,137 | `890274e2c545a3bbc64711d2bce14593003b138599250ae1330c4a2a1859da08` | `synthetic-traces-documents.json` |
