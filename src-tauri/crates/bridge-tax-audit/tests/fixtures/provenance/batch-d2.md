# Batch D2 fixture provenance: `partners_40b_194t` and `tds_interest_201`

Lane B, 2026-09-23. Every book here is invented: no fixture is a Tally read of any real assessee, and every
partner and ledger name is invented ("A Capital", "Interest to Partners", ...).

`partners_40b_194t` is ported only as `tds_interest_201`'s input (orchestrator's exception, 23-Sep-2026):
on the three real books it is real work on one (a firm); the other two are not firms or LLPs. Its parity
therefore rests on one real book, with its own edge books covering the not-applicable path and every
branch the real book does not reach. This bends the two-book bar; the owner may veto it.

## What these fixtures establish, and what they do not

- `golden/synthetic.partners_40b_194t.json` is the reference's dump on the synthetic read, an
  individual: one figure, `applicable` = "no".
- `edge-books/partners_firm_deed.json` (a firm with a deed below the statutory cap): a partner whose
  capital goes into debit for part of the year (those days count zero); a withdrawal on the first day; a
  transfer between two partners' capital; one voucher carrying both the interest and the remuneration
  ledger (read as interest); an interest journal with a TDS line under Duties & Taxes (seen) and one
  without (not seen), the latter with no voucher number; a partner naming its capital ledger twice (its
  opening counts twice) with no interest or remuneration ledger; an optional voucher on a capital
  ledger, outside the population; an s.40(b) excess and an s.194T finding.
- `edge-books/partners_llp_no_deed.json` (an LLP, no deed, rules without `[s194t]`): the deed-missing
  finding, the statutory cap used as the rate, the module's own s.194T default (flagged in the finding's
  limits), an excess reported as judgement-required, a partner credited exactly the s.194T limit (not
  over it, so no finding), and a "TDS" ledger that is not under Duties & Taxes (so no TDS line is seen).
- `edge-books/partners_not_applicable.json` (a company, with partners and a deed configured): one
  figure, `applicable` = "no".
- `golden/synthetic.tds_interest_201.json`: the synthetic read gives tds_payees no finding the rows are built
  from, so it holds the ten figures every run gives (`as_of`, the three rates, six totals and counts).
- `edge-books/tds_interest_rows.json` supplies rows directly and reaches every pricing branch: s.201(1A)
  with both legs known, with only the deduction date known (a range on the second leg), with nothing known
  and a payee return-filing date (the Form 26A figures) and without one, the deductor-status flag;
  s.206C(7) with the payment date known, with only the collection date known and with neither, under three
  spellings of the section; 31 March to 1 April (two months), a same-day deposit (none), a zero and a
  negative tax, a deductible date after `as_of`, and a payee key carrying both quote characters.
- `edge-books/tds_interest_default_rules.json`: rules without `[s201_1a]` or `[s206c_7]` (the module's own
  defaults, flagged in each finding's limits), priced as of an explicit date.
- `edge-books/tds_interest_from_results.json` and `..._from_results_194j.json` build the rows from
  `tds_payees`' and `partners_40b_194t`'s own results on the book, by the reference pack's own
  `_tds_interest_defaults` on one side and `defaults_from` on the other: 194C and 194I findings at both
  rates and an s.194T finding (a firm, copied from `tds_payees_goods_and_cash`), and 194J findings (an
  individual, copied from `tds_payees_194j`), each with the placeholder-turnover flag. Their row tags
  depend on the order of `tds_payees`' findings, so these goldens pin that order too.
- Not reached here: a partner without `capital_ledgers`, a deed that is not a table or whose rate is not
  an integer, and rules without `[entity]`: each is refused (the reference raises), and unit tests cover
  them, as no golden can. TDSI-1 never fires on the builder's own output; a unit test tampers with a
  published bound to show it does.

## Real books (local only; nothing from them is in this repository)

`examples/local_parity` compared the port with the reference at `1038dc05` on three real client reads,
each with that client's own reference-engine config. `partners_40b_194t`: byte-identical dumps on all
three; on the firm, 26 figures and 6 findings (two partners, with their capital, interest and
remuneration ledgers bound by identity); on the other two, `applicable` = "no" alone.
`tds_interest_201`: byte-identical on all three, with 0 module-invariant violations: 186 figures and 16
findings (16 rows), 54 figures and 4 findings (4 rows, two of them s.194T rows from the partners), and the
ten structural figures on the third, which has no row. Binding every `[partners.*]` location changed no
other test on the firm: twelve other ported tests, each run there without caller data (the 26AS pair and
`applicability_44ab` need it), were byte-identical.

## Reference commit and invocations

Produced at the reference engine commit `1038dc05527c3f3818060288feece43324f8c682`, from an archive of
that commit with no client data, under Python 3.13, with ENGINE the archive:

    uv run -q --python 3.13 --with openpyxl --with xlrd --with python-docx --with jsonschema \
        --with striprtf --with pdfplumber python parity/python_golden.py ENGINE \
        tests/fixtures/synthetic-engagement.toml tests/fixtures/golden/synthetic.TEST.json --test TEST
    uv run -q --python 3.13 --with openpyxl --with xlrd --with python-docx --with jsonschema \
        --with striprtf --with pdfplumber python parity/edge_golden.py ENGINE \
        tests/fixtures/edge-books/NAME.json tests/fixtures/golden

## Bytes

| Fixture | Bytes | SHA-256 | Path |
| --- | ---: | --- | --- |
| `partners_firm_deed.json` | 5,597 | `eeeeedb294d954be4baff81635bfea456363c262f63a929647e553cb1f692e93` | `edge-books/partners_firm_deed.json` |
| `partners_llp_no_deed.json` | 3,630 | `4c72097876a7670a5f0f8138f17d5fd419eaaf2d547cc0f515f54a1d8aa08126` | `edge-books/partners_llp_no_deed.json` |
| `partners_not_applicable.json` | 2,796 | `c5141157e39743f91fa0ba1817af82da293537d2bc510de9e932d5a395f9becb` | `edge-books/partners_not_applicable.json` |
| `edge.partners_firm_deed.partners_40b_194t.json` | 21,606 | `ec31de250c4a6d525fd2634edec362681248fb9029036cea0ea39ac5bbddf735` | `golden/edge.partners_firm_deed.partners_40b_194t.json` |
| `edge.partners_llp_no_deed.partners_40b_194t.json` | 17,054 | `e4bcc0e098e0fbd8c567b2a3db9ce71529529881ddb68f0e0de9150210f83f0c` | `golden/edge.partners_llp_no_deed.partners_40b_194t.json` |
| `edge.partners_not_applicable.partners_40b_194t.json` | 1,049 | `d03a931902fd87eb43fcf832cc2bb5297e03da9e34d7dfb0b3a4c51c57677e73` | `golden/edge.partners_not_applicable.partners_40b_194t.json` |
| `synthetic.partners_40b_194t.json` | 1,605 | `671f8ecaa00a79a50499fa1853abfe156c08c48ded2496210c330fb956fae1c5` | `golden/synthetic.partners_40b_194t.json` |
| `tds_interest_rows.json` | 4,167 | `507b0420f1b1da45273a8a0f86cf65fe5bdf7113efb4ea214b346fceb119705d` | `edge-books/tds_interest_rows.json` |
| `tds_interest_default_rules.json` | 1,711 | `01d32f97e869fb592feaf6aabfbf6cae12459089e1ea8baa2871182f59aa05a2` | `edge-books/tds_interest_default_rules.json` |
| `tds_interest_from_results.json` | 5,491 | `3c7c9d88cbba8669c2c69ead906137f6c8bdb89c9f1187fe129c3a2255f18fa3` | `edge-books/tds_interest_from_results.json` |
| `tds_interest_from_results_194j.json` | 4,930 | `3067bed4c2adee38de6896d8f16767fd74faea08c1cde76140a5adfeba305c70` | `edge-books/tds_interest_from_results_194j.json` |
| `edge.tds_interest_rows.tds_interest_201.json` | 66,263 | `844db929a712a20320920bc7254a5ecf9db484eff45808758733933fd256d1df` | `golden/edge.tds_interest_rows.tds_interest_201.json` |
| `edge.tds_interest_default_rules.tds_interest_201.json` | 15,510 | `b452e350107e7f0d8179951ee4b1f4f43bc269eb56c577f31a30f015936bfc0b` | `golden/edge.tds_interest_default_rules.tds_interest_201.json` |
| `edge.tds_interest_from_results.tds_interest_201.json` | 36,093 | `55dda50a742f8b5d0492a044387cd4f44b519b80bd2527159c3b8783d17c2fcd` | `golden/edge.tds_interest_from_results.tds_interest_201.json` |
| `edge.tds_interest_from_results_194j.tds_interest_201.json` | 17,552 | `233459313fa2d6f3b6c6b676adc555a6ad893506f7b7921e2dde8e428785665a` | `golden/edge.tds_interest_from_results_194j.tds_interest_201.json` |
| `synthetic.tds_interest_201.json` | 4,703 | `c30841d3c8ead37dad9219c3a787a5da0bb49f0a0941b5eb1a3a785a24c944c1` | `golden/synthetic.tds_interest_201.json` |
