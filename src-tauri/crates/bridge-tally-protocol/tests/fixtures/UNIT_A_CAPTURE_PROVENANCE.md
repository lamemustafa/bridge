# Unit A live captures — 2026-07/08

Live responses from the synthetic `Aarav Trading Company Demo` (company GUID
`bb8ad19e-…`), added in `7bd6123a` (#106) together with the `.gitattributes`
rule that exempted `*_live.xml` from line-ending normalisation because they
are byte-exact evidence. That commit's comment said their length and SHA-256
were asserted in tests; they were not, which is what the table below now does.
The hashes pin the committed bytes. Port and exact request are not recorded
per file.

| Fixture | Bytes | SHA-256 |
| --- | ---: | --- |
| `unit_a_company_extent_live.xml` | 1731 | `ab580f9db97425ff328e30ebf0a81af000e21862180ef7093e553ccc8ed3d3e3` |
| `unit_a_invalid_char_ref_live.xml` | 8616 | `c6484f004cdee7b1b1ef7065a4156243993956b54c4acf8776ab0c7b68fa5a8d` |
| `unit_a_optional_voucher_live.xml` | 29994 | `0a692e6362101f9c176cddca1f8afb7d00da3ceb97f94ebd42ff682130b99e53` |
| `unit_a_vouchers_wildcard_live.xml` | 1504566 | `6db364089068d13563047a58a491a518e90fd30099dc0723ab39da9ebaf58e99` |

- `unit_a_company_extent_live.xml` — the company identity/extent read
  (`BOOKSFROM` 20240401, `LASTVOUCHERDATE` 20260401). It carries no
  `ALTMSTID`, unlike `native/company_extent_9000.xml`. Date not recorded.
- `unit_a_invalid_char_ref_live.xml` — a Ledger collection containing Tally's
  invalid `&#4;` character reference; drives
  `real_invalid_character_reference_is_narrowly_repaired`. Date and company
  are not recorded, and the file carries no company GUID.
- `unit_a_optional_voucher_live.xml` — live-captured 2026-07-31 after two
  Receipts, one `ISOPTIONAL=Yes`, were written against one bill
  (`docs/tally/UNIT_A_RULING_9.md`). Also pinned in
  `docs/tally/compatibility/compatibility-surface.json`. Ruling 9 reports a
  16-row read-back; this fixture holds 2 vouchers, and whether it is a
  narrower read or a subset is not recorded.
- `unit_a_vouchers_wildcard_live.xml` — 75 vouchers, 20250401–20260302. It
  predates `ISOPTIONAL` joining the sealed request's FETCH list, so tests
  insert `<ISOPTIONAL>No</ISOPTIONAL>` in memory rather than editing it.
  Date not recorded.
