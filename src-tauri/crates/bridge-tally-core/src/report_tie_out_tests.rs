use crate::{
    CoreAccountingBatch, LedgerEntryPolarity, LedgerEntryRecord, LedgerRecord, VoucherRecord,
};

use super::*;

fn source() -> SourceIdentity {
    SourceIdentity {
        bridge_source_lineage: "synthetic".to_string(),
        company_guid: "company-guid".to_string(),
        observed_fingerprint: "fingerprint".to_string(),
    }
}

fn window() -> ReadWindow {
    ReadWindow {
        from_yyyymmdd: "20260701".to_string(),
        to_yyyymmdd: "20260731".to_string(),
    }
}

fn core(optional: bool, cancelled: bool) -> CoreAccountingBatch {
    CoreAccountingBatch {
        ledgers: vec![LedgerRecord {
            source_id: "ledger-a".to_string(),
            name: "A".to_string(),
            parent_source_id: None,
            opening_balance: None,
        }],
        vouchers: vec![VoucherRecord {
            source_id: "voucher-a".to_string(),
            date_yyyymmdd: "20260702".to_string(),
            voucher_type_source_id: "sales".to_string(),
            voucher_number: None,
            cancelled,
            optional,
        }],
        ledger_entries: vec![LedgerEntryRecord {
            source_id: "entry-a".to_string(),
            voucher_source_id: "voucher-a".to_string(),
            ledger_source_id: "ledger-a".to_string(),
            amount: ExactDecimal::parse("-0.001").unwrap(),
            polarity: LedgerEntryPolarity::Debit,
        }],
        ..CoreAccountingBatch::default()
    }
}

fn report(opening: &str, closing: &str) -> LedgerPeriodBalanceReport {
    LedgerPeriodBalanceReport {
        source_identity: source(),
        window: window(),
        ordinary_books_scope_observed: true,
        source_reported_count: 1,
        balances: vec![LedgerPeriodBalance {
            ledger_source_id: "ledger-a".to_string(),
            opening_balance: ExactDecimal::parse(opening).unwrap(),
            closing_balance: ExactDecimal::parse(closing).unwrap(),
        }],
    }
}

#[test]
fn exact_period_movement_ties_out_across_scales() {
    let assessment = assess_core_period_report(
        &core(false, false),
        &source(),
        &window(),
        &report("100.0000", "99.999"),
    )
    .unwrap();
    assert_eq!(assessment.state, TieOutState::Passed);
    assert_eq!(assessment.compared_ledger_count, 1);
}

#[test]
fn sub_cent_report_mismatch_is_not_rounded_away() {
    let assessment = assess_core_period_report(
        &core(false, false),
        &source(),
        &window(),
        &report("100", "100"),
    )
    .unwrap();
    assert_eq!(assessment.state, TieOutState::Mismatch);
    assert_eq!(assessment.mismatched_ledger_source_ids, ["ledger-a"]);
}

#[test]
fn optional_and_cancelled_movements_are_excluded_from_ordinary_books() {
    for core in [core(true, false), core(false, true)] {
        let assessment =
            assess_core_period_report(&core, &source(), &window(), &report("100", "100.0"))
                .unwrap();
        assert_eq!(assessment.state, TieOutState::Passed);
    }
}

#[test]
fn missing_or_unexpected_ledger_rows_are_mismatches() {
    let mut report = report("100", "99.999");
    report.balances[0].ledger_source_id = "ledger-other".to_string();
    let assessment =
        assess_core_period_report(&core(false, false), &source(), &window(), &report).unwrap();
    assert_eq!(assessment.state, TieOutState::Mismatch);
    assert_eq!(assessment.mismatched_ledger_source_ids.len(), 2);
}

#[test]
fn report_scope_and_count_are_strictly_bound() {
    let mut unobserved_profile = report("100", "99.999");
    unobserved_profile.ordinary_books_scope_observed = false;
    let unavailable = assess_core_period_report(
        &core(false, false),
        &source(),
        &window(),
        &unobserved_profile,
    )
    .unwrap();
    assert_eq!(unavailable.state, TieOutState::Unavailable);
    assert!(unavailable
        .safe_reason_codes
        .contains("period_report_profile_unobserved"));

    let mut wrong_scope = report("100", "99.999");
    wrong_scope.window.to_yyyymmdd = "20260730".to_string();
    assert!(
        assess_core_period_report(&core(false, false), &source(), &window(), &wrong_scope,)
            .is_err()
    );

    let mut wrong_count = report("100", "99.999");
    wrong_count.source_reported_count = 2;
    assert!(
        assess_core_period_report(&core(false, false), &source(), &window(), &wrong_count,)
            .is_err()
    );
}

#[test]
fn mismatch_aliases_are_scoped_and_never_contain_raw_ids() {
    let raw = "00000000-0000-4000-8000-000000000777";
    let alias = scoped_mismatch_record_alias("company-fingerprint", "run-1", "window-1", raw);
    assert!(alias.starts_with("rid:"));
    assert_eq!(alias.len(), 68);
    assert!(!alias.contains(raw));
    assert_eq!(
        alias,
        scoped_mismatch_record_alias("company-fingerprint", "run-1", "window-1", raw)
    );
    assert_ne!(
        alias,
        scoped_mismatch_record_alias("company-fingerprint", "run-2", "window-1", raw)
    );
    assert_ne!(
        alias,
        scoped_mismatch_record_alias("other-company", "run-1", "window-1", raw)
    );
}
