# Simulator fixture provenance

Every file in this directory is hand-authored synthetic simulator input. None
is a capture from Tally, and none may be read as evidence of what a real Tally
release emits. They were added together in `0100e6b2` (2026-07-16) under the
fixture rules in `../README.md`: `BRIDGE SYNTHETIC` names, GUIDs in the
reserved `00000000-0000-4000-8000-…` range, `BRIDGE_SYNTHETIC_*` line errors,
and no output copied from a real company or Tally installation. The
`COMPANYCONTEXT` schema strings (`bridge.tally.ledgers/1`,
`bridge.tally.vouchers/2`, `/3`) are Bridge's own, not native Tally.

`../src/fixtures.rs` loads each file with `include_str!` into a `Fixture`
variant. The only transformation is `ScenarioPlan::response_bytes`, which
re-encodes the checked-in UTF-8 as UTF-8, UTF-8 with BOM, or UTF-16LE/BE; the
UTF-16 variants are never checked in.

Because these are authored rather than captured, no byte count or SHA-256 is
declared here. `inconsistent_date_filter.xml` is additionally pinned by
SHA-256 in `docs/tally/compatibility/compatibility-surface.json`.

| Fixture | Models |
| --- | --- |
| `export_status_1.xml` | `STATUS=1` with an empty collection |
| `export_status_0.xml` | `STATUS=0` with a `LINEERROR` — an application rejection |
| `export_status_invalid.xml` | `STATUS=-1` |
| `export_status_missing.xml` | no `STATUS` element |
| `normal_export.xml` | one ledger, `OPENINGBALANCE` -1180.00 |
| `empty_export.xml` | `RECORDCOUNT` 0 |
| `duplicate_identity.xml` | two ledgers sharing one GUID |
| `wrong_company.xml` | a response for a different synthetic company |
| `voucher_export.xml` | one Receipt voucher carrying `REMOTEID` |
| `inconsistent_date_filter.xml` | a declared July window containing a June voucher |
| `record_count_mismatch.xml` | declares two records, contains one |
| `malformed_export_metadata.xml` | `RECORDCOUNT` -1 |
| `duplicate_export_metadata.xml` | `COMPANYCONTEXT` given both as attribute and as child elements |
| `exact_decimals.xml` | boundary decimal amounts, including 999999999999.9999 |
| `import_counters.xml` | clean import counters |
| `import_duplicate.xml` | `IGNORED=1`, `ERRORS=1` with a duplicate-identity line error |
| `import_partial.xml` | mixed import counters with an exception |
| `malformed.xml` | an unclosed element |
| `truncated.xml` | a body cut off mid-element |
| `synthetic_json_semantic_reference.json` | Bridge-shaped JSON restating `normal_export.xml`'s values; not a Tally JSONEX envelope and not a parity fixture |
