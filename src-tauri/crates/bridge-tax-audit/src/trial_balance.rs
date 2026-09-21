//! Trial Balance listing with each ledger's Tally group chain: the grouping schedule a CA reviews
//! before reading any financial statement built from it. A port of the reference Python
//! implementation's `trial_balance` test module, version 1.
//!
//! A plain restatement of Tally's own Trial Balance export -- no figure is derived except the
//! column totals. One row per ledger that carries any opening, movement or closing: its group
//! chain (primary group first), opening, debit, credit and closing (debit positive). Ledger names
//! are shown as they are in the books; whether a name fits its group is the CA's reading.

use crate::book::Book;
use crate::error::{AuditError, Result};
use crate::findings::{EvidenceRef, TestResult, Unit, Value};
use crate::ledger_ids::stable_ledger_tag;
use crate::rules::Rules;
use crate::xml::reserved_value;

pub const TEST_ID: &str = "trial_balance";
pub const VERSION: &str = "1";

/// How Tally itself shows a name: a reserved value without its marker (`&#4; Primary` shows as
/// `Primary`), any other text unchanged -- the reference's `model.display_name`. For rendering
/// only.
fn display_name(text: &str) -> &str {
    reserved_value(text).unwrap_or(text)
}

/// The reference's sort key: the ledger's chain as Tally shows it, primary group first, then the
/// name. A ledger absent from the masters sorts under `~`.
fn sort_key<'a>(book: &'a Book, name: &'a str) -> (Vec<&'a str>, &'a str) {
    let chain = match book.ledgers.get(name) {
        Some(l) => l.chain.iter().rev().map(|g| display_name(g)).collect(),
        None => vec!["~"],
    };
    (chain, name)
}

pub fn run(book: &Book, rules: &Rules) -> Result<TestResult> {
    let mut r = TestResult::new(TEST_ID, VERSION, &rules.version);
    r.population_note =
        "Tally's own Trial Balance export for the period; no voucher is re-read here.".to_string();
    let overflow = || AuditError::Config("trial_balance: a total overflowed i64 paise".to_string());
    let (mut opening, mut debit, mut credit, mut closing) = (0i64, 0i64, 0i64, 0i64);
    let mut rows = 0i64;
    let mut names: Vec<&str> = book.tb.keys().map(String::as_str).collect();
    names.sort_by(|a, b| sort_key(book, a).cmp(&sort_key(book, b)));
    for name in names {
        let t = &book.tb[name];
        if t.opening_paise == 0 && t.debit_paise == 0 && t.credit_paise == 0 && t.closing_paise == 0
        {
            continue;
        }
        rows += 1;
        let h = stable_ledger_tag(book, name)?;
        let chain = match book.ledgers.get(name) {
            Some(l) if !l.chain.is_empty() => l
                .chain
                .iter()
                .rev()
                .map(|g| display_name(g))
                .collect::<Vec<_>>()
                .join(" > "),
            _ => "(group not in the masters)".to_string(),
        };
        let ev = || vec![EvidenceRef::with_label("ledger", name, name)];
        r.fig(
            &format!("tb_group_{h}"),
            Value::Text(chain),
            Unit::Text,
            "The ledger's Tally group chain, primary group first.",
            ev(),
        );
        r.fig(
            &format!("tb_opening_{h}"),
            Value::Int(t.opening_paise),
            Unit::Paise,
            "Opening balance per the Trial Balance (Dr positive).",
            ev(),
        );
        r.fig(
            &format!("tb_debit_{h}"),
            Value::Int(t.debit_paise),
            Unit::Paise,
            "Debits in the period per the Trial Balance.",
            ev(),
        );
        r.fig(
            &format!("tb_credit_{h}"),
            Value::Int(t.credit_paise),
            Unit::Paise,
            "Credits in the period per the Trial Balance.",
            ev(),
        );
        r.fig(
            &format!("tb_closing_{h}"),
            Value::Int(t.closing_paise),
            Unit::Paise,
            "Closing balance per the Trial Balance (Dr positive).",
            ev(),
        );
        opening = opening.checked_add(t.opening_paise).ok_or_else(overflow)?;
        debit = debit.checked_add(t.debit_paise).ok_or_else(overflow)?;
        credit = credit.checked_add(t.credit_paise).ok_or_else(overflow)?;
        closing = closing.checked_add(t.closing_paise).ok_or_else(overflow)?;
    }
    r.fig(
        "ledger_rows_count",
        Value::Int(rows),
        Unit::Count,
        "Ledgers with any opening, movement or closing in the period.",
        Vec::new(),
    );
    r.fig(
        "total_opening",
        Value::Int(opening),
        Unit::Paise,
        "Sum of openings (Dr positive); non-zero is Tally's 'Difference in opening balances'.",
        Vec::new(),
    );
    r.fig(
        "total_debit",
        Value::Int(debit),
        Unit::Paise,
        "Sum of the debit column as Tally exports it (a Stock-in-Hand ledger's column can carry \
stock values that do not move its closing, so this need not equal the credit column).",
        Vec::new(),
    );
    r.fig(
        "total_credit",
        Value::Int(credit),
        Unit::Paise,
        "Sum of the credit column as Tally exports it.",
        Vec::new(),
    );
    r.fig(
        "total_closing",
        Value::Int(closing),
        Unit::Paise,
        "Sum of closings (Dr positive); equals the opening difference when the period's entries \
balance.",
        Vec::new(),
    );
    Ok(r)
}

/// TB-1 and TB-2, independent of [`run`]: each listed row equals the Trial Balance export, and
/// the period's movements (closing less opening) of the listed rows sum to zero. Not "debits
/// equal credits": Tally's export can carry a stock value in a Stock-in-Hand ledger's debit
/// column without moving its closing.
pub fn check_invariants(book: &Book, result: &TestResult) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let mut moved: i64 = 0;
    let overflow = || AuditError::Config("trial_balance: TB-2 overflowed i64 paise".to_string());
    let ints: std::collections::HashMap<&str, i64> = result
        .figures
        .iter()
        .filter_map(|f| match f.value {
            Value::Int(v) => Some((f.id.as_str(), v)),
            _ => None,
        })
        .collect();
    let int = |fid: String| -> Option<i64> { ints.get(fid.as_str()).copied() };
    for (name, t) in &book.tb {
        let h = stable_ledger_tag(book, name)?;
        let closing = int(format!("{TEST_ID}.tb_closing_{h}"));
        if let Some(c) = closing {
            if c != t.closing_paise {
                out.push(format!("TB-1: closing for ledger tag {h} != Trial Balance"));
            }
        }
        if let (Some(c), Some(o)) = (closing, int(format!("{TEST_ID}.tb_opening_{h}"))) {
            let m = c.checked_sub(o).ok_or_else(overflow)?;
            moved = moved.checked_add(m).ok_or_else(overflow)?;
        }
    }
    if moved != 0 {
        out.push(format!(
            "TB-2: the period's movements (closing less opening) sum to {moved} paise, not zero"
        ));
    }
    Ok(out)
}
