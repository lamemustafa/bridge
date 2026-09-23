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
  counted), non-ASCII payee names (hashed over their UTF-8 bytes), and a voucher without a number whose
  GUID's last 12 characters include a non-ASCII one. Each `golden/edge.tds_payees_*.tds_payees.json` is the reference's dump of that book.
- `edge-books/tds_payees_mapping.json` pins how a mapping is read: an empty nature maps nothing (so
  that ledger's credit is a payee), an unknown nature (`194X`) maps the ledger without reporting it
  (so its credit is not a payee), and a debit line on an unmapped ledger is not a payee.
- No book exercises a difference between Python's and Rust's lower-casing: the TDS-ledger match is
  on "tds", which only ASCII `T`, `D` and `S` lower-case to, so none is reachable there.
- They do not establish anything about a real client's books, about reading Tally, or about a
  `[tds]` config that is not a string map: the port refuses a non-string `nature_by_ledger` or
  `payee_aliases` value and a non-integer `previous_year_turnover_paise` where the reference would go
  on (see `src/tds_payees.rs`).

## Real books (local only; nothing from them is in this repository)

`examples/local_parity` compared the port with the reference on three real client reads, each with
that client's own reference-engine config: 0 differences on all three. The test does real work on
two of them: over-limit s.194C rows (including the goods-invoice bucket) and unmapped-s.194J
judgement findings on both, and s.194-I and s.194J professional credits below their limits. The
third configures no nature at all, so only its deductor status (`unknown`) is reached. No real book
reaches a s.194J technical, royalty or s.28(va) credit, a payee-not-named finding, or a TDS ledger
under `Duties & Taxes`; the edge books above carry those.

## Reference commit and invocations

The goldens were produced at the reference engine commit `105b6c3784f8ec09ef9d233d47ad97ccb1ea7832`
(its `tds_payees.py` last changed at `84386b14`, unchanged since the first run at `46324912`), from
an archive of that commit with no client data, under Python 3.13:

    uv run -q --python 3.13 --with openpyxl --with xlrd --with python-docx --with jsonschema \
        --with striprtf --with pdfplumber python parity/python_golden.py ENGINE \
        tests/fixtures/synthetic-engagement.toml tests/fixtures/golden/synthetic.tds_payees.json \
        --test tds_payees
    uv run -q --python 3.13 --with openpyxl --with xlrd --with python-docx --with jsonschema \
        --with striprtf --with pdfplumber python parity/edge_golden.py ENGINE \
        tests/fixtures/edge-books/tds_payees_NAME.json tests/fixtures/golden

The engine re-sync (#597) pinned the crate to `105b6c37`. Between `46324912` and that commit, the
reference's `tds_payees.py`, rules, adapters, config and binding are unchanged. Regenerating all
fourteen goldens there changed only `book_invariants_evaluated`, which gains `POP-5`.

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
| `tds_payees_ledgers_unicode.json` | 4,051 | `e112bf6a4d9a47dd4ff0c14aad255fca89a06da80c49fb6e1bc6009537b7b313` | `edge-books/tds_payees_ledgers_unicode.json` |
| `edge.tds_payees_194c.tds_payees.json` | 24,012 | `e2e085f6b59ac919773e31d9aeef12a48ff904567730709e0495a538e553927b` | `golden/edge.tds_payees_194c.tds_payees.json` |
| `edge.tds_payees_194i.tds_payees.json` | 19,044 | `8dcb445e553f737a89fa40b2612a36eb593f0f401bcb1535baddab58c41b2e70` | `golden/edge.tds_payees_194i.tds_payees.json` |
| `edge.tds_payees_194j.tds_payees.json` | 21,482 | `218e36a6234e7a01ef8c335c7b704cb263b92681b1cf67f44731934f5a68652d` | `golden/edge.tds_payees_194j.tds_payees.json` |
| `edge.tds_payees_194j_default.tds_payees.json` | 18,742 | `cb919733cc488aeb24b7dd759c4a42d1a793ca4f54651af845d7fa9f371f467b` | `golden/edge.tds_payees_194j_default.tds_payees.json` |
| `edge.tds_payees_deductor_capitalised.tds_payees.json` | 15,114 | `2bc7faf7ab54f2dfc9b1310712374fd362b3e9daa6e63062be9605dbd447165d` | `golden/edge.tds_payees_deductor_capitalised.tds_payees.json` |
| `edge.tds_payees_deductor_firm.tds_payees.json` | 15,114 | `2bc7faf7ab54f2dfc9b1310712374fd362b3e9daa6e63062be9605dbd447165d` | `golden/edge.tds_payees_deductor_firm.tds_payees.json` |
| `edge.tds_payees_deductor_huf_unknown.tds_payees.json` | 15,969 | `f5d8a0394e9a57016b9f76a7f88a3ef390f4333e204200fa8727521b83830c10` | `golden/edge.tds_payees_deductor_huf_unknown.tds_payees.json` |
| `edge.tds_payees_deductor_individual_at_threshold.tds_payees.json` | 15,118 | `85c98d2bed6e21e5fcb5674d11d7a0fd691b856dd60ee1b83c89d3bc91e2d1bb` | `golden/edge.tds_payees_deductor_individual_at_threshold.tds_payees.json` |
| `edge.tds_payees_deductor_individual_over.tds_payees.json` | 15,114 | `2bc7faf7ab54f2dfc9b1310712374fd362b3e9daa6e63062be9605dbd447165d` | `golden/edge.tds_payees_deductor_individual_over.tds_payees.json` |
| `edge.tds_payees_deductor_individual_unknown.tds_payees.json` | 15,969 | `f5d8a0394e9a57016b9f76a7f88a3ef390f4333e204200fa8727521b83830c10` | `golden/edge.tds_payees_deductor_individual_unknown.tds_payees.json` |
| `edge.tds_payees_goods_and_cash.tds_payees.json` | 20,931 | `125407225669716db1ffe4d28ffed0bd0435e7895e841a2478fad1b43a65dd7e` | `golden/edge.tds_payees_goods_and_cash.tds_payees.json` |
| `edge.tds_payees_ledgers_unicode.tds_payees.json` | 23,577 | `b2712bfa9907e998723ef3233db80d8a8fca7bc878c35a3c6997940dbb746365` | `golden/edge.tds_payees_ledgers_unicode.tds_payees.json` |
| `synthetic.tds_payees.json` | 16,534 | `cea197f1e517d056082a7f81f9713fac46394d06a1e4a45d17467c142e24d5e5` | `golden/synthetic.tds_payees.json` |
| `tds_payees_mapping.json` | 3,511 | `27acd14da73229f8ed201a3fdad502912dc1ac1b8e3a2a1ee8c8c0f6d11cf774` | `edge-books/tds_payees_mapping.json` |
| `edge.tds_payees_mapping.tds_payees.json` | 15,973 | `6274a8ff84cd3cb958389fb17ff07356f0a793659bacbc482ac0ffd93d946037` | `golden/edge.tds_payees_mapping.tds_payees.json` |
