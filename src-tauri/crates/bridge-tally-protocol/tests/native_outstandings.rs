//! Integration tests for `native_outstandings`, driven entirely by the real
//! fixtures captured live from TallyPrime
//! (`tests/fixtures/native/*.xml`, captured 2026-08-07). No network, no live
//! Tally: every assertion here is against bytes already checked into the
//! repository.

use bridge_tally_primitives::{ExactDecimal, TallyDate};
use bridge_tally_protocol::native_outstandings::{
    age_in_days, compute_native_outstandings, parse_native_bill_rows, parse_native_ledger_snapshot,
    AgeingAnchor, NativeBillRow, NativeOutstandingsError,
};

const BILLS_RECEIVABLE_BILLWISE_LAB: &str =
    include_str!("fixtures/native/bills_receivable_billwise_lab.xml");
const BILLS_PAYABLE_BILLWISE_LAB_EMPTY: &str =
    include_str!("fixtures/native/bills_payable_billwise_lab_empty.xml");
const LEDGER_SNAPSHOT_BILLWISE_LAB: &str =
    include_str!("fixtures/native/ledger_snapshot_billwise_lab.xml");
const BILLS_RECEIVABLE_AGEING_LAB: &str =
    include_str!("fixtures/native/bills_receivable_ageing_lab.xml");

/// `BOOKSFROM` for "Bridge Billwise Lab", from `company_extent_9000.xml`
/// (`<BOOKSFROM TYPE="Date">20240401</BOOKSFROM>`).
const BILLWISE_LAB_BOOKS_FROM: &str = "20240401";
/// `BOOKSFROM` for "Bridge Ageing Lab", from `company_extent_9000.xml`
/// (`<BOOKSFROM TYPE="Date">20260401</BOOKSFROM>`).
const AGEING_LAB_BOOKS_FROM: &str = "20260401";
const NATIVE_CAPTURE_AS_OF: &str = "20260731";

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

#[test]
fn not_yet_due_bill_is_reported_without_becoming_overdue() {
    let receivable = [NativeBillRow {
        party: "Synthetic customer".to_string(),
        reference: "SYNTHETIC-FUTURE-DUE".to_string(),
        bill_date: as_of("20260701"),
        due_date: as_of("20260830"),
        closing_balance: ExactDecimal::parse("-100.00").unwrap(),
        // Tally reports zero overdue days until the due date arrives.
        tally_overdue_days: Some(0),
    }];

    let result = compute_native_outstandings(
        "Synthetic Company",
        &receivable,
        &[],
        &[],
        AgeingAnchor::DueDate,
        &as_of("20260731"),
        0,
    )
    .expect("a future-due bill must not abort the report");

    assert_exact(&result.report.receivable_total, "100");
    assert_eq!(result.report.ageing.days_0_30, ExactDecimal::zero());
    assert_eq!(result.report.ageing.days_90_plus, ExactDecimal::zero());
    assert_eq!(result.report.open_receivable_bill_count, 0);
    assert_eq!(result.report.ageing_bill_counts.days_0_30, 0);
    assert_eq!(result.report.ageing_bill_counts.days_90_plus, 0);
    assert_eq!(
        result.report.top_parties[0].oldest_bill_age_days, None,
        "a future-due bill must not be presented as the oldest overdue bill"
    );
    assert_eq!(
        result.overdue_crosscheck_mismatches, 0,
        "zero overdue days must agree with Tally's own BILLOVERDUE"
    );
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
        &ledgers,
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
    assert_eq!(result.overdue_crosscheck_mismatches, 0);
    assert_exact(&report.receivable_total, "4514597");
    assert_eq!(report.payable_total, ExactDecimal::zero());
    assert_eq!(report.source_voucher_count, 0);
    assert_eq!(report.source_bytes, BILLS_RECEIVABLE_BILLWISE_LAB.len());

    let bill_date_anchor_result = compute_native_outstandings(
        BILLWISE_LAB_COMPANY,
        &receivable_rows,
        &payable_rows,
        &ledgers,
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
        &ledgers,
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
    assert!(result.report.has_unaged_receivable);
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
// (22 bytes) is legitimate zero-row success, not an error.
// ---------------------------------------------------------------------
#[test]
fn empty_bills_response_is_legitimate_zero_row_success() {
    assert_eq!(BILLS_PAYABLE_BILLWISE_LAB_EMPTY.len(), 22);
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
