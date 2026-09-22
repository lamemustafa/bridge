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
  figures beside the books; and an empty alias (not a party, yet not "no alias"), a 26AS row dated
  after the period, a books-only claim with vouchers on both sides of the period, and a Journal on a
  fixed asset from a TCS collector (not a capitalisation event). Its comment names each.
- `edge-books/tds26as_receipts.json` reaches, in `twentysixas_receipts`: billing through Sales,
  Debit Note and Credit Note vouchers (a Journal's Sales line is not billing) within Re 1; interest
  under a lower-case, spaced section compared with income credited, one paisa beyond Re 1; another
  section counted, not compared; a party with nothing in the books; an alias to a ledger missing
  from the book, which both sides report as the module invariant TR-4; an unaliased TAN and a
  Part VI row ignored; an empty alias ignored; and a voucher with no number.
- Both books run both tests. Each golden is the reference's dump of that book and test, module
  invariants included.
- They do not establish anything about a real assessee's documents, or about reading a document:
  this crate reads none. Real-client rows are used only in local parity runs and are not here.

## Real books (local only; nothing from them is in this repository)

`examples/local_parity` compared the port with the reference on three real client reads, each with
that client's own reference-engine config and the Form 26AS/AIS/TIS rows the reference's own
adapters parsed from that client's documents: byte-identical dumps and 0 differences for both tests
on all three. `twentysixas_receipts` raises a substantive difference finding (a deductor's Form
26AS amount against the books) on all three. `tds_tcs_26as` does real work on two: a TCS
capitalisation finding on one and an `amount_differs` finding on the other. The third is thin for
it: one Form 26AS row, AIS figures only, and no finding beyond the PAN-view scope limit the test
always states. No real book reaches a folded deductor, a books-only claim, a different-period
reason or a TIS row; the edge books above carry those.

The figure floors in `registry.rs` are structural, not fixture totals: `tds_tcs_26as` always emits
30 figures (the three real books gave 37 to 40), and every `twentysixas_receipts` figure belongs
to a party with a Part I row, so its floor is 1 (the three real books gave 3 to 10).
`tests/registry.rs` pins both against a run with no documents.

## Reference commit and invocations

Produced at the reference engine commit `105b6c3784f8ec09ef9d233d47ad97ccb1ea7832`, from an archive of
that commit with no client data, under Python 3.13:

    uv run -q --python 3.13 --with openpyxl --with xlrd --with python-docx --with jsonschema \
        --with striprtf --with pdfplumber python parity/python_golden.py ENGINE \
        tests/fixtures/synthetic-engagement.toml tests/fixtures/golden/synthetic.TEST.json \
        --test TEST --traces-documents tests/fixtures/synthetic-traces-documents.json
    uv run -q --python 3.13 --with openpyxl --with xlrd --with python-docx --with jsonschema \
        --with striprtf --with pdfplumber python parity/edge_golden.py ENGINE \
        tests/fixtures/edge-books/tds26as_NAME.json tests/fixtures/golden

The engine re-sync (#597) pinned the crate to `105b6c37`. Between `46324912` (where these goldens
were first produced) and that commit, the reference's `tds_tcs_26as.py`, `twentysixas_receipts.py`,
document adapters, rules, config and binding are unchanged. Regenerating all six goldens there
changed only `book_invariants_evaluated`, which gains `POP-5`. The real-book comparison above was
run at `46324912`.

## Bytes

| File | Bytes | SHA-256 | Path |
| --- | ---: | --- | --- |
| `tds26as_matching.json` | 12,805 | `e591f29a1135c245b9ab913747813f53890921254d72cb37de4723e9ad8f1906` | `edge-books/tds26as_matching.json` |
| `tds26as_receipts.json` | 6,239 | `acbb1ea4523e7de92d70df397d2431f806c2c2bf0eabf7b5281bb488f5ec3ea6` | `edge-books/tds26as_receipts.json` |
| `edge.tds26as_matching.tds_tcs_26as.json` | 27,266 | `9839ab0674d2b43d22c25cd50a594de5c984c6e92edfa3a93df3ac8c8f410387` | `golden/edge.tds26as_matching.tds_tcs_26as.json` |
| `edge.tds26as_matching.twentysixas_receipts.json` | 19,680 | `45f9a1b64fd14b6aad5e72282c3cf327a81f345c0a9d117557a00f2a2bdbcec7` | `golden/edge.tds26as_matching.twentysixas_receipts.json` |
| `edge.tds26as_receipts.tds_tcs_26as.json` | 16,609 | `211a0c7f271b32676bf8fb181cc890b641762a39e5a051b1d976a50bfb176768` | `golden/edge.tds26as_receipts.tds_tcs_26as.json` |
| `edge.tds26as_receipts.twentysixas_receipts.json` | 13,079 | `dffac4a780f6ad167b2d16fbf348e8b10fbabb868f474bc0b35a7354ad2cd121` | `golden/edge.tds26as_receipts.twentysixas_receipts.json` |
| `synthetic.tds_tcs_26as.json` | 16,381 | `9b6c6a16f72db56da424b5d9e7febe9fc6a23dec1b6110a2f7d70df5b4dcbdd8` | `golden/synthetic.tds_tcs_26as.json` |
| `synthetic.twentysixas_receipts.json` | 12,131 | `3cde4a7b95ffe42d9d13f5797d7dd9ceb578550cb6581132ff57a9d33722a194` | `golden/synthetic.twentysixas_receipts.json` |
| `synthetic-traces-documents.json` | 2,137 | `890274e2c545a3bbc64711d2bce14593003b138599250ae1330c4a2a1859da08` | `synthetic-traces-documents.json` |
