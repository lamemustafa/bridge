// SPDX-License-Identifier: Apache-2.0
//! Port of the reference engine's `bank_reconciliation`: a bank statement against the books' bank
//! ledger, for the calendar window the statement covers only.
//!
//! The statement is caller data ([`BankStatementDoc`]), never a Book field: `run` reads it, and
//! [`check_invariants`] re-derives BANK-1 (the statement's own running-balance chain) from the same
//! rows the caller passes it, as the reference reads `eng.bank`.
//!
//! Books side: one row per population voucher touching the bank ledger, summed over that voucher's
//! lines on the ledger (never per line), dated within the statement's window. Matching pairs each
//! books row, in (date, GUID) order, with the nearest-date unmatched statement row that agrees in
//! amount within `TOL_PAISE` and lies within `match_max_days`. What stays unmatched is tried as a
//! split settlement (2 to 4 rows on the other side summing to it), then classified by narration
//! terms and by sign.
//!
//! Python's orders are kept where they decide a result:
//! * the reference iterates `set`s of small list indices, which CPython yields in ascending order
//!   (measured up to n = 100,000 with removals); `BTreeSet<usize>` iterates the same way;
//! * a split is the first hit in `itertools.combinations` order over that pool, by size;
//! * `guid[-12:]` and `narration[:60]` count code points.
//!
//! A figure id the reference would repeat (two books rows sharing a GUID's hash) is refused with
//! an error, as the reference's `fig` raises, never a panic.

use std::collections::{BTreeMap, BTreeSet};

use bridge_tally_primitives::TallyDate;

use crate::book::Book;
use crate::depreciation::civil_day_number;
use crate::documents::{BankStatementDoc, BankStatementRow};
use crate::error::{AuditError, Result};
use crate::findings::{Confidence, EvidenceRef, Finding, TestResult, Unit, Value};
use crate::read::{iso, Window};
use crate::rules::Rules;
use crate::support::{count, hash8, overflow, py_repr_str, py_upper, voucher_label};

pub const TEST_ID: &str = "bank_reconciliation";
pub const VERSION: &str = "1";

const TOL_PAISE: i64 = 100;
/// This module's own analytical window, not tax law.
pub const MATCH_MAX_DAYS: i64 = 7;
/// Bound on a split-settlement combination.
const MAX_SPLIT_PARTS: usize = 4;
/// A larger pool is not searched (the reference's defensive bound).
const MAX_SPLIT_POOL: usize = 40;

const REASON_CHEQUE_NOT_PRESENTED: &str = "cheque_issued_not_presented";
const REASON_DEPOSIT_NOT_CREDITED: &str = "deposit_not_credited";
const REASON_BANK_ONLY_CHARGE: &str = "bank_only_charge_or_interest";
const REASON_SPLIT_SETTLEMENT: &str = "split_settlement";
const REASON_NOT_FOUND: &str = "not_found";
const REASON_UNCLASSIFIED: &str = "unclassified";
const REASONS: [&str; 6] = [
    REASON_CHEQUE_NOT_PRESENTED,
    REASON_DEPOSIT_NOT_CREDITED,
    REASON_BANK_ONLY_CHARGE,
    REASON_SPLIT_SETTLEMENT,
    REASON_NOT_FOUND,
    REASON_UNCLASSIFIED,
];

/// The first `n` characters (code points), as Python's `text[:n]`.
fn prefix_chars(text: &str, n: usize) -> String {
    text.chars().take(n).collect()
}

fn add(a: i64, b: i64) -> Result<i64> {
    a.checked_add(b).ok_or_else(|| overflow(TEST_ID))
}

fn sub(a: i64, b: i64) -> Result<i64> {
    a.checked_sub(b).ok_or_else(|| overflow(TEST_ID))
}

/// A statement row's signed amount: credit (money in) minus debit (money out).
fn signed(s: &BankStatementRow) -> Result<i64> {
    sub(s.credit_paise, s.debit_paise)
}

/// `abs(a - b)`, refusing an overflow.
fn distance(a: i64, b: i64) -> Result<i64> {
    sub(a, b)?.checked_abs().ok_or_else(|| overflow(TEST_ID))
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

/// The optional `[roles].bank_charge_narration_terms`, as a list of strings; empty when absent.
///
/// Divergence, deliberate, and not parity (as `cash_book_integrity::own_account_terms`): the
/// reference passes the value to `set(...)` unchecked, so a single string becomes the set of its
/// characters and a table the set of its keys. Here both, and a list holding a non-string, refuse.
pub fn charge_terms(raw: Option<&toml::Value>) -> Result<BTreeSet<String>> {
    let Some(raw) = raw else {
        return Ok(BTreeSet::new());
    };
    let key = "[roles].bank_charge_narration_terms";
    raw.as_array()
        .ok_or_else(|| AuditError::Config(format!("{TEST_ID}: {key} is not a list")))?
        .iter()
        .map(|v| match v.as_str() {
            // An empty term is in every narration, so it would make every row a charge.
            Some("") => Err(AuditError::Config(format!(
                "{TEST_ID}: {key} holds an empty term"
            ))),
            Some(s) => Ok(s.to_string()),
            None => Err(AuditError::Config(format!(
                "{TEST_ID}: {key} holds a non-string"
            ))),
        })
        .collect()
}

/// One books row: a population voucher's net amount on the bank ledger, the ledger's own sign
/// (Dr+ money in, Cr- money out).
struct BookRow {
    guid: String,
    label: String,
    date: TallyDate,
    amount_paise: i64,
}

fn books_rows(
    book: &Book,
    bank_ledger: &str,
    start: &TallyDate,
    end: &TallyDate,
) -> Result<Vec<BookRow>> {
    let mut out = Vec::new();
    for v in book.population()? {
        if !(start <= &v.date && &v.date <= end) {
            continue;
        }
        let mut amount = 0_i64;
        for l in v.lines.iter().filter(|l| l.ledger == bank_ledger) {
            amount = add(amount, l.amount_paise)?;
        }
        if amount != 0 {
            out.push(BookRow {
                guid: v.guid.clone(),
                label: voucher_label(v),
                date: v.date.clone(),
                amount_paise: amount,
            });
        }
    }
    Ok(out)
}

/// TB opening plus every population line on the ledger dated before `start`.
fn books_balance_before(book: &Book, bank_ledger: &str, start: &TallyDate) -> Result<i64> {
    let mut total = book.tb.get(bank_ledger).map_or(0, |t| t.opening_paise);
    for v in book.population()? {
        if &v.date >= start {
            continue;
        }
        for l in v.lines.iter().filter(|l| l.ledger == bank_ledger) {
            total = add(total, l.amount_paise)?;
        }
    }
    Ok(total)
}

fn days_apart(a: &TallyDate, b: &TallyDate) -> i64 {
    (civil_day_number(a) - civil_day_number(b)).abs()
}

/// Books row indices in (date, GUID) order.
fn books_order(rows: &[BookRow], of: impl IntoIterator<Item = usize>) -> Vec<usize> {
    let mut order: Vec<usize> = of.into_iter().collect();
    order.sort_by(|&a, &b| (&rows[a].date, &rows[a].guid).cmp(&(&rows[b].date, &rows[b].guid)));
    order
}

/// Statement row indices in (date, index) order.
fn statement_order(rows: &[BankStatementRow], of: impl IntoIterator<Item = usize>) -> Vec<usize> {
    let mut order: Vec<usize> = of.into_iter().collect();
    order.sort_by(|&a, &b| (&rows[a].txn_date, a).cmp(&(&rows[b].txn_date, b)));
    order
}

/// The reference's `_match`: each books row, in (date, GUID) order, takes the unmatched statement
/// row with the smallest (day gap, index) that agrees within `tol` and `max_days`.
fn match_rows(
    books: &[BookRow],
    stmt: &[BankStatementRow],
    tol: i64,
    max_days: i64,
) -> Result<Vec<(usize, usize)>> {
    let mut used: BTreeSet<usize> = BTreeSet::new();
    let mut pairs = Vec::new();
    for bi in books_order(books, 0..books.len()) {
        let b = &books[bi];
        let mut best: Option<(i64, usize)> = None;
        for (si, s) in stmt.iter().enumerate() {
            if used.contains(&si) {
                continue;
            }
            if distance(signed(s)?, b.amount_paise)? > tol {
                continue;
            }
            let gap = days_apart(&s.txn_date, &b.date);
            if gap > max_days {
                continue;
            }
            if best.is_none_or(|x| (gap, si) < x) {
                best = Some((gap, si));
            }
        }
        if let Some((_, si)) = best {
            used.insert(si);
            pairs.push((bi, si));
        }
    }
    Ok(pairs)
}

/// The reference's `_find_split`: the first combination of 2 to 4 members of `pool`, by size and
/// then in `itertools.combinations` order, whose amounts sum to `target` within `tol`; `None` for a
/// pool larger than `MAX_SPLIT_POOL`.
fn find_split(target: i64, pool: &[(usize, i64)], tol: i64) -> Result<Option<Vec<usize>>> {
    if pool.len() > MAX_SPLIT_POOL {
        return Ok(None);
    }
    for size in 2..=MAX_SPLIT_PARTS.min(pool.len()) {
        let mut idx: Vec<usize> = (0..size).collect();
        loop {
            let mut total = 0_i64;
            for &i in &idx {
                total = add(total, pool[i].1)?;
            }
            if distance(total, target)? <= tol {
                return Ok(Some(idx.iter().map(|&i| pool[i].0).collect()));
            }
            // The next combination in lexicographic order, as itertools.combinations yields them.
            let n = pool.len();
            let Some(k) = (0..size).rev().find(|&k| idx[k] != k + n - size) else {
                break;
            };
            idx[k] += 1;
            for j in k + 1..size {
                idx[j] = idx[j - 1] + 1;
            }
        }
    }
    Ok(None)
}

/// Run `bank_reconciliation` on a book against one statement. `period` is the engagement's; the
/// books' Trial Balance closing is reported only when the statement's window ends on its last day.
pub fn run(
    book: &Book,
    rules: &Rules,
    period: &Window,
    statement: &BankStatementDoc,
    bank_ledger: &str,
    bank_charge_narration_terms: &BTreeSet<String>,
    match_max_days: i64,
) -> Result<TestResult> {
    let mut r = TestResult::new(TEST_ID, VERSION, &rules.version);
    let terms: BTreeSet<String> = bank_charge_narration_terms
        .iter()
        .map(|t| py_upper(t))
        .collect();
    let (start, end) = (&statement.start, &statement.end);
    let stmt = &statement.rows;
    // The opening is the TB opening plus earlier lines of this year's book, so it is a balance
    // only for a window inside that year.
    if start < &period.from || end > &period.to {
        return Err(AuditError::Config(format!(
            "{TEST_ID}: the statement window {} to {} is not inside the engagement year",
            iso(start),
            iso(end)
        )));
    }

    r.population_note = format!(
        "Books population (optional, cancelled and post-dated vouchers excluded), ledger \
         '{bank_ledger}', window {} to {} only -- the calendar span this statement covers. No \
         other month is reconciled here.",
        iso(start),
        iso(end)
    );
    let note = r.population_note.clone();
    fig(
        &mut r,
        "months_covered",
        Value::Text(format!("{}..{}", iso(start), iso(end))),
        Unit::Text,
        &note,
        vec![],
    )?;
    fig(
        &mut r,
        "months_not_reconciled_note",
        Value::Text(
            "Any FY month outside the window above has no bank statement in this \
                     engagement and is not reconciled."
                .to_string(),
        ),
        Unit::Text,
        "Scope statement.",
        vec![],
    )?;
    fig(
        &mut r,
        "bank_statement_source_sha256",
        Value::Text(statement.source_sha256.clone()),
        Unit::Text,
        "sha256 of the raw bank-statement document bytes.",
        vec![],
    )?;
    fig(
        &mut r,
        "bank_statement_account_ref",
        Value::Text(statement.account_ref.clone()),
        Unit::Text,
        "Masked account number, as extracted.",
        vec![],
    )?;

    // ------------------------------------------------------------ opening/closing tie
    let books_opening = books_balance_before(book, bank_ledger, start)?;
    let book_rows = books_rows(book, bank_ledger, start, end)?;
    let mut books_closing = books_opening;
    for b in &book_rows {
        books_closing = add(books_closing, b.amount_paise)?;
    }
    let f_books_open = fig(
        &mut r,
        "books_opening_paise",
        Value::Int(books_opening),
        Unit::Paise,
        &format!(
            "TB opening plus every population line on '{bank_ledger}' dated before {}.",
            iso(start)
        ),
        vec![],
    )?;
    let f_stmt_open = fig(
        &mut r,
        "statement_opening_paise",
        Value::Int(statement.opening_balance_paise),
        Unit::Paise,
        "The statement's own declared opening balance.",
        vec![],
    )?;
    fig(
        &mut r,
        "opening_tie_diff_paise",
        Value::Int(sub(books_opening, statement.opening_balance_paise)?),
        Unit::Paise,
        "books_opening_paise minus statement_opening_paise; reported even when zero.",
        vec![],
    )?;
    let f_books_close = fig(
        &mut r,
        "books_closing_paise",
        Value::Int(books_closing),
        Unit::Paise,
        "books_opening_paise plus every books row in the window (below).",
        vec![],
    )?;
    let f_stmt_close = fig(
        &mut r,
        "statement_closing_paise",
        Value::Int(statement.closing_balance_paise),
        Unit::Paise,
        "The statement's own declared closing balance.",
        vec![],
    )?;
    fig(
        &mut r,
        "closing_tie_diff_paise",
        Value::Int(sub(books_closing, statement.closing_balance_paise)?),
        Unit::Paise,
        "books_closing_paise minus statement_closing_paise; reported even when zero.",
        vec![],
    )?;
    if *end == period.to {
        let closing = book.tb.get(bank_ledger).map_or(0, |t| t.closing_paise);
        fig(
            &mut r,
            "tb_closing_paise",
            Value::Int(closing),
            Unit::Paise,
            &format!(
                "'{bank_ledger}' Trial Balance closing (informational -- window end equals \
                      the FY end)."
            ),
            vec![],
        )?;
    }

    let opening_ties = distance(books_opening, statement.opening_balance_paise)? <= TOL_PAISE;
    let closing_ties = distance(books_closing, statement.closing_balance_paise)? <= TOL_PAISE;
    if !(opening_ties && closing_ties) {
        r.findings.push(Finding {
            id: format!("{TEST_ID}/opening_closing_tie"),
            clauses: Vec::new(),
            title: "The books balance on the bank ledger does not tie to the statement's own \
                    declared opening or closing balance for this window"
                .to_string(),
            facts: vec![
                ("books_opening_paise".to_string(), f_books_open),
                ("statement_opening_paise".to_string(), f_stmt_open),
                ("books_closing_paise".to_string(), f_books_close),
                ("statement_closing_paise".to_string(), f_stmt_close),
            ],
            evidence: Vec::new(),
            confidence: Confidence::NeedsDocument,
            limits: vec![
                "A tie failure here can also mean the ledger name or statement window \
                          supplied to this test is wrong, not necessarily a books error."
                    .to_string(),
            ],
            ask_client: vec![
                "Confirm the opening/closing balance shown on the bank's own \
                              statement for this exact window."
                    .to_string(),
            ],
        });
    }

    fig(
        &mut r,
        "books_rows_count",
        count(TEST_ID, book_rows.len())?,
        Unit::Count,
        &note,
        vec![],
    )?;
    fig(
        &mut r,
        "statement_rows_count",
        count(TEST_ID, stmt.len())?,
        Unit::Count,
        "Statement transaction rows in this window.",
        vec![],
    )?;

    // ------------------------------------------------------------ matching
    let pairs = match_rows(&book_rows, stmt, TOL_PAISE, match_max_days)?;
    let used_books: BTreeSet<usize> = pairs.iter().map(|p| p.0).collect();
    let used_stmt: BTreeSet<usize> = pairs.iter().map(|p| p.1).collect();
    for &(bi, si) in &pairs {
        let (b, s) = (&book_rows[bi], &stmt[si]);
        let h = hash8(&b.guid);
        fig(
            &mut r,
            &format!("match_pair_{h}"),
            Value::Text("matched".to_string()),
            Unit::Text,
            &format!("Books row (tag {h}) matched to statement row #{}.", s.row),
            vec![
                EvidenceRef::with_label("voucher", &b.guid, &b.label),
                EvidenceRef::new("document_row", &format!("{}#{}", s.doc, s.row)),
            ],
        )?;
    }
    fig(
        &mut r,
        "category_matched_count",
        count(TEST_ID, pairs.len())?,
        Unit::Count,
        &format!("Books rows matched to a statement row within Re 1 and {match_max_days} day(s)."),
        vec![],
    )?;

    let unmatched_books: Vec<usize> = (0..book_rows.len())
        .filter(|i| !used_books.contains(i))
        .collect();
    let unmatched_stmt: Vec<usize> = (0..stmt.len()).filter(|i| !used_stmt.contains(i)).collect();

    // ------------------------------------------------------------ split-settlement pass
    let mut reason_books: BTreeMap<usize, &'static str> = BTreeMap::new();
    let mut reason_stmt: BTreeMap<usize, &'static str> = BTreeMap::new();
    let mut split_groups: Vec<usize> = Vec::new(); // books rows settled by several statement rows
    let mut split_groups_rev: Vec<usize> = Vec::new(); // statement rows settled by several books rows
    let mut remaining_books: BTreeSet<usize> = unmatched_books.iter().copied().collect();
    let mut remaining_stmt: BTreeSet<usize> = unmatched_stmt.iter().copied().collect();

    for bi in books_order(&book_rows, remaining_books.clone()) {
        let b = &book_rows[bi];
        let mut pool = Vec::new();
        for &si in &remaining_stmt {
            if days_apart(&stmt[si].txn_date, &b.date) <= match_max_days {
                pool.push((si, signed(&stmt[si])?));
            }
        }
        if let Some(found) = find_split(b.amount_paise, &pool, TOL_PAISE)? {
            reason_books.insert(bi, REASON_SPLIT_SETTLEMENT);
            for si in found {
                reason_stmt.insert(si, REASON_SPLIT_SETTLEMENT);
                remaining_stmt.remove(&si);
            }
            split_groups.push(bi);
            remaining_books.remove(&bi);
        }
    }

    for si in statement_order(stmt, remaining_stmt.clone()) {
        let s = &stmt[si];
        let mut pool = Vec::new();
        for &bi in &remaining_books {
            if days_apart(&book_rows[bi].date, &s.txn_date) <= match_max_days {
                pool.push((bi, book_rows[bi].amount_paise));
            }
        }
        if let Some(found) = find_split(signed(s)?, &pool, TOL_PAISE)? {
            reason_stmt.insert(si, REASON_SPLIT_SETTLEMENT);
            for bi in found {
                reason_books.insert(bi, REASON_SPLIT_SETTLEMENT);
                remaining_books.remove(&bi);
            }
            split_groups_rev.push(si);
            remaining_stmt.remove(&si);
        }
    }

    // ------------------------------------------------------------ narration / direction pass
    for si in statement_order(stmt, remaining_stmt.clone()) {
        let narration = py_upper(&stmt[si].narration);
        if terms.iter().any(|t| narration.contains(t.as_str())) {
            reason_stmt.insert(si, REASON_BANK_ONLY_CHARGE);
        }
    }
    for bi in books_order(&book_rows, remaining_books.clone()) {
        let reason = if book_rows[bi].amount_paise < 0 {
            REASON_CHEQUE_NOT_PRESENTED
        } else {
            REASON_DEPOSIT_NOT_CREDITED
        };
        reason_books.insert(bi, reason);
    }
    for si in statement_order(stmt, remaining_stmt.clone()) {
        reason_stmt.entry(si).or_insert(REASON_NOT_FOUND);
    }

    // ------------------------------------------------------------ figures: reasons
    let reason_of = |m: &BTreeMap<usize, &'static str>, i: usize| -> &'static str {
        m.get(&i).copied().unwrap_or(REASON_UNCLASSIFIED)
    };
    let mut books_by_reason: BTreeMap<&str, Vec<usize>> =
        REASONS.iter().map(|&k| (k, Vec::new())).collect();
    for &bi in &unmatched_books {
        books_by_reason
            .entry(reason_of(&reason_books, bi))
            .or_default()
            .push(bi);
    }
    let mut stmt_by_reason: BTreeMap<&str, Vec<usize>> =
        REASONS.iter().map(|&k| (k, Vec::new())).collect();
    for &si in &unmatched_stmt {
        stmt_by_reason
            .entry(reason_of(&reason_stmt, si))
            .or_default()
            .push(si);
    }

    for reason in REASONS {
        let rows_b = &books_by_reason[reason];
        let ev_b = rows_b
            .iter()
            .map(|&i| EvidenceRef::with_label("voucher", &book_rows[i].guid, &book_rows[i].label))
            .collect();
        fig(
            &mut r,
            &format!("books_only_reason_{reason}_count"),
            count(TEST_ID, rows_b.len())?,
            Unit::Count,
            &format!("Unmatched books rows classified '{reason}'."),
            ev_b,
        )?;
        let mut total_b = 0_i64;
        for &i in rows_b {
            total_b = add(total_b, book_rows[i].amount_paise)?;
        }
        fig(
            &mut r,
            &format!("books_only_reason_{reason}_paise"),
            Value::Int(total_b),
            Unit::Paise,
            &format!("Sum of books amount, reason '{reason}'."),
            vec![],
        )?;
        let rows_s = &stmt_by_reason[reason];
        let ev_s = rows_s
            .iter()
            .map(|&i| {
                EvidenceRef::with_label(
                    "document_row",
                    &format!("{}#{}", stmt[i].doc, stmt[i].row),
                    &prefix_chars(&stmt[i].narration, 60),
                )
            })
            .collect();
        fig(
            &mut r,
            &format!("statement_only_reason_{reason}_count"),
            count(TEST_ID, rows_s.len())?,
            Unit::Count,
            &format!("Unmatched statement rows classified '{reason}'."),
            ev_s,
        )?;
        let mut total_s = 0_i64;
        for &i in rows_s {
            total_s = add(total_s, signed(&stmt[i])?)?;
        }
        fig(
            &mut r,
            &format!("statement_only_reason_{reason}_paise"),
            Value::Int(total_s),
            Unit::Paise,
            &format!("Sum of statement amount, reason '{reason}'."),
            vec![],
        )?;
    }

    let unclassified =
        books_by_reason[REASON_UNCLASSIFIED].len() + stmt_by_reason[REASON_UNCLASSIFIED].len();
    fig(
        &mut r,
        "unclassified_count",
        count(TEST_ID, unclassified)?,
        Unit::Count,
        "books_only + statement_only rows with no reason from the closed set (BANK-3: must be 0).",
        vec![],
    )?;
    fig(
        &mut r,
        "category_books_only_count",
        count(TEST_ID, unmatched_books.len())?,
        Unit::Count,
        "Books rows with no statement counterpart.",
        vec![],
    )?;
    fig(
        &mut r,
        "category_statement_only_count",
        count(TEST_ID, unmatched_stmt.len())?,
        Unit::Count,
        "Statement rows with no books counterpart.",
        vec![],
    )?;

    let fid = |name: &str| format!("{TEST_ID}.{name}");
    if !books_by_reason[REASON_NOT_FOUND].is_empty() || !stmt_by_reason[REASON_NOT_FOUND].is_empty()
    {
        r.findings.push(Finding {
            id: format!("{TEST_ID}/not_found"),
            clauses: Vec::new(),
            title: "A books or statement bank entry in this window has no counterpart and no \
                    structural (timing, charge or part-settlement) explanation"
                .to_string(),
            facts: vec![
                (
                    "books_not_found_count".to_string(),
                    fid(&format!("books_only_reason_{REASON_NOT_FOUND}_count")),
                ),
                (
                    "statement_not_found_count".to_string(),
                    fid(&format!("statement_only_reason_{REASON_NOT_FOUND}_count")),
                ),
            ],
            evidence: Vec::new(),
            confidence: Confidence::NeedsDocument,
            limits: vec![
                "cheque_issued_not_presented/deposit_not_credited are the standard timing \
                          explanations for the OTHER unmatched rows, not proof either actually \
                          cleared later; only a subsequent statement would confirm that."
                    .to_string(),
            ],
            ask_client: vec![
                "For each not_found row, the underlying voucher or bank advice.".to_string(),
            ],
        });
    }

    if !split_groups.is_empty() || !split_groups_rev.is_empty() {
        let mut evidence: Vec<EvidenceRef> = split_groups
            .iter()
            .map(|&bi| {
                EvidenceRef::with_label("voucher", &book_rows[bi].guid, &book_rows[bi].label)
            })
            .collect();
        evidence.extend(split_groups_rev.iter().map(|&si| {
            EvidenceRef::new(
                "document_row",
                &format!("{}#{}", stmt[si].doc, stmt[si].row),
            )
        }));
        r.findings.push(Finding {
            id: format!("{TEST_ID}/split_settlement"),
            clauses: Vec::new(),
            title: "A single bank entry on one side settles as several entries on the other, \
                    within the matching window"
                .to_string(),
            facts: vec![
                (
                    "books_part_settlement_count".to_string(),
                    fid(&format!(
                        "books_only_reason_{REASON_SPLIT_SETTLEMENT}_count"
                    )),
                ),
                (
                    "statement_part_settlement_count".to_string(),
                    fid(&format!(
                        "statement_only_reason_{REASON_SPLIT_SETTLEMENT}_count"
                    )),
                ),
            ],
            evidence,
            confidence: Confidence::Indicative,
            limits: vec![
                "A sum match within the window is consistent with, not proof of, a \
                          genuine settlement paid in several parts; an unrelated coincidence of \
                          amounts cannot be ruled out from the statement alone."
                    .to_string(),
            ],
            ask_client: Vec::new(),
        });
    }

    Ok(r)
}

// ---------------------------------------------------------------- invariants

/// BANK-1: the statement's own running-balance chain, from the rows in `row` order. A pair where
/// either balance is absent is skipped, as the reference skips it.
fn bank1_balance_chain(rows: &[BankStatementRow]) -> Result<Vec<String>> {
    let mut sorted: Vec<&BankStatementRow> = rows.iter().collect();
    sorted.sort_by_key(|r| r.row);
    let mut out = Vec::new();
    for pair in sorted.windows(2) {
        let (prev, cur) = (pair[0], pair[1]);
        let (Some(pb), Some(cb)) = (prev.balance_paise, cur.balance_paise) else {
            continue;
        };
        let expected = sub(add(pb, cur.credit_paise)?, cur.debit_paise)?;
        if distance(expected, cb)? > TOL_PAISE {
            out.push(format!(
                "BANK-1: statement row #{} ({}) balance {cb}p != prior balance {pb}p + credit {}p \
                 - debit {}p (expected {expected}p)",
                cur.row,
                iso(&cur.txn_date),
                cur.credit_paise,
                cur.debit_paise
            ));
        }
    }
    Ok(out)
}

fn figure_int(result: &TestResult, id: &str) -> Option<i64> {
    result
        .figures
        .iter()
        .find(|f| f.id == id)
        .and_then(|f| match f.value {
            Value::Int(v) => Some(v),
            _ => None,
        })
}

/// BANK-2: every books and statement row is matched or unmatched exactly once, and no statement
/// row is cited twice across the figures.
fn bank2_partition_and_no_reuse(result: &TestResult, prefix: &str) -> Result<Vec<String>> {
    let val = |name: &str| figure_int(result, &format!("{prefix}{name}"));
    let matched = val("category_matched_count").unwrap_or(0);
    let books_only = val("category_books_only_count").unwrap_or(0);
    let stmt_only = val("category_statement_only_count").unwrap_or(0);
    let mut out = Vec::new();
    if let Some(total) = val("books_rows_count") {
        let sum = add(matched, books_only)?;
        if sum != total {
            out.push(format!(
                "BANK-2: matched+books_only ({sum}) != books_rows_count ({total})"
            ));
        }
    }
    if let Some(total) = val("statement_rows_count") {
        let sum = add(matched, stmt_only)?;
        if sum != total {
            out.push(format!(
                "BANK-2: matched+statement_only ({sum}) != statement_rows_count ({total})"
            ));
        }
    }
    let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
    for f in result.figures.iter().filter(|f| f.id.starts_with(prefix)) {
        for e in f.evidence.iter().filter(|e| e.kind == "document_row") {
            *seen.entry(e.id.as_str()).or_insert(0) += 1;
        }
    }
    let dupes: Vec<String> = seen
        .into_iter()
        .filter(|&(_, n)| n > 1)
        .map(|(k, _)| py_repr_str(k))
        .take(10)
        .collect();
    if !dupes.is_empty() {
        out.push(format!(
            "BANK-2: statement row(s) referenced more than once across figures: [{}]",
            dupes.join(", ")
        ));
    }
    Ok(out)
}

/// BANK-3: no unmatched row lacks a reason.
fn bank3_no_unclassified(result: &TestResult, prefix: &str) -> Vec<String> {
    let id = format!("{prefix}unclassified_count");
    match result.figures.iter().find(|f| f.id == id).map(|f| &f.value) {
        Some(Value::Int(0)) | None => Vec::new(),
        Some(Value::Int(v)) => vec![format!(
            "BANK-3: {v} unmatched row(s) have no classification reason"
        )],
        Some(other) => vec![format!(
            "BANK-3: {other:?} unmatched row(s) have no classification reason"
        )],
    }
}

/// The reference's `check_invariants`: BANK-1 over `statement_rows` (the rows the caller passed to
/// `run`, as the reference reads `eng.bank`), then BANK-2 and BANK-3 over the result.
pub fn check_invariants(
    statement_rows: &[BankStatementRow],
    result: &TestResult,
) -> Result<Vec<String>> {
    let prefix = format!("{}.", result.test_id);
    let mut out = bank1_balance_chain(statement_rows)?;
    out.extend(bank2_partition_and_no_reuse(result, &prefix)?);
    out.extend(bank3_no_unclassified(result, &prefix));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_split_is_the_first_hit_in_combinations_order() {
        // Python: next(c for size in 2..4 for c in combinations(pool, size) if within tol)
        let pool = [(10, 5), (11, 3), (12, 2), (13, 1)];
        assert_eq!(find_split(5, &pool, 0).unwrap(), Some(vec![11, 12]));
        assert_eq!(find_split(6, &pool, 0).unwrap(), Some(vec![10, 13]));
        assert_eq!(find_split(9, &pool, 0).unwrap(), Some(vec![10, 11, 13]));
        assert_eq!(
            find_split(11, &pool, 0).unwrap(),
            Some(vec![10, 11, 12, 13])
        );
        assert_eq!(find_split(100, &pool, 0).unwrap(), None);
        assert_eq!(
            find_split(5, &[(1, 5)], 0).unwrap(),
            None,
            "one row is never a split"
        );
        let big: Vec<(usize, i64)> = (0..41).map(|i| (i, 1)).collect();
        assert_eq!(
            find_split(2, &big, 0).unwrap(),
            None,
            "a pool over 40 is not searched"
        );
        assert_eq!(find_split(2, &big[..40], 0).unwrap(), Some(vec![0, 1]));
    }

    #[test]
    fn a_match_takes_exactly_re_1_and_exactly_7_days_and_a_tie_goes_to_the_lower_index() {
        let row = |date: &str, credit_paise: i64| BankStatementRow {
            doc: "bank:unit".to_string(),
            row: 0,
            account_ref: "XXXXXX0001".to_string(),
            txn_date: TallyDate::parse(date).unwrap(),
            narration: String::new(),
            debit_paise: 0,
            credit_paise,
            balance_paise: None,
        };
        let books = [BookRow {
            guid: "g1".to_string(),
            label: "g1".to_string(),
            date: TallyDate::parse("20260310").unwrap(),
            amount_paise: 10_000,
        }];
        let pairs = |stmt: &[BankStatementRow]| {
            match_rows(&books, stmt, TOL_PAISE, MATCH_MAX_DAYS).unwrap()
        };
        assert_eq!(pairs(&[row("20260310", 10_100)]), vec![(0, 0)], "Re 1 off");
        assert_eq!(pairs(&[row("20260310", 10_101)]), vec![], "Re 1.01 off");
        assert_eq!(pairs(&[row("20260317", 10_000)]), vec![(0, 0)], "7 days");
        assert_eq!(pairs(&[row("20260318", 10_000)]), vec![], "8 days");
        assert_eq!(
            pairs(&[row("20260313", 10_000), row("20260307", 10_000)]),
            vec![(0, 0)],
            "an equal gap goes to the lower statement index"
        );
        // Books rows go in (date, GUID) order, and a statement row is taken once.
        let book = |guid: &str| BookRow {
            guid: guid.to_string(),
            label: guid.to_string(),
            date: TallyDate::parse("20260310").unwrap(),
            amount_paise: 10_000,
        };
        let competing = [book("gB"), book("gA")];
        assert_eq!(
            match_rows(&competing, &[row("20260310", 10_000)], TOL_PAISE, 7).unwrap(),
            vec![(1, 0)]
        );
    }

    #[test]
    fn the_window_holds_its_first_and_last_days_and_lies_inside_the_year() {
        use crate::book::{LedgerLine, TbRow, Voucher, VoucherStatus};
        let voucher = |guid: &str, date: &str, amount_paise: i64| Voucher {
            guid: guid.to_string(),
            date: TallyDate::parse(date).unwrap(),
            status: VoucherStatus::Regular,
            lines: vec![
                LedgerLine {
                    ledger: "Bank".to_string(),
                    amount_paise,
                },
                LedgerLine {
                    ledger: "Sales".to_string(),
                    amount_paise: -amount_paise,
                },
            ],
            ..Default::default()
        };
        let book = Book {
            company_name: "Synthetic".to_string(),
            company_guid: "test-guid".to_string(),
            read_at: String::new(),
            groups: BTreeMap::new(),
            group_masters: BTreeMap::new(),
            ledgers: BTreeMap::new(),
            vouchers: vec![
                voucher("v1", "20260228", 20_000),
                voucher("v2", "20260301", 5_000),
                voucher("v3", "20260331", 1_000),
                voucher("v4", "20260401", 1_000),
            ],
            tb: BTreeMap::from([(
                "Bank".to_string(),
                TbRow {
                    opening_paise: 100_000,
                    debit_paise: 27_000,
                    credit_paise: 0,
                    closing_paise: 127_000,
                },
            )]),
        };
        let start = TallyDate::parse("20260301").unwrap();
        let end = TallyDate::parse("20260331").unwrap();
        assert_eq!(
            books_balance_before(&book, "Bank", &start).unwrap(),
            120_000
        );
        let rows = books_rows(&book, "Bank", &start, &end).unwrap();
        let guids: Vec<&str> = rows.iter().map(|r| r.guid.as_str()).collect();
        assert_eq!(guids, ["v2", "v3"]);

        let year = Window {
            from: TallyDate::parse("20250401").unwrap(),
            to: TallyDate::parse("20260331").unwrap(),
        };
        let statement = |start: &str, end: &str| BankStatementDoc {
            doc_id: "bank:unit".to_string(),
            source_sha256: String::new(),
            account_ref: "XXXXXX0001".to_string(),
            bank: "Invented Bank".to_string(),
            start: TallyDate::parse(start).unwrap(),
            end: TallyDate::parse(end).unwrap(),
            opening_balance_paise: 0,
            closing_balance_paise: 0,
            rows: Vec::new(),
        };
        let rules = Rules::vendored().unwrap();
        let terms = BTreeSet::new();
        let run_on = |s: &BankStatementDoc| run(&book, &rules, &year, s, "Bank", &terms, 7);
        assert!(run_on(&statement("20260301", "20260331")).is_ok());
        for (start, end) in [("20260301", "20260401"), ("20250331", "20250430")] {
            let err = run_on(&statement(start, end)).unwrap_err();
            assert!(
                format!("{err}").contains("not inside the engagement year"),
                "{err}"
            );
        }
        let empty = toml::Value::Array(vec![toml::Value::String(String::new())]);
        assert!(
            charge_terms(Some(&empty)).is_err(),
            "an empty term matches every narration"
        );
    }
}
