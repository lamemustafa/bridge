# Native fixture byte provenance

The following files were normalised by Git on their first commit. Their original
captured bytes are unrecoverable: parse-level content is believed intact, but
byte-level fidelity is not. Each is pending a future re-capture from live Tally.

- `bills_payable_aarav.xml`
- `bills_receivable_aarav.xml`
- `bills_receivable_ageing_lab.xml`
- `bills_receivable_billwise_lab.xml`
- `bills_receivable_unloaded_company_failure.xml`
- `company_collection_live.xml`
- `company_extent_9000.xml`
- `ledger_snapshot_aarav.xml`
- `ledger_snapshot_billwise_lab.xml`

Do not establish byte-length or SHA-256 assertions for these files until their
live re-captures replace the normalised copies. The exception is
`bills_payable_billwise_lab_empty.xml`: its complete 23-byte content is
independently determined and was repaired separately; it is not a re-capture.

## Bridge Validation Lab capture — 2026-08-17

These fixtures are byte-exact responses captured from the purpose-built,
synthetic `Bridge Validation Lab` on TallyPrime port 9001. Each POST was issued
alone and bracketed by successful `/status` identity checks. The existing
native Bills Receivable, Bills Payable, and `List of Ledgers` request builders
were used with `SVTODATE=20260817`; a separate existing
`CompanyBookExtentV1` read established `BOOKSFROM=20250401`.

| Fixture | Bytes | SHA-256 |
| --- | ---: | --- |
| `bills_receivable_validation_lab.xml` | 1170 | `a7f4ff5209c98b145970112a3ba1be9e6d303008b270786e7bfb286c3a99697b` |
| `bills_payable_validation_lab.xml` | 257 | `62063a77ebaccdaebdae42a431bc8859388f415035812e82e212808c64ee83fd` |
| `ledger_snapshot_validation_lab.xml` | 7696 | `64cc585f6bfa2bdc076c2fc28e8732c26e931819dac8f53086e932daeb053a3a` |

The captured values are synthetic. A bounded pre-commit scan found no bytes
above ASCII, email addresses, 10-digit phone patterns, GSTINs, or PANs. The
fixture names are limited to the `BVL` test namespace and Tally built-ins.
The source copies and fixture copies compared byte-for-byte before staging;
the repository fixture-integrity gate supplies the committed-object check.

## Period-pinned native ledger exports — 2026-08-21

These are verbatim BOM-less UTF-16LE response bytes from the production
`List of Ledgers` request. Each request sent `SVFROMDATE=BOOKSFROM` and
`SVTODATE=LASTVOUCHERDATE` for the GUID-verified company extent, so
`OPENINGBALANCE` is the ledger master opening rather than the opening for
Tally's currently loaded display period. The three synthetic companies were
healthy before and after capture; each row carried `ALTERID`.

| Fixture | Company | Book range | Rows | Bytes | SHA-256 |
| --- | --- | --- | ---: | ---: | --- |
| `ledgers_native_aarav.utf16le.xml` | Aarav Trading Company Demo | 20240401–20260401 | 88 | 101,984 | `36d3fa3236cd40826ac9d54077276d7a9c75fdb47653c077a14f43c3b36aa351` |
| `ledgers_native_wr2_core_window.utf16le.xml` | WR2 Unicode Lab | 20260401–20260801 | 9 | 12,648 | `64708e189f2ed6e71bf6311cee810cd15281793f77d7687f20a2910945cf3e05` |
| `ledgers_native_bvl.utf16le.xml` | Bridge Validation Lab | 20250401–20260801 | 13 | 16,806 | `ac32b3d4c8b36f342a1062e4a2b7443e85653f82cd0a6fcb978aad3edf1b8113` |

The Aarav fixture intentionally carries Tally's stored double-encoded names,
including `ZZ CafÃ© NaÃ¯ve Ledger`. This is an observed source-byte property,
not a capture defect: do not normalize, repair, or hand-edit it. The WR2
fixture carries clean non-ASCII names and is the suitable fixture for tests
requiring a clean Unicode ledger name.
