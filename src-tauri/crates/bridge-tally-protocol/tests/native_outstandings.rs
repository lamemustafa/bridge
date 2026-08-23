//! Integration tests for `native_outstandings`, primarily driven by real
//! fixtures captured live from TallyPrime
//! (`tests/fixtures/native/*.xml`, captured 2026-08-07). The few synthetic
//! policy boundaries identify themselves in their test comments; no synthetic
//! XML is presented as a capture. No test makes a network or live-Tally call.

use bridge_tally_primitives::{ExactDecimal, TallyDate};
use bridge_tally_protocol::native_outstandings::{
    age_in_days, compute_native_outstandings, parse_native_bill_rows, parse_native_group_snapshot,
    parse_native_ledger_snapshot, AgeingAnchor, NativeBillRow, NativeGroupSnapshot,
    NativeMasterSnapshot, NativeOutstandingsError, NativeOverdueCrosscheck,
};

/// A synthetic company GUID used to bind the native group snapshot rows
/// constructed in this file, matching the pattern of the real captured GUIDs
/// (`<company-guid>-<row-sequence>`).
const RESERVEDNAME_TESTS_COMPANY_GUID: &str = "11111111-1111-1111-1111-111111111111";

const BILLS_RECEIVABLE_BILLWISE_LAB: &str =
    include_str!("fixtures/native/bills_receivable_billwise_lab.xml");
const BILLS_PAYABLE_BILLWISE_LAB_EMPTY: &str =
    include_str!("fixtures/native/bills_payable_billwise_lab_empty.xml");
const LEDGER_SNAPSHOT_BILLWISE_LAB: &str =
    include_str!("fixtures/native/ledger_snapshot_billwise_lab.xml");
const BILLS_RECEIVABLE_AGEING_LAB: &str =
    include_str!("fixtures/native/bills_receivable_ageing_lab.xml");
const BILLS_RECEIVABLE_VALIDATION_LAB: &str =
    include_str!("fixtures/native/bills_receivable_validation_lab.xml");
const BILLS_PAYABLE_VALIDATION_LAB: &str =
    include_str!("fixtures/native/bills_payable_validation_lab.xml");
const LEDGER_SNAPSHOT_VALIDATION_LAB: &str =
    include_str!("fixtures/native/ledger_snapshot_validation_lab.xml");

/// `BOOKSFROM` for "Bridge Billwise Lab", from `company_extent_9000.xml`
/// (`<BOOKSFROM TYPE="Date">20240401</BOOKSFROM>`).
const BILLWISE_LAB_BOOKS_FROM: &str = "20240401";
/// `BOOKSFROM` for "Bridge Ageing Lab", from `company_extent_9000.xml`
/// (`<BOOKSFROM TYPE="Date">20260401</BOOKSFROM>`).
const AGEING_LAB_BOOKS_FROM: &str = "20260401";
const NATIVE_CAPTURE_AS_OF: &str = "20260731";
/// `BOOKSFROM` for the purpose-built `Bridge Validation Lab`, observed via
/// the paired `CompanyBookExtentV1` read that bracketed the 2026-08-17 capture.
const VALIDATION_LAB_BOOKS_FROM: &str = "20250401";
const VALIDATION_CAPTURE_AS_OF: &str = "20260817";

const BILLWISE_LAB_COMPANY: &str = "Bridge Billwise Lab";

fn as_of(yyyymmdd: &str) -> TallyDate {
    TallyDate::parse(yyyymmdd).unwrap()
}

/// `ExactDecimal`'s `PartialEq` is literal-lexeme equality, not numeric
/// equality (TALLY_PROTOCOL_REFERENCE: it deliberately never converts
/// through floating point, and preserves whatever scale it was constructed
/// with). Every value that has passed through `checked_add`/`checked_subtract`
/// is canonicalized (trailing fractional zeros stripped), so assertions
/// compare against that canonical form rather than the fixture's own
/// `X.00` lexeme.
fn assert_exact(actual: &ExactDecimal, canonical: &str) {
    assert_eq!(actual.as_str(), canonical);
}

/// Synthetic policy rows for the crosscheck decision table. They never make a
/// network call and intentionally model only counter/date evidence, not a
/// captured Tally response.
fn crosscheck_bill(reference: &str, due: &str, tally_overdue_days: Option<i64>) -> NativeBillRow {
    NativeBillRow {
        party: "Synthetic policy party".to_string(),
        reference: reference.to_string(),
        bill_date: as_of("20260101"),
        due_date: as_of(due),
        closing_balance: ExactDecimal::parse("-1").expect("synthetic exact amount"),
        tally_overdue_days,
    }
}

fn crosscheck_for(rows: &[NativeBillRow], requested_as_of: &str) -> NativeOverdueCrosscheck {
    compute_native_outstandings(
        "Synthetic policy company",
        rows,
        &[],
        NativeMasterSnapshot {
            ledgers: &[],
            groups: NativeGroupSnapshot::LegacyFixtureWithoutGroups,
        },
        AgeingAnchor::DueDate,
        &as_of(requested_as_of),
        0,
    )
    .expect("policy rows produce a classifier result")
    .overdue_crosscheck
}

#[test]
fn zero_bill_rows_with_nonzero_ledger_residual_are_unconfirmed() {
    let group_bytes = r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY>
        <DATA><COLLECTION>
        <GROUP NAME="North Region" RESERVEDNAME=""><GUID>11111111-1111-1111-1111-111111111111-00000001</GUID><PARENT>Sundry Debtors</PARENT></GROUP>
        <GROUP NAME="Sundry Debtors" RESERVEDNAME="Sundry Debtors"><GUID>11111111-1111-1111-1111-111111111111-00000002</GUID><PARENT>Primary</PARENT></GROUP>
        </COLLECTION></DATA></BODY></ENVELOPE>"#;
    let ledger_bytes = r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>
        <LEDGER NAME="Nested Customer"><PARENT>North Region</PARENT>
        <CLOSINGBALANCE>-100.00</CLOSINGBALANCE><OPENINGBALANCE>0.00</OPENINGBALANCE>
        <ISBILLWISEON>No</ISBILLWISEON></LEDGER>
        </COLLECTION></DATA></BODY></ENVELOPE>"#;
    let groups = parse_native_group_snapshot(group_bytes, RESERVEDNAME_TESTS_COMPANY_GUID)
        .expect("raw group hierarchy parses");
    let ledgers = parse_native_ledger_snapshot(ledger_bytes).expect("raw ledger snapshot parses");

    let result = compute_native_outstandings(
        "Synthetic Company",
        &[],
        &[],
        NativeMasterSnapshot {
            ledgers: &ledgers,
            groups: NativeGroupSnapshot::Complete(&groups),
        },
        AgeingAnchor::DueDate,
        &as_of(NATIVE_CAPTURE_AS_OF),
        group_bytes.len() + ledger_bytes.len(),
    )
    .expect("nested debtor ancestry is complete");

    assert_exact(&result.residual_total, "100");
    assert_eq!(result.residuals[0].party, "Nested Customer");
    assert_eq!(
        result.overdue_crosscheck,
        NativeOverdueCrosscheck::UnconfirmedAsOfWithoutBillReferences
    );
}

#[test]
fn crosscheck_table_no_rows_with_zero_residual_is_unconfirmed() {
    let result = compute_native_outstandings(
        "Synthetic Empty Company",
        &[],
        &[],
        NativeMasterSnapshot {
            ledgers: &[],
            groups: NativeGroupSnapshot::LegacyFixtureWithoutGroups,
        },
        AgeingAnchor::DueDate,
        &as_of(NATIVE_CAPTURE_AS_OF),
        0,
    )
    .expect("an empty response remains a classifiable policy boundary");

    assert_eq!(result.residual_total, ExactDecimal::zero());
    assert_eq!(
        result.overdue_crosscheck,
        NativeOverdueCrosscheck::UnconfirmedAsOfWithoutEffectiveDateEvidence
    );
}

#[test]
fn complete_group_snapshot_refuses_empty_or_unresolved_ancestry() {
    let ledger_bytes = r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>
        <LEDGER NAME="Nested Customer"><PARENT>Sundry Debtors</PARENT>
        <CLOSINGBALANCE>-100.00</CLOSINGBALANCE><OPENINGBALANCE>0.00</OPENINGBALANCE>
        <ISBILLWISEON>No</ISBILLWISEON></LEDGER>
        </COLLECTION></DATA></BODY></ENVELOPE>"#;
    let ledgers = parse_native_ledger_snapshot(ledger_bytes).expect("raw ledger snapshot parses");

    for (group_bytes, expected_code) in [
        (None, "group_snapshot_empty"),
        (
            Some(
                r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>
                <GROUP NAME="Unrelated Group" RESERVEDNAME=""><GUID>11111111-1111-1111-1111-111111111111-00000003</GUID><PARENT>Primary</PARENT></GROUP>
                </COLLECTION></DATA></BODY></ENVELOPE>"#,
            ),
            "ledger_group_parent_unresolved",
        ),
    ] {
        let groups = group_bytes
            .map(|bytes| {
                parse_native_group_snapshot(bytes, RESERVEDNAME_TESTS_COMPANY_GUID)
                    .expect("raw group snapshot parses")
            })
            .unwrap_or_default();
        let error = compute_native_outstandings(
            "Synthetic Company",
            &[],
            &[],
            NativeMasterSnapshot {
                ledgers: &ledgers,
                groups: NativeGroupSnapshot::Complete(&groups),
            },
            AgeingAnchor::DueDate,
            &as_of(NATIVE_CAPTURE_AS_OF),
            group_bytes.map_or(0, str::len) + ledger_bytes.len(),
        )
        .expect_err("an incomplete production group snapshot must fail closed");

        assert_eq!(
            error,
            NativeOutstandingsError::InvalidResponse(expected_code)
        );
    }
}

#[test]
fn complete_group_snapshot_requires_reservedname_evidence() {
    let group_xml = format!(
        r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>
        <GROUP NAME="Sundry Debtors"><GUID>{guid}-00000001</GUID><PARENT>Primary</PARENT></GROUP>
        </COLLECTION></DATA></BODY></ENVELOPE>"#,
        guid = RESERVEDNAME_TESTS_COMPANY_GUID,
    );
    let groups = parse_native_group_snapshot(&group_xml, RESERVEDNAME_TESTS_COMPANY_GUID)
        .expect("group snapshot without RESERVEDNAME evidence parses at the XML boundary");
    assert_eq!(groups[0].reserved_name, None);
    let ledgers = parse_native_ledger_snapshot(
        "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>\
         <LEDGER NAME=\"Fallback Customer\"><PARENT>Sundry Debtors</PARENT>\
         <CLOSINGBALANCE>-500.00</CLOSINGBALANCE><OPENINGBALANCE>0</OPENINGBALANCE>\
         <ISBILLWISEON>No</ISBILLWISEON></LEDGER>\
         </COLLECTION></DATA></BODY></ENVELOPE>",
    )
    .expect("raw ledger snapshot parses");

    assert_eq!(
        compute_native_outstandings(
            "Synthetic Company",
            &[],
            &[],
            NativeMasterSnapshot {
                ledgers: &ledgers,
                groups: NativeGroupSnapshot::Complete(&groups),
            },
            AgeingAnchor::DueDate,
            &as_of(NATIVE_CAPTURE_AS_OF),
            group_xml.len(),
        ),
        Err(NativeOutstandingsError::InvalidResponse(
            "group_reserved_name_missing"
        )),
        "a Complete group snapshot cannot claim classification evidence while omitting RESERVEDNAME"
    );
}

/// U2 -- the silent-empty-report defect: classifying a party group by
/// mutable `NAME` instead of Tally's immutable `RESERVEDNAME`. Renaming
/// "Sundry Debtors" over `Import Data` / `All Masters` succeeds in live
/// Tally (measured 2026-08-20, WR2 Unicode Lab) and leaves `RESERVEDNAME`
/// untouched; before this fix, that rename made Bridge classify zero
/// parties under the renamed group and produce an empty report with no
/// error.
#[test]
fn renamed_sundry_debtors_group_still_classifies_its_ledgers_by_reservedname() {
    let group_xml = format!(
        r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>
        <GROUP NAME="WR5 Renamed Suspense" RESERVEDNAME="Sundry Debtors"><GUID>{guid}-00000001</GUID><PARENT>Primary</PARENT></GROUP>
        </COLLECTION></DATA></BODY></ENVELOPE>"#,
        guid = RESERVEDNAME_TESTS_COMPANY_GUID,
    );
    let groups = parse_native_group_snapshot(&group_xml, RESERVEDNAME_TESTS_COMPANY_GUID)
        .expect("renamed predefined group snapshot parses");
    assert_eq!(
        groups[0].name, "WR5 Renamed Suspense",
        "sanity: NAME really did change"
    );
    assert_eq!(
        groups[0].reserved_name.as_deref(),
        Some("Sundry Debtors"),
        "sanity: RESERVEDNAME kept the original identity through the rename"
    );

    let ledgers = parse_native_ledger_snapshot(
        "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>\
         <LEDGER NAME=\"Renamed Group Customer\"><PARENT>WR5 Renamed Suspense</PARENT>\
         <CLOSINGBALANCE>-500.00</CLOSINGBALANCE><OPENINGBALANCE>0</OPENINGBALANCE>\
         <ISBILLWISEON>No</ISBILLWISEON></LEDGER>\
         </COLLECTION></DATA></BODY></ENVELOPE>",
    )
    .expect("raw ledger snapshot parses");

    let result = compute_native_outstandings(
        "Synthetic Company",
        &[],
        &[],
        NativeMasterSnapshot {
            ledgers: &ledgers,
            groups: NativeGroupSnapshot::Complete(&groups),
        },
        AgeingAnchor::DueDate,
        &as_of(NATIVE_CAPTURE_AS_OF),
        group_xml.len(),
    )
    .expect("renamed-group ancestry resolves");

    // Before the fix, classification matched on NAME ("wr5 renamed
    // suspense" != "sundry debtors"), so this ledger was silently dropped
    // and `residuals` was empty -- a legitimately configured book reporting
    // zero receivables with no error.
    assert_eq!(
        result.residuals.len(),
        1,
        "a ledger under the renamed predefined group must still be classified as a party"
    );
    assert_eq!(result.residuals[0].party, "Renamed Group Customer");
    assert_exact(&result.residual_total, "500");
}

/// The Sundry Creditors mirror of
/// `renamed_sundry_debtors_group_still_classifies_its_ledgers_by_reservedname`.
#[test]
fn renamed_sundry_creditors_group_still_classifies_its_ledgers_by_reservedname() {
    let group_xml = format!(
        r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>
        <GROUP NAME="WR5 Renamed Payables" RESERVEDNAME="Sundry Creditors"><GUID>{guid}-00000001</GUID><PARENT>Primary</PARENT></GROUP>
        </COLLECTION></DATA></BODY></ENVELOPE>"#,
        guid = RESERVEDNAME_TESTS_COMPANY_GUID,
    );
    let groups = parse_native_group_snapshot(&group_xml, RESERVEDNAME_TESTS_COMPANY_GUID)
        .expect("renamed predefined group snapshot parses");

    let ledgers = parse_native_ledger_snapshot(
        "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>\
         <LEDGER NAME=\"Renamed Group Vendor\"><PARENT>WR5 Renamed Payables</PARENT>\
         <CLOSINGBALANCE>750.00</CLOSINGBALANCE><OPENINGBALANCE>0</OPENINGBALANCE>\
         <ISBILLWISEON>No</ISBILLWISEON></LEDGER>\
         </COLLECTION></DATA></BODY></ENVELOPE>",
    )
    .expect("raw ledger snapshot parses");

    let result = compute_native_outstandings(
        "Synthetic Company",
        &[],
        &[],
        NativeMasterSnapshot {
            ledgers: &ledgers,
            groups: NativeGroupSnapshot::Complete(&groups),
        },
        AgeingAnchor::DueDate,
        &as_of(NATIVE_CAPTURE_AS_OF),
        group_xml.len(),
    )
    .expect("renamed-group ancestry resolves");

    assert_eq!(
        result.residuals.len(),
        1,
        "a ledger under the renamed predefined creditors group must still be classified as a party"
    );
    assert_eq!(result.residuals[0].party, "Renamed Group Vendor");
    assert_exact(&result.residual_total, "750");
}

/// The deliberate other-direction case: a USER-CREATED group that merely
/// happens to be named "Sundry Debtors". Predefined groups cannot be
/// deleted, only renamed, so if a real predefined "Sundry Debtors" group
/// exists in this book it is a different row (carrying the non-empty
/// RESERVEDNAME) -- this row's empty RESERVEDNAME is Tally's own signal that
/// THIS group is not it. Falling back to NAME here would let the lookalike
/// masquerade as the predefined group, reviving the identical name/identity
/// confusion this fix removes, just pointed the other way. A book that has
/// always used a custom group named "Sundry Debtors" for its receivables
/// does lose those ledgers from this classification path -- but only for
/// ledgers with bill-wise tracking off; a bill-wise-on ledger there still
/// reaches the report through `is_party_ledger`'s other trigger.
#[test]
fn user_created_group_merely_named_sundry_debtors_is_not_treated_as_predefined() {
    let group_xml = format!(
        r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>
        <GROUP NAME="Sundry Debtors" RESERVEDNAME=""><GUID>{guid}-00000001</GUID><PARENT>Primary</PARENT></GROUP>
        </COLLECTION></DATA></BODY></ENVELOPE>"#,
        guid = RESERVEDNAME_TESTS_COMPANY_GUID,
    );
    let groups = parse_native_group_snapshot(&group_xml, RESERVEDNAME_TESTS_COMPANY_GUID)
        .expect("custom lookalike group snapshot parses");
    assert_eq!(
        groups[0].reserved_name.as_deref(),
        Some(""),
        "sanity: Tally's own empty RESERVEDNAME marks this row user-created"
    );

    let ledgers = parse_native_ledger_snapshot(
        "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>\
         <LEDGER NAME=\"Lookalike Group Customer\"><PARENT>Sundry Debtors</PARENT>\
         <CLOSINGBALANCE>-500.00</CLOSINGBALANCE><OPENINGBALANCE>0</OPENINGBALANCE>\
         <ISBILLWISEON>No</ISBILLWISEON></LEDGER>\
         </COLLECTION></DATA></BODY></ENVELOPE>",
    )
    .expect("raw ledger snapshot parses");

    let result = compute_native_outstandings(
        "Synthetic Company",
        &[],
        &[],
        NativeMasterSnapshot {
            ledgers: &ledgers,
            groups: NativeGroupSnapshot::Complete(&groups),
        },
        AgeingAnchor::DueDate,
        &as_of(NATIVE_CAPTURE_AS_OF),
        group_xml.len(),
    )
    .expect("lookalike-group ancestry resolves");

    assert_eq!(
        result.residuals.len(),
        0,
        "a group merely named \"Sundry Debtors\", with RESERVEDNAME explicitly empty, must not be trusted as the predefined party group"
    );
    assert_exact(&result.residual_total, "0");
}

/// Historical fixtures never carried group snapshots. Their labelled legacy
/// mode keeps the pre-group-ancestry, NAME-only classification intact.
#[test]
fn legacy_fixture_without_groups_keeps_name_only_party_classification() {
    let ledgers = parse_native_ledger_snapshot(
        "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>\
         <LEDGER NAME=\"Fallback Customer\"><PARENT>Sundry Debtors</PARENT>\
         <CLOSINGBALANCE>-500.00</CLOSINGBALANCE><OPENINGBALANCE>0</OPENINGBALANCE>\
         <ISBILLWISEON>No</ISBILLWISEON></LEDGER>\
         </COLLECTION></DATA></BODY></ENVELOPE>",
    )
    .expect("raw ledger snapshot parses");

    let result = compute_native_outstandings(
        "Synthetic Company",
        &[],
        &[],
        NativeMasterSnapshot {
            ledgers: &ledgers,
            groups: NativeGroupSnapshot::LegacyFixtureWithoutGroups,
        },
        AgeingAnchor::DueDate,
        &as_of(NATIVE_CAPTURE_AS_OF),
        0,
    )
    .expect("legacy no-group fixture keeps the historical NAME-only classification");

    assert_eq!(
        result.residuals.len(),
        1,
        "legacy no-group fixtures retain their historical direct-parent NAME match"
    );
    assert_exact(&result.residual_total, "500");
}

#[test]
fn crosscheck_table_zero_only_future_counters_leave_the_as_of_date_unconfirmed() {
    let receivable = [NativeBillRow {
        party: "Synthetic customer".to_string(),
        reference: "SYNTHETIC-FUTURE-DUE".to_string(),
        bill_date: as_of("20260701"),
        due_date: as_of("20260830"),
        closing_balance: ExactDecimal::parse("-100.00").unwrap(),
        // Some rows encode not-yet-overdue as zero; the captured validation
        // book encodes it as empty. Neither representation is an overdue age.
        tally_overdue_days: Some(0),
    }];

    let result = compute_native_outstandings(
        "Synthetic Company",
        &receivable,
        &[],
        NativeMasterSnapshot {
            ledgers: &[],
            groups: NativeGroupSnapshot::LegacyFixtureWithoutGroups,
        },
        AgeingAnchor::DueDate,
        &as_of("20260731"),
        0,
    )
    .expect("a future-due bill must not abort the report");

    assert_exact(&result.report.receivable_total, "100");
    assert_exact(&result.report.ageing.days_0_30, "100");
    assert_eq!(result.report.ageing.days_90_plus, ExactDecimal::zero());
    assert_eq!(result.report.open_receivable_bill_count, 1);
    assert_eq!(result.report.ageing_bill_counts.days_0_30, 1);
    assert_eq!(result.report.ageing_bill_counts.days_90_plus, 0);
    assert_eq!(
        result.report.top_parties[0].oldest_bill_age_days, None,
        "a future-due bill must not be presented as the oldest overdue bill"
    );
    assert_eq!(
        result.overdue_crosscheck,
        NativeOverdueCrosscheck::UnconfirmedAsOfWithoutEffectiveDateEvidence
    );
}

#[test]
fn validation_lab_empty_billoverdue_parses_as_not_applicable() {
    let rows = parse_native_bill_rows(
        BILLS_RECEIVABLE_VALIDATION_LAB,
        &as_of(VALIDATION_LAB_BOOKS_FROM),
        &as_of(VALIDATION_CAPTURE_AS_OF),
    )
    .expect("the captured empty BILLOVERDUE value must not fail the read");

    assert_eq!(rows.len(), 5);
    let future = rows
        .iter()
        .find(|row| row.reference == "ALPHA-FUTURE")
        .expect("the captured future-due bill is present");
    assert_eq!(future.due_date.as_str(), "20261001");
    assert_eq!(future.tally_overdue_days, None);
}

#[test]
fn captured_validation_lab_one_positive_counter_remains_a_withheld_inconsistency() {
    // The fixture has one positive counter implying 2026-08-01, three zero
    // counters due on that date, and one future empty counter. The zero/empty
    // rows are compatible but non-identifying, so they cannot promote this to
    // a refused-period claim. The totals stay withheld either way.
    let rows = parse_native_bill_rows(
        BILLS_RECEIVABLE_VALIDATION_LAB,
        &as_of(VALIDATION_LAB_BOOKS_FROM),
        &as_of(VALIDATION_CAPTURE_AS_OF),
    )
    .expect("captured validation bytes parse");
    let result = compute_native_outstandings(
        "Bridge Validation Lab",
        &rows,
        &[],
        NativeMasterSnapshot {
            ledgers: &[],
            groups: NativeGroupSnapshot::LegacyFixtureWithoutGroups,
        },
        AgeingAnchor::DueDate,
        &as_of(VALIDATION_CAPTURE_AS_OF),
        BILLS_RECEIVABLE_VALIDATION_LAB.len(),
    )
    .expect("the policy boundary remains computable");

    assert_eq!(
        result.overdue_crosscheck,
        NativeOverdueCrosscheck::Inconsistent
    );
}

#[test]
fn crosscheck_table_empty_only_counters_leave_the_as_of_date_unconfirmed() {
    // Synthetic policy boundary, not a captured refusal response: the current
    // capture contains one empty counter alongside four informative ones, but
    // no captured response has every counter empty. This construction proves
    // only Bridge's fail-closed rule that absence cannot establish acceptance.
    assert_eq!(
        crosscheck_for(
            &[
                crosscheck_bill("EMPTY-EARLIER", "20260801", None),
                crosscheck_bill("EMPTY-EQUAL", "20260817", None),
                crosscheck_bill("EMPTY-LATER", "20260901", None),
            ],
            "20260817",
        ),
        NativeOverdueCrosscheck::UnconfirmedAsOfWithoutEffectiveDateEvidence
    );
}

#[test]
fn crosscheck_table_nonpositive_counters_cover_earlier_equal_and_later_due_dates() {
    let requested = "20260822";
    let zero_only = [
        crosscheck_bill("ZERO-EARLIER", "20260801", Some(0)),
        crosscheck_bill("ZERO-EQUAL", requested, Some(0)),
        crosscheck_bill("ZERO-LATER", "20260901", Some(0)),
    ];
    let empty_and_zero = [
        crosscheck_bill("EMPTY-EARLIER", "20260801", None),
        crosscheck_bill("ZERO-EQUAL", requested, Some(0)),
        crosscheck_bill("EMPTY-LATER", "20260901", None),
    ];

    for rows in [&zero_only[..], &empty_and_zero[..]] {
        assert_eq!(
            crosscheck_for(rows, requested),
            NativeOverdueCrosscheck::UnconfirmedAsOfWithoutEffectiveDateEvidence
        );
    }
}

#[test]
fn crosscheck_table_negative_counter_is_inconsistent() {
    assert_eq!(
        crosscheck_for(
            &[crosscheck_bill("NEGATIVE", "20260801", Some(-1))],
            "20260822"
        ),
        NativeOverdueCrosscheck::Inconsistent
    );
}

#[test]
fn crosscheck_table_requested_positive_evidence_accepts_only_compatible_companions() {
    let requested = "20260822";
    let positives = [
        crosscheck_bill("POSITIVE-ONE", "20260701", Some(52)),
        crosscheck_bill("POSITIVE-TWO", "20260601", Some(82)),
    ];
    let empty_later = crosscheck_bill("EMPTY-LATER", "20260901", None);
    let zero_equal = crosscheck_bill("ZERO-EQUAL", requested, Some(0));
    let zero_later = crosscheck_bill("ZERO-LATER", "20260901", Some(0));
    let empty_earlier = crosscheck_bill("EMPTY-EARLIER", "20260801", None);
    let empty_equal = crosscheck_bill("EMPTY-EQUAL", requested, None);
    let zero_earlier = crosscheck_bill("ZERO-EARLIER", "20260801", Some(0));

    assert_eq!(
        crosscheck_for(&positives, requested),
        NativeOverdueCrosscheck::Honored
    );
    assert_eq!(
        crosscheck_for(
            &[positives[0].clone(), positives[1].clone(), empty_later],
            requested
        ),
        NativeOverdueCrosscheck::Honored
    );
    assert_eq!(
        crosscheck_for(
            &[
                positives[0].clone(),
                positives[1].clone(),
                zero_equal,
                zero_later,
            ],
            requested,
        ),
        NativeOverdueCrosscheck::Honored
    );
    assert_eq!(
        crosscheck_for(
            &[positives[0].clone(), positives[1].clone(), empty_equal],
            requested
        ),
        NativeOverdueCrosscheck::Inconsistent
    );
    assert_eq!(
        crosscheck_for(
            &[positives[0].clone(), positives[1].clone(), empty_earlier],
            requested
        ),
        NativeOverdueCrosscheck::Inconsistent
    );
    assert_eq!(
        crosscheck_for(
            &[positives[0].clone(), positives[1].clone(), zero_earlier],
            requested
        ),
        NativeOverdueCrosscheck::Inconsistent
    );
}

#[test]
fn crosscheck_table_substituted_date_requires_two_compatible_positive_counters() {
    let requested = "20260822";
    let alternative_one = crosscheck_bill("ALTERNATIVE-ONE", "20260701", Some(31));
    let alternative_two = crosscheck_bill("ALTERNATIVE-TWO", "20260601", Some(61));

    assert_eq!(
        crosscheck_for(std::slice::from_ref(&alternative_one), requested),
        NativeOverdueCrosscheck::Inconsistent
    );
    assert_eq!(
        crosscheck_for(
            &[
                alternative_one.clone(),
                alternative_two.clone(),
                crosscheck_bill("EMPTY-FUTURE", "20260901", None),
                crosscheck_bill("ZERO-AT-ALTERNATIVE", "20260801", Some(0)),
            ],
            requested,
        ),
        NativeOverdueCrosscheck::RefusedAsOf {
            tally_as_of: as_of("20260801"),
        }
    );
    assert_eq!(
        crosscheck_for(
            &[
                alternative_one,
                alternative_two,
                crosscheck_bill("ZERO-BEFORE-ALTERNATIVE", "20260731", Some(0)),
            ],
            requested,
        ),
        NativeOverdueCrosscheck::Inconsistent
    );
}

#[test]
fn scattered_implied_dates_remain_a_genuine_crosscheck_inconsistency() {
    let rows = parse_native_bill_rows(
        "<ENVELOPE>\
         <BILLFIXED><BILLDATE>1-Jul-26</BILLDATE><BILLREF>ONE</BILLREF><BILLPARTY>Lab</BILLPARTY></BILLFIXED><BILLCL>-1</BILLCL><BILLDUE>1-Jul-26</BILLDUE><BILLOVERDUE>30</BILLOVERDUE>\
         <BILLFIXED><BILLDATE>2-Jul-26</BILLDATE><BILLREF>TWO</BILLREF><BILLPARTY>Lab</BILLPARTY></BILLFIXED><BILLCL>-1</BILLCL><BILLDUE>2-Jul-26</BILLDUE><BILLOVERDUE>31</BILLOVERDUE>\
         </ENVELOPE>",
        &as_of("20260101"),
        &as_of("20260822"),
    )
    .unwrap();
    let result = compute_native_outstandings(
        "Bridge Ageing Lab",
        &rows,
        &[],
        NativeMasterSnapshot {
            ledgers: &[],
            groups: NativeGroupSnapshot::LegacyFixtureWithoutGroups,
        },
        AgeingAnchor::DueDate,
        &as_of("20260822"),
        0,
    )
    .unwrap();

    assert_eq!(
        result.overdue_crosscheck,
        NativeOverdueCrosscheck::Inconsistent
    );
}

#[test]
fn honoured_as_of_keeps_native_outstandings_complete() {
    let rows = parse_native_bill_rows(
        "<ENVELOPE>\
         <BILLFIXED><BILLDATE>1-Jun-26</BILLDATE><BILLREF>JUN</BILLREF><BILLPARTY>Lab</BILLPARTY></BILLFIXED><BILLCL>-1</BILLCL><BILLDUE>1-Jun-26</BILLDUE><BILLOVERDUE>82</BILLOVERDUE>\
         <BILLFIXED><BILLDATE>1-Jul-26</BILLDATE><BILLREF>JUL</BILLREF><BILLPARTY>Lab</BILLPARTY></BILLFIXED><BILLCL>-1</BILLCL><BILLDUE>1-Jul-26</BILLDUE><BILLOVERDUE>52</BILLOVERDUE>\
         </ENVELOPE>",
        &as_of("20260101"),
        &as_of("20260822"),
    )
    .unwrap();
    let result = compute_native_outstandings(
        "Bridge Ageing Lab",
        &rows,
        &[],
        NativeMasterSnapshot {
            ledgers: &[],
            groups: NativeGroupSnapshot::LegacyFixtureWithoutGroups,
        },
        AgeingAnchor::DueDate,
        &as_of("20260822"),
        0,
    )
    .unwrap();

    assert_eq!(result.overdue_crosscheck, NativeOverdueCrosscheck::Honored);
    assert_eq!(result.report.as_of_yyyymmdd, "20260822");
}

#[test]
fn absent_billoverdue_is_not_the_same_as_present_but_empty() {
    let xml = "<ENVELOPE>\
        <BILLFIXED><BILLDATE>1-Aug-26</BILLDATE><BILLREF>FUTURE</BILLREF><BILLPARTY>P</BILLPARTY></BILLFIXED>\
        <BILLCL>-10.00</BILLCL><BILLDUE>1-Oct-26</BILLDUE>\
        </ENVELOPE>";
    assert_eq!(
        parse_native_bill_rows(
            xml,
            &as_of(VALIDATION_LAB_BOOKS_FROM),
            &as_of(VALIDATION_CAPTURE_AS_OF),
        ),
        Err(NativeOutstandingsError::InvalidResponse(
            "bills_fixed_row_missing_billoverdue"
        ))
    );
}

#[test]
fn report_direction_contradictions_in_raw_bill_bytes_fail_closed() {
    let parse = |amount: &str| {
        parse_native_bill_rows(
            &format!(
                "<ENVELOPE><BILLFIXED><BILLDATE>1-Jul-26</BILLDATE>\
                 <BILLREF>SIGN</BILLREF><BILLPARTY>P</BILLPARTY></BILLFIXED>\
                 <BILLCL>{amount}</BILLCL><BILLDUE>1-Jul-26</BILLDUE>\
                 <BILLOVERDUE>30</BILLOVERDUE></ENVELOPE>"
            ),
            &as_of(VALIDATION_LAB_BOOKS_FROM),
            &as_of(VALIDATION_CAPTURE_AS_OF),
        )
        .expect("synthetic row parses")
    };

    let positive_receivable = parse("100.00");
    assert_eq!(
        compute_native_outstandings(
            "Synthetic Company",
            &positive_receivable,
            &[],
            NativeMasterSnapshot {
                ledgers: &[],
                groups: NativeGroupSnapshot::LegacyFixtureWithoutGroups,
            },
            AgeingAnchor::DueDate,
            &as_of(VALIDATION_CAPTURE_AS_OF),
            0,
        ),
        Err(NativeOutstandingsError::InvalidResponse(
            "receivable_bill_sign_contradiction"
        ))
    );

    let negative_payable = parse("-100.00");
    assert_eq!(
        compute_native_outstandings(
            "Synthetic Company",
            &[],
            &negative_payable,
            NativeMasterSnapshot {
                ledgers: &[],
                groups: NativeGroupSnapshot::LegacyFixtureWithoutGroups,
            },
            AgeingAnchor::DueDate,
            &as_of(VALIDATION_CAPTURE_AS_OF),
            0,
        ),
        Err(NativeOutstandingsError::InvalidResponse(
            "payable_bill_sign_contradiction"
        ))
    );
}

#[test]
fn unaged_receivable_classification_uses_the_residual_not_the_net_ledger_sign() {
    let receivable = parse_native_bill_rows(
        "<ENVELOPE><BILLFIXED><BILLDATE>1-Jul-26</BILLDATE><BILLREF>R</BILLREF>\
         <BILLPARTY>Receivable flips payable</BILLPARTY></BILLFIXED><BILLCL>-100.00</BILLCL>\
         <BILLDUE>1-Jul-26</BILLDUE><BILLOVERDUE>30</BILLOVERDUE></ENVELOPE>",
        &as_of(VALIDATION_LAB_BOOKS_FROM),
        &as_of(VALIDATION_CAPTURE_AS_OF),
    )
    .expect("raw receivable parses");
    let payable = parse_native_bill_rows(
        "<ENVELOPE><BILLFIXED><BILLDATE>1-Jul-26</BILLDATE><BILLREF>P</BILLREF>\
         <BILLPARTY>Payable flips receivable</BILLPARTY></BILLFIXED><BILLCL>100.00</BILLCL>\
         <BILLDUE>1-Jul-26</BILLDUE><BILLOVERDUE>30</BILLOVERDUE></ENVELOPE>",
        &as_of(VALIDATION_LAB_BOOKS_FROM),
        &as_of(VALIDATION_CAPTURE_AS_OF),
    )
    .expect("raw payable parses");
    let ledgers = parse_native_ledger_snapshot(
        "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>\
         <LEDGER NAME=\"Receivable flips payable\"><PARENT>Sundry Debtors</PARENT>\
         <CLOSINGBALANCE>-50.00</CLOSINGBALANCE><OPENINGBALANCE>0</OPENINGBALANCE><ISBILLWISEON>Yes</ISBILLWISEON></LEDGER>\
         <LEDGER NAME=\"Payable flips receivable\"><PARENT>Sundry Creditors</PARENT>\
         <CLOSINGBALANCE>50.00</CLOSINGBALANCE><OPENINGBALANCE>0</OPENINGBALANCE><ISBILLWISEON>Yes</ISBILLWISEON></LEDGER>\
         </COLLECTION></DATA></BODY></ENVELOPE>",
    )
    .expect("raw ledgers parse");

    let payable_residual = compute_native_outstandings(
        "Synthetic Company",
        &receivable,
        &[],
        NativeMasterSnapshot {
            ledgers: &ledgers[..1],
            groups: NativeGroupSnapshot::LegacyFixtureWithoutGroups,
        },
        AgeingAnchor::DueDate,
        &as_of(VALIDATION_CAPTURE_AS_OF),
        0,
    )
    .expect("positive residual computes");
    assert!(!payable_residual.report.has_unaged_receivable);
    assert_eq!(payable_residual.residuals[0].amount.as_str(), "50");

    let receivable_residual = compute_native_outstandings(
        "Synthetic Company",
        &[],
        &payable,
        NativeMasterSnapshot {
            ledgers: &ledgers[1..],
            groups: NativeGroupSnapshot::LegacyFixtureWithoutGroups,
        },
        AgeingAnchor::DueDate,
        &as_of(VALIDATION_CAPTURE_AS_OF),
        0,
    )
    .expect("negative residual computes");
    assert!(receivable_residual.report.has_unaged_receivable);
    assert_eq!(receivable_residual.residuals[0].amount.as_str(), "-50");
}

#[test]
fn validation_lab_accounts_for_every_debtor_rupee_and_matches_tally_ageing() {
    let receivable = parse_native_bill_rows(
        BILLS_RECEIVABLE_VALIDATION_LAB,
        &as_of(VALIDATION_LAB_BOOKS_FROM),
        &as_of(VALIDATION_CAPTURE_AS_OF),
    )
    .expect("captured receivables parse");
    let payable = parse_native_bill_rows(
        BILLS_PAYABLE_VALIDATION_LAB,
        &as_of(VALIDATION_LAB_BOOKS_FROM),
        &as_of(VALIDATION_CAPTURE_AS_OF),
    )
    .expect("captured payables parse");
    let ledgers = parse_native_ledger_snapshot(LEDGER_SNAPSHOT_VALIDATION_LAB)
        .expect("captured ledger snapshot parses");

    let result = compute_native_outstandings(
        "Bridge Validation Lab",
        &receivable,
        &payable,
        NativeMasterSnapshot {
            ledgers: &ledgers,
            groups: NativeGroupSnapshot::LegacyFixtureWithoutGroups,
        },
        AgeingAnchor::DueDate,
        &as_of(VALIDATION_CAPTURE_AS_OF),
        BILLS_RECEIVABLE_VALIDATION_LAB.len()
            + BILLS_PAYABLE_VALIDATION_LAB.len()
            + LEDGER_SNAPSHOT_VALIDATION_LAB.len(),
    )
    .expect("validation-book outstandings compute");

    assert_exact(&result.report.receivable_total, "255553");
    assert_exact(&result.report.payable_total, "66666");
    assert_exact(&result.report.ageing.days_0_30, "244442");
    assert_eq!(result.report.ageing.days_31_60, ExactDecimal::zero());
    assert_eq!(result.report.ageing.days_61_90, ExactDecimal::zero());
    assert_exact(&result.report.ageing.days_90_plus, "11111");
    assert_eq!(result.report.ageing_bill_counts.days_0_30, 4);
    assert_eq!(result.report.ageing_bill_counts.days_31_60, 0);
    assert_eq!(result.report.ageing_bill_counts.days_61_90, 0);
    assert_eq!(result.report.ageing_bill_counts.days_90_plus, 1);
    assert_eq!(result.report.open_receivable_bill_count, 5);

    let beta = result
        .residuals
        .iter()
        .find(|residual| residual.party == "BVL Beta Supplies")
        .expect("a bill-wise-off debtor must remain visible as an unaged residual");
    assert_exact(&beta.amount, "-33333");
    let gamma = result
        .residuals
        .iter()
        .find(|residual| residual.party == "BVL Gamma Opening")
        .expect("the named-opening residual remains visible");
    assert_exact(&gamma.amount, "-44444");
    assert_eq!(
        result.residuals.len(),
        7,
        "only Sundry Debtor/Creditor ledgers belong in party residuals"
    );
    assert_exact(&result.residual_total, "77777");
    assert!(result.report.has_unaged_receivable);

    let accounted_debtor_exposure = result
        .report
        .receivable_total
        .checked_add(&result.residual_total)
        .expect("validation-book accounting remains exact");
    assert_exact(&accounted_debtor_exposure, "333330");
}

// ---------------------------------------------------------------------
// A: Bills Receivable on Billwise Lab — 48 rows, sum(BILLCL) = -4514597.00,
// all negative, 10 distinct parties.
// ---------------------------------------------------------------------
#[test]
fn bills_receivable_billwise_lab_matches_measured_totals() {
    let rows = parse_native_bill_rows(
        BILLS_RECEIVABLE_BILLWISE_LAB,
        &as_of(BILLWISE_LAB_BOOKS_FROM),
        &as_of(NATIVE_CAPTURE_AS_OF),
    )
    .expect("the real Billwise Lab capture parses");

    assert_eq!(rows.len(), 48);

    let mut sum = ExactDecimal::zero();
    for row in &rows {
        assert!(
            row.closing_balance.is_negative(),
            "every Billwise Lab receivable row is negative: {} was not",
            row.closing_balance.as_str()
        );
        sum = sum.checked_add(&row.closing_balance).unwrap();
    }
    assert_exact(&sum, "-4514597");

    let mut parties: Vec<&str> = rows.iter().map(|row| row.party.as_str()).collect();
    parties.sort_unstable();
    parties.dedup();
    assert_eq!(parties.len(), 10);
}

// ---------------------------------------------------------------------
// C: Ageing buckets on Billwise Lab at as-of 2026-07-31 = counts
// [4,4,4,36], amounts [395100.00, 890305.00, 445750.00, 2783442.00].
// ---------------------------------------------------------------------
#[test]
fn ageing_buckets_billwise_lab_match_measured_values_at_as_of() {
    let receivable_rows = parse_native_bill_rows(
        BILLS_RECEIVABLE_BILLWISE_LAB,
        &as_of(BILLWISE_LAB_BOOKS_FROM),
        &as_of(NATIVE_CAPTURE_AS_OF),
    )
    .unwrap();
    let payable_rows = parse_native_bill_rows(
        BILLS_PAYABLE_BILLWISE_LAB_EMPTY,
        &as_of(BILLWISE_LAB_BOOKS_FROM),
        &as_of(NATIVE_CAPTURE_AS_OF),
    )
    .unwrap();
    let ledgers = parse_native_ledger_snapshot(LEDGER_SNAPSHOT_BILLWISE_LAB).unwrap();

    let result = compute_native_outstandings(
        BILLWISE_LAB_COMPANY,
        &receivable_rows,
        &payable_rows,
        NativeMasterSnapshot {
            ledgers: &ledgers,
            groups: NativeGroupSnapshot::LegacyFixtureWithoutGroups,
        },
        AgeingAnchor::DueDate,
        &as_of("20260731"),
        BILLS_RECEIVABLE_BILLWISE_LAB.len(),
    )
    .expect("Billwise Lab computes");

    let report = &result.report;
    assert_eq!(report.open_receivable_bill_count, 48);
    assert_eq!(report.ageing_bill_counts.days_0_30, 4);
    assert_eq!(report.ageing_bill_counts.days_31_60, 4);
    assert_eq!(report.ageing_bill_counts.days_61_90, 4);
    assert_eq!(report.ageing_bill_counts.days_90_plus, 36);

    assert_exact(&report.ageing.days_0_30, "395100");
    assert_exact(&report.ageing.days_31_60, "890305");
    assert_exact(&report.ageing.days_61_90, "445750");
    assert_exact(&report.ageing.days_90_plus, "2783442");

    // In this book BILLDUE == BILLDATE for all 48 rows, so both anchors
    // agree; the DueDate default reproduces Tally's own BILLOVERDUE exactly.
    assert_eq!(result.overdue_crosscheck, NativeOverdueCrosscheck::Honored);
    assert_exact(&report.receivable_total, "4514597");
    assert_eq!(report.payable_total, ExactDecimal::zero());
    assert_eq!(report.source_voucher_count, 0);
    assert_eq!(report.source_bytes, BILLS_RECEIVABLE_BILLWISE_LAB.len());

    let bill_date_anchor_result = compute_native_outstandings(
        BILLWISE_LAB_COMPANY,
        &receivable_rows,
        &payable_rows,
        NativeMasterSnapshot {
            ledgers: &ledgers,
            groups: NativeGroupSnapshot::LegacyFixtureWithoutGroups,
        },
        AgeingAnchor::BillDate,
        &as_of("20260731"),
        BILLS_RECEIVABLE_BILLWISE_LAB.len(),
    )
    .expect("Billwise Lab computes under the BillDate anchor too");
    // BILLDUE == BILLDATE everywhere in this book, so the two anchors must
    // agree exactly here (the divergence is exercised on the Ageing Lab
    // fixture below, which has a genuine credit-period bill).
    assert_eq!(bill_date_anchor_result.report.ageing, report.ageing);
    assert_eq!(
        bill_date_anchor_result.report.ageing_bill_counts,
        report.ageing_bill_counts
    );
}

// ---------------------------------------------------------------------
// D: On-Account identity, per party:
// ledger CLOSINGBALANCE - sum(BILLCL receivable) - sum(BILLCL payable).
// 6 of 10 parties give exactly 0.00; 4 give non-zero (Lotus Home Stores
// 37500.00, Metro Trade Link 15000.00, Sharma Traders 22500.00, Sunrise
// Electronics 30000.00) totalling exactly 105000.00.
// ---------------------------------------------------------------------
#[test]
fn on_account_residuals_billwise_lab_match_measured_totals() {
    let receivable_rows = parse_native_bill_rows(
        BILLS_RECEIVABLE_BILLWISE_LAB,
        &as_of(BILLWISE_LAB_BOOKS_FROM),
        &as_of(NATIVE_CAPTURE_AS_OF),
    )
    .unwrap();
    let payable_rows = parse_native_bill_rows(
        BILLS_PAYABLE_BILLWISE_LAB_EMPTY,
        &as_of(BILLWISE_LAB_BOOKS_FROM),
        &as_of(NATIVE_CAPTURE_AS_OF),
    )
    .unwrap();
    let ledgers = parse_native_ledger_snapshot(LEDGER_SNAPSHOT_BILLWISE_LAB).unwrap();

    let result = compute_native_outstandings(
        BILLWISE_LAB_COMPANY,
        &receivable_rows,
        &payable_rows,
        NativeMasterSnapshot {
            ledgers: &ledgers,
            groups: NativeGroupSnapshot::LegacyFixtureWithoutGroups,
        },
        AgeingAnchor::DueDate,
        &as_of("20260731"),
        BILLS_RECEIVABLE_BILLWISE_LAB.len(),
    )
    .unwrap();

    // Only the 10 bill-wise (Sundry Debtor) ledgers carry a residual; Cash,
    // Profit & Loss A/c, and Sales are present in the snapshot but are not
    // bill-wise and must not appear here.
    assert_eq!(result.residuals.len(), 10);

    let zero_count = result
        .residuals
        .iter()
        .filter(|residual| residual.amount.is_zero())
        .count();
    assert_eq!(zero_count, 6);

    let expect_residual = |party: &str, canonical_amount: &str| {
        let found = result
            .residuals
            .iter()
            .find(|residual| residual.party == party)
            .unwrap_or_else(|| panic!("residual for {party} present"));
        assert_exact(&found.amount, canonical_amount);
    };
    expect_residual("Lotus Home Stores", "37500");
    expect_residual("Metro Trade Link", "15000");
    expect_residual("Sharma Traders", "22500");
    expect_residual("Sunrise Electronics", "30000");

    assert_exact(&result.residual_total, "105000");
    assert!(
        !result.report.has_unaged_receivable,
        "the measured Rs 1,05,000 residual is payable exposure, not receivable"
    );
}

// ---------------------------------------------------------------------
// B: Ageing anchor — Tally ages by DUE DATE. On the Ageing Lab fixture, the
// DueDate anchor reproduces Tally's own BILLOVERDUE for all 5 bills; the
// BillDate anchor does NOT for CREDIT-30 (BILLDATE=1-May-26,
// BILLDUE=31-May-26, BILLOVERDUE=61 == age-from-DUE, not age-from-BILLDATE
// which is 91).
// ---------------------------------------------------------------------
#[test]
fn ageing_lab_due_date_anchor_reproduces_tally_overdue_bill_date_does_not() {
    let rows = parse_native_bill_rows(
        BILLS_RECEIVABLE_AGEING_LAB,
        &as_of(AGEING_LAB_BOOKS_FROM),
        &as_of(NATIVE_CAPTURE_AS_OF),
    )
    .expect("the real Ageing Lab capture parses");
    assert_eq!(rows.len(), 5);

    // SVTODATE that produced these BILLOVERDUE values: 1-Apr-24 + 851 days
    // == 31-Jul-26 in the sibling Billwise Lab capture, and independently
    // 1-May-26 + 91 days == 31-Jul-26 here (CANARY-1). Both captures were
    // taken from the same live run.
    let as_of_date = as_of("20260731");

    let mut credit_30_checked = false;
    for row in &rows {
        let tally_overdue = row
            .tally_overdue_days
            .expect("every Ageing Lab row carries BILLOVERDUE");

        let age_from_due = age_in_days(&row.due_date, &as_of_date).unwrap();
        assert_eq!(
            i64::from(age_from_due),
            tally_overdue,
            "DueDate anchor must reproduce Tally's own BILLOVERDUE for {}",
            row.reference
        );

        let age_from_bill_date = age_in_days(&row.bill_date, &as_of_date).unwrap();
        if row.reference == "CREDIT-30" {
            assert_eq!(row.bill_date.as_str(), "20260501");
            assert_eq!(row.due_date.as_str(), "20260531");
            assert_eq!(tally_overdue, 61);
            assert_eq!(age_from_due, 61);
            assert_eq!(
                age_from_bill_date, 91,
                "CREDIT-30's BillDate anchor must NOT reproduce BILLOVERDUE"
            );
            assert_ne!(i64::from(age_from_bill_date), tally_overdue);
            credit_30_checked = true;
        } else {
            // Every other Ageing Lab bill has BILLDATE == BILLDUE, so both
            // anchors necessarily agree with BILLOVERDUE there too.
            assert_eq!(i64::from(age_from_bill_date), tally_overdue);
        }
    }
    assert!(
        credit_30_checked,
        "CREDIT-30 must be present in the fixture"
    );
}

// ---------------------------------------------------------------------
// Grammar rule 2 / empty-response handling: a bare `<ENVELOPE></ENVELOPE>`
// (23 bytes) is legitimate zero-row success, not an error.
// ---------------------------------------------------------------------
#[test]
fn empty_bills_response_is_legitimate_zero_row_success() {
    assert_eq!(BILLS_PAYABLE_BILLWISE_LAB_EMPTY.len(), 23);
    let rows = parse_native_bill_rows(
        BILLS_PAYABLE_BILLWISE_LAB_EMPTY,
        &as_of(BILLWISE_LAB_BOOKS_FROM),
        &as_of(NATIVE_CAPTURE_AS_OF),
    )
    .expect("a bare ENVELOPE is success, not an error");
    assert!(rows.is_empty());
}

// ---------------------------------------------------------------------
// CMPINFO counter trap: `ledger_snapshot_billwise_lab.xml`'s DESC/CMPINFO
// block carries a bare `<LEDGER>0</LEDGER>` counter, sharing its tag name
// with real rows. Scanning must be scoped to DATA/COLLECTION only.
//
// Ground-truth correction: the response actually carries 13 ledger master
// rows in DATA/COLLECTION (the 10 Sundry Debtors plus Cash, Profit & Loss
// A/c, and Sales — all present in the same "List of Ledgers" export), not
// 10. A parser that also counts the CMPINFO counter would return 14. The
// meaningful assertion is 13 (not 14), with exactly 10 of those 13 flagged
// bill-wise — matching the "10 Sundry ledgers" / "10 distinct parties" figure
// elsewhere in this suite.
// ---------------------------------------------------------------------
#[test]
fn ledger_snapshot_ignores_the_cmpinfo_counter_trap() {
    let ledgers = parse_native_ledger_snapshot(LEDGER_SNAPSHOT_BILLWISE_LAB)
        .expect("the real ledger snapshot capture parses");

    assert_eq!(
        ledgers.len(),
        13,
        "must scan only DATA/COLLECTION rows (13), neither missing rows nor \
         double-counting the DESC/CMPINFO <LEDGER>0</LEDGER> counter (14)"
    );

    let bill_wise_count = ledgers.iter().filter(|ledger| ledger.bill_wise_on).count();
    assert_eq!(bill_wise_count, 10);

    let names: Vec<&str> = ledgers.iter().map(|ledger| ledger.name.as_str()).collect();
    for expected in ["Cash", "Sales", "Profit & Loss A/c"] {
        assert!(
            names.contains(&expected),
            "{expected} is a real non-bill-wise row in this export and must still be returned"
        );
    }
}

// ---------------------------------------------------------------------
// Grammar rule 2, inverted verification: a `<STATUS>` element anywhere in
// this report shape means Tally reported failure. Success never carries one.
// ---------------------------------------------------------------------
#[test]
fn status_bearing_bills_response_fails_closed() {
    let xml = "<ENVELOPE><STATUS>0</STATUS></ENVELOPE>";
    let result = parse_native_bill_rows(
        xml,
        &as_of(BILLWISE_LAB_BOOKS_FROM),
        &as_of(NATIVE_CAPTURE_AS_OF),
    );
    assert_eq!(result, Err(NativeOutstandingsError::TallyReportedFailure));
}

// ---------------------------------------------------------------------
// Grammar rule 1: a scalar appearing before any BILLFIXED must fail closed.
// ---------------------------------------------------------------------
#[test]
fn billcl_before_any_billfixed_fails_closed() {
    let xml = "<ENVELOPE><BILLCL>-100.00</BILLCL></ENVELOPE>";
    let result = parse_native_bill_rows(
        xml,
        &as_of(BILLWISE_LAB_BOOKS_FROM),
        &as_of(NATIVE_CAPTURE_AS_OF),
    );
    assert_eq!(
        result,
        Err(NativeOutstandingsError::InvalidResponse(
            "bills_scalar_before_fixed"
        ))
    );
}

// ---------------------------------------------------------------------
// Grammar rule 1: a BILLFIXED lacking a BILLCL must fail closed, even when a
// later, fully-formed row follows it.
// ---------------------------------------------------------------------
#[test]
fn billfixed_missing_billcl_fails_closed() {
    let xml = "<ENVELOPE>\
        <BILLFIXED><BILLDATE>1-Apr-24</BILLDATE><BILLREF>INV-1</BILLREF><BILLPARTY>P</BILLPARTY></BILLFIXED>\
        <BILLFIXED><BILLDATE>1-Apr-24</BILLDATE><BILLREF>INV-2</BILLREF><BILLPARTY>P</BILLPARTY></BILLFIXED>\
        <BILLCL>-10.00</BILLCL><BILLDUE>1-Apr-24</BILLDUE><BILLOVERDUE>1</BILLOVERDUE>\
        </ENVELOPE>";
    let result = parse_native_bill_rows(
        xml,
        &as_of(BILLWISE_LAB_BOOKS_FROM),
        &as_of(NATIVE_CAPTURE_AS_OF),
    );
    assert_eq!(
        result,
        Err(NativeOutstandingsError::InvalidResponse(
            "bills_fixed_row_missing_billcl"
        ))
    );
}

#[test]
fn empty_bill_party_from_raw_bytes_fails_closed_before_double_counting() {
    let bills = "<ENVELOPE>\
        <BILLFIXED><BILLDATE>1-Jul-26</BILLDATE><BILLREF>SYNTHETIC-INV-1</BILLREF><BILLPARTY></BILLPARTY></BILLFIXED>\
        <BILLCL>-100.00</BILLCL><BILLDUE>1-Jul-26</BILLDUE><BILLOVERDUE>30</BILLOVERDUE>\
        </ENVELOPE>";
    let ledger_bytes = "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>\
        <LEDGER NAME=\"Synthetic Customer\"><PARENT>Sundry Debtors</PARENT>\
        <CLOSINGBALANCE>-100.00</CLOSINGBALANCE><OPENINGBALANCE>0.00</OPENINGBALANCE>\
        <ISBILLWISEON>Yes</ISBILLWISEON></LEDGER>\
        </COLLECTION></DATA></BODY></ENVELOPE>";

    let result = parse_native_bill_rows(
        bills,
        &as_of(AGEING_LAB_BOOKS_FROM),
        &as_of(NATIVE_CAPTURE_AS_OF),
    )
    .and_then(|receivable| {
        let ledgers = parse_native_ledger_snapshot(ledger_bytes)?;
        compute_native_outstandings(
            "Synthetic Company",
            &receivable,
            &[],
            NativeMasterSnapshot {
                ledgers: &ledgers,
                groups: NativeGroupSnapshot::LegacyFixtureWithoutGroups,
            },
            AgeingAnchor::DueDate,
            &as_of(NATIVE_CAPTURE_AS_OF),
            bills.len() + ledger_bytes.len(),
        )
    });

    match result {
        Err(error) => assert_eq!(
            error,
            NativeOutstandingsError::InvalidResponse("bills_fixed_empty_billparty")
        ),
        Ok(result) => {
            assert_exact(&result.report.receivable_total, "100");
            assert_exact(&result.residual_total, "100");
            let disclosed_total = result
                .report
                .receivable_total
                .checked_add(&result.residual_total)
                .expect("the pinned synthetic total is in range");
            assert_exact(&disclosed_total, "200");
            panic!(
                "the raw-byte read completed with a double-counted total of {}",
                disclosed_total.as_str()
            );
        }
    }
}

#[test]
fn whitespace_and_self_closing_bill_party_use_the_distinct_empty_party_error() {
    for party in ["<BILLPARTY> \n\t </BILLPARTY>", "<BILLPARTY/>"] {
        let xml = format!(
            "<ENVELOPE><BILLFIXED><BILLDATE>1-Jul-26</BILLDATE>\
             <BILLREF>SYNTHETIC-INV-1</BILLREF>{party}</BILLFIXED>\
             <BILLCL>-100.00</BILLCL><BILLDUE>1-Jul-26</BILLDUE>\
             <BILLOVERDUE>30</BILLOVERDUE></ENVELOPE>"
        );
        assert_eq!(
            parse_native_bill_rows(
                &xml,
                &as_of(AGEING_LAB_BOOKS_FROM),
                &as_of(NATIVE_CAPTURE_AS_OF),
            ),
            Err(NativeOutstandingsError::InvalidResponse(
                "bills_fixed_empty_billparty"
            ))
        );
    }
}

#[test]
fn self_closing_empty_billoverdue_is_none_and_still_rejects_duplicates() {
    let one_empty = "<ENVELOPE>\
        <BILLFIXED><BILLDATE>1-Aug-26</BILLDATE><BILLREF>FUTURE</BILLREF><BILLPARTY>P</BILLPARTY></BILLFIXED>\
        <BILLCL>-10.00</BILLCL><BILLDUE>1-Oct-26</BILLDUE><BILLOVERDUE/>\
        </ENVELOPE>";
    let rows = parse_native_bill_rows(
        one_empty,
        &as_of(VALIDATION_LAB_BOOKS_FROM),
        &as_of(VALIDATION_CAPTURE_AS_OF),
    )
    .expect("a self-closing empty value has the same field-specific meaning");
    assert_eq!(rows[0].tally_overdue_days, None);

    let duplicate = one_empty.replace(
        "<BILLOVERDUE/>",
        "<BILLOVERDUE/><BILLOVERDUE>0</BILLOVERDUE>",
    );
    assert_eq!(
        parse_native_bill_rows(
            &duplicate,
            &as_of(VALIDATION_LAB_BOOKS_FROM),
            &as_of(VALIDATION_CAPTURE_AS_OF),
        ),
        Err(NativeOutstandingsError::InvalidResponse(
            "bills_duplicate_billoverdue"
        ))
    );
}

#[test]
fn bills_sanitize_illegal_numeric_references_before_decoding_text_fields() {
    let xml = "<ENVELOPE>\
        <BILLFIXED><BILLDATE>1-Apr-24</BILLDATE><BILLREF>&#4; REFERENCE</BILLREF><BILLPARTY>&#4; PARTY</BILLPARTY></BILLFIXED>\
        <BILLCL>-10.00</BILLCL><BILLDUE>1-Apr-24</BILLDUE><BILLOVERDUE>1</BILLOVERDUE>\
        </ENVELOPE>";
    let rows = parse_native_bill_rows(
        xml,
        &as_of(BILLWISE_LAB_BOOKS_FROM),
        &as_of(NATIVE_CAPTURE_AS_OF),
    )
    .expect("Tally's illegal text references are made XML-1.0-safe at the boundary");

    assert_eq!(rows[0].party, "\u{fffd}#4; PARTY");
    assert_eq!(rows[0].reference, "\u{fffd}#4; REFERENCE");
}

#[test]
fn ledger_snapshot_sanitizes_illegal_numeric_references_before_decoding_text_fields() {
    let xml = "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>\
        <LEDGER NAME=\"&#4; LEDGER\"><PARENT>&#4; PARENT</PARENT><CLOSINGBALANCE>-10.00</CLOSINGBALANCE>\
        <OPENINGBALANCE>0.00</OPENINGBALANCE><ISBILLWISEON>Yes</ISBILLWISEON></LEDGER>\
        </COLLECTION></DATA></BODY></ENVELOPE>";
    let rows = parse_native_ledger_snapshot(xml)
        .expect("Tally's illegal text references are made XML-1.0-safe at the boundary");

    assert_eq!(rows[0].name, "\u{fffd}#4; LEDGER");
    assert_eq!(rows[0].parent.as_deref(), Some("\u{fffd}#4; PARENT"));
}

#[test]
fn ledger_snapshot_requires_the_collection_even_when_status_is_success() {
    assert_eq!(
        parse_native_ledger_snapshot(
            "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA/></BODY></ENVELOPE>"
        ),
        Err(NativeOutstandingsError::InvalidResponse(
            "ledger_collection_missing"
        ))
    );
}

#[test]
fn illegal_numeric_references_in_amounts_remain_fail_closed() {
    let bills = "<ENVELOPE>\
        <BILLFIXED><BILLDATE>1-Apr-24</BILLDATE><BILLREF>REFERENCE</BILLREF><BILLPARTY>PARTY</BILLPARTY></BILLFIXED>\
        <BILLCL>&#4; -10.00</BILLCL><BILLDUE>1-Apr-24</BILLDUE><BILLOVERDUE>1</BILLOVERDUE>\
        </ENVELOPE>";
    assert_eq!(
        parse_native_bill_rows(
            bills,
            &as_of(BILLWISE_LAB_BOOKS_FROM),
            &as_of(NATIVE_CAPTURE_AS_OF),
        ),
        Err(NativeOutstandingsError::InvalidAmount)
    );

    let ledgers = "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>\
        <LEDGER NAME=\"LEDGER\"><PARENT>PARENT</PARENT><CLOSINGBALANCE>&#4; -10.00</CLOSINGBALANCE>\
        <OPENINGBALANCE>0.00</OPENINGBALANCE><ISBILLWISEON>Yes</ISBILLWISEON></LEDGER>\
        </COLLECTION></DATA></BODY></ENVELOPE>";
    assert_eq!(
        parse_native_ledger_snapshot(ledgers),
        Err(NativeOutstandingsError::InvalidAmount)
    );
}
