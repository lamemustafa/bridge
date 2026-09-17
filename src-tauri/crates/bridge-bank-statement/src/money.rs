//! Amounts, balances and the statement's own arithmetic, in exact decimals.

use crate::bank::{BALANCE, CREDIT, DEBIT};
use crate::parse::Row;
use crate::refusal::Refusal;
use crate::text::strip;
use bridge_tally_primitives::ExactDecimal;
use regex::Regex;
use std::sync::LazyLock;

// ASCII digits only. Python's `\d` also accepts other scripts' decimal digits
// and `Decimal()` reads them; refusing them here is a deliberate, fail-closed
// divergence.
static AMOUNT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[0-9]+(\.[0-9]{1,2})?$").unwrap());
static SIGNED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^-?[0-9]+(\.[0-9]{1,2})?$").unwrap());

fn decimal(
    text: &str,
    pattern: &Regex,
    field: &str,
    row: usize,
    category: &'static str,
) -> Result<Option<ExactDecimal>, Refusal> {
    let text = strip(text);
    if text.is_empty() {
        return Ok(None);
    }
    let refusal = || {
        Refusal::at_row(
            category,
            row,
            format!("row {row}: the {field} cell is not an amount with at most two decimal places"),
        )
    };
    if !pattern.is_match(text) {
        return Err(refusal());
    }
    ExactDecimal::parse(text).map(Some).map_err(|_| refusal())
}

/// A transaction amount: unsigned, at most two decimal places. Blank is `None`;
/// anything else that does not parse is a refusal, never a zero — a misread
/// cell read as zero lets the balance replay "prove" a row it never read.
pub fn money(text: &str, field: &str, row: usize) -> Result<Option<ExactDecimal>, Refusal> {
    decimal(text, &AMOUNT, field, row, "malformed_amount")
}

/// A running balance, which may be negative on an overdrawn account.
pub fn balance(text: &str, field: &str, row: usize) -> Result<Option<ExactDecimal>, Refusal> {
    decimal(text, &SIGNED, field, row, "malformed_balance")
}

/// An operator-supplied control total read off the printed statement.
/// Thousands separators are accepted (`1,00,000.00`); a total of withdrawals
/// or deposits may not be negative.
pub fn control_value(text: &str, name: &str, signed: bool) -> Result<ExactDecimal, Refusal> {
    let cleaned = text.replace(',', "");
    let cleaned = strip(&cleaned);
    let pattern = if signed { &*SIGNED } else { &*AMOUNT };
    let refusal = || {
        Refusal::new(
            "malformed_control_value",
            format!(
                "{name} is not an amount with at most two decimal places{}",
                if signed {
                    ""
                } else {
                    " (a total of withdrawals or deposits is never negative)"
                }
            ),
        )
    };
    if !pattern.is_match(cleaned) {
        return Err(refusal());
    }
    ExactDecimal::parse(cleaned).map_err(|_| refusal())
}

fn arithmetic(error: impl std::fmt::Debug) -> Refusal {
    Refusal::new(
        "arithmetic_out_of_range",
        format!("a running total exceeded the exact-decimal bound ({error:?})"),
    )
}

/// The figures the statement itself prints. The closing balance alone is the
/// NET of the rows, so a dropped tail whose debits and credits cancel still
/// lands on it; the two totals are independent of the net and of each other.
///
/// The totals are optional only because some layouts do not print them (see
/// [`crate::bank::Bank::prints_totals`]); the pipeline refuses to go without
/// them for a layout that does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Controls {
    pub opening: ExactDecimal,
    pub closing: ExactDecimal,
    pub debits: Option<ExactDecimal>,
    pub credits: Option<ExactDecimal>,
}

impl Controls {
    pub fn parse(
        opening: &str,
        closing: &str,
        debits: &str,
        credits: &str,
    ) -> Result<Self, Refusal> {
        Self::parse_optional(opening, closing, Some(debits), Some(credits))
    }

    /// Both totals or neither: one without the other is refused.
    pub fn parse_optional(
        opening: &str,
        closing: &str,
        debits: Option<&str>,
        credits: Option<&str>,
    ) -> Result<Self, Refusal> {
        if debits.is_some() != credits.is_some() {
            return Err(Refusal::new(
                "control_totals_incomplete",
                "supply both total debits and total credits, or neither",
            ));
        }
        Ok(Self {
            opening: control_value(opening, "opening balance", true)?,
            closing: control_value(closing, "closing balance", true)?,
            debits: debits
                .map(|text| control_value(text, "total debits", false))
                .transpose()?,
            credits: credits
                .map(|text| control_value(text, "total credits", false))
                .transpose()?,
        })
    }
}

/// Replay the statement's running balance (`reconcile`). Returns the closing
/// balance.
///
/// This is the parser's correctness proof: a misread digit, a dropped row or a
/// row counted twice breaks the chain. The chain alone cannot see a truncated
/// parse, so the replay must also land on the printed closing balance — and
/// that is still not sufficient, which is why [`verify_against_statement`]
/// exists.
pub fn reconcile(
    rows: &[Row],
    opening: &ExactDecimal,
    expect_closing: &ExactDecimal,
) -> Result<ExactDecimal, Refusal> {
    if rows.is_empty() {
        return Err(Refusal::new(
            "empty_statement",
            "no transaction rows were parsed: the statement is empty, or the layout has changed and the row anchors no longer match",
        ));
    }
    let mut running = opening.clone();
    for (index, row) in rows.iter().enumerate() {
        let number = index + 1;
        // `is_some` asks whether the cell was FILLED, which is what a
        // column-geometry failure looks like. A `0.00` in one column and an
        // amount in the other must still refuse.
        let debit = money(row.get(DEBIT), DEBIT, number)?;
        let credit = money(row.get(CREDIT), CREDIT, number)?;
        if debit.is_some() && credit.is_some() {
            return Err(Refusal::at_row(
                "two_sided_row",
                number,
                format!(
                    "row {number} fills both amount columns. A statement row has one side; this is a column-geometry failure, not a transaction."
                ),
            ));
        }
        let printed = balance(row.get(BALANCE), BALANCE, number)?;
        running = running
            .checked_subtract(&debit.unwrap_or_else(ExactDecimal::zero))
            .and_then(|value| value.checked_add(&credit.unwrap_or_else(ExactDecimal::zero)))
            .map_err(arithmetic)?;
        if !printed.is_some_and(|printed| printed.numeric_eq(&running)) {
            return Err(Refusal::at_row(
                "balance_chain_broken",
                number,
                format!("row {number} breaks the running balance"),
            ));
        }
    }
    if !running.numeric_eq(expect_closing) {
        return Err(Refusal::new(
            "extent_unproven",
            "the replayed chain does not close at the statement's printed closing balance. Every row read reconciles, so rows are missing: the parse stopped early or started late.",
        ));
    }
    Ok(running)
}

/// Debit and credit totals over every parsed row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Totals {
    pub debits: ExactDecimal,
    pub credits: ExactDecimal,
}

/// Debit and credit totals over every parsed row, unchecked.
pub fn statement_totals(rows: &[Row]) -> Result<Totals, Refusal> {
    let mut totals = Totals {
        debits: ExactDecimal::zero(),
        credits: ExactDecimal::zero(),
    };
    for (index, row) in rows.iter().enumerate() {
        if let Some(debit) = money(row.get(DEBIT), DEBIT, index + 1)? {
            totals.debits = totals.debits.checked_add(&debit).map_err(arithmetic)?;
        }
        if let Some(credit) = money(row.get(CREDIT), CREDIT, index + 1)? {
            totals.credits = totals.credits.checked_add(&credit).map_err(arithmetic)?;
        }
    }
    Ok(totals)
}

/// Prove the parse reproduces the statement's printed debit and credit totals
/// (`verify_against_statement`). They are the only check that sees a dropped
/// tail whose two sides cancel.
pub fn verify_against_statement(
    rows: &[Row],
    debits: &ExactDecimal,
    credits: &ExactDecimal,
) -> Result<Totals, Refusal> {
    let totals = statement_totals(rows)?;
    for (label, actual, expected) in [
        ("debits", &totals.debits, debits),
        ("credits", &totals.credits, credits),
    ] {
        if !actual.numeric_eq(expected) {
            return Err(Refusal::new(
                "control_total_mismatch",
                format!(
                    "the {label} total does not match the statement's printed figure. Rows are missing or misread; the closing balance alone cannot see this, because it is the net of the two."
                ),
            ));
        }
    }
    Ok(totals)
}
