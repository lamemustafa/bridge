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
