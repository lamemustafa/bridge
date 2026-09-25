//! Stale debtor and creditor balances and s.41(1): ledgers whose opening equals closing because no
//! voucher touched them all year, and -- for creditors -- whether an unchanged trade liability
//! raises a remission or cessation question under s.41(1). A port of the reference Python
//! implementation's `stale_balances_41_1` test module, version 1.
//!
//! Every figure is read straight off the Trial Balance, never a filtered voucher table. Scope is
//! structural: every ledger under Tally's own `Sundry Debtors` / `Sundry Creditors` groups. A
//! balance is active when abs(TB closing) is over Re 1, and stale when it is active and both TB
//! period movements are nil. Every s.41(1) reading is judgement-required, never a conclusion that
//! a liability has ceased.

use std::collections::{BTreeMap, BTreeSet};

use crate::book::{Book, TbRow};
use crate::error::Result;
use crate::findings::{Confidence, EvidenceRef, Finding, TestResult, Unit, Value};
use crate::ledger_ids::stable_ledger_tag;
use crate::rules::Rules;
use crate::support;

pub const TEST_ID: &str = "stale_balances_41_1";
pub const VERSION: &str = "1";

const DEBTOR_GROUP: &str = "Sundry Debtors";
const CREDITOR_GROUP: &str = "Sundry Creditors";
/// Re 1, the reference's AGE-1 / BKQ-1 convention.
const ACTIVE_TOL_PAISE: i64 = 100;

fn ledger_refs<'a>(names: impl Iterator<Item = &'a String>) -> Vec<EvidenceRef> {
    names.map(|n| EvidenceRef::new("ledger", n)).collect()
}

pub fn run(book: &Book, rules: &Rules) -> Result<TestResult> {
    let mut r = TestResult::new(TEST_ID, VERSION, &rules.version);
    r.population_note = "Figures in this test are read directly from the Trial Balance (TOT-1); \
no voucher population walk is used to compute them."
        .to_string();
    let overflow = || support::overflow(TEST_ID);

    for (role, group) in [("debtor", DEBTOR_GROUP), ("creditor", CREDITOR_GROUP)] {
        let ledgers = book.ledgers_under_any(&[group.to_string()]);
        // Sorted by name, as the reference sorts every list it cites.
        let mut active: BTreeMap<&String, &TbRow> = BTreeMap::new();
        let mut stale: BTreeMap<&String, &TbRow> = BTreeMap::new();
        for name in &ledgers {
            let Some(tb) = book.tb.get(name) else {
                continue;
            };
            if tb.closing_paise.checked_abs().ok_or_else(overflow)? > ACTIVE_TOL_PAISE {
                active.insert(name, tb);
                if tb.debit_paise == 0 && tb.credit_paise == 0 {
                    stale.insert(name, tb);
                }
            }
        }
        let sum = |rows: &BTreeMap<&String, &TbRow>| -> Result<i64> {
            rows.values().try_fold(0i64, |acc, t| {
                acc.checked_add(t.closing_paise).ok_or_else(overflow)
            })
        };

        r.fig(
            &format!("{role}_ledger_count"),
            support::count(TEST_ID, ledgers.len())?,
            Unit::Count,
            &format!("Ledgers under Tally's '{group}' group."),
            Vec::new(),
        );
        r.fig(
            &format!("{role}_active_count"),
            support::count(TEST_ID, active.len())?,
            Unit::Count,
            &format!(
                "Ledgers under Tally's '{group}' group whose Trial Balance closing balance, taken \
without its sign, is over ₹{}.",
                ACTIVE_TOL_PAISE / 100
            ),
            ledger_refs(active.keys().copied()),
        );
        r.fig(
            &format!("{role}_active_closing_total_paise"),
            Value::Int(sum(&active)?),
            Unit::Paise,
            &format!(
                "Sum of Trial Balance closing balances across the '{group}' ledgers whose closing \
balance, taken without its sign, is over ₹{}.",
                ACTIVE_TOL_PAISE / 100
            ),
            Vec::new(),
        );
        let f_stale_count = r.fig(
            &format!("{role}_stale_count"),
            support::count(TEST_ID, stale.len())?,
            Unit::Count,
            &format!(
                "'{group}' ledgers whose closing balance, taken without its sign, is over ₹{} and \
whose Trial Balance period debit and credit movement are both nil for the year (opening equal to \
closing; no voucher touched them).",
                ACTIVE_TOL_PAISE / 100
            ),
            ledger_refs(stale.keys().copied()),
        );
        let f_stale_total = r.fig(
            &format!("{role}_stale_closing_total_paise"),
            Value::Int(sum(&stale)?),
            Unit::Paise,
            &format!(
                "Sum of Trial Balance closing balances across the '{group}' ledgers whose closing \
balance, taken without its sign, is over ₹{} and whose period debit and credit movement are both nil.",
                ACTIVE_TOL_PAISE / 100
            ),
            Vec::new(),
        );
        for (name, t) in &stale {
            let h = stable_ledger_tag(book, name)?;
            r.fig(
                &format!("{role}_unmoved_ledger_{h}"),
                Value::Int(t.closing_paise),
                Unit::Paise,
                &format!(
                    "TB closing balance of one stale {role} ledger (tag {h}), unchanged all year."
                ),
                vec![EvidenceRef::new("ledger", name)],
            );
        }

        if stale.is_empty() {
            continue;
        }
        let facts = vec![
            ("stale_count".to_string(), f_stale_count),
            ("stale_closing_total".to_string(), f_stale_total),
        ];
        let evidence = ledger_refs(stale.keys().copied());
        if role == "creditor" {
            r.findings.push(Finding {
                id: format!("{TEST_ID}/s41_1/{role}"),
                clauses: vec!["s.41(1)".to_string(), "3CD-25".to_string()],
                title: format!(
                    "{} trade-creditor ledgers have an unchanged balance (no voucher posted) for \
the whole year",
                    stale.len()
                ),
                facts,
                evidence,
                confidence: Confidence::JudgementRequired,
                limits: vec![
                    "The books here cover only this financial year; s.41(1) turns on whether the \
liability has, in substance, ceased to exist or been remitted -- which needs prior-year Trial \
Balances (to see how long the balance has genuinely sat unchanged) that this engagement does not \
carry."
                        .to_string(),
                    "A stale balance is equally consistent with a valid, still-payable liability \
(a dormant but genuine trade balance, or a related-party loan) as with a ceased one; this test \
draws no conclusion either way."
                        .to_string(),
                ],
                ask_client: vec![
                    "For each ledger above, confirm how long the balance has been outstanding \
(prior years, not just this one) and whether any liability has been written back, waived, or \
otherwise remitted or ceased."
                        .to_string(),
                ],
            });
        } else {
            r.findings.push(Finding {
                id: format!("{TEST_ID}/recoverability/{role}"),
                clauses: Vec::new(),
                title: format!(
                    "{} debtor ledgers have an unchanged balance (no voucher posted) for the \
whole year",
                    stale.len()
                ),
                facts,
                evidence,
                confidence: Confidence::JudgementRequired,
                limits: vec![
                    "An unmoved debtor balance is a recoverability question (s.36(1)(vii)/(2) if \
later written off), not itself evidence of anything wrong; the books here cover only this \
financial year."
                        .to_string(),
                ],
                ask_client: vec![
                    "Confirm the recoverability of each balance above, and whether any has since \
been written off or is expected to be."
                        .to_string(),
                ],
            });
        }
    }
    Ok(r)
}

/// STL-1, independent of [`run`]: a ledger reported as `<role>_unmoved_ledger_<tag>` (TB debit
/// and credit movement both nil) must have no books-population voucher line posting a nonzero
/// amount to it, re-derived by walking the population directly.
pub fn check_invariants(book: &Book, result: &TestResult) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let tag_to_name = support::ledgers_by_tag(book)?;
    let mut touched: BTreeSet<&str> = BTreeSet::new();
    for v in book.population()? {
        for l in &v.lines {
            if l.amount_paise != 0 {
                touched.insert(l.ledger.as_str());
            }
        }
    }
    let mut ids: Vec<&str> = result.figures.iter().map(|f| f.id.as_str()).collect();
    ids.sort_unstable();
    for role in ["debtor", "creditor"] {
        let marker = format!("{}.{role}_unmoved_ledger_", result.test_id);
        for fid in &ids {
            let Some(tag) = fid.strip_prefix(marker.as_str()) else {
                continue;
            };
            match tag_to_name.get(tag) {
                None => out.push(format!(
                    "STL-1: cannot resolve a {role} ledger for figure {fid} (tag {tag})"
                )),
                Some(name) if touched.contains(name.as_str()) => out.push(format!(
                    "STL-1: {name} reported as an unmoved {role} (TB debit/credit both nil) but \
at least one books-population voucher line posts a nonzero amount to it"
                )),
                Some(_) => {}
            }
        }
    }
    Ok(out)
}
