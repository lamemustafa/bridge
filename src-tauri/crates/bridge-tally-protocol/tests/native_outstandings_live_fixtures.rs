// SPDX-License-Identifier: Apache-2.0

//! Acceptance tests for the native outstandings path, driven by responses
//! captured live from TallyPrime on 2026-08-07.
//!
//! Every expected number here is ground truth from an independent source, not
//! from this implementation:
//!
//! - The Billwise Lab figures (48 open bills, Rs 45,14,597, ageing 4/4/4/36)
//!   are the Unit A exit-criterion constants, which were themselves agreed by
//!   Tally's own Bills Receivable report *and* a raw-XML computation before
//!   any of this code existed.
//! - The Rs 1,05,000 residual equals Unit A's independently derived payable
//!   total for the same book.
//! - The Ageing Lab expectations come from Tally's own `BILLOVERDUE` column.
//!
//! That matters because `TALLY_PROTOCOL_REFERENCE.md` defines VERIFIED
//! evidence as a live observation with a captured request and response: a test
//! whose expectations are copied from the implementation it checks will
//! survive the implementation being wrong.

use bridge_tally_primitives::{ExactDecimal, TallyDate};
use bridge_tally_protocol::native_outstandings::{
    age_in_days, compute_native_outstandings, parse_native_bill_rows, parse_native_group_snapshot,
    parse_native_ledger_snapshot, AgeingAnchor, NativeGroupSnapshot, NativeMasterSnapshot,
};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/native");

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!("{FIXTURES}/{name}"))
        .unwrap_or_else(|error| panic!("fixture {name} unreadable: {error}"))
}

/// The `Aarav Trading Company Demo` company GUID, as captured live in
/// `group_snapshot_aarav_with_computed_company_guid.xml`.
const AARAV_COMPANY_GUID: &str = "bb8ad19e-6aef-4239-a917-87fec0c6215e";

/// Verbatim UTF-8 bytes from a read-only `List of Groups` response with the
/// request-computed `BRIDGECOMPANYGUID` field. The parser must handle Tally's
/// observed invalid control characters without manufacturing row identity.
const GROUP_SNAPSHOT_AARAV_WITH_COMPUTED_COMPANY_GUID: &str =
    include_str!("fixtures/native/group_snapshot_aarav_with_computed_company_guid.xml");

fn as_of() -> TallyDate {
    TallyDate::parse("20260731").expect("valid as-of")
}

fn books_from(yyyymmdd: &str) -> TallyDate {
    TallyDate::parse(yyyymmdd).expect("valid books-from date")
}

/// The whole thesis in one assertion: one native request reproduces the
/// 54-request, 8.30 s voucher scan exactly.
#[test]
fn billwise_lab_reproduces_the_unit_a_exit_criteria_exactly() {
    let receivable = parse_native_bill_rows(
        &fixture("bills_receivable_billwise_lab.xml"),
        &books_from("20240401"),
        &as_of(),
    )
    .expect("receivable rows parse");
    let payable = parse_native_bill_rows(
        &fixture("bills_payable_billwise_lab_empty.xml"),
        &books_from("20240401"),
        &as_of(),
    )
    .expect("empty payable parses as success, not failure");
    let ledgers = parse_native_ledger_snapshot(&fixture("ledger_snapshot_billwise_lab.xml"))
        .expect("ledger snapshot parses");

    assert_eq!(receivable.len(), 48, "Unit A ground truth: 48 open bills");
    assert!(payable.is_empty(), "this book has no credit-balance bills");

    let result = compute_native_outstandings(
        "Bridge Billwise Lab",
        &receivable,
        &payable,
        NativeMasterSnapshot {
            ledgers: &ledgers,
            groups: NativeGroupSnapshot::LegacyFixtureWithoutGroups,
        },
        AgeingAnchor::DueDate,
        &as_of(),
        11_030,
    )
    .expect("native computation succeeds");

    let report = &result.report;
    assert_eq!(report.receivable_total.as_str(), "4514597");
    assert_eq!(report.open_receivable_bill_count, 48);
    assert_eq!(report.ageing_bill_counts.days_0_30, 4);
    assert_eq!(report.ageing_bill_counts.days_31_60, 4);
    assert_eq!(report.ageing_bill_counts.days_61_90, 4);
    assert_eq!(report.ageing_bill_counts.days_90_plus, 36);

    // The buckets must sum to the receivable total. A wrong as-of moves only
    // the buckets and leaves the total right, so this is the assertion that
    // catches it.
    let summed = [
        &report.ageing.days_0_30,
        &report.ageing.days_31_60,
        &report.ageing.days_61_90,
        &report.ageing.days_90_plus,
    ]
    .into_iter()
    .try_fold(ExactDecimal::zero(), |total, bucket| {
        total.checked_add(bucket)
    })
    .expect("bucket sum is exact");
    assert_eq!(summed.as_str(), report.receivable_total.as_str());

    // This path reads no vouchers. Claiming otherwise on screen would be a
    // false provenance claim.
    assert_eq!(report.source_voucher_count, 0);
}

/// The named bills are not the whole exposure. The ledger read recovers the
/// rest, and it must equal Unit A's separately derived payable figure.
#[test]
fn billwise_lab_residual_equals_unit_a_payable_total_to_the_rupee() {
    let receivable = parse_native_bill_rows(
        &fixture("bills_receivable_billwise_lab.xml"),
        &books_from("20240401"),
        &as_of(),
    )
    .expect("receivable rows parse");
    let payable = parse_native_bill_rows(
        &fixture("bills_payable_billwise_lab_empty.xml"),
        &books_from("20240401"),
        &as_of(),
    )
    .expect("payable parses");
    let ledgers = parse_native_ledger_snapshot(&fixture("ledger_snapshot_billwise_lab.xml"))
        .expect("ledger snapshot parses");

    let result = compute_native_outstandings(
        "Bridge Billwise Lab",
        &receivable,
        &payable,
        NativeMasterSnapshot {
            ledgers: &ledgers,
            groups: NativeGroupSnapshot::LegacyFixtureWithoutGroups,
        },
        AgeingAnchor::DueDate,
        &as_of(),
        11_030,
    )
    .expect("native computation succeeds");

    assert_eq!(
        result.residual_total.as_str(),
        "105000",
        "Unit A independently derived Rs 1,05,000 payable for this book"
    );

    let non_zero = result
        .residuals
        .iter()
        .filter(|residual| !residual.amount.is_zero())
        .count();
    assert_eq!(non_zero, 4, "Lotus, Metro, Sharma and Sunrise carry it");
}

/// The opposite composition. Here the named bills are a rounding error and the
/// residual is the answer -- a report that omitted it would be short by 96%.
#[test]
fn aarav_residual_dominates_and_every_bill_carrying_party_reconciles_exactly() {
    let receivable = parse_native_bill_rows(
        &fixture("bills_receivable_aarav.xml"),
        &books_from("20240401"),
        &as_of(),
    )
    .expect("parse");
    let payable = parse_native_bill_rows(
        &fixture("bills_payable_aarav.xml"),
        &books_from("20240401"),
        &as_of(),
    )
    .expect("parse");
    let ledgers =
        parse_native_ledger_snapshot(&fixture("ledger_snapshot_aarav.xml")).expect("parse");
    let groups = parse_native_group_snapshot(
        GROUP_SNAPSHOT_AARAV_WITH_COMPUTED_COMPANY_GUID,
        AARAV_COMPANY_GUID,
    )
    .expect("parse");

    assert_eq!(receivable.len(), 22);
    assert_eq!(payable.len(), 21);
    assert_eq!(groups.len(), 28, "captured native group collection rows");

    let result = compute_native_outstandings(
        "Aarav Trading Company Demo",
        &receivable,
        &payable,
        NativeMasterSnapshot {
            ledgers: &ledgers,
            groups: NativeGroupSnapshot::Complete(&groups),
        },
        AgeingAnchor::DueDate,
        &as_of(),
        51_003,
    )
    .expect("native computation succeeds");
    let legacy_result = compute_native_outstandings(
        "Aarav Trading Company Demo",
        &receivable,
        &payable,
        NativeMasterSnapshot {
            ledgers: &ledgers,
            groups: NativeGroupSnapshot::LegacyFixtureWithoutGroups,
        },
        AgeingAnchor::DueDate,
        &as_of(),
        51_003,
    )
    .expect("legacy fixture computation succeeds");

    // Named bills total ~Rs 10.36 lakh across 43 rows.
    let named = result
        .report
        .receivable_total
        .checked_add(&result.report.payable_total)
        .expect("exact");
    assert_eq!(named.as_str(), "1035702.2");

    // The invariant that actually matters: every party that carries bills
    // reconciles to the paisa. A non-zero residual on such a party would mean
    // the bill report and the ledger disagree, and no total could be trusted.
    let bill_carrying_but_unreconciled = result
        .residuals
        .iter()
        .filter(|residual| !residual.amount.is_zero())
        .filter(|residual| {
            receivable
                .iter()
                .chain(payable.iter())
                .any(|row| row.party == residual.party)
        })
        .count();
    assert_eq!(
        bill_carrying_but_unreconciled, 0,
        "all 7 bill-carrying parties must reconcile exactly"
    );

    // Residual is summed as gross magnitude, not net: a creditor's unallocated
    // credit must not cancel a debtor's unallocated debit, because they are
    // different counterparties. Net would be Rs 2,78,57,843.69; gross is
    // Rs 20,74,00,748.79. Either way the named bills are a rounding error
    // beside it, which is the point.
    assert_eq!(legacy_result.residual_total.as_str(), "207400748.79");
    assert_eq!(result.residual_total.as_str(), "207400748.79");
    assert_eq!(legacy_result.residuals.len(), 60);
    assert_eq!(
        result.residuals.len(),
        60,
        "the complete group snapshot must never reduce the classified party set"
    );
    assert!(
        result.report.has_unaged_receivable,
        "unallocated debtor exposure must be disclosed, never silently dropped"
    );
}

/// Settles the ageing-anchor question. Tally's own BILLOVERDUE column is the
/// oracle, and it disagrees with bill-date anchoring exactly where a credit
/// period exists.
#[test]
fn due_date_anchor_matches_tallys_own_overdue_column_where_bill_date_does_not() {
    let rows = parse_native_bill_rows(
        &fixture("bills_receivable_ageing_lab.xml"),
        &books_from("20260401"),
        &as_of(),
    )
    .expect("parse");
    assert_eq!(rows.len(), 5);
    let as_of = as_of();

    let mut credit_period_bills = 0;
    for row in &rows {
        let tally = row
            .tally_overdue_days
            .expect("every Ageing Lab row carries BILLOVERDUE");
        let from_due = age_in_days(&row.due_date, &as_of).expect("age computes");
        assert_eq!(
            i64::from(from_due),
            tally,
            "due-date anchor must reproduce Tally's own column for {}",
            row.reference
        );

        if row.bill_date != row.due_date {
            credit_period_bills += 1;
            let from_bill_date = age_in_days(&row.bill_date, &as_of).expect("age computes");
            assert_ne!(
                i64::from(from_bill_date),
                tally,
                "{} carries a credit period, so bill-date anchoring MUST diverge \
                 -- if this ever passes, the fixture stopped proving its premise",
                row.reference
            );
        }
    }
    assert_eq!(
        credit_period_bills, 1,
        "CREDIT-30 is the only bill able to distinguish the two anchors; \
         without it this whole test proves nothing"
    );
}

/// An unloaded company must refuse rather than silently answer for whichever
/// company happens to be open. This is the failure the Collection path does
/// NOT protect against.
#[test]
fn unloaded_company_response_fails_closed() {
    let result = parse_native_bill_rows(
        &fixture("bills_receivable_unloaded_company_failure.xml"),
        &books_from("20240401"),
        &as_of(),
    );
    assert!(
        result.is_err(),
        "a STATUS-bearing response is a failure on this path, never an empty result"
    );
}

/// The ledger collection ships a CMPINFO block full of bare counter elements
/// like `<LEDGER>0</LEDGER>`. Parsing outside `<DATA>` picks them up as rows.
#[test]
fn ledger_parser_ignores_the_cmpinfo_counter_elements() {
    let ledgers =
        parse_native_ledger_snapshot(&fixture("ledger_snapshot_billwise_lab.xml")).expect("parse");
    assert_eq!(
        ledgers.len(),
        13,
        "13 real ledgers; a parser that scans the whole body also counts CMPINFO"
    );
    assert!(
        ledgers.iter().all(|ledger| !ledger.name.is_empty()),
        "a counter element would parse as a nameless ledger"
    );
    let sundry = ledgers
        .iter()
        .filter(|ledger| {
            ledger
                .parent
                .as_deref()
                .is_some_and(|parent| parent.contains("Sundry"))
        })
        .count();
    assert_eq!(sundry, 10);
}
/// A real group snapshot captured from one company must not be accepted as
/// evidence for a different one. This is the exact substitution Tally is
/// known to make silently: this response is genuinely from
/// `Aarav Trading Company Demo`, and asking for a different company's GUID
/// must fail closed rather than quietly classify ledgers under the wrong
/// company's group ancestry.
#[test]
fn a_real_group_snapshot_does_not_bind_to_a_different_companys_guid() {
    const WR2_COMPANY_GUID: &str = "61c6de69-1748-461c-ad3f-162cb949df9f";
    let result = parse_native_group_snapshot(
        GROUP_SNAPSHOT_AARAV_WITH_COMPUTED_COMPANY_GUID,
        WR2_COMPANY_GUID,
    );
    assert!(
        result.is_err(),
        "a captured Aarav response must not bind to the wr2 company's GUID"
    );
}

#[test]
fn real_captured_company_collection_yields_all_three() {
    let xml = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/native/company_collection_live.xml"
    ))
    .expect("fixture");
    let companies = bridge_tally_protocol::parse_companies_from_collection(&xml)
        .expect("the exact live bytes must parse");
    assert_eq!(companies.len(), 3, "got: {companies:?}");
    assert!(companies.iter().all(|c| c.guid.is_some()));
}
