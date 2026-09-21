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
    let closing_stock_paise = match &v["closing_stock_paise"] {
        serde_json::Value::Null => None,
        x => Some(x.as_i64().ok_or_else(|| {
            AuditError::Config("report totals: closing_stock_paise is not an integer".to_string())
        })?),
    };
    let source = match &v["source"] {
        serde_json::Value::Null => None,
        x => Some(
            x.as_str()
                .ok_or_else(|| {
                    AuditError::Config("report totals: source is not a string".to_string())
                })?
                .to_string(),
        ),
    };
    Ok(ReportTotals {
        net_profit_paise,
        closing_stock_paise,
        source,
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
    fn find_is_by_exact_id() {
        assert!(super::find("trial_balance").is_some());
        assert!(super::find("trial").is_none());
        assert!(super::find("cash").is_none());
        assert!(super::find("trial_balance ").is_none());
    }

    #[test]
    fn caller_data_is_read_whole_and_refused_when_mistyped() {
        use super::{report_totals_from_json, turnover_inputs_from_json};
        use serde_json::json;
        let t = turnover_inputs_from_json(&json!({
            "gstr1": {"turnover_paise": 5, "coverage": "full"},
            "gstr3b": null,
            "ais": {"turnover_paise": 7, "coverage": "partial"}
        }))
        .unwrap();
        assert_eq!(t.gstr1.unwrap().turnover_paise, 5);
        assert!(t.gstr3b.is_none());
        let ais = t.ais.unwrap();
        assert_eq!((ais.turnover_paise, ais.coverage.as_str()), (7, "partial"));
        for bad in [
            json!({"gstr1": {"turnover_paise": "5", "coverage": "full"}}),
            json!({"ais": {"coverage": "full"}}),
        ] {
            assert!(turnover_inputs_from_json(&bad).is_err(), "{bad}");
        }
        let r =
            report_totals_from_json(&json!({"net_profit_paise": 3, "closing_stock_paise": null}))
                .unwrap();
        assert_eq!((r.net_profit_paise, r.closing_stock_paise), (3, None));
        for bad in [
            json!({}),
            json!({"net_profit_paise": "3"}),
            json!({"net_profit_paise": 3, "closing_stock_paise": "4"}),
            json!({"net_profit_paise": 3, "source": 9}),
        ] {
            assert!(report_totals_from_json(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_registry_is_sorted_and_unique() {
        let ids: Vec<&str> = PORTED.iter().map(|t| t.id).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(ids, sorted);
    }
}
