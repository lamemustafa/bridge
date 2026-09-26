//! Profit and Loss and Balance Sheet lines derived from one native Trial
//! Balance and the same company's group tree (#692, source B). No Tally I/O
//! belongs in this module.
//!
//! Every figure is a sum of signed Trial Balance amounts, a debit negative
//! (protocol reference §5.6), grouped by the reserved identity of each
//! ledger's primary group: the last hop of its ancestry. A P&L line is the
//! window's movement, `DEBITTOTALS + CREDITTOTALS`; a Balance Sheet line is
//! `TBALCLOSING`. A sum is over the amounts Tally returned; each line counts
//! the empty amounts it left out.
//!
//! A statement that leaves something out must not show a result. So no
//! result is established while any of these holds:
//! - a ledger that cannot be classified carries an amount (it is listed);
//! - a ledger under Stock-in-Hand carries an amount, since closing stock is
//!   not derivable from the Trial Balance;
//! - Tally's own Balance Sheet for the same window does not tie, line for
//!   line, to the derived one. This is the gate for what the Trial Balance
//!   cannot see as at `to`, such as stock valued from stock items (expected,
//!   not yet measured on an inventory book).
//!
//! Gross and net are the window's movement, which the Balance Sheet does not
//! pin: stock held at `from` and gone by `to` could pass it. So where Tally's
//! own Profit and Loss is supplied, gross and net are gated on it too.

use bridge_tally_core::ExactDecimal;
use crate::tally::runtime::SingleCurrencyTrialBalance;
use bridge_tally_protocol::{
    group_ancestry::{AncestryGap, GroupIndex},
    native_statement_reports::{NativeStatement, NativeStatementAmount, NativeStatementKind},
    native_trial_balance::{NativeTrialBalanceAmount, NativeTrialBalanceRow},
    is_tally_reserved_root, TallyNamedMaster,
};
use serde::Serialize;

/// Tally's reserved primary groups, in the spelling a captured group
/// collection returns for `RESERVEDNAME`.
const PROFIT_AND_LOSS_PRIMARY_GROUPS: [&str; 6] = [
    "Sales Accounts",
    "Direct Incomes",
    "Purchase Accounts",
    "Direct Expenses",
    "Indirect Incomes",
    "Indirect Expenses",
];
/// The four whose movement makes up gross profit.
const TRADING_PRIMARY_GROUPS: [&str; 4] = [
    "Sales Accounts",
    "Direct Incomes",
    "Purchase Accounts",
    "Direct Expenses",
];
const BALANCE_SHEET_PRIMARY_GROUPS: [&str; 9] = [
    "Capital Account",
    "Loans (Liability)",
    "Current Liabilities",
    "Suspense A/c",
    "Branch / Divisions",
    "Fixed Assets",
    "Investments",
    "Current Assets",
    "Misc. Expenses (ASSET)",
];
/// A reserved subgroup, matched anywhere in a ledger's ancestry.
const STOCK_IN_HAND_GROUP: &str = "Stock-in-Hand";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StatementSum {
    pub sum: ExactDecimal,
    /// Amounts present in the source.
    pub present_count: usize,
    /// Amounts Tally returned empty, excluded from `sum`.
    pub empty_count: usize,
}

impl StatementSum {
    fn new() -> Self {
        Self {
            sum: ExactDecimal::zero(),
            present_count: 0,
            empty_count: 0,
        }
    }

    fn add(&mut self, amount: &NativeTrialBalanceAmount) -> Result<(), StatementsError> {
        match amount {
            NativeTrialBalanceAmount::Present(value) => {
                self.sum = self
                    .sum
                    .checked_add(value)
                    .map_err(|_| StatementsError::Arithmetic)?;
                self.present_count += 1;
            }
            NativeTrialBalanceAmount::PresentEmpty => self.empty_count += 1,
        }
        Ok(())
    }

    fn merge(&mut self, other: &Self) -> Result<(), StatementsError> {
        self.sum = self
            .sum
            .checked_add(&other.sum)
            .map_err(|_| StatementsError::Arithmetic)?;
        self.present_count += other.present_count;
        self.empty_count += other.empty_count;
        Ok(())
    }
}

/// One primary group's line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PrimaryGroupLine {
    /// Tally's reserved identity, never the book's naming.
    pub reserved_name: &'static str,
    /// The book's own name for that group, as the group collection returned it.
    pub display_name: String,
    pub amount: StatementSum,
    pub ledger_count: usize,
}

/// The reserved-root ledger, Tally's Profit & Loss A/c.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProfitAndLossLedger {
    pub name: String,
    pub closing: NativeTrialBalanceAmount,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnclassifiedLedger {
    pub name: String,
    pub guid: String,
    pub reason: &'static str,
    pub closing: NativeTrialBalanceAmount,
    pub debit: NativeTrialBalanceAmount,
    pub credit: NativeTrialBalanceAmount,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Established {
    Established {
        value: ExactDecimal,
    },
    NotEstablished {
        reason: &'static str,
        /// For `tally_balance_sheet_differs`, the lines that did not tie: a
        /// Tally line's name, or a derived line's display name.
        lines: Vec<String>,
    },
}

impl Established {
    fn blocked(reason: &'static str) -> Self {
        Self::NotEstablished {
            reason,
            lines: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DerivedStatements {
    /// In [`PROFIT_AND_LOSS_PRIMARY_GROUPS`] order: every one the group tree
    /// holds at the root, and any a ledger reached, with or without ledgers.
    pub profit_and_loss: Vec<PrimaryGroupLine>,
    /// In [`BALANCE_SHEET_PRIMARY_GROUPS`] order, on the same terms.
    pub balance_sheet: Vec<PrimaryGroupLine>,
    pub profit_and_loss_ledger: Option<ProfitAndLossLedger>,
    pub unclassified: Vec<UnclassifiedLedger>,
    /// Ledgers under Stock-in-Hand with a present non-zero amount.
    pub stock_ledger_count: usize,
    /// Sales, Direct Incomes, Purchase and Direct Expenses movement; a profit
    /// is positive.
    pub gross_result: Established,
    /// Every P&L line's movement; a profit is positive.
    pub net_result: Established,
    /// The Balance Sheet's Profit & Loss line: the Profit & Loss A/c ledger's
    /// closing plus every P&L ledger's closing.
    pub balance_sheet_profit_and_loss: Established,
    /// Tally's own Balance Sheet against the derived one: the gate.
    pub balance_sheet_tie: TieOut,
    /// Tally's own Profit and Loss against the derived lines, where supplied:
    /// the gate for gross and net.
    pub profit_and_loss_tie: Option<TieOut>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum StatementsError {
    #[error("statement_arithmetic_overflow")]
    Arithmetic,
    /// A row where opening plus the window's debit and credit, an empty amount
    /// taken as zero for this check alone, is not its closing.
    #[error("statement_trial_balance_row_inconsistent")]
    RowInconsistent,
    /// Two ledgers at the Tally root, where only Profit & Loss A/c can sit.
    #[error("statement_root_ledger_repeated")]
    RootLedgerRepeated,
    /// The gate was handed a statement that is not a Balance Sheet.
    #[error("statement_gate_not_a_balance_sheet")]
    GateNotABalanceSheet,
    /// The P&L gate was handed a statement that is not a Profit and Loss.
    #[error("statement_gate_not_a_profit_and_loss")]
    GateNotAProfitAndLoss,
}

impl StatementsError {
    pub fn code(self) -> &'static str {
        match self {
            Self::Arithmetic => "statement_arithmetic_overflow",
            Self::RowInconsistent => "statement_trial_balance_row_inconsistent",
            Self::RootLedgerRepeated => "statement_root_ledger_repeated",
            Self::GateNotABalanceSheet => "statement_gate_not_a_balance_sheet",
            Self::GateNotAProfitAndLoss => "statement_gate_not_a_profit_and_loss",
        }
    }
}

enum Placement {
    ProfitAndLoss(usize, String),
    BalanceSheet(usize, String),
    Root,
    Unclassified(&'static str),
}

/// Derives both statements and gates every result on `tally_balance_sheet`,
/// Tally's own Balance Sheet for the same company and window, and gross and
/// net also on `tally_profit_and_loss` where supplied. Nothing in either
/// response identifies the company (§12a.1); the caller's bracket binds it.
pub fn derive_statements(
    trial_balance: &SingleCurrencyTrialBalance,
    groups: &[TallyNamedMaster],
    tally_balance_sheet: &NativeStatement,
    tally_profit_and_loss: Option<&NativeStatement>,
) -> Result<DerivedStatements, StatementsError> {
    if tally_balance_sheet.kind != NativeStatementKind::BalanceSheet {
        return Err(StatementsError::GateNotABalanceSheet);
    }
    if tally_profit_and_loss.is_some_and(|tally| tally.kind != NativeStatementKind::ProfitAndLoss)
    {
        return Err(StatementsError::GateNotAProfitAndLoss);
    }
    let trial_balance = trial_balance.report();
    let index = GroupIndex::build(groups.iter().cloned());
    let mut profit_and_loss: Vec<Option<PrimaryGroupLine>> =
        vec![None; PROFIT_AND_LOSS_PRIMARY_GROUPS.len()];
    let mut balance_sheet: Vec<Option<PrimaryGroupLine>> =
        vec![None; BALANCE_SHEET_PRIMARY_GROUPS.len()];
    let mut profit_and_loss_closing = StatementSum::new();
    let mut profit_and_loss_ledger = None;
    let mut unclassified = Vec::new();
    let mut stock_ledger_count = 0;

    for row in &trial_balance.rows {
        check_row_identity(row)?;
        let chain = index.ancestry_chain(row.parent.nonempty_returned_text());
        if chain
            .hops
            .iter()
            .any(|hop| hop.reserved_name.trim() == STOCK_IN_HAND_GROUP)
            && [&row.opening, &row.debit, &row.credit, &row.closing]
                .iter()
                .any(|amount| present_nonzero(amount))
        {
            stock_ledger_count += 1;
        }
        match place(&chain) {
            Placement::ProfitAndLoss(slot, display_name) => {
                let line = profit_and_loss[slot].get_or_insert_with(|| PrimaryGroupLine {
                    reserved_name: PROFIT_AND_LOSS_PRIMARY_GROUPS[slot],
                    display_name,
                    amount: StatementSum::new(),
                    ledger_count: 0,
                });
                line.amount.add(&row.debit)?;
                line.amount.add(&row.credit)?;
                line.ledger_count += 1;
                profit_and_loss_closing.add(&row.closing)?;
            }
            Placement::BalanceSheet(slot, display_name) => {
                let line = balance_sheet[slot].get_or_insert_with(|| PrimaryGroupLine {
                    reserved_name: BALANCE_SHEET_PRIMARY_GROUPS[slot],
                    display_name,
                    amount: StatementSum::new(),
                    ledger_count: 0,
                });
                line.amount.add(&row.closing)?;
                line.ledger_count += 1;
            }
            Placement::Root => {
                if profit_and_loss_ledger.is_some() {
                    return Err(StatementsError::RootLedgerRepeated);
                }
                profit_and_loss_ledger = Some(ProfitAndLossLedger {
                    name: row.name.clone(),
                    closing: row.closing.clone(),
                });
            }
            Placement::Unclassified(reason) => unclassified.push(UnclassifiedLedger {
                name: row.name.clone(),
                guid: row.guid.clone(),
                reason,
                closing: row.closing.clone(),
                debit: row.debit.clone(),
                credit: row.credit.clone(),
            }),
        }
    }

    // Tally's own statements list a primary group with no ledger under it, so
    // the derived side does too, from the group tree itself.
    for group in groups {
        if !group
            .parent
            .nonempty_returned_text()
            .is_some_and(is_tally_reserved_root)
        {
            continue;
        }
        let Some(reserved) = group.reserved_name.as_deref().map(str::trim) else {
            continue;
        };
        let empty_line = |reserved_name| PrimaryGroupLine {
            reserved_name,
            display_name: group.name.clone(),
            amount: StatementSum::new(),
            ledger_count: 0,
        };
        if let Some(slot) = PROFIT_AND_LOSS_PRIMARY_GROUPS.iter().position(|name| *name == reserved) {
            profit_and_loss[slot].get_or_insert_with(|| empty_line(PROFIT_AND_LOSS_PRIMARY_GROUPS[slot]));
        } else if let Some(slot) = BALANCE_SHEET_PRIMARY_GROUPS.iter().position(|name| *name == reserved) {
            balance_sheet[slot].get_or_insert_with(|| empty_line(BALANCE_SHEET_PRIMARY_GROUPS[slot]));
        }
    }
    let profit_and_loss: Vec<_> = profit_and_loss.into_iter().flatten().collect();
    let balance_sheet: Vec<_> = balance_sheet.into_iter().flatten().collect();
    let omitted = unclassified.iter().any(|ledger| {
        [&ledger.closing, &ledger.debit, &ledger.credit]
            .iter()
            .any(|amount| present_nonzero(amount))
    });
    // Stock blocks the carried line too: Tally's carries the change in stock.
    let blocked = if omitted {
        Some("unclassified_ledger_carries_an_amount")
    } else if stock_ledger_count > 0 {
        Some("closing_stock_not_derivable_from_trial_balance")
    } else if profit_and_loss_ledger.is_none() {
        // Tally's carried line then has nothing to tie to; name the cause.
        Some("profit_and_loss_ledger_not_returned")
    } else {
        None
    };
    let sum_of = |filter: &dyn Fn(&str) -> bool| -> Result<ExactDecimal, StatementsError> {
        let mut total = StatementSum::new();
        for line in profit_and_loss.iter().filter(|line| filter(line.reserved_name)) {
            total.merge(&line.amount)?;
        }
        Ok(total.sum)
    };
    let gross = sum_of(&|name| TRADING_PRIMARY_GROUPS.contains(&name))?;
    let net = sum_of(&|_| true)?;
    let carried = match &profit_and_loss_ledger {
        None => Established::blocked("profit_and_loss_ledger_not_returned"),
        Some(ledger) => {
            let mut total = profit_and_loss_closing;
            total.add(&ledger.closing)?;
            Established::Established { value: total.sum }
        }
    };

    // The gate compares the candidate figures before any is established.
    let balance_sheet_tie = tie_lines(
        &balance_sheet,
        profit_and_loss_ledger
            .as_ref()
            .map(|ledger| (ledger.name.as_str(), &carried)),
        tally_balance_sheet,
    );
    let failures = gate_failures(&balance_sheet_tie);
    let gated = |value: ExactDecimal| match blocked {
        Some(reason) => Established::blocked(reason),
        None if !failures.is_empty() => Established::NotEstablished {
            reason: "tally_balance_sheet_differs",
            lines: failures.clone(),
        },
        None => Established::Established { value },
    };
    // Gross and net are gated on Tally's own Profit and Loss too, where supplied.
    let cost_of_sales = sum_of(&|name| COST_OF_SALES_GROUPS.contains(&name))?;
    let profit_and_loss_tie =
        tally_profit_and_loss.map(|tally| tie_lines(&profit_and_loss, None, tally));
    let profit_and_loss_failures = profit_and_loss_tie
        .as_ref()
        .map(|tie| profit_and_loss_gate_failures(tie, &cost_of_sales))
        .unwrap_or_default();
    let movement_gated = |value: ExactDecimal| match gated(value) {
        Established::Established { .. } if !profit_and_loss_failures.is_empty() => {
            Established::NotEstablished {
                reason: "tally_profit_and_loss_differs",
                lines: profit_and_loss_failures.clone(),
            }
        }
        result => result,
    };
    let gross_result = movement_gated(gross);
    let net_result = movement_gated(net);
    let balance_sheet_profit_and_loss = match carried {
        Established::Established { value } => gated(value),
        blocked_carried => match blocked {
            Some(reason) => Established::blocked(reason),
            None => blocked_carried,
        },
    };

    Ok(DerivedStatements {
        profit_and_loss,
        balance_sheet,
        profit_and_loss_ledger,
        unclassified,
        stock_ledger_count,
        gross_result,
        net_result,
        balance_sheet_profit_and_loss,
        balance_sheet_tie,
        profit_and_loss_tie,
    })
}

/// Every Balance Sheet line that does not tie: a Tally line that differs, or
/// that carries an amount nothing derived was compared with, and a derived
/// line with an amount that Tally does not show.
fn gate_failures(tie: &TieOut) -> Vec<String> {
    let mut failures: Vec<String> = tie
        .lines
        .iter()
        .filter(|line| match &line.status {
            TieStatus::Matched | TieStatus::MatchedEmptyAsZero => false,
            TieStatus::Differs { .. } => true,
            TieStatus::NotCompared { .. } => carries_an_amount(line),
        })
        .map(|line| line.name.clone())
        .collect();
    failures.extend(tie.derived_only.iter().cloned());
    failures
}

/// Tally's P&L heading for the trading section, spelled byte for byte as
/// captured once on licensed 7.1 (the full-year lab capture). It is the one
/// uncompared line allowed an amount, and only while that amount is exactly
/// the derived Purchase Accounts and Direct Expenses: the cost of sales with
/// no stock. Any other spelling is uncompared, and an amount there refuses.
const COST_OF_SALES_HEADING: &str = "Cost of Sales :";
const COST_OF_SALES_GROUPS: [&str; 2] = ["Purchase Accounts", "Direct Expenses"];

/// As [`gate_failures`], for Tally's own Profit and Loss, with the one
/// heading allowed when its amount is the derived cost of sales. A stock line
/// (Opening or Closing Stock) is uncompared and refuses, and stock also moves
/// the heading off the derived cost of sales.
fn profit_and_loss_gate_failures(tie: &TieOut, cost_of_sales: &ExactDecimal) -> Vec<String> {
    let mut failures: Vec<String> = tie
        .lines
        .iter()
        .filter(|line| match &line.status {
            TieStatus::Matched | TieStatus::MatchedEmptyAsZero => false,
            TieStatus::Differs { .. } => true,
            TieStatus::NotCompared { .. } if !carries_an_amount(line) => false,
            TieStatus::NotCompared { .. } if line.name == COST_OF_SALES_HEADING => {
                !single_amount(line).is_some_and(|amount| amount.numeric_eq(cost_of_sales))
            }
            TieStatus::NotCompared { .. } => true,
        })
        .map(|line| line.name.clone())
        .collect();
    failures.extend(tie.derived_only.iter().cloned());
    failures
}

fn carries_an_amount(line: &TieLine) -> bool {
    [&line.tally_sub, &line.tally_main]
        .iter()
        .any(|amount| matches!(amount, NativeStatementAmount::Present(value) if !value.is_zero()))
}

/// The line's amount when exactly one of its two columns is present.
fn single_amount(line: &TieLine) -> Option<&ExactDecimal> {
    match (&line.tally_sub, &line.tally_main) {
        (NativeStatementAmount::Present(value), NativeStatementAmount::Empty)
        | (NativeStatementAmount::Empty, NativeStatementAmount::Present(value)) => Some(value),
        _ => None,
    }
}

fn place(chain: &bridge_tally_protocol::group_ancestry::AncestryChain) -> Placement {
    if let Some(gap) = chain.gap {
        return Placement::Unclassified(gap_reason(gap));
    }
    let Some(primary) = chain.hops.last() else {
        return Placement::Root;
    };
    let reserved = primary.reserved_name.trim();
    if reserved.is_empty() {
        return Placement::Unclassified("primary_group_user_created");
    }
    if let Some(slot) = PROFIT_AND_LOSS_PRIMARY_GROUPS
        .iter()
        .position(|name| *name == reserved)
    {
        return Placement::ProfitAndLoss(slot, primary.name.clone());
    }
    if let Some(slot) = BALANCE_SHEET_PRIMARY_GROUPS
        .iter()
        .position(|name| *name == reserved)
    {
        return Placement::BalanceSheet(slot, primary.name.clone());
    }
    Placement::Unclassified("primary_group_reserved_name_unknown")
}

fn gap_reason(gap: AncestryGap) -> &'static str {
    match gap {
        AncestryGap::NoParent => "parent_not_returned",
        AncestryGap::ReachedRoot => "reached_root",
        AncestryGap::GroupAbsent => "group_absent",
        AncestryGap::GroupNameRepeated => "group_name_repeated",
        AncestryGap::ReservedNameMissing => "reserved_name_missing",
        AncestryGap::Cycle => "group_cycle",
        AncestryGap::Exhausted => "group_chain_exhausted",
    }
}

fn present_nonzero(amount: &NativeTrialBalanceAmount) -> bool {
    matches!(amount, NativeTrialBalanceAmount::Present(value) if !value.is_zero())
}

/// Opening plus the window's debit and credit is the closing (§5.6).
///
/// Only here is an empty amount taken as zero, and only because this check can
/// refuse a read but never produce a figure. Required all four present, it
/// fired on none of the rows in either tie capture; this way it holds on every
/// row of every captured Trial Balance, and it catches a P&L row whose closing
/// carries an amount its movement columns do not.
fn check_row_identity(row: &NativeTrialBalanceRow) -> Result<(), StatementsError> {
    let zero = ExactDecimal::zero();
    let value = |amount: &NativeTrialBalanceAmount| match amount {
        NativeTrialBalanceAmount::Present(value) => value.clone(),
        NativeTrialBalanceAmount::PresentEmpty => zero.clone(),
    };
    let moved = value(&row.opening)
        .checked_add(&value(&row.debit))
        .and_then(|total| total.checked_add(&value(&row.credit)))
        .map_err(|_| StatementsError::Arithmetic)?;
    if moved.numeric_eq(&value(&row.closing)) {
        Ok(())
    } else {
        Err(StatementsError::RowInconsistent)
    }
}

/// How one of Tally's own statement lines compares with the derived figures.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum TieStatus {
    Matched,
    /// Tally left the line empty, and the derived sum is zero.
    MatchedEmptyAsZero,
    Differs { derived: ExactDecimal },
    NotCompared { reason: &'static str },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TieLine {
    pub name: String,
    pub tally_sub: NativeStatementAmount,
    pub tally_main: NativeStatementAmount,
    #[serde(flatten)]
    pub status: TieStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TieOut {
    pub lines: Vec<TieLine>,
    /// Derived lines with a present non-zero amount that no Tally line names.
    pub derived_only: Vec<String>,
}

fn tie_lines(
    lines: &[PrimaryGroupLine],
    carried: Option<(&str, &Established)>,
    builtin: &NativeStatement,
) -> TieOut {
    let mut named = Vec::new();
    let tie_lines = builtin
        .lines
        .iter()
        .map(|line| {
            let status = if let Some(group) = lines.iter().find(|group| group.display_name == line.name) {
                named.push(group.display_name.as_str());
                compare(&line.sub, &line.main, &group.amount.sum)
            } else if let Some((_, result)) = carried.filter(|(name, _)| *name == line.name) {
                match result {
                    Established::Established { value } => compare(&line.sub, &line.main, value),
                    Established::NotEstablished { reason, .. } => TieStatus::NotCompared { reason },
                }
            } else {
                TieStatus::NotCompared {
                    reason: "no_derived_line_of_that_name",
                }
            };
            TieLine {
                name: line.name.clone(),
                tally_sub: line.sub.clone(),
                tally_main: line.main.clone(),
                status,
            }
        })
        .collect();
    let derived_only = lines
        .iter()
        .filter(|group| !named.contains(&group.display_name.as_str()) && !group.amount.sum.is_zero())
        .map(|group| group.display_name.clone())
        .collect();
    TieOut {
        lines: tie_lines,
        derived_only,
    }
}

fn compare(sub: &NativeStatementAmount, main: &NativeStatementAmount, derived: &ExactDecimal) -> TieStatus {
    let tally = match (sub, main) {
        (NativeStatementAmount::Present(value), NativeStatementAmount::Empty)
        | (NativeStatementAmount::Empty, NativeStatementAmount::Present(value)) => value,
        (NativeStatementAmount::Empty, NativeStatementAmount::Empty) => {
            return if derived.is_zero() {
                TieStatus::MatchedEmptyAsZero
            } else {
                TieStatus::Differs {
                    derived: derived.clone(),
                }
            };
        }
        (NativeStatementAmount::Present(_), NativeStatementAmount::Present(_)) => {
            return TieStatus::NotCompared {
                reason: "both_tally_columns_present",
            };
        }
    };
    if tally.numeric_eq(derived) {
        TieStatus::Matched
    } else {
        TieStatus::Differs {
            derived: derived.clone(),
        }
    }
}

#[cfg(test)]
#[path = "statements_tests.rs"]
mod tests;
