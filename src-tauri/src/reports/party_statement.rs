//! Builds one party's statement from data Bridge already holds.
//!
//! `fetch_tally_outstandings` already returns every open bill and every
//! party's unallocated balance in `OutstandingsLoadResult::Complete`. This
//! module issues no Tally request of its own -- it only reshapes rows the UI
//! already has in hand into the numbers for one party's statement.

use bridge_tally_core::ExactDecimal;

use crate::tally::{ExposureDirection, OpenBillRow, OutstandingsAgeingAnchor, UnallocatedParty};

/// Which ageing bucket a bill's age falls into. Boundaries match
/// `bridge_tally_protocol::native_outstandings::compute` exactly, so a
/// statement's bucket for a bill agrees with the ageing panel the bill was
/// drilled down from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgeingBucket {
    Days0To30,
    Days31To60,
    Days61To90,
    Days90Plus,
}

impl AgeingBucket {
    fn for_age(age_days: u32) -> Self {
        match age_days {
            0..=30 => Self::Days0To30,
            31..=60 => Self::Days31To60,
            61..=90 => Self::Days61To90,
            _ => Self::Days90Plus,
        }
    }

    /// Short label for both the bill table's Bucket column and any
    /// per-bucket summary.
    pub fn label(self) -> &'static str {
        match self {
            Self::Days0To30 => "0-30 days",
            Self::Days31To60 => "31-60 days",
            Self::Days61To90 => "61-90 days",
            Self::Days90Plus => "90+ days",
        }
    }
}

/// One bill on a party's statement, carrying the bucket it was sorted into
/// so the spreadsheet writer never has to recompute it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatementBill {
    pub reference: String,
    pub bill_date: String,
    pub due_date: String,
    pub amount: ExactDecimal,
    pub age_days: Option<u32>,
    /// Balance direction -- see `OpenBillRow::kind`.
    pub kind: ExposureDirection,
    pub bucket: Option<AgeingBucket>,
}

/// Exact ageing subtotals for one direction in a party's bill table.
///
/// Bills without a meaningful age because they are not yet due remain
/// separate from 0-30 days: folding them into an aged bucket would misstate
/// the party's ageing position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectionalAgeingSubtotals {
    pub not_yet_due: ExactDecimal,
    pub days_0_30: ExactDecimal,
    pub days_31_60: ExactDecimal,
    pub days_61_90: ExactDecimal,
    pub days_90_plus: ExactDecimal,
}

impl DirectionalAgeingSubtotals {
    fn zero() -> Self {
        Self {
            not_yet_due: ExactDecimal::zero(),
            days_0_30: ExactDecimal::zero(),
            days_31_60: ExactDecimal::zero(),
            days_61_90: ExactDecimal::zero(),
            days_90_plus: ExactDecimal::zero(),
        }
    }

    /// Sums every rendered ageing category through checked exact-decimal
    /// addition so a statement cannot appear reconciled through a lossy
    /// display-only conversion.
    pub fn total(&self) -> Result<ExactDecimal, PartyStatementError> {
        self.not_yet_due
            .checked_add(&self.days_0_30)
            .and_then(|value| value.checked_add(&self.days_31_60))
            .and_then(|value| value.checked_add(&self.days_61_90))
            .and_then(|value| value.checked_add(&self.days_90_plus))
            .map_err(|_| PartyStatementError::ArithmeticOverflow)
    }
}

/// Exact ageing subtotals split by the direction shown on each bill row.
/// Positive Tally magnitudes are not netted: [`AgeingSubtotals::total`]
/// remains exactly equal to [`PartyStatement::bill_total`], while every
/// rendered bucket makes its receivable/payable direction explicit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgeingSubtotals {
    pub receivable: DirectionalAgeingSubtotals,
    pub payable: DirectionalAgeingSubtotals,
}

impl AgeingSubtotals {
    /// Sums every rendered directional ageing category through checked exact-
    /// decimal addition so a statement cannot appear reconciled through a
    /// lossy display-only conversion.
    pub fn total(&self) -> Result<ExactDecimal, PartyStatementError> {
        let receivable = self.receivable.total()?;
        let payable = self.payable.total()?;
        receivable
            .checked_add(&payable)
            .map_err(|_| PartyStatementError::ArithmeticOverflow)
    }

    pub fn by_direction(&self) -> [(&'static str, &DirectionalAgeingSubtotals); 2] {
        [("Receivable", &self.receivable), ("Payable", &self.payable)]
    }
}

/// One party's statement: its bills, their ageing subtotals, its
/// unallocated exposure, and the grand total of the two.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartyStatement {
    pub company: String,
    pub party: String,
    pub as_of_yyyymmdd: String,
    /// The operator-selected basis that produced every bill age and bucket in
    /// this statement. Renderers must disclose this rather than assume due date.
    pub ageing_anchor: OutstandingsAgeingAnchor,
    /// Oldest bill first (largest `age_days` first), matching the order the
    /// party's drill-down panel already shows.
    pub bills: Vec<StatementBill>,
    pub subtotals: AgeingSubtotals,
    /// Sum of every row in `bills`. Excludes `unallocated` deliberately --
    /// see `PartyStatement::grand_total`.
    pub bill_total: ExactDecimal,
    /// Exposure on this party's ledger with no bill reference. Zero when the
    /// party has none, distinguished from "unknown" because the caller
    /// always has the full `unallocated_by_party` slice in hand -- there is
    /// no "not computed" case at this layer, unlike the report-level total.
    pub unallocated: ExactDecimal,
    pub unallocated_direction: Option<ExposureDirection>,
    /// `bill_total + unallocated`. Kept as a field, not left for a caller to
    /// recompute, so every consumer of a `PartyStatement` sees the same
    /// figure the exact-decimal addition actually produced.
    pub grand_total: ExactDecimal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PartyStatementError {
    /// Neither an open bill nor an unallocated balance names this party --
    /// nothing to put in a statement. Most likely a stale party name from a
    /// UI that has since refreshed.
    #[error("Bridge has no recorded exposure for this party")]
    PartyNotFound,
    /// An amount could not be totalled without exceeding `ExactDecimal`'s
    /// bound. Not expected on any book actually observed, but the addition
    /// is checked rather than assumed to succeed.
    #[error("a statement amount could not be totalled exactly")]
    ArithmeticOverflow,
}

/// Builds one party's statement from the `open_bills` and
/// `unallocated_by_party` slices `OutstandingsLoadResult::Complete` already
/// carries. Pure: no I/O, no Tally request, and no reliance on anything
/// beyond its arguments.
pub fn build_party_statement(
    company: &str,
    as_of_yyyymmdd: &str,
    party: &str,
    open_bills: &[OpenBillRow],
    unallocated_by_party: &[UnallocatedParty],
) -> Result<PartyStatement, PartyStatementError> {
    build_party_statement_with_ageing_anchor(
        company,
        as_of_yyyymmdd,
        party,
        open_bills,
        unallocated_by_party,
        OutstandingsAgeingAnchor::DueDate,
    )
}

/// Builds a statement with the selected report basis retained for every
/// client-facing renderer. The compatibility wrapper above is due-date only
/// for existing internal callers that predate the selector.
pub fn build_party_statement_with_ageing_anchor(
    company: &str,
    as_of_yyyymmdd: &str,
    party: &str,
    open_bills: &[OpenBillRow],
    unallocated_by_party: &[UnallocatedParty],
    ageing_anchor: OutstandingsAgeingAnchor,
) -> Result<PartyStatement, PartyStatementError> {
    let mut bills: Vec<StatementBill> = open_bills
        .iter()
        .filter(|row| row.party == party)
        .map(|row| StatementBill {
            reference: row.reference.clone(),
            bill_date: row.bill_date.clone(),
            due_date: row.due_date.clone(),
            amount: row.amount.clone(),
            age_days: row.age_days,
            kind: row.kind,
            bucket: row.age_days.map(AgeingBucket::for_age),
        })
        .collect();
    // Oldest first: largest age first, then the same tie-breaks
    // `open_bill_rows` uses so a party's statement lists bills in the same
    // order its drill-down panel already showed them in.
    bills.sort_by(|left, right| {
        right
            .age_days
            .cmp(&left.age_days)
            .then_with(|| left.due_date.cmp(&right.due_date))
            .then_with(|| left.reference.cmp(&right.reference))
    });

    let (unallocated, unallocated_direction) = unallocated_by_party
        .iter()
        .find(|entry| entry.party == party)
        .map(|entry| (entry.amount.clone(), Some(entry.direction)))
        .unwrap_or_else(|| (ExactDecimal::zero(), None));

    if bills.is_empty() && unallocated.is_zero() {
        return Err(PartyStatementError::PartyNotFound);
    }

    let mut subtotals = AgeingSubtotals {
        receivable: DirectionalAgeingSubtotals::zero(),
        payable: DirectionalAgeingSubtotals::zero(),
    };
    let mut bill_total = ExactDecimal::zero();
    for bill in &bills {
        bill_total = bill_total
            .checked_add(&bill.amount)
            .map_err(|_| PartyStatementError::ArithmeticOverflow)?;
        let directional_subtotals = match bill.kind {
            ExposureDirection::Receivable => &mut subtotals.receivable,
            ExposureDirection::Payable => &mut subtotals.payable,
        };
        let bucket_subtotal = match bill.bucket {
            Some(AgeingBucket::Days0To30) => &mut directional_subtotals.days_0_30,
            Some(AgeingBucket::Days31To60) => &mut directional_subtotals.days_31_60,
            Some(AgeingBucket::Days61To90) => &mut directional_subtotals.days_61_90,
            Some(AgeingBucket::Days90Plus) => &mut directional_subtotals.days_90_plus,
            None => &mut directional_subtotals.not_yet_due,
        };
        *bucket_subtotal = bucket_subtotal
            .checked_add(&bill.amount)
            .map_err(|_| PartyStatementError::ArithmeticOverflow)?;
    }

    let grand_total = bill_total
        .checked_add(&unallocated)
        .map_err(|_| PartyStatementError::ArithmeticOverflow)?;

    Ok(PartyStatement {
        company: company.to_string(),
        party: party.to_string(),
        as_of_yyyymmdd: as_of_yyyymmdd.to_string(),
        ageing_anchor,
        bills,
        subtotals,
        bill_total,
        unallocated,
        unallocated_direction,
        grand_total,
    })
}

#[cfg(test)]
#[path = "party_statement_tests.rs"]
mod tests;
