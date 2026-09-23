//! Cash share of receipts and payments for the proviso to s.44AB(a) (Form 3CD clause 8
//! context). A port of the reference Python implementation's `cash_44ab` test module, version 1.
//!
//! Cash is the ledgers under the engagement's cash groups, bank the ledgers under its bank
//! groups; Contra vouchers are excluded from both legs; each share is cash / (cash + bank),
//! receipts and payments separately, in basis points.

use std::collections::BTreeSet;

use crate::book::Book;
use crate::error::{AuditError, Result};
use crate::findings::{pct_bp, Confidence, EvidenceRef, Finding, TestResult, Unit, Value};
use crate::rules::Rules;

pub const TEST_ID: &str = "cash_44ab";
pub const VERSION: &str = "1";

const POPULATION: &str =
    "Books population (optional, cancelled and post-dated vouchers excluded); Contra excluded";

pub fn run(
    book: &Book,
    rules: &Rules,
    cash: &BTreeSet<String>,
    bank: &BTreeSet<String>,
) -> Result<TestResult> {
    let mut r = TestResult::new(TEST_ID, VERSION, &rules.version);
    let (mut cr, mut cp, mut br, mut bp) = (0i64, 0i64, 0i64, 0i64);
    let overflow = || AuditError::Config("cash_44ab: a total overflowed i64 paise".to_string());
    for v in book.population()? {
        if v.base_type == "Contra" {
            continue;
        }
        for l in &v.lines {
            let a = l.amount_paise;
            let slot = if cash.contains(&l.ledger) {
                if a > 0 {
                    &mut cr
                } else {
                    &mut cp
                }
            } else if bank.contains(&l.ledger) {
                if a > 0 {
                    &mut br
                } else {
                    &mut bp
                }
            } else {
                continue;
            };
            *slot = slot.checked_add(a.abs()).ok_or_else(overflow)?;
        }
    }
    r.population_note = POPULATION.to_string();
    let cash_ev: Vec<EvidenceRef> = cash.iter().map(|x| EvidenceRef::new("ledger", x)).collect();
    let bank_ev: Vec<EvidenceRef> = bank.iter().map(|x| EvidenceRef::new("ledger", x)).collect();
    let def = |what: &str| format!("{what}. {POPULATION}");
    let cash_receipts_id = r.fig(
        "cash_receipts",
        Value::Int(cr),
        Unit::Paise,
        &def("Debits to cash ledgers"),
        cash_ev.clone(),
    );
    r.fig(
        "cash_payments",
        Value::Int(cp),
        Unit::Paise,
        &def("Credits to cash ledgers"),
        cash_ev.clone(),
    );
    r.fig(
        "bank_receipts",
        Value::Int(br),
        Unit::Paise,
        &def("Debits to bank ledgers"),
        bank_ev.clone(),
    );
    r.fig(
        "bank_payments",
        Value::Int(bp),
        Unit::Paise,
        &def("Credits to bank ledgers"),
        bank_ev.clone(),
    );
    let total = |a: i64, b: i64| a.checked_add(b).ok_or_else(overflow);
    let rec_bp = pct_bp(cr, total(cr, br)?).ok_or_else(overflow)?;
    let pay_bp = pct_bp(cp, total(cp, bp)?).ok_or_else(overflow)?;
    let share_receipts_id = r.fig(
        "cash_share_receipts",
        rec_bp.clone(),
        Unit::BasisPoints,
        "Cash receipts as a share of cash and bank receipts together.",
        Vec::new(),
    );
    let share_payments_id = r.fig(
        "cash_share_payments",
        pay_bp.clone(),
        Unit::BasisPoints,
        "Cash payments as a share of cash and bank payments together.",
        Vec::new(),
    );
    let lim = rules.cash_share_limit_bp;
    let within = matches!((&rec_bp, &pay_bp), (Value::Int(rec), Value::Int(pay)) if *rec <= lim && *pay <= lim);
    let thr = if within {
        rules.turnover_threshold_low_cash_paise
    } else {
        rules.turnover_threshold_paise
    };
    let threshold_id = r.fig(
        "applicable_threshold",
        Value::Int(thr),
        Unit::Paise,
        "₹10 crore if both cash shares are within 5%, else ₹1 crore",
        vec![EvidenceRef::new("rule", "s44ab")],
    );
    // Turnover is not supplied in this slice (the reference dump passes none), so the two
    // turnover figures and their facts are never emitted.
    let facts = vec![
        ("cash_share_receipts".to_string(), share_receipts_id),
        ("cash_share_payments".to_string(), share_payments_id),
        ("applicable_threshold".to_string(), threshold_id),
        ("cash_receipts".to_string(), cash_receipts_id),
    ];
    r.findings.push(Finding {
        id: format!("{TEST_ID}/applicability"),
        clauses: vec!["s.44AB(a)".to_string(), "3CD-8".to_string()],
        title: "Cash share of receipts and payments on the books".to_string(),
        facts,
        evidence: cash_ev.into_iter().chain(bank_ev).collect(),
        confidence: Confidence::NeedsDocument,
        limits: vec![
            "Books only: a cheque or draft that is not account-payee counts as cash and is not visible in Tally.".to_string(),
            "Bank ledgers are taken as recorded; full-year bank statements are needed to confirm.".to_string(),
        ],
        ask_client: vec!["Full-year bank statements for every account".to_string()],
    });
    Ok(r)
}
