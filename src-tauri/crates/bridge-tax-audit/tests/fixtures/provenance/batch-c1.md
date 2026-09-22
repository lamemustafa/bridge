# Batch C1 fixture provenance: `tds_payees`

Lane C, 2026-09-22. Every book here is invented; none is a Tally read and none holds client data.

## What these fixtures establish, and what they do not

- `golden/synthetic.tds_payees.json` is the reference implementation's canonical dump of
  `tds_payees` on the shared synthetic read, with the `[tds]` table this batch adds to
  `synthetic-engagement.toml` (invented: `Freight Inward` as s.194C, `Shop Rent` as s.194-I, and two
  carriers merged into one payee entity by `payee_aliases`; no previous-year turnover, so the
  individual's deductor status is `unknown`). It establishes that the port and the reference agree
  on that read, on figures that are mostly zero there; the edge books carry the branches.
- Each `edge-books/tds_payees_*.json` names, in its own `comment`, the boundaries and branches it
  reaches: the s.194C single-sum and aggregate limits at and one paisa over, per voucher rather than
  per line, with an alias-merged payee; the s.194-I month limit and months that are not summed; each
  s.194J category tested separately, the unmapped category, and the rules' `[s194j]` table removed
  (the module's default limit and its extra limit text); the goods-invoice and payee-not-named
  buckets; the deductor status for a firm, for a HUF without a previous-year turnover, for an
  individual without one, at the threshold and one paisa over it, and for an entity type written
  with a capital letter (matched exactly, as the reference does, so a deductor); and ledgers under
  `Duties & Taxes` named with "TDS" in upper, lower and mixed case (one outside that group not
  counted), non-ASCII payee names, and a voucher without a number whose GUID ends in non-ASCII
  characters. Each `golden/edge.tds_payees_*.tds_payees.json` is the reference's dump of that book.
- No book exercises a difference between Python's and Rust's lower-casing: the TDS-ledger match is
  on "tds", which only ASCII `T`, `D` and `S` lower-case to, so none is reachable there.
- They do not establish anything about a real client's books, about reading Tally, or about a
  `[tds]` config that is not a string map: the port refuses a non-string `nature_by_ledger` or
  `payee_aliases` value and a non-integer `previous_year_turnover_paise` where the reference would go
  on (see `src/tds_payees.rs`).

## Reference commit and invocations

The goldens were produced at the reference engine commit `4632491210c6383d46d9203c61716d191fd7fd6c`
(its `tds_payees.py` last changed at `84386b14`), under Python 3.13:

    uv run -q --python 3.13 --with openpyxl --with xlrd --with python-docx --with jsonschema \
        --with striprtf --with pdfplumber python parity/python_golden.py ENGINE \
        tests/fixtures/synthetic-engagement.toml tests/fixtures/golden/synthetic.tds_payees.json \
        --test tds_payees
    uv run -q --python 3.13 --with openpyxl --with xlrd --with python-docx --with jsonschema \
        --with striprtf --with pdfplumber python parity/edge_golden.py ENGINE \
        tests/fixtures/edge-books/tds_payees_NAME.json tests/fixtures/golden

Batch C1 does not merge until the engine re-sync (POP-5) lands at the owner-pushed commit; these
goldens are regenerated there and this section updated with that commit.

## Rules vendored for this batch

`rules/ay2026-27.s44ab.toml` gains `[s194c]`, `[s194i]` and `[deductor]` in full, and `[s194j]` as
three blocks: its header, its three value lines, and `status` cut at its value. The cuts remove the
table's header comment lines and the status line's trailing comment, which cite private research
notes; nothing the port reads is cut. Every block is still a verbatim substring of the reference's
rules file, whose SHA-256 is unchanged (`src/rules.rs`).

## Bytes

| File | Bytes | SHA-256 | Path |
| --- | ---: | --- | --- |
| `tds_payees_194c.json` | 8,263 | `c2ec1a731cd12deae13102dcf7be60ee61203e027c23f2cd2ec22cee7115bc09` | `edge-books/tds_payees_194c.json` |
| `tds_payees_194i.json` | 4,104 | `c24f56557d2d77203b633816cf4a1e8f7d8cec7769121a815ad543bd61063e46` | `edge-books/tds_payees_194i.json` |
| `tds_payees_194j.json` | 5,000 | `f197f86384183d0cb83b6dcb035195b03c4f1719c99f4ab1864d89fdbd284d54` | `edge-books/tds_payees_194j.json` |
| `tds_payees_194j_default.json` | 2,203 | `02636541c59a8acf01998a390014d761e00208cc1ecb920f7c0425e97f4c988f` | `edge-books/tds_payees_194j_default.json` |
| `tds_payees_deductor_capitalised.json` | 1,584 | `7ed78089cfefedccfe357d93402be206c80a1ed473b0a90c5a99432a079f22e0` | `edge-books/tds_payees_deductor_capitalised.json` |
| `tds_payees_deductor_firm.json` | 1,531 | `d329ec9a28c446a45c72ac12be5fb1ae1f00fe79119f6005947512d284713385` | `edge-books/tds_payees_deductor_firm.json` |
| `tds_payees_deductor_huf_unknown.json` | 1,550 | `5e1600a58ef6f0bdcc0c1813bf274fcf1ae0ae70ef3d3d773997a2fb026fac71` | `edge-books/tds_payees_deductor_huf_unknown.json` |
| `tds_payees_deductor_individual_at_threshold.json` | 1,623 | `b2cbca55bc534ef067ee7d08062a074bf6ae19e57d75e73273a442bb39b2e011` | `edge-books/tds_payees_deductor_individual_at_threshold.json` |
| `tds_payees_deductor_individual_over.json` | 1,607 | `5845ece1f90e88770ceeb122149731af71f5b537a14b0ed5818a8ca2ceba5949` | `edge-books/tds_payees_deductor_individual_over.json` |
| `tds_payees_deductor_individual_unknown.json` | 1,603 | `c7bf6bf712b75a5db11f4299197e7cc4815d5c796912c3dcc661b5e212c17b6d` | `edge-books/tds_payees_deductor_individual_unknown.json` |
| `tds_payees_goods_and_cash.json` | 4,594 | `6b46e256f74547810dd22c9e0bfd12650bc298792a930556144d6bf3370bc1b5` | `edge-books/tds_payees_goods_and_cash.json` |
| `tds_payees_ledgers_unicode.json` | 4,060 | `ced54ebcf28c3571f6eeac0f15a044da0502f86b9a81e4f9d1f6200c6bea4f61` | `edge-books/tds_payees_ledgers_unicode.json` |
| `edge.tds_payees_194c.tds_payees.json` | 23,066 | `209ea847a06e146ae8b14e3464288c1897efcc8b2b83843aceb69e5fddc7629f` | `golden/edge.tds_payees_194c.tds_payees.json` |
| `edge.tds_payees_194i.tds_payees.json` | 18,098 | `8a3e036eb8794e93cd6182a6b68ad506c570698542a049c58ab64d9f54f6bc5c` | `golden/edge.tds_payees_194i.tds_payees.json` |
| `edge.tds_payees_194j.tds_payees.json` | 20,536 | `2873ea733d6866e48e9ffd1f45a219115346d9e955b5127c65a52464de265b42` | `golden/edge.tds_payees_194j.tds_payees.json` |
| `edge.tds_payees_194j_default.tds_payees.json` | 17,801 | `a3c162c9f9a7cd7acd80b86d16585ea58f879d464e2a2b6068a3c43d7dbc786f` | `golden/edge.tds_payees_194j_default.tds_payees.json` |
| `edge.tds_payees_deductor_capitalised.tds_payees.json` | 14,168 | `2e1bb1ef76ba85d4c1c7a7e322af6a2c0604db25eddeb438a2baf1035a8438f1` | `golden/edge.tds_payees_deductor_capitalised.tds_payees.json` |
| `edge.tds_payees_deductor_firm.tds_payees.json` | 14,168 | `2e1bb1ef76ba85d4c1c7a7e322af6a2c0604db25eddeb438a2baf1035a8438f1` | `golden/edge.tds_payees_deductor_firm.tds_payees.json` |
| `edge.tds_payees_deductor_huf_unknown.tds_payees.json` | 15,023 | `29fa076442ab01db299dc94ae4a442441242f21abc19cfe0185b1a35031e42b6` | `golden/edge.tds_payees_deductor_huf_unknown.tds_payees.json` |
| `edge.tds_payees_deductor_individual_at_threshold.tds_payees.json` | 14,172 | `2111f3685bed8f04110650b3a7ce77e5c936b71b8f2585e309c7b37bb03067e3` | `golden/edge.tds_payees_deductor_individual_at_threshold.tds_payees.json` |
| `edge.tds_payees_deductor_individual_over.tds_payees.json` | 14,168 | `2e1bb1ef76ba85d4c1c7a7e322af6a2c0604db25eddeb438a2baf1035a8438f1` | `golden/edge.tds_payees_deductor_individual_over.tds_payees.json` |
| `edge.tds_payees_deductor_individual_unknown.tds_payees.json` | 15,023 | `29fa076442ab01db299dc94ae4a442441242f21abc19cfe0185b1a35031e42b6` | `golden/edge.tds_payees_deductor_individual_unknown.tds_payees.json` |
| `edge.tds_payees_goods_and_cash.tds_payees.json` | 19,985 | `2f3065355f12a1f1733133a801021d61f100cef0af775577a17e5a854a7e7368` | `golden/edge.tds_payees_goods_and_cash.tds_payees.json` |
| `edge.tds_payees_ledgers_unicode.tds_payees.json` | 22,631 | `876b7c8720dfd7cbabb55a9c05f02accc49ea4b12c84beb68dfdc22c5071fc63` | `golden/edge.tds_payees_ledgers_unicode.tds_payees.json` |
| `synthetic.tds_payees.json` | 15,588 | `3da41c7a74b5d074c7748f3a2bcc69922d3c2021bc74309ed1e8f771e52c571f` | `golden/synthetic.tds_payees.json` |
