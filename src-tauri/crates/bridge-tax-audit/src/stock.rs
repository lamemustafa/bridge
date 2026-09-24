// SPDX-License-Identifier: Apache-2.0
//! Port of the reference engine's `stock`: whether closing stock was typed in, the books figure
//! (two ways) against Tally's Stock Summary valuation (two dates), non-goods items, and negative
//! stock at year end and during the year.
//!
//! * "Typed in": population vouchers with a line on a Stock-in-Hand ledger; none means the closing
//!   figure was keyed onto the ledger directly.
//! * The books figure is read off the Trial Balance both ways (Tally's closing field, and opening +
//!   debit - credit); the Stock Summary figures are Tally's item valuation. Gaps are figures, never
//!   a conclusion about which side is right.
//! * A value-only item (BASEUNITS is Tally's reserved "Not Applicable") is left out of every
//!   quantity figure and counted separately when its value is negative.
//! * "Went negative during the year": each item's running quantity from its period-start quantity
//!   (the opening Stock Summary's, else the master's own opening), walked through the population's
//!   inventory lines by date. Same-day order is not in the export, so the walk runs twice (stock-in
//!   first, stock-out first) and each count is a range. A movement's direction is the stock
//!   journal's IN/OUT tag, else the sign of the line's amount; a line with no quantity or no amount
//!   sign moves nothing.
//!
//! Python's floating-point order is kept: quantities are f64 added in the reference's own order
//! (population order, inventory-line order, dates ascending, then a stable sort within a day).
//! The reference's per-item lowest quantities reach nothing `run` emits, so they are not tracked.

use crate::book::Book;
use crate::error::{AuditError, Result};
use crate::findings::TestResult;
use crate::rules::Rules;
use crate::stock_read::StockInputs;

pub const TEST_ID: &str = "stock";
pub const VERSION: &str = "1";

/// Tally's own reserved group name, not client data.
pub const STOCK_IN_HAND_GROUP: &str = "Stock-in-Hand";

/// Float quantity tolerance, as the reference's `QTY_TOL`.
pub const QTY_TOL: f64 = 1e-6;

/// Failing first: a `run` and a STK-1 that are not ported yet.
pub fn run(_book: &Book, _rules: &Rules, _inputs: &StockInputs) -> Result<TestResult> {
    Err(AuditError::Config(format!("{TEST_ID}: not ported yet")))
}

pub fn check_invariants(
    _book: &Book,
    _result: &TestResult,
    _inputs: &StockInputs,
) -> Result<Vec<String>> {
    Ok(Vec::new())
}

#[cfg(test)]
mod tests {
    /// STK-1's message formats quantities as the reference's `f"{x:.3f}"` does: measured equal on
    /// exact binary ties and signed zeros (rustc 1.96, Python 3.13, 2026-09-25); pinned here so a
    /// toolchain change cannot move the dump's text silently.
    #[test]
    fn quantities_format_as_python_fixed_three() {
        let cases = [
            (0.0625, "0.062"),
            (0.0015, "0.002"),
            (-0.0001, "-0.000"),
            (-0.0, "-0.000"),
        ];
        for (x, want) in cases {
            assert_eq!(format!("{x:.3}"), want, "{x}");
        }
    }
}
