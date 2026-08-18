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
