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

use std::collections::BTreeSet;

use crate::book::Book;
use crate::error::{AuditError, Result};
use crate::findings::TestResult;
use crate::read::Window;
use crate::rules::Rules;

pub const TEST_ID: &str = "party_monthly";
pub const VERSION: &str = "1";

/// The number of parties shown by name per block; the rest are one "Others" row.
pub const PARTY_TOP_N: usize = 50;

/// Failing first: a `run` and PWM checks that are not ported yet.
pub fn run(
    _book: &Book,
    _rules: &Rules,
    _period: &Window,
    _cash: &BTreeSet<String>,
    _bank: &BTreeSet<String>,
    _top_n: usize,
) -> Result<TestResult> {
    Err(AuditError::Config(format!("{TEST_ID}: not ported yet")))
}

pub fn check_invariants(
    _book: &Book,
    _period: &Window,
    _result: &TestResult,
) -> Result<Vec<String>> {
    Ok(Vec::new())
}
