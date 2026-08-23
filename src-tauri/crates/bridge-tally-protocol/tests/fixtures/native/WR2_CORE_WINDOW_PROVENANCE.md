# WR2 complete core-window capture — 2026-08-20

These fixtures are mechanically decoded from captured BOM-less UTF-16LE
response bytes from the synthetic `WR2 Unicode Lab` corpus. The source corpus
was bracketed by successful `/status` responses for every request; its four
exports are mutually referentially complete for the stated window. The native
ledger fixture was replaced on 2026-08-21 with the raw, period-pinned bytes;
its current provenance is in `PROVENANCE.md`.

| Fixture | Captured UTF-16LE SHA-256 | Decoded UTF-8 SHA-256 |
| --- | --- | --- |
| `group_snapshot_wr2.xml` | `03f32d0874c5e1069ff2d7da12c7f2a1335b0a350f001decb89d09788bcaaf5e` | `29a92683c1cbceae9c9fc35acf95602a9f83eb14d60dc909e009de0649112e74` |
| `voucher_types_native_wr2.xml` | `99ca090afc56cb4722c8592538ca1e44767e49dd30c59da2f6a99a409931f9be` | `2285763f6e1598d1672f3adb5adc686d54c4ddb3a36b0c25e1a454a9e4cafc22` |
| `vouchers_native_wr2.xml` | `d0f47292ac1d84174dd330e02670e1a2e1e84d7379d711cb871fc816ce253c43` | `94450598cb0ca1f214e579bad55729a01a604fa5e41dd1252ed74c269e2b054d` |
