use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::exact_arithmetic::{is_negative_nonzero, numeric_equal, ExactDecimalAccumulator};
use crate::{CoreAccountingBatch, LedgerEntryPolarity, LedgerEntryRecord};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckState {
    Passed,
    Mismatch,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct CoreAccountingChecks {
    pub reference_integrity: CheckState,
    pub voucher_entry_balance: CheckState,
    pub voucher_entry_polarity: CheckState,
    pub voucher_entry_applicability: CheckState,
    pub voucher_header_entry_total: CheckState,
}

/// Raw source IDs are intentionally kept in a non-serializable intermediate.
/// The application must replace them with local-only aliases before any
/// operator-visible persistence or export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountingIssue {
    pub safe_reason_code: &'static str,
    pub source_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreAccountingAssessment {
    pub checks: CoreAccountingChecks,
    pub issues: Vec<AccountingIssue>,
}

pub fn assess_core_accounting(core: &CoreAccountingBatch) -> CoreAccountingAssessment {
    let voucher_types = core
        .voucher_types
        .iter()
        .map(|record| record.source_id.as_str())
        .collect::<BTreeSet<_>>();
    let vouchers = core
        .vouchers
        .iter()
        .map(|record| record.source_id.as_str())
        .collect::<BTreeSet<_>>();
    let ledgers = core
        .ledgers
        .iter()
        .map(|record| record.source_id.as_str())
        .collect::<BTreeSet<_>>();
    let mut issues = Vec::new();

    push_issue(
        &mut issues,
        "voucher_type_reference_missing",
        core.vouchers
            .iter()
            .filter(|record| !voucher_types.contains(record.voucher_type_source_id.as_str()))
            .map(|record| record.source_id.clone())
            .collect(),
    );
    push_issue(
        &mut issues,
        "voucher_reference_missing",
        core.ledger_entries
            .iter()
            .filter(|record| !vouchers.contains(record.voucher_source_id.as_str()))
            .map(|record| record.source_id.clone())
            .collect(),
    );
    push_issue(
        &mut issues,
        "ledger_reference_missing",
        core.ledger_entries
            .iter()
            .filter(|record| !ledgers.contains(record.ledger_source_id.as_str()))
            .map(|record| record.source_id.clone())
            .collect(),
    );
    let reference_integrity = state_for_codes(
        &issues,
        &[
            "voucher_type_reference_missing",
            "voucher_reference_missing",
            "ledger_reference_missing",
        ],
    );

    let excluded_from_books = core
        .vouchers
        .iter()
        .filter(|voucher| voucher.cancelled || voucher.optional)
        .map(|voucher| voucher.source_id.as_str())
        .collect::<BTreeSet<_>>();

    push_issue(
        &mut issues,
        "voucher_entry_polarity_mismatch",
        inverted_voucher_entries(core, &excluded_from_books),
    );
    let voucher_entry_polarity =
        if state_for_codes(&issues, &["voucher_entry_polarity_mismatch"]) == CheckState::Mismatch {
            CheckState::Mismatch
        } else if core
            .ledger_entries
            .iter()
            .filter(|entry| !excluded_from_books.contains(entry.voucher_source_id.as_str()))
            .any(entry_amount_is_zero)
        {
            CheckState::Unavailable
        } else {
            CheckState::Passed
        };

    let mut totals: BTreeMap<&str, ExactDecimalAccumulator> = BTreeMap::new();
    for entry in &core.ledger_entries {
        if !excluded_from_books.contains(entry.voucher_source_id.as_str()) {
            totals
                .entry(entry.voucher_source_id.as_str())
                .or_default()
                .add(entry.amount.as_str());
        }
    }
    push_issue(
        &mut issues,
        "voucher_entries_unbalanced",
        core.vouchers
            .iter()
            .filter(|voucher| !voucher.cancelled && !voucher.optional)
            .filter(|voucher| {
                totals
                    .get(voucher.source_id.as_str())
                    .is_some_and(|total| !total.is_zero())
            })
            .map(|voucher| voucher.source_id.clone())
            .collect(),
    );
    let voucher_entry_balance = state_for_codes(&issues, &["voucher_entries_unbalanced"]);
    let voucher_entry_applicability = if core
        .vouchers
        .iter()
        .filter(|voucher| !voucher.cancelled && !voucher.optional)
        .any(|voucher| !totals.contains_key(voucher.source_id.as_str()))
    {
        CheckState::Unavailable
    } else {
        CheckState::Passed
    };

    CoreAccountingAssessment {
        checks: CoreAccountingChecks {
            reference_integrity,
            voucher_entry_balance,
            voucher_entry_polarity,
            voucher_entry_applicability,
            // The Core v2 model has no independently observed voucher header
            // total. Absence is a gap, never an inferred pass.
            voucher_header_entry_total: CheckState::Unavailable,
        },
        issues,
    }
}

fn push_issue(
    issues: &mut Vec<AccountingIssue>,
    safe_reason_code: &'static str,
    mut source_ids: Vec<String>,
) {
    if source_ids.is_empty() {
        return;
    }
    source_ids.sort();
    source_ids.dedup();
    issues.push(AccountingIssue {
        safe_reason_code,
        source_ids,
    });
}

fn state_for_codes(issues: &[AccountingIssue], codes: &[&str]) -> CheckState {
    if issues
        .iter()
        .any(|issue| codes.contains(&issue.safe_reason_code))
    {
        CheckState::Mismatch
    } else {
        CheckState::Passed
    }
}

/// The entries of every voucher whose polarity is inverted *as a whole*.
///
/// A single entry disagreeing with its amount sign is not a defect. Tally's
/// `ISDEEMEDPOSITIVE` records the column an entry was made in, not its
/// arithmetic sign, and the two legitimately diverge — this document's own
/// protocol reference already records that bill-allocation polarity is
/// "contextual, not an amount-sign invariant", measured in both directions,
/// and outstandings deliberately does not validate one against the other.
///
/// Measured on twelve monthly windows of a real trading book, by two
/// independent parsers: **on the order of 110 of ~6,900 entries** disagree
/// (111 and 104 respectively -- the ~5% gap is unresolved and is most likely
/// XML-parsing edge cases, so treat the count as approximate). Both passes
/// agree exactly on the shape that matters: the disagreement is **always
/// exactly one entry** of a voucher four to six wide, in both directions, and
/// **never** the whole voucher. Flagging each was ~110 reconciliation
/// mismatches on an ordinary book, which is how a check teaches its reader to
/// ignore it.
///
/// What the check is actually for survives: a voucher can sum to zero while
/// every entry's polarity is inverted, which the balance check cannot see
/// (`numeric_zero_sum_cannot_hide_contradictory_tally_polarity`). That shape is
/// systematic and cannot be explained by contextual polarity, so it is still
/// reported. On the same real book it occurs **zero** times.
///
/// `polarity` itself is unchanged and still comes from the flag. It is read
/// here and in `transport_qualification.rs`, which projects it into a transport
/// parity fingerprint; no accounting total is derived from it.
fn inverted_voucher_entries(
    core: &CoreAccountingBatch,
    excluded_from_books: &BTreeSet<&str>,
) -> Vec<String> {
    // Judge only the entries that carry a sign. A zero amount agrees with
    // either polarity by definition -- `entry_polarity_matches_amount` returns
    // true for every one of them -- so folding them in would let a single zero
    // leg prove the voucher "not wholly inverted" and silently disable this
    // check for that voucher. Zero legs are ordinary: 227 of 6,854 entries in
    // the captured year. They are counted in `judged` nowhere, so a voucher of
    // nothing but zero amounts reports nothing rather than everything.
    struct Tally {
        judged: usize,
        inverted: usize,
        entries: Vec<String>,
    }
    let mut by_voucher: BTreeMap<&str, Tally> = BTreeMap::new();
    for entry in core
        .ledger_entries
        .iter()
        .filter(|entry| !excluded_from_books.contains(entry.voucher_source_id.as_str()))
    {
        let slot = by_voucher
            .entry(entry.voucher_source_id.as_str())
            .or_insert(Tally {
                judged: 0,
                inverted: 0,
                entries: Vec::new(),
            });
        if !entry_amount_is_zero(entry) {
            slot.judged += 1;
            if !entry_polarity_matches_amount(entry) {
                slot.inverted += 1;
            }
        }
        slot.entries.push(entry.source_id.clone());
    }
    by_voucher
        .into_values()
        .filter(|tally| tally.judged > 0 && tally.inverted == tally.judged)
        .flat_map(|tally| tally.entries)
        .collect()
}

fn entry_polarity_matches_amount(entry: &LedgerEntryRecord) -> bool {
    let zero = numeric_equal(entry.amount.as_str(), "0");
    let negative = is_negative_nonzero(entry.amount.as_str());
    zero || matches!(
        (entry.polarity, negative),
        (LedgerEntryPolarity::Debit, true) | (LedgerEntryPolarity::Credit, false)
    )
}

fn entry_amount_is_zero(entry: &LedgerEntryRecord) -> bool {
    numeric_equal(entry.amount.as_str(), "0")
}

#[cfg(test)]
mod tests {
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
}
