//! Ledger scrutiny: the entry-level indicators a CA reviews across every expense ledger -- large
//! single entries, round-sum entries, entries on the last days of the year, the share paid in
//! cash, ledgers whose whole-year movement is journal-only, and ledgers that close with a
//! contra-nature (credit) balance. A port of the reference Python implementation's
//! `ledger_scrutiny` test module, version 1.
//!
//! Scope is every ledger under Tally's own `Direct Expenses` or `Indirect Expenses` groups. One
//! entry is one population voucher's own line(s) on one ledger, summed to a single net Dr+/Cr-
//! amount. Every figure is a plain books fact; every finding is indicative only.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use bridge_tally_primitives::TallyDate;

use crate::book::{Book, Voucher};
use crate::depreciation::civil_day_number;
use crate::error::{AuditError, Result};
use crate::findings::{pct_bp, Confidence, EvidenceRef, Finding, TestResult, Unit, Value};
use crate::ledger_ids::stable_ledger_tag;
use crate::read::{iso, Window};
use crate::rules::Rules;

pub const TEST_ID: &str = "ledger_scrutiny";
pub const VERSION: &str = "1";

const DIRECT_EXPENSES_GROUP: &str = "Direct Expenses";
const INDIRECT_EXPENSES_GROUP: &str = "Indirect Expenses";
const JOURNAL_BASE_TYPE: &str = "Journal";
/// Rs 1,000: this test's own analytical definition of a round sum, not tax law.
const ROUND_SUM_MULTIPLE_PAISE: i64 = 100_000;
/// The last 7 days of the period, inclusive of period end.
const LAST_DAYS_OF_YEAR_WINDOW: i64 = 7;
/// 30%: below this, the cash-share figure is shown but not flagged.
const CASH_SHARE_NOTABLE_BP: i64 = 3000;
/// LSC-1's tolerance, Re 1.
const LSC1_TOL_PAISE: i64 = 100;

type Entries<'a> = BTreeMap<String, (&'a Voucher, i64)>;

#[derive(Default)]
struct Row<'a> {
    entries: Entries<'a>,
    large: Entries<'a>,
    round_sum: Entries<'a>,
    last_days: Entries<'a>,
    cash_paid: Entries<'a>,
    journal: BTreeSet<String>,
    non_journal: BTreeSet<String>,
}

fn guid_tail12(guid: &str) -> &str {
    let cut = guid.len().saturating_sub(12);
    &guid[cut..]
}

fn voucher_label(v: &Voucher) -> String {
    let num = if v.number.is_empty() {
        guid_tail12(&v.guid)
    } else {
        v.number.as_str()
    };
    format!("{} {} on {}", v.vtype, num, iso(&v.date))
}

fn evidence(entries: &Entries) -> Vec<EvidenceRef> {
    entries
        .iter()
        .map(|(g, (v, _))| EvidenceRef::with_label("voucher", g, &voucher_label(v)))
        .collect()
}

/// The first day of the last-days window: the period end less six days.
fn last_days_start(period: &Window) -> Result<TallyDate> {
    let span = civil_day_number(&period.to) - civil_day_number(&period.from);
    let offset = u32::try_from(span - (LAST_DAYS_OF_YEAR_WINDOW - 1)).map_err(|_| {
        AuditError::Config(format!(
            "{TEST_ID}: the period is shorter than the {LAST_DAYS_OF_YEAR_WINDOW}-day window"
        ))
    })?;
    period
        .from
        .add_days(offset)
        .map_err(|e| AuditError::Config(format!("{TEST_ID}: window start: {e}")))
}

fn plural_entry(n: usize) -> &'static str {
    if n == 1 {
        "y"
    } else {
        "ies"
    }
}

pub fn run(
    book: &Book,
    rules: &Rules,
    period: &Window,
    cash: &BTreeSet<String>,
) -> Result<TestResult> {
    let mut r = TestResult::new(TEST_ID, VERSION, &rules.version);
    r.population_note = "Books population (optional, cancelled and post-dated vouchers \
excluded). One entry = one voucher's own line(s) on one ledger, summed to a single net Dr+/Cr- \
amount per voucher."
        .to_string();
    let overflow = || AuditError::Config(format!("{TEST_ID}: a total overflowed i64 paise"));
    let large_entry_paise = rules.ledger_scrutiny_large_entry_paise;
    let start = last_days_start(period)?;

    r.fig(
        "large_entry_threshold_paise",
        Value::Int(large_entry_paise),
        Unit::Paise,
        "Threshold used for the large-single-entry indicator (rules/ay2026-27.toml \
[ledger_scrutiny].large_entry_paise).",
        Vec::new(),
    );
    let expense_ledgers = book.ledgers_under_any(&[
        DIRECT_EXPENSES_GROUP.to_string(),
        INDIRECT_EXPENSES_GROUP.to_string(),
    ]);

    let mut rows: BTreeMap<&str, Row> = expense_ledgers
        .iter()
        .map(|n| (n.as_str(), Row::default()))
        .collect();
    for v in book.population()? {
        let mut ledger_lines: BTreeMap<&str, i64> = BTreeMap::new();
        for l in &v.lines {
            if expense_ledgers.contains(&l.ledger) {
                let e = ledger_lines.entry(l.ledger.as_str()).or_insert(0);
                *e = e.checked_add(l.amount_paise).ok_or_else(overflow)?;
            }
        }
        if ledger_lines.is_empty() {
            continue;
        }
        let has_cash_leg = v
            .lines
            .iter()
            .any(|l| cash.contains(&l.ledger) && l.amount_paise != 0);
        for (name, net) in ledger_lines {
            let row = rows.get_mut(name).expect("every expense ledger has a row");
            if v.base_type == JOURNAL_BASE_TYPE {
                row.journal.insert(v.guid.clone());
            } else {
                row.non_journal.insert(v.guid.clone());
            }
            if net == 0 {
                continue;
            }
            let abs = net.checked_abs().ok_or_else(overflow)?;
            row.entries.insert(v.guid.clone(), (v, net));
            if abs > large_entry_paise {
                row.large.insert(v.guid.clone(), (v, net));
            }
            if abs % ROUND_SUM_MULTIPLE_PAISE == 0 {
                row.round_sum.insert(v.guid.clone(), (v, net));
            }
            if v.date >= start {
                row.last_days.insert(v.guid.clone(), (v, net));
            }
            if net > 0 && has_cash_leg {
                row.cash_paid.insert(v.guid.clone(), (v, net));
            }
        }
    }

    r.fig(
        "expense_ledger_count",
        Value::Int(i64::try_from(expense_ledgers.len()).map_err(|_| overflow())?),
        Unit::Count,
        &format!(
            "Ledgers under Tally's '{DIRECT_EXPENSES_GROUP}' or '{INDIRECT_EXPENSES_GROUP}' groups."
        ),
        Vec::new(),
    );

    let sum = |e: &Entries| -> Result<i64> {
        e.values()
            .try_fold(0i64, |acc, (_, n)| acc.checked_add(*n).ok_or_else(overflow))
    };
    let count =
        |n: usize| -> Result<Value> { Ok(Value::Int(i64::try_from(n).map_err(|_| overflow())?)) };
    let mut flagged: i64 = 0;
    for (name, d) in &rows {
        if d.entries.is_empty() {
            continue;
        }
        let h = stable_ledger_tag(book, name)?;
        let ledger_ev = vec![EvidenceRef::new("ledger", name)];

        let total_debit = d
            .entries
            .values()
            .filter(|(_, n)| *n > 0)
            .try_fold(0i64, |acc, (_, n)| acc.checked_add(*n).ok_or_else(overflow))?;
        let f_total = r.fig(
            &format!("total_debit_paise_{h}"),
            Value::Int(total_debit),
            Unit::Paise,
            &format!("Sum of positive (debit/expense) population entries on one ledger (tag {h})."),
            ledger_ev.clone(),
        );
        let f_large_count = r.fig(
            &format!("large_entry_count_{h}"),
            count(d.large.len())?,
            Unit::Count,
            &format!(
                "Entries on this ledger (tag {h}) with abs(net amount) over the large-entry \
threshold."
            ),
            evidence(&d.large),
        );
        let f_large_total = r.fig(
            &format!("large_entry_total_paise_{h}"),
            Value::Int(sum(&d.large)?),
            Unit::Paise,
            &format!("Sum of net amounts across the large entries above (tag {h})."),
            Vec::new(),
        );
        let f_round_count = r.fig(
            &format!("round_sum_count_{h}"),
            count(d.round_sum.len())?,
            Unit::Count,
            &format!(
                "Entries on this ledger (tag {h}) whose net amount is an exact, nonzero multiple \
of \u{20b9}1,000."
            ),
            evidence(&d.round_sum),
        );
        let f_round_total = r.fig(
            &format!("round_sum_total_paise_{h}"),
            Value::Int(sum(&d.round_sum)?),
            Unit::Paise,
            &format!("Sum of net amounts across the round-sum entries above (tag {h})."),
            Vec::new(),
        );
        let f_last_count = r.fig(
            &format!("last_days_count_{h}"),
            count(d.last_days.len())?,
            Unit::Count,
            &format!(
                "Entries on this ledger (tag {h}) dated in the last {LAST_DAYS_OF_YEAR_WINDOW} \
days of the period ({} to {}).",
                iso(&start),
                iso(&period.to)
            ),
            evidence(&d.last_days),
        );
        let f_last_total = r.fig(
            &format!("last_days_total_paise_{h}"),
            Value::Int(sum(&d.last_days)?),
            Unit::Paise,
            &format!("Sum of net amounts across the last-days entries above (tag {h})."),
            Vec::new(),
        );
        let cash_paid = sum(&d.cash_paid)?;
        let cash_share_bp: Option<i64> = if total_debit != 0 {
            match pct_bp(cash_paid, total_debit) {
                Some(Value::Int(v)) => Some(v),
                _ => return Err(overflow()),
            }
        } else {
            None
        };
        let f_cash_paid = r.fig(
            &format!("cash_paid_paise_{h}"),
            Value::Int(cash_paid),
            Unit::Paise,
            &format!(
                "Of this ledger's (tag {h}) debit entries, the sum where the same voucher also \
carries a nonzero line on a configured cash ledger."
            ),
            evidence(&d.cash_paid),
        );
        let f_cash_share = r.fig(
            &format!("cash_share_bp_{h}"),
            Value::Int(cash_share_bp.unwrap_or(0)),
            Unit::BasisPoints,
            &format!(
                "cash_paid_paise_{h} / total_debit_paise_{h} (0 when the ledger has no debit \
entries at all)."
            ),
            Vec::new(),
        );
        let journal_only = !d.journal.is_empty() && d.non_journal.is_empty();
        let f_journal_only = r.fig(
            &format!("journal_only_{h}"),
            Value::Text(if journal_only { "yes" } else { "no" }.to_string()),
            Unit::Text,
            &format!(
                "Whether every population voucher touching this ledger (tag {h}) has base type \
'{JOURNAL_BASE_TYPE}' (and at least one does)."
            ),
            Vec::new(),
        );
        let closing = book.tb.get(*name).map_or(0, |t| t.closing_paise);
        let contra_nature = closing < 0;
        let f_closing = r.fig(
            &format!("closing_paise_{h}"),
            Value::Int(closing),
            Unit::Paise,
            &format!("TB closing balance of this ledger (tag {h}), Dr+/Cr-."),
            ledger_ev.clone(),
        );
        let f_contra = r.fig(
            &format!("contra_nature_closing_{h}"),
            Value::Text(if contra_nature { "yes" } else { "no" }.to_string()),
            Unit::Text,
            &format!(
                "Whether this expense ledger's (tag {h}) TB closing balance is a credit (unusual \
on an expense ledger)."
            ),
            Vec::new(),
        );

        let mut flags: Vec<String> = Vec::new();
        if !d.large.is_empty() {
            let n = d.large.len();
            flags.push(format!("{n} large single entr{}", plural_entry(n)));
        }
        if !d.round_sum.is_empty() {
            let n = d.round_sum.len();
            flags.push(format!("{n} round-sum entr{}", plural_entry(n)));
        }
        if !d.last_days.is_empty() {
            let n = d.last_days.len();
            flags.push(format!(
                "{n} entr{} in the last {LAST_DAYS_OF_YEAR_WINDOW} days of the period",
                plural_entry(n)
            ));
        }
        if let Some(bp) = cash_share_bp {
            if bp >= CASH_SHARE_NOTABLE_BP {
                #[allow(clippy::cast_precision_loss)] // basis points of a ratio: far below 2^52
                flags.push(format!("cash share {:.2}%", bp as f64 / 100.0));
            }
        }
        if journal_only {
            flags.push("moved only by journal voucher all year".to_string());
        }
        if contra_nature {
            flags.push("closes with a contra-nature (credit) balance".to_string());
        }
        if flags.is_empty() {
            continue;
        }
        flagged += 1;
        let mut ev = ledger_ev;
        ev.extend(evidence(&d.entries));
        r.findings.push(Finding {
            id: format!("{TEST_ID}/flags/{h}"),
            clauses: Vec::new(),
            title: format!(
                "Ledger scrutiny indicators on one expense ledger: {}",
                flags.join("; ")
            ),
            facts: [
                ("total_debit", f_total),
                ("large_entry_count", f_large_count),
                ("large_entry_total", f_large_total),
                ("round_sum_count", f_round_count),
                ("round_sum_total", f_round_total),
                ("last_days_count", f_last_count),
                ("last_days_total", f_last_total),
                ("cash_paid", f_cash_paid),
                ("cash_share_bp", f_cash_share),
                ("journal_only", f_journal_only),
                ("closing", f_closing),
                ("contra_nature", f_contra),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect(),
            evidence: ev,
            confidence: Confidence::Indicative,
            limits: vec![
                "Each indicator is a structural, entry-level fact only; it is not, by itself, \
evidence of a wrong classification, a disallowable expense or a TDS default -- the ledger's own \
name and nature still need the CA's own reading, never a keyword match on the ledger name."
                    .to_string(),
                "A round or large entry, one near year end, or a cash-paid share, can all have an \
entirely ordinary explanation (a lump annual payment, a year-end provision, a genuinely \
cash-heavy trade)."
                    .to_string(),
            ],
            ask_client: vec![
                "For each flagged ledger, confirm the nature and supporting document for the \
large/round-sum/last-days entries listed."
                    .to_string(),
            ],
        });
    }
    r.fig(
        "flagged_ledger_count",
        Value::Int(flagged),
        Unit::Count,
        "Expense ledgers (of expense_ledger_count) with at least one scrutiny indicator triggered \
above.",
        Vec::new(),
    );
    Ok(r)
}

/// LSC-1, independent of [`run`]: a ledger's population-walk `total_debit_paise` can never
/// exceed its own TB period debit movement (the TB covers every exported voucher, the walk a
/// subset of them), within Re 1.
pub fn check_invariants(book: &Book, result: &TestResult) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let mut tag_to_name: HashMap<String, &String> = HashMap::new();
    for name in book.ledgers.keys() {
        tag_to_name.insert(stable_ledger_tag(book, name)?, name);
    }
    let marker = format!("{}.total_debit_paise_", result.test_id);
    let mut figs: Vec<_> = result
        .figures
        .iter()
        .filter(|f| f.id.starts_with(marker.as_str()))
        .collect();
    figs.sort_by(|a, b| a.id.cmp(&b.id));
    for f in figs {
        let tag = &f.id[marker.len()..];
        let Some(name) = tag_to_name.get(tag) else {
            out.push(format!(
                "LSC-1: cannot resolve an expense ledger for figure {} (tag {tag})",
                f.id
            ));
            continue;
        };
        let tb_debit = book.tb.get(*name).map_or(0, |t| t.debit_paise);
        let Value::Int(reported) = f.value else {
            continue;
        };
        if i128::from(reported) > i128::from(tb_debit) + i128::from(LSC1_TOL_PAISE) {
            out.push(format!(
                "LSC-1: {name} population-walk total_debit_paise ({reported}p) exceeds its own TB \
period debit movement ({tb_debit}p); the population walk is a subset of the TB and cannot exceed \
it"
            ));
        }
    }
    Ok(out)
}
