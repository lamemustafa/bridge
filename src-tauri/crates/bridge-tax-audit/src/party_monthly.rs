// SPDX-License-Identifier: Apache-2.0
//! Port of the reference engine's `party_monthly`: sales, purchases and expenses by party and
//! month, a working-paper analysis of who the business traded with, and when.
//!
//! * Four blocks by group ancestry (Sales Accounts, Purchase Accounts, Direct Expenses, Indirect
//!   Expenses). A block whose group the book does not carry publishes nothing and raises a finding.
//! * A voucher's amount is its lines on the block's ledgers, business-signed (sales negated). Its
//!   party is its one line under Sundry Debtors or Creditors; several such lines are "Several
//!   parties"; none is "Cash or bank (no party)" for sales and purchases with a cash or bank line,
//!   else "No party". Parties are per ledger, never merged.
//! * Month columns only for an April-to-March period; a voucher dated outside the period is in its
//!   own column, not the year. Credit and debit notes are also shown as their own column.
//! * The top `top_n` parties by absolute year total are shown by name, the rest as one "Others"
//!   row, then the fixed rows and a total. A nil cell publishes no figure.
//! * Each block's total is set beside the Trial Balance's period movement on its ledgers. A
//!   difference over a rupee is a finding, which names excluded vouchers only when exactly one set
//!   of them (one status, or all together) matches it.
//! * Vouchers are counted as vouchers, never by GUID: a blank or repeated GUID never merges two.
//!
//! A figure id the reference would repeat (two ledgers sharing a tag) is refused with an error, as
//! the reference's `fig` raises, never a panic (#644).

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};

use bridge_tally_primitives::TallyDate;

use crate::book::{Book, Voucher, VoucherStatus};
use crate::error::{AuditError, Result};
use crate::findings::{Confidence, EvidenceRef, Finding, TestResult, Unit, Value};
use crate::ledger_ids::stable_ledger_tag;
use crate::read::Window;
use crate::rules::Rules;
use crate::support::{count, hash8, py_repr_str, voucher_label};

pub const TEST_ID: &str = "party_monthly";
pub const VERSION: &str = "1";

/// The number of parties shown by name per block; the rest are one "Others" row.
pub const PARTY_TOP_N: usize = 50;

/// (block, primary group, sign applied to Tally's debit-positive line amount).
const BLOCKS: [(&str, &str, i64); 4] = [
    ("sales", "Sales Accounts", -1),
    ("purchases", "Purchase Accounts", 1),
    ("direct_expenses", "Direct Expenses", 1),
    ("indirect_expenses", "Indirect Expenses", 1),
];
const PARTY_GROUPS: [&str; 2] = ["Sundry Debtors", "Sundry Creditors"];
const MONTHS: [&str; 12] = [
    "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec", "jan", "feb", "mar",
];
const MONTH_NAMES: [&str; 12] = [
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
    "January",
    "February",
    "March",
];
const NOTE_TYPES: [&str; 2] = ["Credit Note", "Debit Note"];
/// The excluded statuses in the reference's `STATUS_WORDS` order: (status, key, word).
const STATUSES: [(VoucherStatus, &str, &str); 3] = [
    (VoucherStatus::Postdated, "postdated", "post-dated"),
    (VoucherStatus::Optional, "optional", "optional"),
    (VoucherStatus::Cancelled, "cancelled", "cancelled"),
];
const TIE_TOLERANCE_PAISE: i64 = 100;

fn overflow() -> AuditError {
    crate::support::overflow(TEST_ID)
}

fn add(a: i64, b: i64) -> Result<i64> {
    a.checked_add(b).ok_or_else(overflow)
}

fn abs(a: i64) -> Result<i64> {
    a.checked_abs().ok_or_else(overflow)
}

/// Add a figure, refusing a repeated id as the reference's `fig` raises on one.
fn fig(
    r: &mut TestResult,
    name: &str,
    value: Value,
    unit: Unit,
    definition: &str,
    evidence: Vec<EvidenceRef>,
) -> Result<String> {
    let id = format!("{TEST_ID}.{name}");
    if r.figures.iter().any(|f| f.id == id) {
        return Err(AuditError::Config(format!(
            "{TEST_ID}: figure id {id} would repeat"
        )));
    }
    Ok(r.fig(name, value, unit, definition, evidence))
}

/// (year, month, day) of a validated `YYYYMMDD` date.
fn ymd(d: &TallyDate) -> (i32, u32, u32) {
    let s = d.as_str();
    let num = |a: usize, b: usize| s[a..b].parse::<u32>().unwrap_or(0);
    (i32::try_from(num(0, 4)).unwrap_or(0), num(4, 6), num(6, 8))
}

/// Whether the period runs 1 April to 31 March of the next year.
fn is_fy(start: &TallyDate, end: &TallyDate) -> bool {
    let ((sy, sm, sd), (ey, em, ed)) = (ymd(start), ymd(end));
    (sm, sd, em, ed) == (4, 1, 3, 31) && ey == sy + 1
}

/// The month column of a date inside an April-to-March period.
fn month_col(d: &TallyDate) -> &'static str {
    let m = ymd(d).1 as usize;
    MONTHS[(m + 8) % 12]
}

/// A row's key: a named party, or one of the fixed rows (ordered as the reference emits them).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Key {
    Party(String),
    CashBank,
    NoParty,
    Several,
}

#[derive(Default)]
struct Row {
    cols: BTreeMap<&'static str, i64>,
    year: i64,
    returns: i64,
    /// Population indices: a voucher is counted once, whatever its GUID.
    vouchers: BTreeSet<usize>,
}

impl Row {
    fn absorb(&mut self, other: &Row) -> Result<()> {
        for (c, a) in &other.cols {
            let cell = self.cols.entry(*c).or_insert(0);
            *cell = add(*cell, *a)?;
        }
        self.year = add(self.year, other.year)?;
        self.returns = add(self.returns, other.returns)?;
        self.vouchers.extend(other.vouchers.iter().copied());
        Ok(())
    }
}

fn row_hash(block: &str, key: &str) -> String {
    hash8(&format!("{block}:{key}"))
}

#[allow(clippy::too_many_lines)] // one pass per block, as the reference lays it out
pub fn run(
    book: &Book,
    rules: &Rules,
    period: &Window,
    cash: &BTreeSet<String>,
    bank: &BTreeSet<String>,
    top_n: usize,
) -> Result<TestResult> {
    let mut r = TestResult::new(TEST_ID, VERSION, &rules.version);
    let pop = book.population()?;
    let (start, end) = (&period.from, &period.to);
    r.population_note =
        "Vouchers in the books (optional, cancelled and post-dated vouchers excluded). \
Parties are per ledger: one person with two ledgers shows as two rows; nothing is merged."
            .to_string();
    let fy = is_fy(start, end);
    if !fy {
        r.findings.push(Finding {
            id: format!("{TEST_ID}/period_not_fy"),
            clauses: Vec::new(),
            title: "Month columns need an April-to-March period; only year totals are shown"
                .to_string(),
            facts: Vec::new(),
            evidence: Vec::new(),
            confidence: Confidence::Computed,
            limits: vec![
                "The period of these books does not run from 1 April to 31 March.".to_string(),
            ],
            ask_client: Vec::new(),
        });
    }
    let parties: BTreeSet<&str> = book
        .ledgers
        .values()
        .filter(|l| PARTY_GROUPS.iter().any(|g| l.under(g)))
        .map(|l| l.name.as_str())
        .collect();
    let in_period = |d: &TallyDate| start <= d && d <= end;

    for (block, group, sign) in BLOCKS {
        if !book.groups.contains_key(group) {
            r.findings.push(Finding {
                id: format!("{TEST_ID}/no_group/{block}"),
                clauses: Vec::new(),
                title: format!(
                    "The books carry no '{group}' group, so this block shows no figures"
                ),
                facts: Vec::new(),
                evidence: Vec::new(),
                confidence: Confidence::NeedsDocument,
                limits: vec!["A renamed or missing primary group is not guessed at.".to_string()],
                ask_client: vec![format!(
                    "Confirm which group holds the ledgers Tally reports as '{group}'."
                )],
            });
            continue;
        }
        let scope: BTreeSet<&str> = book
            .ledgers
            .values()
            .filter(|l| l.under(group))
            .map(|l| l.name.as_str())
            .collect();
        let sense = if sign < 0 {
            "credits less debits"
        } else {
            "debits less credits"
        };
        let signed_sum = |v: &Voucher| -> Result<i64> {
            let mut s = 0_i64;
            for l in v.lines.iter().filter(|l| scope.contains(l.ledger.as_str())) {
                s = add(s, l.amount_paise)?;
            }
            s.checked_mul(sign).ok_or_else(overflow)
        };

        let mut rows: BTreeMap<Key, Row> = BTreeMap::new();
        for (idx, v) in pop.iter().enumerate() {
            if !v.lines.iter().any(|l| scope.contains(l.ledger.as_str())) {
                continue;
            }
            let amount = signed_sum(v)?;
            let named: BTreeSet<&str> = v
                .lines
                .iter()
                .map(|l| l.ledger.as_str())
                .filter(|l| parties.contains(l))
                .collect();
            let key = if named.len() == 1 {
                Key::Party(named.iter().next().copied().unwrap_or_default().to_string())
            } else if !named.is_empty() {
                Key::Several
            } else if (block == "sales" || block == "purchases")
                && v.lines
                    .iter()
                    .any(|l| cash.contains(&l.ledger) || bank.contains(&l.ledger))
            {
                Key::CashBank
            } else {
                Key::NoParty
            };
            let row = rows.entry(key).or_default();
            if in_period(&v.date) {
                row.year = add(row.year, amount)?;
                if fy {
                    let cell = row.cols.entry(month_col(&v.date)).or_insert(0);
                    *cell = add(*cell, amount)?;
                }
                if NOTE_TYPES.contains(&v.base_type.as_str()) {
                    row.returns = add(row.returns, amount)?;
                }
            } else {
                let cell = row.cols.entry("outside").or_insert(0);
                *cell = add(*cell, amount)?;
            }
            row.vouchers.insert(idx);
        }

        // Named parties by absolute year total, largest first, ties by name.
        let mut named_rows: Vec<(i64, &String)> = Vec::new();
        for (k, row) in &rows {
            if let Key::Party(name) = k {
                named_rows.push((abs(row.year)?, name));
            }
        }
        named_rows.sort_by(|a, b| (Reverse(a.0), a.1).cmp(&(Reverse(b.0), b.1)));
        let cut = top_n.min(named_rows.len());
        let (shown, rest) = named_rows.split_at(cut);

        // (tag, row, evidence), in the reference's order.
        let mut ordered: Vec<(String, &Row, EvidenceRef, bool)> = Vec::new();
        for (_, name) in shown {
            let row = &rows[&Key::Party(name.to_string())];
            let tag = stable_ledger_tag(book, name)?;
            ordered.push((
                tag,
                row,
                EvidenceRef::with_label("ledger", name, name),
                false,
            ));
        }
        let mut others = Row::default();
        if !rest.is_empty() {
            for (_, name) in rest {
                others.absorb(&rows[&Key::Party(name.to_string())])?;
            }
        }
        let mut total = Row::default();
        for row in rows.values() {
            total.absorb(row)?;
        }
        if !rest.is_empty() {
            let n = rest.len();
            let label = format!("Others ({n} {})", if n == 1 { "party" } else { "parties" });
            ordered.push((
                row_hash(block, "others"),
                &others,
                EvidenceRef::with_label("row", &format!("{block}:others"), &label),
                false,
            ));
        }
        for (k, key, label) in [
            (Key::CashBank, "cash_bank", "Cash or bank (no party)"),
            (Key::NoParty, "no_party", "No party"),
            (Key::Several, "several", "Several parties"),
        ] {
            if let Some(row) = rows.get(&k) {
                ordered.push((
                    row_hash(block, key),
                    row,
                    EvidenceRef::with_label("row", &format!("{block}:{key}"), label),
                    false,
                ));
            }
        }
        ordered.push((
            row_hash(block, "total"),
            &total,
            EvidenceRef::with_label("row", &format!("{block}:total"), "Total"),
            true,
        ));

        for (h, row, label_ref, is_total) in &ordered {
            for (c, name) in MONTHS.iter().zip(MONTH_NAMES) {
                if let Some(a) = row.cols.get(c).filter(|a| **a != 0) {
                    fig(
                        &mut r,
                        &format!("{block}_{c}_{h}"),
                        Value::Int(*a),
                        Unit::Paise,
                        &format!("{group}: amount dated in {name} for this row ({sense})."),
                        vec![label_ref.clone()],
                    )?;
                }
            }
            if let Some(a) = row.cols.get("outside").filter(|a| **a != 0) {
                fig(&mut r, &format!("{block}_outside_{h}"), Value::Int(*a), Unit::Paise,
                    &format!("{group}: amount dated outside the period for this row ({sense}; not in the \
                              period total)."),
                    vec![label_ref.clone()])?;
            }
            if row.year != 0 || *is_total {
                fig(
                    &mut r,
                    &format!("{block}_year_{h}"),
                    Value::Int(row.year),
                    Unit::Paise,
                    &format!("{group}: amount dated inside the period for this row ({sense})."),
                    vec![label_ref.clone()],
                )?;
            }
            if row.returns != 0 {
                fig(
                    &mut r,
                    &format!("{block}_returns_{h}"),
                    Value::Int(row.returns),
                    Unit::Paise,
                    &format!(
                        "{group}: the part of this row's year amount on credit and debit notes."
                    ),
                    vec![label_ref.clone()],
                )?;
            }
            fig(
                &mut r,
                &format!("{block}_vouchers_{h}"),
                count(TEST_ID, row.vouchers.len())?,
                Unit::Count,
                &format!("{group}: vouchers in this row, including any dated outside the period."),
                vec![label_ref.clone()],
            )?;
        }

        // ---- the Trial Balance ----
        let mut tb_movement = 0_i64;
        let mut openings: BTreeMap<&str, i64> = BTreeMap::new();
        for (n, t) in &book.tb {
            if scope.contains(n.as_str()) {
                let mv = t
                    .debit_paise
                    .checked_sub(t.credit_paise)
                    .ok_or_else(overflow)?;
                tb_movement = add(tb_movement, mv)?;
                if t.opening_paise != 0 {
                    openings.insert(n.as_str(), t.opening_paise);
                }
            }
        }
        let tb_movement = tb_movement.checked_mul(sign).ok_or_else(overflow)?;
        let f_tb = fig(
            &mut r,
            &format!("{block}_tb_movement"),
            Value::Int(tb_movement),
            Unit::Paise,
            &format!(
                "{group}: the Trial Balance's period {sense} on its ledgers (opening balances \
                      excluded)."
            ),
            scope
                .iter()
                .map(|n| EvidenceRef::new("ledger", n))
                .collect(),
        )?;
        let diff = total.year.checked_sub(tb_movement).ok_or_else(overflow)?;
        let f_diff = fig(
            &mut r,
            &format!("{block}_tb_difference"),
            Value::Int(diff),
            Unit::Paise,
            &format!(
                "{group}: the period total of the Total row less the Trial Balance's period \
                      movement on its ledgers."
            ),
            Vec::new(),
        )?;
        if !openings.is_empty() {
            let mut sum = 0_i64;
            for a in openings.values() {
                sum = add(sum, *a)?;
            }
            let evidence: Vec<EvidenceRef> = openings
                .keys()
                .map(|n| EvidenceRef::new("ledger", n))
                .collect();
            let f_open = fig(&mut r, &format!("{block}_opening_balance"),
                Value::Int(sum.checked_mul(sign).ok_or_else(overflow)?), Unit::Paise,
                &format!("{group}: opening balances on its ledgers ({} balance positive). Not in these \
                          figures; the financial statements, which read closing balances, include them.",
                         if sign < 0 { "a credit" } else { "a debit" }),
                evidence.clone())?;
            r.findings.push(Finding {
                id: format!("{TEST_ID}/opening_balance/{block}"),
                clauses: Vec::new(),
                title: format!(
                    "'{group}' ledgers carry an opening balance, which these figures leave out"
                ),
                facts: vec![("opening_balance".to_string(), f_open)],
                evidence,
                confidence: Confidence::Computed,
                limits: vec![
                    "An income or expense ledger normally opens the year at nil. The balance \
cited is not in the party-wise figures, but it is in the financial statements, so the difference \
between the two includes it."
                        .to_string(),
                ],
                ask_client: Vec::new(),
            });
        }
        if abs(diff)? > TIE_TOLERANCE_PAISE {
            // Excluded vouchers dated in the period with an amount on these ledgers, by status.
            let mut outside: BTreeMap<&str, Vec<(&Voucher, i64)>> = BTreeMap::new();
            for x in &book.vouchers {
                if let Some((_, key, _)) = STATUSES.iter().find(|(s, _, _)| *s == x.status) {
                    if in_period(&x.date) {
                        let amount = signed_sum(x)?;
                        if amount != 0 {
                            outside.entry(*key).or_default().push((x, amount));
                        }
                    }
                }
            }
            let mut candidates: Vec<(&str, Vec<(&Voucher, i64)>)> = STATUSES
                .iter()
                .filter_map(|(_, key, _)| outside.get(key).map(|es| (*key, es.clone())))
                .collect();
            if candidates.len() > 1 {
                let all: Vec<(&Voucher, i64)> = candidates
                    .iter()
                    .flat_map(|(_, es)| es.iter().copied())
                    .collect();
                candidates.push(("excluded", all));
            }
            let mut matches = Vec::new();
            for (k, es) in candidates {
                let mut s = diff;
                for (_, a) in &es {
                    s = add(s, *a)?;
                }
                if abs(s)? <= TIE_TOLERANCE_PAISE {
                    matches.push((k, es));
                }
            }
            let facts = vec![
                ("difference".to_string(), f_diff),
                ("trial_balance_movement".to_string(), f_tb),
            ];
            if matches.len() == 1 {
                let (k, mut es) = matches.remove(0);
                let word = STATUSES
                    .iter()
                    .find(|(_, key, _)| *key == k)
                    .map_or("optional, cancelled or post-dated", |(_, _, w)| *w);
                es.sort_by(|a, b| a.0.guid.cmp(&b.0.guid)); // stable, as Python's sorted
                let mut sum = 0_i64;
                for (_, a) in &es {
                    sum = add(sum, *a)?;
                }
                let evidence: Vec<EvidenceRef> = es
                    .iter()
                    .map(|(x, _)| {
                        EvidenceRef::with_label("excluded_voucher", &x.guid, &voucher_label(x))
                    })
                    .collect();
                let f_out = fig(&mut r, &format!("{block}_vouchers_left_out"), Value::Int(sum), Unit::Paise,
                    &format!("{group}: amount on these ledgers ({sense}) of the {word} vouchers cited, which \
                              the books leave out."),
                    evidence.clone())?;
                let mut facts = facts;
                facts.push(("vouchers_left_out".to_string(), f_out));
                r.findings.push(Finding {
                    id: format!("{TEST_ID}/tb_difference/{block}"),
                    clauses: Vec::new(),
                    title: format!("'{group}' by party differs from the Trial Balance by an amount matching \
                                    {word} vouchers (listed under Technical definitions)"),
                    facts,
                    evidence,
                    confidence: Confidence::Computed,
                    limits: vec![format!("These figures, like every books figure, leave out {word} vouchers. \
The amount of the voucher(s) listed under Technical definitions on these ledgers matches the difference \
to within a rupee, so the Trial Balance may include them; that is inferred from the amounts, not read \
from the Trial Balance. No other set of such vouchers matches it. Whether they belong to this year is \
for the CA to confirm from the vouchers themselves.")],
                    ask_client: Vec::new(),
                });
            } else {
                r.findings.push(Finding {
                    id: format!("{TEST_ID}/tb_difference/{block}"),
                    clauses: Vec::new(),
                    title: format!("'{group}' by party differs from the Trial Balance"),
                    facts,
                    evidence: Vec::new(),
                    confidence: Confidence::Computed,
                    limits: vec!["No one set of optional, cancelled or post-dated vouchers matches it. \
Possible causes: a voucher the Trial Balance and the voucher export disagree on, a voucher outside the \
books that the Trial Balance counts, or a ledger with vouchers but no Trial Balance row. It is not \
balanced away."
                        .to_string()],
                    ask_client: Vec::new(),
                });
            }
        }
    }
    Ok(r)
}

/// One row as PWM reads it back from the published figures.
#[derive(Default)]
struct Published {
    evidence: Option<EvidenceRef>,
    cells: BTreeMap<String, i128>,
}

fn int(v: &Value) -> i128 {
    match v {
        Value::Int(i) => i128::from(*i),
        _ => 0,
    }
}

/// PWM-1 and PWM-2, independent of `run`: they fire only on an internal inconsistency, never on a
/// data matter. Scope is the group name anywhere in a ledger's chain; rows are found from the
/// published figures and their evidence and compared with fresh sums from the vouchers, each
/// voucher counted once. Sums are i128, so no check can overflow where the reference's integers
/// would not.
#[allow(clippy::too_many_lines)] // one pass per block, as the reference lays it out
pub fn check_invariants(book: &Book, period: &Window, result: &TestResult) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let prefix = format!("{}.", result.test_id);
    let names = MONTHS;
    let columns: Vec<&str> = names
        .iter()
        .copied()
        .chain(["outside", "year", "returns", "vouchers"])
        .collect();
    let (start, end) = (&period.from, &period.to);
    let fy = is_fy(start, end);
    let pop = book.population()?;
    let parties: BTreeSet<&str> = book
        .ledgers
        .values()
        .filter(|l| {
            l.chain
                .iter()
                .any(|g| g == "Sundry Debtors" || g == "Sundry Creditors")
        })
        .map(|l| l.name.as_str())
        .collect();
    let figures: BTreeMap<&str, &Value> = result
        .figures
        .iter()
        .map(|f| (f.id.as_str(), &f.value))
        .collect();
    let checked: Vec<&str> = ["year", "outside", "returns", "vouchers"]
        .into_iter()
        .chain(if fy { names.to_vec() } else { Vec::new() })
        .collect();

    for (block, group, sign) in [
        ("sales", "Sales Accounts", -1_i128),
        ("purchases", "Purchase Accounts", 1),
        ("direct_expenses", "Direct Expenses", 1),
        ("indirect_expenses", "Indirect Expenses", 1),
    ] {
        if !book.groups.contains_key(group) {
            continue;
        }
        let mv_fid = format!("{prefix}{block}_tb_movement");
        let Some(published_mv) = figures.get(mv_fid.as_str()) else {
            out.push(format!(
                "PWM-1: the book carries '{group}' but no {block} figures were published"
            ));
            continue;
        };
        let scope: BTreeSet<&str> = book
            .ledgers
            .values()
            .filter(|l| l.chain.iter().any(|g| g == group))
            .map(|l| l.name.as_str())
            .collect();
        let movement: i128 = sign
            * book
                .tb
                .iter()
                .filter(|(n, _)| scope.contains(n.as_str()))
                .map(|(_, t)| i128::from(t.debit_paise) - i128::from(t.credit_paise))
                .sum::<i128>();
        if int(published_mv) != movement {
            out.push(format!(
                "PWM-1: {block}_tb_movement is {}p but the Trial Balance's period columns give {movement}p",
                int(published_mv)
            ));
        }
        let mut rows: BTreeMap<String, Published> = BTreeMap::new();
        for f in &result.figures {
            for col in &columns {
                let marker = format!("{prefix}{block}_{col}_");
                if let Some(h) = f.id.strip_prefix(&marker).filter(|h| !h.contains('_')) {
                    let row = rows.entry(h.to_string()).or_insert_with(|| Published {
                        evidence: f.evidence.first().cloned(),
                        cells: BTreeMap::new(),
                    });
                    row.cells.insert((*col).to_string(), int(&f.value));
                }
            }
        }
        let cell =
            |row: &Published, col: &str| -> i128 { row.cells.get(col).copied().unwrap_or(0) };
        let Some(total) = rows
            .iter()
            .find(|(_, row)| {
                row.evidence
                    .as_ref()
                    .is_some_and(|e| e.kind == "row" && e.label == "Total")
            })
            .map(|(h, _)| h.clone())
        else {
            out.push(format!("PWM-1: the {block} block has no total row"));
            continue;
        };
        for col in &columns {
            let others: i128 = rows
                .iter()
                .filter(|(h, _)| **h != total)
                .map(|(_, r)| cell(r, col))
                .sum();
            if others != cell(&rows[&total], col) {
                out.push(format!(
                    "PWM-1: the {block} rows sum to {others}p in {col} but the total row has {}p",
                    cell(&rows[&total], col)
                ));
            }
        }
        if fy {
            for row in rows.values() {
                let months: i128 = names.iter().map(|m| cell(row, m)).sum();
                if months != cell(row, "year") {
                    out.push(format!(
                        "PWM-2: a {block} row's months sum to {months}p but its year is {}p",
                        cell(row, "year")
                    ));
                }
            }
        }

        // Fresh figures from the vouchers: a named party, several parties, or no party.
        #[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
        enum Kind {
            Party(String),
            Several,
            Unnamed,
        }
        let mut fresh: BTreeMap<Kind, BTreeMap<String, i128>> = BTreeMap::new();
        for v in &pop {
            let in_scope: Vec<i128> = v
                .lines
                .iter()
                .filter(|l| scope.contains(l.ledger.as_str()))
                .map(|l| i128::from(l.amount_paise))
                .collect();
            if in_scope.is_empty() {
                continue;
            }
            let amount = sign * in_scope.iter().sum::<i128>();
            let named: BTreeSet<&str> = v
                .lines
                .iter()
                .map(|l| l.ledger.as_str())
                .filter(|l| parties.contains(l))
                .collect();
            let kind = match named.len() {
                1 => Kind::Party(named.iter().next().copied().unwrap_or_default().to_string()),
                0 => Kind::Unnamed,
                _ => Kind::Several,
            };
            let c = fresh.entry(kind).or_default();
            *c.entry("vouchers".to_string()).or_insert(0) += 1;
            if !(start <= &v.date && &v.date <= end) {
                *c.entry("outside".to_string()).or_insert(0) += amount;
                continue;
            }
            *c.entry("year".to_string()).or_insert(0) += amount;
            if fy {
                *c.entry(month_col(&v.date).to_string()).or_insert(0) += amount;
            }
            if NOTE_TYPES.contains(&v.base_type.as_str()) {
                *c.entry("returns".to_string()).or_insert(0) += amount;
            }
        }
        let summed = |cells: Vec<&BTreeMap<String, i128>>| -> BTreeMap<String, i128> {
            let mut acc = BTreeMap::new();
            for c in cells {
                for col in &checked {
                    *acc.entry((*col).to_string()).or_insert(0) +=
                        c.get(*col).copied().unwrap_or(0);
                }
            }
            acc
        };
        let empty = BTreeMap::new();
        let compare = |what: &str, published: &Published, expected: &BTreeMap<String, i128>| {
            let mut msgs = Vec::new();
            for col in &checked {
                let (p, e) = (
                    cell(published, col),
                    expected.get(*col).copied().unwrap_or(0),
                );
                if p != e {
                    msgs.push(format!(
                        "PWM-2: {what} has {p}p in {col} but the vouchers give {e}p"
                    ));
                }
            }
            msgs
        };
        out.extend(compare(
            &format!("the {block} total"),
            &rows[&total],
            &summed(fresh.values().collect()),
        ));
        let fresh_party: BTreeMap<&str, &BTreeMap<String, i128>> = fresh
            .iter()
            .filter_map(|(k, c)| match k {
                Kind::Party(p) => Some((p.as_str(), c)),
                _ => None,
            })
            .collect();
        let mut shown: BTreeSet<String> = BTreeSet::new();
        let (mut others, mut several) = (None, None);
        let mut unnamed: Vec<&Published> = Vec::new();
        for (h, row) in &rows {
            let Some(e) = row.evidence.as_ref().filter(|_| *h != total) else {
                continue;
            };
            if e.kind == "ledger" {
                shown.insert(e.id.clone());
                let what = format!("the {block} row for {}", py_repr_str(&e.id));
                out.extend(compare(
                    &what,
                    row,
                    fresh_party.get(e.id.as_str()).copied().unwrap_or(&empty),
                ));
            } else if e.id.ends_with(":others") {
                others = Some(row);
            } else if e.id.ends_with(":several") {
                several = Some(row);
            } else {
                unnamed.push(row);
            }
        }
        let rest: Vec<&BTreeMap<String, i128>> = fresh_party
            .iter()
            .filter(|(p, _)| !shown.contains(**p as &str))
            .map(|(_, c)| *c)
            .collect();
        out.extend(compare(
            &format!("the {block} Others row"),
            others.unwrap_or(&Published::default()),
            &summed(rest.clone()),
        ));
        if let Some(o) = others {
            let n = rest.len();
            let want = format!("Others ({n} {})", if n == 1 { "party" } else { "parties" });
            let label = o.evidence.as_ref().map_or("", |e| e.label.as_str());
            if label != want {
                out.push(format!(
                    "PWM-2: the {block} Others row is labelled {} but {n} {} not shown by name",
                    py_repr_str(label),
                    if n == 1 { "party is" } else { "parties are" }
                ));
            }
        }
        let year_abs = |c: &BTreeMap<String, i128>| c.get("year").copied().unwrap_or(0).abs();
        let smallest = shown
            .iter()
            .map(|p| fresh_party.get(p.as_str()).map_or(0, |c| year_abs(c)))
            .min();
        if let Some(smallest) = smallest {
            if !rest.is_empty() && smallest < rest.iter().map(|c| year_abs(c)).max().unwrap_or(0) {
                out.push(format!(
                    "PWM-2: a {block} party in Others is larger than a party shown by name"
                ));
            }
        }
        out.extend(compare(
            &format!("the {block} several-parties row"),
            several.unwrap_or(&Published::default()),
            fresh.get(&Kind::Several).unwrap_or(&empty),
        ));
        let unnamed_published = Published {
            evidence: None,
            cells: {
                let mut acc = BTreeMap::new();
                for row in &unnamed {
                    for col in &checked {
                        *acc.entry((*col).to_string()).or_insert(0) += cell(row, col);
                    }
                }
                acc
            },
        };
        out.extend(compare(
            &format!("the {block} sum of the rows with no party"),
            &unnamed_published,
            fresh.get(&Kind::Unnamed).unwrap_or(&empty),
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::book::{Ledger, LedgerLine, TbRow};

    fn date(d: &str) -> TallyDate {
        TallyDate::parse(d).unwrap()
    }

    fn ledger(name: &str, group: &str) -> (String, Ledger) {
        let l = Ledger {
            name: name.to_string(),
            parent: group.to_string(),
            chain: vec![group.to_string()],
            chain_complete: true,
            master_opening_paise: 0,
            guid: String::new(),
            masterid: None,
        };
        (name.to_string(), l)
    }

    /// A balanced voucher of `amount` paise debited to `dr` and credited to `cr`.
    fn voucher(guid: &str, on: &str, base_type: &str, dr: &str, cr: &str, amount: i64) -> Voucher {
        Voucher {
            guid: guid.to_string(),
            date: date(on),
            base_type: base_type.to_string(),
            status: VoucherStatus::Regular,
            lines: vec![
                LedgerLine {
                    ledger: dr.to_string(),
                    amount_paise: amount,
                },
                LedgerLine {
                    ledger: cr.to_string(),
                    amount_paise: -amount,
                },
            ],
            ..Default::default()
        }
    }

    fn tb(opening_paise: i64, debit_paise: i64, credit_paise: i64) -> TbRow {
        TbRow {
            opening_paise,
            debit_paise,
            credit_paise,
            closing_paise: opening_paise + debit_paise - credit_paise,
        }
    }

    #[test]
    fn a_whole_run_pins_the_period_ranking_cut_and_trial_balance_ties() {
        // Sales, top 4 by absolute year: E (-500, a credit note) above A (300, on 1 April), B,
        // then C and D tied at 100, where name order shows C; D's large credit note after the
        // period is its own column, not its rank and not its returns. The TB differs from the
        // total by exactly Re 1, which ties. Purchases differ by Rs 50, matched to within Re 1 by
        // the one cancelled voucher inside the period, not the one after it.
        let mut v = vec![
            voucher("s1", "20250401", "Sales", "Cust A", "Sales", 30_000),
            voucher("s2", "20250510", "Sales", "Cust B", "Sales", 20_000),
            voucher("s3", "20250610", "Sales", "Cust C", "Sales", 10_000),
            voucher("s4", "20250610", "Sales", "Cust D", "Sales", 10_000),
            voucher("s5", "20250601", "Credit Note", "Sales", "Cust E", 50_000),
            voucher("s6", "20260405", "Credit Note", "Sales", "Cust D", 100_000),
            voucher("p1", "20250701", "Purchase", "Purchases", "Supp X", 40_000),
            voucher("p2", "20250702", "Purchase", "Purchases", "Supp X", 4_900),
            voucher("p3", "20260402", "Purchase", "Purchases", "Supp X", 5_000),
        ];
        v[7].status = VoucherStatus::Cancelled;
        v[8].status = VoucherStatus::Cancelled;
        let book = Book {
            groups: [
                "Sales Accounts",
                "Purchase Accounts",
                "Sundry Debtors",
                "Sundry Creditors",
            ]
            .into_iter()
            .map(|g| (g.to_string(), None))
            .collect(),
            ledgers: [
                ledger("Sales", "Sales Accounts"),
                ledger("Purchases", "Purchase Accounts"),
                ledger("Cust A", "Sundry Debtors"),
                ledger("Cust B", "Sundry Debtors"),
                ledger("Cust C", "Sundry Debtors"),
                ledger("Cust D", "Sundry Debtors"),
                ledger("Cust E", "Sundry Debtors"),
                ledger("Supp X", "Sundry Creditors"),
            ]
            .into_iter()
            .collect(),
            vouchers: v,
            tb: BTreeMap::from([
                ("Sales".to_string(), tb(-7_000, 50_000, 69_900)),
                ("Purchases".to_string(), tb(0, 45_000, 0)),
            ]),
            ..Default::default()
        };
        let period = Window {
            from: date("20250401"),
            to: date("20260331"),
        };
        let rules = Rules::vendored().unwrap();
        let none = BTreeSet::new();
        let r = run(&book, &rules, &period, &none, &none, 4).unwrap();
        let value = |id: &str| -> Option<Value> {
            let id = format!("{TEST_ID}.{id}");
            r.figures
                .iter()
                .find(|f| f.id == id)
                .map(|f| f.value.clone())
        };
        let row_value = |prefix: &str, label: &str| -> Option<Value> {
            let prefix = format!("{TEST_ID}.{prefix}_");
            r.figures
                .iter()
                .find(|f| f.id.starts_with(&prefix) && f.evidence[0].label == label)
                .map(|f| f.value.clone())
        };
        let shown: Vec<&str> = r
            .figures
            .iter()
            .filter(|f| f.id.starts_with(&format!("{TEST_ID}.sales_vouchers_")))
            .map(|f| f.evidence[0].label.as_str())
            .collect();
        assert_eq!(
            shown,
            [
                "Cust E",
                "Cust A",
                "Cust B",
                "Cust C",
                "Others (1 party)",
                "Total"
            ]
        );
        assert_eq!(row_value("sales_apr", "Cust A"), Some(Value::Int(30_000)));
        assert_eq!(
            row_value("sales_returns", "Cust E"),
            Some(Value::Int(-50_000))
        );
        assert_eq!(
            row_value("sales_outside", "Others (1 party)"),
            Some(Value::Int(-100_000))
        );
        assert_eq!(row_value("sales_returns", "Others (1 party)"), None);
        assert_eq!(value("sales_tb_difference"), Some(Value::Int(100)));
        assert_eq!(value("sales_opening_balance"), Some(Value::Int(7_000)));
        let found = |id: &str| r.findings.iter().any(|f| f.id == format!("{TEST_ID}/{id}"));
        assert!(!found("tb_difference/sales"), "Re 1 ties");
        assert!(found("tb_difference/purchases"));
        assert_eq!(value("purchases_tb_difference"), Some(Value::Int(-5_000)));
        assert_eq!(
            value("purchases_vouchers_left_out"),
            Some(Value::Int(4_900))
        );
    }
}
