# Education import-counter responses — 2026-07-29

`live_education_w1_ledger_sanitized.xml`,
`live_education_w4_voucher_sanitized.xml` and
`live_education_w7_baddate_sanitized.xml` are import responses **derived from**
live captures taken 2026-07-29 in Tally Education mode; the captures themselves
are not committed. The only recorded transformation is that w7's `LINEERROR`
text was replaced with `REDACTED_LIVE_LINEERROR` while its presence was kept
(`../import_evidence.rs`). Whether anything else was changed is not recorded,
nor are port, company or request, so no byte count or SHA-256 is declared.

They are counter-shape evidence: w1 and w4 are clean single creations; w7 is a
rejection despite `ERRORS=0`, because it reports `EXCEPTIONS=1` and a
`LINEERROR`. Added in `7bd6123a` (#106).
