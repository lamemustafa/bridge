# Native Trial Balance capture provenance — 2026-08-26

This fixture is a byte-exact, read-only response from the purpose-built
synthetic `BRIDGE PROBE B SANDBOX` on the loopback TallyPrime Silver 7.1
endpoint at port 9001. The selected company was pinned by GUID
`ec4454ae-5c4c-4bfa-b3b0-68182a749689` before and after the capture programme.
Its unchanged extent was:

- `BOOKSFROM=20250401`
- `LASTVOUCHERDATE=20260814`
- `ALTVCHID=2785`
- `ALTMSTID=229`

The production-shaped `List of Ledgers` request fetched `TBALOPENING` and
`TBALCLOSING` alongside the ledger identity and balance-sheet presentation
fields. It was dispatched alone and sent twice, with successful `/status`
reads before, between, and after the pair. Both 18,196-byte responses were
byte-identical with SHA-256
`04d9b9784d846bf46ccc695e5aceac54d3bdbdd78d7980b165023be548549592`.
The request is 602 bytes with SHA-256
`04e42cd8d847f91b679a417c3d50fd691158ec0d9eaf33e653f38aa287150f20`.
Measured response times were 0.082051 and 0.049602 seconds.

The paired company-extent responses bracketing the wider read programme were
byte-identical at 7,069 bytes with SHA-256
`0cde5a8036e4aeb15f5f72bd1a34950ca4e12dabff11456699da97c1cb43da74`.
The status bodies contained `TallyPrime Server is Running` and had SHA-256
`655415972de8e54d65743a548e00c2218810683fc0c4cab76cf6ea97ff1d3800`.
No Tally write was sent.

The response contains 24 synthetic ledger rows. Every opening amount is
explicitly `0.00`; the exact sum of `TBALCLOSING - TBALOPENING` is zero. The
fixture proves the native request and parser path for this synthetic book. It
does not prove customer-book completeness, Windows workbook behavior, gross
debit/credit turnover, or support outside the captured product/profile.

A bounded privacy audit found no email, address, phone, pincode, GSTIN, or PAN
fields and no GSTIN- or PAN-shaped values. All company and ledger labels belong
to the repository's synthetic validation corpus. `.gitattributes` disables
text normalization for the captured response.
