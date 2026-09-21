// SPDX-License-Identifier: Apache-2.0
//! Every ported test, one entry each, sorted by test id: how to run it on a book, and the
//! least number of figures a parity comparison of it must see. Porting a test adds one entry
//! here and one runner in `parity/python_golden.py`'s `RUNNERS`; `examples/local_parity.rs`,
//! `compare::default_min_figures` and the registry-wide CI parity (`tests/registry.rs`) read
//! this table instead of listing tests themselves.

use crate::applicability_44ab::{ComparisonTurnover, TurnoverInputs};
use crate::book::Book;
use crate::error::{AuditError, Result};
use crate::financial_statements::ReportTotals;
use crate::rules::Rules;
use crate::Engagement;

/// Data a test takes from its caller rather than from the book: Tally's own Profit & Loss report
/// totals (`financial_statements`) and the comparison turnover (`applicability_44ab`). Empty is
/// "none supplied", which each of those tests handles itself.
#[derive(Debug, Clone, Default)]
pub struct CallerData {
    pub report_totals: Option<ReportTotals>,
    pub turnover_inputs: TurnoverInputs,
}

/// One ported test.
pub struct PortedTest {
    pub id: &'static str,
    /// The fewest figures a parity comparison of this test accepts (see `compare`).
    pub min_figures: usize,
    /// Run the test on an already-loaded book and return its canonical parity dump.
    pub run_on: fn(&Engagement, &Book, &Rules, &CallerData) -> Result<serde_json::Value>,
}

pub const PORTED: &[PortedTest] = &[
    PortedTest {
        id: "applicability_44ab",
        min_figures: 9,
        run_on: |e, b, r, c| crate::applicability_44ab_on(e, b, r, &c.turnover_inputs),
    },
    PortedTest {
        id: "cash_44ab",
        min_figures: 7,
        run_on: |e, b, r, _| crate::cash_44ab_on(e, b, r),
    },
    PortedTest {
        id: "cash_book_integrity",
        min_figures: 1,
        run_on: |e, b, r, _| crate::cash_book_integrity_on(e, b, r),
    },
    PortedTest {
        id: "cash_payments_40a3",
        min_figures: 18,
        run_on: |e, b, r, _| crate::cash_payments_40a3_on(e, b, r),
    },
    PortedTest {
        id: "depreciation",
        min_figures: 2,
        run_on: |e, b, r, _| crate::depreciation_on(e, b, r),
    },
    PortedTest {
        id: "financial_statements",
        min_figures: 18,
        run_on: |e, b, r, c| crate::financial_statements_on(e, b, r, c.report_totals.as_ref()),
    },
    PortedTest {
        id: "ledger_scrutiny",
        min_figures: 1,
        run_on: |e, b, r, _| crate::ledger_scrutiny_on(e, b, r),
    },
    PortedTest {
        id: "stale_balances_41_1",
        min_figures: 1,
        run_on: |e, b, r, _| crate::stale_balances_41_1_on(e, b, r),
    },
    PortedTest {
        id: "trial_balance",
        min_figures: 1,
        run_on: |e, b, r, _| crate::trial_balance_on(e, b, r),
    },
];

/// The registry entry for `id`, if that test is ported.
pub fn find(id: &str) -> Option<&'static PortedTest> {
    PORTED.iter().find(|t| t.id == id)
}

/// Read, verify and build the book, then run test `id` on it: its canonical parity dump.
pub fn run_canonical(
    id: &str,
    engagement: &Engagement,
    rules: &Rules,
    caller: &CallerData,
) -> Result<serde_json::Value> {
    let test = find(id).ok_or_else(|| AuditError::Config(format!("{id} is not a ported test")))?;
    (test.run_on)(engagement, &crate::load_book(engagement)?, rules, caller)
}

/// `financial_statements`' report totals from the JSON `parity/python_golden.py
/// --emit-report-totals` writes.
pub fn report_totals_from_json(v: &serde_json::Value) -> Result<ReportTotals> {
    let net_profit_paise = v["net_profit_paise"].as_i64().ok_or_else(|| {
        AuditError::Config("report totals: net_profit_paise is not an integer".to_string())
    })?;
    Ok(ReportTotals {
        net_profit_paise,
        closing_stock_paise: v["closing_stock_paise"].as_i64(),
        source: v["source"].as_str().map(str::to_string),
    })
}

/// `applicability_44ab`'s comparison turnover from the JSON `parity/python_golden.py
/// --emit-turnover-inputs` writes. The books turnover is the test's own, never the caller's.
pub fn turnover_inputs_from_json(v: &serde_json::Value) -> Result<TurnoverInputs> {
    let source = |key: &str| -> Result<Option<ComparisonTurnover>> {
        let s = &v[key];
        if s.is_null() {
            return Ok(None);
        }
        match (s["turnover_paise"].as_i64(), s["coverage"].as_str()) {
            (Some(turnover_paise), Some(coverage)) => Ok(Some(ComparisonTurnover {
                turnover_paise,
                coverage: coverage.to_string(),
            })),
            _ => Err(AuditError::Config(format!(
                "turnover inputs: {key} needs turnover_paise and coverage"
            ))),
        }
    };
    Ok(TurnoverInputs {
        books_turnover_paise: None,
        gstr1: source("gstr1")?,
        gstr3b: source("gstr3b")?,
        ais: source("ais")?,
    })
}

#[cfg(test)]
mod tests {
    use super::PORTED;

    #[test]
    fn the_registry_is_sorted_and_unique() {
        let ids: Vec<&str> = PORTED.iter().map(|t| t.id).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(ids, sorted);
    }
}
