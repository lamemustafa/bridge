# Native fixture byte provenance

## Native Trial Balance captures — 2026-09-08

`trial_balance_known_lab.xml` and `trial_balance_opening_year.xml` are
byte-exact, synthetic `List of Ledgers` collection responses captured from a
read-only native Trial Balance experiment. The first has six ledger rows and
records present-empty native Amount elements; the second has eight rows and
contains genuine non-zero brought-forward openings. Both envelopes carry
`STATUS=1`, their row GUIDs identify the selected synthetic company, and the
captured response includes the native `TBALOPENING`, `DEBITTOTALS`,
`CREDITTOTALS`, and `TBALCLOSING` fields.

| Fixture | Bytes | SHA-256 |
| --- | ---: | --- |
| `trial_balance_known_lab.xml` | 6,313 | `79ec1ca08428cade945e18136bf04ec67213935e3e229814343c42e2bf64bb4d` |
| `trial_balance_opening_year.xml` | 7,919 | `be3d72463d9840ad1553e2b3c7b7f15a041646e94c818b39efc73ae4694ccfef` |

The captures were screened before commit for customer identifiers and contact
fields. They are parser evidence only: the production request fetches the
smaller Trial Balance field set, so its request/response pairing requires its
own live qualification.

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

## Computed Group company GUID — 2026-08-31

`group_snapshot_aarav_with_computed_company_guid.xml` is the verbatim UTF-8
response to the exact request rendered by
`render_native_group_snapshot_request("Aarav Trading Company Demo")` in
`../../../src/native_outstandings/request.rs` at capture time. That request is a
read-only native `List of Groups` collection request with:

```xml
<FETCH>NAME, PARENT, GUID, MASTERID, ALTERID, RESERVEDNAME</FETCH>
<COMPUTE>BRIDGECOMPANYGUID:$GUID:Company:##SVCurrentCompany</COMPUTE>
```

It was sent once to the licensed TallyPrime 7.1 instance with the synthetic
`Aarav Trading Company Demo` selected. `/status` was healthy before and after.
The returned envelope had `STATUS=1`, was 27,140 bytes, contained 28 Group
rows, and contained `BRIDGECOMPANYGUID` in all 28 rows. Every value was the
selected company's `bb8ad19e-6aef-4239-a917-87fec0c6215e`; the fixture SHA-256
is `bb2c20f7d9e11634f9cf1f6429f655dc31d50b60fca72c71a6ce981c47db099c`.

**VERIFIED — single captured profile only.** This proves the exact request and
synthetic company/profile recorded above; it does not establish that every
Tally release, mode, or Group collection shape emits this computed field.

The capture was screened before commit: it contains no GSTIN, PAN, contact,
address, email, phone, website, or PIN-code fields. Tally's observed invalid
XML control characters are intentionally retained so this fixture continues to
exercise the tolerant parser. This provenance pairs the response with the
request contract; do not regenerate or edit either as a synthetic substitute.

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

## Aarav Group snapshot without company GUID — 2026-08-20

`group_snapshot_aarav.xml` is a live response from TallyPrime 7.1 EDU on port
9001 with the synthetic `Aarav Trading Company Demo` selected, `/status` 200
before and after (`41f97f58`, #158). The response arrived as BOM-less UTF-16LE
and was stored decoded to UTF-8; the commit states it was not hand-edited. It
predates the GUID-widened Group request, so its 28 rows carry no `GUID` — which
is what `wire_group_tests.rs` uses it to prove is refused. Its CRLF line
endings survived commit, so it is not among the Git-normalised files above.
The original UTF-16LE bytes' SHA-256 and the exact request were not recorded;
the hash below pins the committed decoded bytes only.

| Fixture | Bytes | SHA-256 |
| --- | ---: | --- |
| `group_snapshot_aarav.xml` | 17,959 | `2d8a1acd8b7c7a2f49f3c588fb1fb0581a7e2a5c640526f0aad4ec89fd22dcfa` |

## Master fields lab — 2026-08-28

Live responses from one licensed TallyPrime Silver synthetic company,
`BRIDGE MASTER FIELDS LAB`, added in `375d8bd6` (#193).
`docs/tally/TALLY_PROTOCOL_REFERENCE.md` §9.4a records the run: every request
bracketed by a 200 `/status`, every write scoped to the lab company, and the
exact native responses retained here. Port and Tally release number are not
recorded, and no transformation is recorded; the `.utf8` suffix names the
stored encoding, not a documented decode step.

The native master/balance/group reads:

| Fixture | Bytes | SHA-256 |
| --- | ---: | --- |
| `ledgers_native_master_fields_lab.utf8.xml` | 15,146 | `def766e42d0e36b4b73d7a176fa0ad08d1a7467e301000650fb5ba9a2ae06f29` |
| `ledger_snapshot_master_fields_lab.utf8.xml` | 9,796 | `7e7af264d7251713d5179c6b6614f47329d860e941587ec9a5245597bf059f77` |
| `group_snapshot_master_fields_lab.utf8.xml` | 24,453 | `a9839b578ed776b707597e5dad93d155ab443b93c03d5ecb0b70bfd7ac207b1b` |

These predate the response-bound `BRIDGECOMPANYGUID` compute; tests that need
it inject it at test time rather than editing the fixture.

The partial-`Alter` experiment §9.4a describes, in order — create, readback,
alter, readback:

| Fixture | Bytes | SHA-256 |
| --- | ---: | --- |
| `master_fields_lab_partial_alter_create.response.xml` | 1,917 | `2d23cac5f83c0f119d3545f27a07472faec8cd963d1bbd725e2073d0bdb7990a` |
| `master_fields_lab_partial_alter_before.response.xml` | 215,611 | `042f8a2997cb9a4b6910a6edc1cdab08a69f2cf6dff627c5e98bd6219d5ade76` |
| `master_fields_lab_partial_alter.response.xml` | 1,917 | `56e11c463b5251d90e506157a29237813ea5ff04958beb46431e8b30bd2aa7fc` |
| `master_fields_lab_partial_alter_after.response.xml` | 215,611 | `1f367a0f00a2b3dd10881f17beeafb72d813247292d8873304b2c7265d036aa1` |

`_before` is the readback taken before the Alter (PAN `ZZZZZ0000Z`,
`ALTERID` 208); `_after` differs from it only in the PAN (`ZZZZZ0001Z`) and
`ALTERID` 209, and still carries the Party GSTIN.

The three requests — `master_fields_lab_partial_alter_create.request.xml`,
`master_fields_lab_partial_alter.request.xml` and
`master_fields_lab_partial_alter_readback.request.xml` — record what the lab
run sent, but their authorship is not recorded: no Bridge request builder
emits the `All Masters` import or the `List of Accounts` export they contain,
and whether their trailing newline was on the wire is unknown. They are
documented here as named-only evidence, with no byte-exact claim.
