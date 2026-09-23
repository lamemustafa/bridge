# Batch D2a fixture provenance: `partners_40b_194t`

Lane B, 2026-09-23. Every book here is invented: no fixture is a Tally read of any real assessee, and every
partner and ledger name is invented ("A Capital", "Interest to Partners", ...).

`partners_40b_194t` is ported only as `tds_interest_201`'s input (orchestrator's exception, 23-Sep-2026);
`tds_interest_201` itself follows in batch D2b, once the reference's own fix to its rows lands:
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
- Not reached here: a partner without `capital_ledgers`, a deed that is not a table or whose rate is not
  an integer, and rules without `[entity]`: each is refused (the reference raises), and unit tests cover
  them, as no golden can.

## Real books (local only; nothing from them is in this repository)

`examples/local_parity` compared the port with the reference at `1038dc05` on three real client reads,
each with that client's own reference-engine config: byte-identical dumps on all three; on the firm, 26
figures and 6 findings (two partners, with their capital, interest and remuneration ledgers bound by
identity); on the other two, `applicable` = "no" alone. Binding every `[partners.*]` location changed no
other test on the firm: all fifteen other ported tests were byte-identical there (the 26AS pair and
`applicability_44ab` with that client's own documents and turnover, emitted by the reference and fed to
both sides).

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
