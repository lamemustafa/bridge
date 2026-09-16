use crate::{
    CoreAccountingBatch, ExactDecimal, LedgerEntryPolarity, LedgerEntryRecord, LedgerRecord,
    VoucherRecord, VoucherTypeRecord,
};

use super::*;

fn batch(amounts: &[(&str, LedgerEntryPolarity)], cancelled: bool) -> CoreAccountingBatch {
    CoreAccountingBatch {
        ledgers: vec![
            LedgerRecord {
                source_id: "ledger-a".to_string(),
                name: "A".to_string(),
                parent_source_id: None,
                opening_balance: None,
            },
            LedgerRecord {
                source_id: "ledger-b".to_string(),
                name: "B".to_string(),
                parent_source_id: None,
                opening_balance: None,
            },
        ],
        voucher_types: vec![VoucherTypeRecord {
            source_id: "sales".to_string(),
            name: "Sales".to_string(),
        }],
        vouchers: vec![VoucherRecord {
            source_id: "voucher".to_string(),
            date_yyyymmdd: "20260701".to_string(),
            voucher_type_source_id: "sales".to_string(),
            voucher_number: None,
            cancelled,
            optional: false,
        }],
        ledger_entries: amounts
            .iter()
            .enumerate()
            .map(|(index, (amount, polarity))| LedgerEntryRecord {
                source_id: format!("entry-{index}"),
                voucher_source_id: "voucher".to_string(),
                ledger_source_id: if index % 2 == 0 {
                    "ledger-a".to_string()
                } else {
                    "ledger-b".to_string()
                },
                amount: ExactDecimal::parse(*amount).unwrap(),
                polarity: *polarity,
            })
            .collect(),
        ..CoreAccountingBatch::default()
    }
}

#[test]
fn exact_mixed_scale_and_large_amounts_balance_without_float_math() {
    let assessment = assess_core_accounting(&batch(
        &[
            ("-999999999999999999999.001", LedgerEntryPolarity::Debit),
            ("999999999999999999999.0010", LedgerEntryPolarity::Credit),
        ],
        false,
    ));
    assert_eq!(assessment.checks.voucher_entry_balance, CheckState::Passed);
    assert_eq!(assessment.checks.voucher_entry_polarity, CheckState::Passed);
}

#[test]
fn numeric_zero_sum_cannot_hide_contradictory_tally_polarity() {
    let assessment = assess_core_accounting(&batch(
        &[
            ("-100.00", LedgerEntryPolarity::Credit),
            ("100", LedgerEntryPolarity::Debit),
        ],
        false,
    ));
    assert_eq!(assessment.checks.voucher_entry_balance, CheckState::Passed);
    assert_eq!(
        assessment.checks.voucher_entry_polarity,
        CheckState::Mismatch
    );
}

#[test]
fn a_single_rounding_entry_is_contextual_polarity_not_a_mismatch() {
    // The shape bridge#392 is about, and the one an ordinary Indian
    // trading book produces constantly: a balanced voucher whose round-off
    // leg was entered in the debit column while carrying a positive
    // amount. `ISDEEMEDPOSITIVE` records the column, not the sign.
    //
    // Measured over twelve monthly windows of a real book: 111 of 6,957
    // entries look like this, every one of them the sole disagreeing entry
    // in a voucher of four to six, and in both directions. Reporting each
    // as a reconciliation mismatch made the check fire 111 times on a book
    // with nothing wrong with it.
    let assessment = assess_core_accounting(&batch(
        &[
            ("-1000.00", LedgerEntryPolarity::Debit),
            ("900.00", LedgerEntryPolarity::Credit),
            ("99.50", LedgerEntryPolarity::Credit),
            ("0.50", LedgerEntryPolarity::Debit),
        ],
        false,
    ));
    assert_eq!(assessment.checks.voucher_entry_balance, CheckState::Passed);
    assert_eq!(assessment.checks.voucher_entry_polarity, CheckState::Passed);
    assert!(assessment
        .issues
        .iter()
        .all(|issue| issue.safe_reason_code != "voucher_entry_polarity_mismatch"));
}

#[test]
fn whole_voucher_inversion_is_still_reported_at_any_entry_count() {
    // The defect the check exists for, at a width where a single
    // contextual entry could not explain it. Guards against "fix" the
    // false positive by deleting the check.
    let assessment = assess_core_accounting(&batch(
        &[
            ("1000.00", LedgerEntryPolarity::Debit),
            ("-900.00", LedgerEntryPolarity::Credit),
            ("-100.00", LedgerEntryPolarity::Credit),
        ],
        false,
    ));
    assert_eq!(assessment.checks.voucher_entry_balance, CheckState::Passed);
    assert_eq!(
        assessment.checks.voucher_entry_polarity,
        CheckState::Mismatch
    );
}

#[test]
fn a_zero_leg_does_not_disable_whole_voucher_inversion_detection() {
    // Found in review. `entry_polarity_matches_amount` reports true for
    // every zero amount, so folding zero legs into "is every entry
    // inverted?" let one of them prove the voucher innocent and silently
    // switched this check off for that voucher -- reporting nothing where
    // the per-entry check had reported two issues. Zero legs are ordinary
    // (227 of 6,854 entries in the captured year), so this is not a corner.
    let assessment = assess_core_accounting(&batch(
        &[
            ("0.00", LedgerEntryPolarity::Debit),
            ("900.00", LedgerEntryPolarity::Debit),
            ("-900.00", LedgerEntryPolarity::Credit),
        ],
        false,
    ));
    assert_eq!(
        assessment.checks.voucher_entry_polarity,
        CheckState::Mismatch
    );
    assert!(assessment
        .issues
        .iter()
        .any(|issue| issue.safe_reason_code == "voucher_entry_polarity_mismatch"));
}

#[test]
fn sub_cent_imbalance_is_detected_exactly() {
    let assessment = assess_core_accounting(&batch(
        &[
            ("-100.000", LedgerEntryPolarity::Debit),
            ("99.999", LedgerEntryPolarity::Credit),
        ],
        false,
    ));
    assert_eq!(
        assessment.checks.voucher_entry_balance,
        CheckState::Mismatch
    );
}

#[test]
fn cancelled_empty_voucher_is_not_a_missing_entry_claim() {
    let assessment = assess_core_accounting(&batch(&[], true));
    assert_eq!(
        assessment.checks.voucher_entry_applicability,
        CheckState::Passed
    );
    assert!(assessment.issues.is_empty());
}

#[test]
fn cancelled_entries_do_not_claim_book_effect_polarity_mismatches() {
    let assessment = assess_core_accounting(&batch(
        &[
            ("-100.00", LedgerEntryPolarity::Credit),
            ("100.00", LedgerEntryPolarity::Debit),
        ],
        true,
    ));
    assert_eq!(assessment.checks.voucher_entry_balance, CheckState::Passed);
    assert_eq!(assessment.checks.voucher_entry_polarity, CheckState::Passed);
    assert!(assessment.issues.is_empty());
}

#[test]
fn optional_entries_are_excluded_from_ordinary_book_effect_checks() {
    let mut core = batch(
        &[
            ("-100.00", LedgerEntryPolarity::Credit),
            ("100.00", LedgerEntryPolarity::Debit),
        ],
        false,
    );
    core.vouchers[0].optional = true;
    let assessment = assess_core_accounting(&core);
    assert_eq!(assessment.checks.voucher_entry_balance, CheckState::Passed);
    assert_eq!(assessment.checks.voucher_entry_polarity, CheckState::Passed);
    assert_eq!(
        assessment.checks.voucher_entry_applicability,
        CheckState::Passed
    );
    assert!(assessment.issues.is_empty());
}

#[test]
fn non_cancelled_empty_voucher_applicability_is_unavailable() {
    let assessment = assess_core_accounting(&batch(&[], false));
    assert_eq!(
        assessment.checks.voucher_entry_applicability,
        CheckState::Unavailable
    );
}

#[test]
fn zero_amount_cannot_claim_observed_polarity() {
    for polarity in [LedgerEntryPolarity::Debit, LedgerEntryPolarity::Credit] {
        let assessment =
            assess_core_accounting(&batch(&[("0.00", polarity), ("-0.000", polarity)], false));
        assert_eq!(
            assessment.checks.voucher_entry_polarity,
            CheckState::Unavailable
        );
    }
}
