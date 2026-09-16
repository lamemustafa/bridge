use bridge_tally_primitives::{ExactDecimal, TallyDate};
use sha2::{Digest, Sha256};

use crate::{
    decode_tally_xml_response_bytes_limited,
    outstandings::{
        AlterIdRange, BillAllocation, BillReferenceKind, CompleteScan, CreditPeriod,
        DateBoundaryProfile, DateWindow, LedgerEntry, MoneyValue, PinnedCompany, Voucher,
        VoucherAlterId, VoucherAlterIdHighWater,
    },
    xml_read_profiles::ValidatedCompanyName,
    ExpectedTallyTextEncoding,
};

use super::super::parser::parse_segment;
use super::{add_credit_period, compute_outstandings, compute_outstandings_with_ageing_anchor};

const AGEING_CORPUS: &[u8] =
    include_bytes!("../../tests/fixtures/vouchers_ageing_corpus_live.utf16le.xml");
const GST_CREDIT_PERIOD_CORPUS: &[u8] =
    include_bytes!("../../tests/fixtures/vouchers_gst_credit_periods_live.utf16le.xml");
const SETTLE_THEN_REOPEN_CORPUS: &[u8] =
    include_bytes!("../../tests/fixtures/vouchers_settle_then_reopen_live.utf16le.xml");
const AGEING_CORPUS_GUID: &str = "2f65b86f-edf4-471c-99ed-da0de7163836";
const GST_CREDIT_PERIOD_CORPUS_GUID: &str = "46faa869-1208-4119-8961-f28db4df3b8e";
const SETTLE_THEN_REOPEN_CORPUS_GUID: &str = "ec4454ae-5c4c-4bfa-b3b0-68182a749689";

#[test]
fn credit_periods_produce_calendar_due_dates_without_unit_guessing() {
    assert_eq!(
        add_credit_period(
            &TallyDate::parse("20260131").unwrap(),
            &CreditPeriod::Months(1)
        )
        .unwrap()
        .as_str(),
        "20260228"
    );
    assert_eq!(
        add_credit_period(
            &TallyDate::parse("20260101").unwrap(),
            &CreditPeriod::Weeks(3)
        )
        .unwrap()
        .as_str(),
        "20260122"
    );
    assert_eq!(
        add_credit_period(
            &TallyDate::parse("20260101").unwrap(),
            &CreditPeriod::Days(45)
        )
        .unwrap()
        .as_str(),
        "20260215"
    );
}

#[test]
fn captured_ageing_corpus_moves_seven_of_eight_bills_between_anchors() {
    let scan = captured_scan(
        AGEING_CORPUS,
        "BRIDGE CORPUS AGEING",
        AGEING_CORPUS_GUID,
        "20250401",
        "20260331",
        8,
        "497aec1804603b5c79a6ece404554c1f0ee1fb005ce3187b627f3292c51605f6",
    );
    let as_of = TallyDate::parse("20260331").unwrap();
    let bill_date = compute_outstandings_with_ageing_anchor(
        &scan,
        as_of.clone(),
        crate::outstandings::AgeingAnchor::BillDate,
    )
    .expect("captured bill-date ageing computes");
    let due_date = compute_outstandings_with_ageing_anchor(
        &scan,
        as_of,
        crate::outstandings::AgeingAnchor::DueDate,
    )
    .expect("captured due-date ageing computes");

    assert_eq!(bill_date.ageing_bill_counts.days_0_30, 1);
    assert_eq!(bill_date.ageing_bill_counts.days_31_60, 2);
    assert_eq!(bill_date.ageing_bill_counts.days_61_90, 2);
    assert_eq!(bill_date.ageing_bill_counts.days_90_plus, 3);
    assert_eq!(due_date.ageing_bill_counts.days_0_30, 4);
    assert_eq!(due_date.ageing_bill_counts.days_31_60, 3);
    assert_eq!(due_date.ageing_bill_counts.days_61_90, 1);
    assert_eq!(due_date.ageing_bill_counts.days_90_plus, 0);
    assert_ne!(bill_date.ageing, due_date.ageing);
}

#[test]
fn captured_gst_corpus_parses_week_and_month_credit_period_units() {
    let scan = captured_scan(
        GST_CREDIT_PERIOD_CORPUS,
        "BRIDGE CORPUS GST",
        GST_CREDIT_PERIOD_CORPUS_GUID,
        "20250401",
        "20250420",
        40,
        "1e340126eda8e767d2b53cab8bb2086add1ed35f53f7216a16edbc16624b30b8",
    );
    let periods = scan
        .vouchers()
        .iter()
        .flat_map(|voucher| voucher.ledger_entries.iter())
        .flat_map(|entry| entry.bill_allocations.iter())
        .map(|allocation| &allocation.credit_period)
        .collect::<Vec<_>>();

    assert!(periods.contains(&&CreditPeriod::Days(15)));
    assert!(periods.contains(&&CreditPeriod::Days(30)));
    assert!(periods.contains(&&CreditPeriod::Weeks(2)));
    assert!(periods.contains(&&CreditPeriod::Months(1)));
    assert!(periods.contains(&&CreditPeriod::Months(2)));
}

#[test]
fn captured_settle_then_reopen_preserves_each_original_age_date() {
    let xml = decode_tally_xml_response_bytes_limited(
        SETTLE_THEN_REOPEN_CORPUS,
        "text/xml; charset=utf-16",
        ExpectedTallyTextEncoding::Utf16Le,
        SETTLE_THEN_REOPEN_CORPUS.len(),
    )
    .expect("captured BOM-less UTF-16LE response decodes")
    .text;
    assert_eq!(xml.matches("<BILLALLOCATIONS.LIST>").count(), 38);

    let scan = captured_scan(
        SETTLE_THEN_REOPEN_CORPUS,
        "BRIDGE PROBE B SANDBOX",
        SETTLE_THEN_REOPEN_CORPUS_GUID,
        "20260810",
        "20260814",
        2785,
        "02a93b8a12c1ba80445ee136c690e72236ed372c25b333491a90e1fcd72f2737",
    );
    assert_eq!(scan.vouchers().len(), 19);
    let allocations = scan
        .vouchers()
        .iter()
        .flat_map(|voucher| voucher.ledger_entries.iter())
        .flat_map(|entry| entry.bill_allocations.iter())
        .collect::<Vec<_>>();
    assert_eq!(
        allocations.len(),
        19,
        "only populated allocation rows parse"
    );

    let reopened_due_dates = scan
        .vouchers()
        .iter()
        .filter(|voucher| voucher.date.as_str() == "20260814")
        .flat_map(|voucher| voucher.ledger_entries.iter())
        .flat_map(|entry| entry.bill_allocations.iter())
        .filter(|allocation| allocation.bill_type == BillReferenceKind::AgstRef)
        .map(|allocation| {
            (
                allocation
                    .name
                    .as_deref()
                    .expect("captured named bill reference"),
                add_credit_period(
                    allocation
                        .bill_date
                        .as_ref()
                        .expect("captured original BILLDATE"),
                    &allocation.credit_period,
                )
                .expect("captured credit period produces due date")
                .as_str()
                .to_string(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        reopened_due_dates,
        vec![
            ("RO-INV-001", "20260909".to_string()),
            ("RO-INV-002", "20260924".to_string()),
            ("RO-INV-003", "20261010".to_string()),
            ("RO-INV-004", "20260831".to_string()),
            ("RO-INV-005", "20261009".to_string()),
        ],
        "each reopening must retain its own original bill date and credit period"
    );

    let as_of = TallyDate::parse("20261015").unwrap();
    let due_date = compute_outstandings_with_ageing_anchor(
        &scan,
        as_of.clone(),
        crate::outstandings::AgeingAnchor::DueDate,
    )
    .expect("captured due-date reopening computes");
    assert_eq!(due_date.open_receivable_bill_count, 5);
    assert_eq!(due_date.receivable_total.as_str(), "3850");
    assert_eq!(due_date.ageing.days_0_30.as_str(), "2550");
    assert_eq!(due_date.ageing.days_31_60.as_str(), "1300");
    assert_eq!(due_date.ageing_bill_counts.days_0_30, 3);
    assert_eq!(due_date.ageing_bill_counts.days_31_60, 2);
    assert_eq!(due_date.ageing_bill_counts.days_61_90, 0);
    assert_eq!(due_date.ageing_bill_counts.days_90_plus, 0);

    let bill_date = compute_outstandings_with_ageing_anchor(
        &scan,
        as_of,
        crate::outstandings::AgeingAnchor::BillDate,
    )
    .expect("captured bill-date reopening computes");
    assert_eq!(bill_date.open_receivable_bill_count, 5);
    assert_eq!(bill_date.ageing.days_61_90.as_str(), "3850");
    assert_eq!(bill_date.ageing_bill_counts.days_61_90, 5);
}

#[test]
fn exact_bill_balances_age_and_split_receivable_from_payable() {
    let company = PinnedCompany::verified(
        ValidatedCompanyName::new("Synthetic Company").unwrap(),
        "synthetic-guid".to_string(),
    )
    .unwrap();
    let window =
        DateWindow::parse(DateBoundaryProfile::ModeAgnostic, "20260101", "20260401").unwrap();
    let scan = CompleteScan {
        company,
        reporting_window: window,
        voucher_alter_id_high_water: VoucherAlterIdHighWater::parse("5").unwrap(),
        vouchers: vec![
            voucher(
                "sale",
                "20260101",
                "Customer",
                "Invoice-1",
                "New Ref",
                "-100.00",
            ),
            voucher(
                "receipt",
                "20260201",
                "Customer",
                "Invoice-1",
                "Agst Ref",
                "40.00",
            ),
            voucher(
                "recent",
                "20260331",
                "Customer",
                "Invoice-2",
                "New Ref",
                "-10.00",
            ),
            voucher(
                "purchase",
                "20260201",
                "Vendor",
                "Purchase-1",
                "New Ref",
                "50.00",
            ),
            voucher(
                "customer-payable",
                "20260331",
                "Customer",
                "Customer-Credit-1",
                "New Ref",
                "5.00",
            ),
        ],
        encoded_bytes: 4096,
        empty_partition_witnesses: Vec::new(),
    };
    let report = compute_outstandings(&scan, TallyDate::parse("20260401").unwrap()).unwrap();
    assert_eq!(report.receivable_total.as_str(), "70");
    assert_eq!(report.payable_total.as_str(), "55");
    assert!(!report.has_unaged_receivable);
    assert_eq!(report.ageing.days_0_30.as_str(), "10");
    assert_eq!(report.ageing.days_61_90.as_str(), "60");
    assert_eq!(report.open_receivable_bill_count, 2);
    assert_eq!(report.ageing_bill_counts.days_0_30, 1);
    assert_eq!(report.ageing_bill_counts.days_31_60, 0);
    assert_eq!(report.ageing_bill_counts.days_61_90, 1);
    assert_eq!(report.ageing_bill_counts.days_90_plus, 0);
    assert_eq!(report.top_parties[0].party, "Customer");
    assert_eq!(report.top_parties[0].payable.as_str(), "5");
    assert_eq!(report.top_parties[0].outstanding_total.as_str(), "75");
    assert_eq!(report.top_parties[0].oldest_bill_age_days, Some(90));

    let later = compute_outstandings(&scan, TallyDate::parse("20260501").unwrap()).unwrap();
    assert_eq!(later.receivable_total, report.receivable_total);
    assert_eq!(later.payable_total, report.payable_total);
    assert_eq!(
        later.open_receivable_bill_count,
        report.open_receivable_bill_count
    );
    assert_ne!(later.ageing, report.ageing);
    assert_ne!(later.ageing_bill_counts, report.ageing_bill_counts);
    assert_eq!(later.as_of_yyyymmdd, "20260501");
}

#[test]
fn settled_reference_reuse_restarts_age_from_the_new_open_balance() {
    let company = PinnedCompany::verified(
        ValidatedCompanyName::new("Synthetic Company").unwrap(),
        "synthetic-guid".to_string(),
    )
    .unwrap();
    let scan = CompleteScan {
        company,
        reporting_window: DateWindow::parse(
            DateBoundaryProfile::ModeAgnostic,
            "20260101",
            "20260401",
        )
        .unwrap(),
        voucher_alter_id_high_water: VoucherAlterIdHighWater::parse("3").unwrap(),
        // Deliberately not chronological: computation must not inherit the
        // scan's GUID ordering when rebuilding a bill lifecycle.
        vouchers: vec![
            voucher(
                "new-cycle",
                "20260331",
                "Customer",
                "REUSED-REF",
                "New Ref",
                "-50.00",
            ),
            voucher(
                "old-cycle-settlement",
                "20260201",
                "Customer",
                "REUSED-REF",
                "Agst Ref",
                "100.00",
            ),
            voucher(
                "old-cycle",
                "20260101",
                "Customer",
                "REUSED-REF",
                "New Ref",
                "-100.00",
            ),
        ],
        encoded_bytes: 1024,
        empty_partition_witnesses: Vec::new(),
    };
    let report = compute_outstandings(&scan, TallyDate::parse("20260401").unwrap()).unwrap();
    assert_eq!(report.receivable_total.as_str(), "50");
    assert_eq!(report.ageing.days_0_30.as_str(), "50");
    assert_eq!(report.ageing.days_90_plus.as_str(), "0");
    assert_eq!(report.open_receivable_bill_count, 1);
    assert_eq!(report.ageing_bill_counts.days_0_30, 1);
    assert_eq!(report.top_parties[0].oldest_bill_age_days, Some(1));
}

#[test]
fn future_due_open_bill_is_bucketed_without_claiming_an_overdue_age() {
    let company = PinnedCompany::verified(
        ValidatedCompanyName::new("Synthetic Company").unwrap(),
        "synthetic-guid".to_string(),
    )
    .unwrap();
    let mut future_due = voucher(
        "future-due",
        "20260315",
        "Customer",
        "Invoice-future",
        "New Ref",
        "-100.00",
    );
    future_due.ledger_entries[0].bill_allocations[0].credit_period = CreditPeriod::Days(30);
    let scan = CompleteScan {
        company,
        reporting_window: DateWindow::parse(
            DateBoundaryProfile::ModeAgnostic,
            "20260101",
            "20260331",
        )
        .unwrap(),
        voucher_alter_id_high_water: VoucherAlterIdHighWater::parse("1").unwrap(),
        vouchers: vec![future_due],
        encoded_bytes: 1024,
        empty_partition_witnesses: Vec::new(),
    };

    let report = compute_outstandings(&scan, TallyDate::parse("20260331").unwrap())
        .expect("a future-due bill must not fail the complete report");

    assert_eq!(report.receivable_total.as_str(), "100");
    assert_eq!(report.ageing.days_0_30.as_str(), "100");
    assert_eq!(report.open_receivable_bill_count, 1);
    assert_eq!(report.top_parties[0].oldest_bill_age_days, None);
}

fn captured_scan(
    bytes: &[u8],
    company_name: &str,
    company_guid: &str,
    from: &str,
    to: &str,
    high_water: u64,
    expected_sha256: &str,
) -> CompleteScan {
    let observed_sha256 = Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    assert_eq!(
        observed_sha256, expected_sha256,
        "captured wire bytes changed"
    );
    let xml = decode_tally_xml_response_bytes_limited(
        bytes,
        "text/xml; charset=utf-16",
        ExpectedTallyTextEncoding::Utf16Le,
        bytes.len(),
    )
    .expect("captured BOM-less UTF-16LE response decodes")
    .text;
    let company = PinnedCompany::verified(
        ValidatedCompanyName::new(company_name).expect("synthetic capture company name is valid"),
        company_guid.to_string(),
    )
    .expect("captured company identity is pinned");
    let window = DateWindow::parse(DateBoundaryProfile::ModeAgnostic, from, to)
        .expect("captured date window is valid");
    let parsed = parse_segment(
        &xml,
        &company,
        &window,
        AlterIdRange::new(0, high_water).expect("captured AlterID range is valid"),
    )
    .expect("captured response parses");
    assert_eq!(parsed.raw_row_count, parsed.vouchers.len());
    CompleteScan {
        company,
        reporting_window: window,
        voucher_alter_id_high_water: VoucherAlterIdHighWater::parse(&high_water.to_string())
            .unwrap(),
        vouchers: parsed.vouchers,
        encoded_bytes: bytes.len(),
        empty_partition_witnesses: Vec::new(),
    }
}

#[test]
fn optional_vouchers_are_excluded_from_ordinary_book_totals() {
    // Optional vouchers are non-posting in Tally. Tally's own
    // bank-statement import creates vouchers as Optional by default, so a
    // real book can carry them with full bill allocations; counting them
    // would inflate receivables against the customer's actual ledger.
    let company = PinnedCompany::verified(
        ValidatedCompanyName::new("Synthetic Company").unwrap(),
        "synthetic-guid".to_string(),
    )
    .unwrap();
    let window =
        DateWindow::parse(DateBoundaryProfile::ModeAgnostic, "20260101", "20260401").unwrap();
    let posted = voucher(
        "sale",
        "20260101",
        "Customer",
        "Invoice-1",
        "New Ref",
        "-100.00",
    );
    let mut optional = voucher(
        "optional-sale",
        "20260101",
        "Customer",
        "Invoice-2",
        "New Ref",
        "-250.00",
    );
    optional.optional = true;

    let scan = CompleteScan {
        company,
        reporting_window: window,
        voucher_alter_id_high_water: VoucherAlterIdHighWater::parse("5").unwrap(),
        vouchers: vec![posted, optional],
        encoded_bytes: 2048,
        empty_partition_witnesses: Vec::new(),
    };
    let report = compute_outstandings(&scan, TallyDate::parse("20260401").unwrap()).unwrap();

    // Only the posted 100.00 is receivable; the optional 250.00 is absent
    // from the total AND from the open-bill count.
    assert_eq!(report.receivable_total.as_str(), "100");
    assert_eq!(report.open_receivable_bill_count, 1);
    let customer = report
        .top_parties
        .iter()
        .find(|party| party.party == "Customer")
        .expect("the posted voucher's party is present");
    assert_eq!(
        customer.receivable.as_str(),
        "100",
        "an optional voucher reached top-party exposure"
    );
}

fn voucher(
    guid: &str,
    date: &str,
    party: &str,
    reference: &str,
    bill_type: &str,
    amount: &str,
) -> Voucher {
    let amount = ExactDecimal::parse(amount).unwrap();
    Voucher {
        guid: guid.to_string(),
        master_id: guid.to_string(),
        alter_id: VoucherAlterId::parse("1").unwrap(),
        date: TallyDate::parse(date).unwrap(),
        voucher_type: "Synthetic".to_string(),
        voucher_number: None,
        party_ledger_name: Some(party.to_string()),
        cancelled: false,
        deleted: false,
        optional: false,
        ledger_entries: vec![LedgerEntry {
            ledger_name: party.to_string(),
            bill_allocations: vec![BillAllocation {
                bill_date: matches!(bill_type, "New Ref" | "Agst Ref")
                    .then(|| TallyDate::parse(date).unwrap()),
                name: Some(reference.to_string()),
                bill_type: match bill_type {
                    "New Ref" => BillReferenceKind::NewRef,
                    "Agst Ref" => BillReferenceKind::AgstRef,
                    "Advance" => BillReferenceKind::Advance,
                    "On Account" => BillReferenceKind::OnAccount,
                    _ => panic!("synthetic test must use a known kind"),
                },
                amount: MoneyValue::Exact(amount),
                credit_period: CreditPeriod::Days(0),
            }],
        }],
    }
}
