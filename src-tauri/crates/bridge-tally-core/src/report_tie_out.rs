use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::exact_arithmetic::ExactDecimalAccumulator;
use crate::{CoreAccountingBatch, ExactDecimal, ReadWindow, SourceIdentity, TallyError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TieOutState {
    Passed,
    Mismatch,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerPeriodBalance {
    pub ledger_source_id: String,
    pub opening_balance: ExactDecimal,
    pub closing_balance: ExactDecimal,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerPeriodBalanceReport {
    pub source_identity: SourceIdentity,
    pub window: ReadWindow,
    /// True only after the exact Tally release/profile has independently
    /// demonstrated that this view matches Bridge's ordinary-books model.
    /// A request echo must never set this authority bit.
    pub ordinary_books_scope_observed: bool,
    pub source_reported_count: u64,
    pub balances: Vec<LedgerPeriodBalance>,
}

/// Raw source IDs remain in this non-serializable intermediate. The native
/// application must replace them with bounded, proof-local aliases before
/// operator display or persistence outside encrypted run state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreReportTieOutAssessment {
    pub state: TieOutState,
    pub compared_ledger_count: u64,
    pub safe_reason_codes: BTreeSet<&'static str>,
    pub mismatched_ledger_source_ids: Vec<String>,
}

/// Converts a raw source identifier into a bounded, run-local token before it
/// may enter durable reconciliation evidence.
pub fn scoped_mismatch_record_alias(
    company_fingerprint: &str,
    run_id: &str,
    window_id: &str,
    raw_source_id: &str,
) -> String {
    let mut digest = Sha256::new();
    digest.update(b"bridge-report-mismatch-record-alias-v1\0");
    digest.update(company_fingerprint.as_bytes());
    digest.update(b"\0");
    digest.update(run_id.as_bytes());
    digest.update(b"\0");
    digest.update(window_id.as_bytes());
    digest.update(b"\0");
    digest.update(raw_source_id.as_bytes());
    let hex = digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("rid:{hex}")
}

pub fn assess_core_period_report(
    core: &CoreAccountingBatch,
    expected_source: &SourceIdentity,
    expected_window: &ReadWindow,
    report: &LedgerPeriodBalanceReport,
) -> Result<CoreReportTieOutAssessment, TallyError> {
    if &report.source_identity != expected_source || &report.window != expected_window {
        return Err(invalid_data("period_report_scope_mismatch"));
    }
    if !report.ordinary_books_scope_observed {
        return Ok(unavailable("period_report_profile_unobserved"));
    }
    if report.source_reported_count != report.balances.len() as u64 {
        return Err(invalid_data("period_report_count_mismatch"));
    }

    let ledger_ids = core
        .ledgers
        .iter()
        .map(|ledger| ledger.source_id.as_str())
        .collect::<BTreeSet<_>>();
    if ledger_ids.len() != core.ledgers.len() {
        return Err(invalid_data("period_report_core_ledger_identity_duplicate"));
    }

    let mut report_by_ledger = BTreeMap::new();
    for balance in &report.balances {
        if balance.ledger_source_id.is_empty()
            || report_by_ledger
                .insert(balance.ledger_source_id.as_str(), balance)
                .is_some()
        {
            return Err(invalid_data("period_report_ledger_identity_invalid"));
        }
    }
    let report_ids = report_by_ledger.keys().copied().collect::<BTreeSet<_>>();
    if report_ids != ledger_ids {
        let mut assessment = mismatch("period_report_ledger_coverage_mismatch");
        assessment.mismatched_ledger_source_ids = report_ids
            .symmetric_difference(&ledger_ids)
            .map(|value| (*value).to_string())
            .collect();
        return Ok(assessment);
    }

    let posted_vouchers = core
        .vouchers
        .iter()
        .filter(|voucher| !voucher.cancelled && !voucher.optional)
        .map(|voucher| voucher.source_id.as_str())
        .collect::<BTreeSet<_>>();
    let mut movements: BTreeMap<&str, ExactDecimalAccumulator> = BTreeMap::new();
    for entry in &core.ledger_entries {
        if posted_vouchers.contains(entry.voucher_source_id.as_str()) {
            movements
                .entry(entry.ledger_source_id.as_str())
                .or_default()
                .add(entry.amount.as_str());
        }
    }

    let mut mismatched = Vec::new();
    for ledger_id in &ledger_ids {
        let balance = report_by_ledger
            .get(ledger_id)
            .expect("ledger-set equality was established");
        let mut report_movement = ExactDecimalAccumulator::default();
        report_movement.add(balance.closing_balance.as_str());
        report_movement.subtract(balance.opening_balance.as_str());
        if movements.get(ledger_id).cloned().unwrap_or_default() != report_movement {
            mismatched.push((*ledger_id).to_string());
        }
    }
    if mismatched.is_empty() {
        Ok(CoreReportTieOutAssessment {
            state: TieOutState::Passed,
            compared_ledger_count: ledger_ids.len() as u64,
            safe_reason_codes: BTreeSet::new(),
            mismatched_ledger_source_ids: Vec::new(),
        })
    } else {
        Ok(CoreReportTieOutAssessment {
            state: TieOutState::Mismatch,
            compared_ledger_count: ledger_ids.len() as u64,
            safe_reason_codes: BTreeSet::from(["period_report_movement_mismatch"]),
            mismatched_ledger_source_ids: mismatched,
        })
    }
}

fn unavailable(code: &'static str) -> CoreReportTieOutAssessment {
    CoreReportTieOutAssessment {
        state: TieOutState::Unavailable,
        compared_ledger_count: 0,
        safe_reason_codes: BTreeSet::from([code]),
        mismatched_ledger_source_ids: Vec::new(),
    }
}

fn mismatch(code: &'static str) -> CoreReportTieOutAssessment {
    CoreReportTieOutAssessment {
        state: TieOutState::Mismatch,
        compared_ledger_count: 0,
        safe_reason_codes: BTreeSet::from([code]),
        mismatched_ledger_source_ids: Vec::new(),
    }
}

fn invalid_data(code: &'static str) -> TallyError {
    TallyError::InvalidData {
        code: code.to_string(),
    }
}

#[cfg(test)]
mod tests {
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
}
