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
#[path = "report_tie_out_tests.rs"]
mod tests;
