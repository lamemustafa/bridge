//! Builds a complete, dual-ageing working paper from one finished native
//! outstandings read. This module performs no Tally I/O.

use std::collections::{BTreeMap, BTreeSet};

use bridge_tally_core::{ExactDecimal, TallyDate};

use crate::tally::{
    ExposureDirection, OpenBillRow, OutstandingsAgeingAnchor, OutstandingsCurrencyAssertion,
    UnallocatedParty,
};

#[derive(Debug, Clone)]
pub struct OutstandingsWorkingPaperSource {
    pub company: String,
    pub company_guid: String,
    pub as_of_yyyymmdd: String,
    pub currency_assertion: OutstandingsCurrencyAssertion,
    pub synced_at_unix_ms: i64,
    pub source_bytes: usize,
    pub source_ageing_anchor: OutstandingsAgeingAnchor,
    pub receivable_bill_total: ExactDecimal,
    pub payable_bill_total: ExactDecimal,
    pub unallocated_total: ExactDecimal,
    pub open_bills: Vec<OpenBillRow>,
    pub unallocated_by_party: Vec<UnallocatedParty>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DualAgeBillRow {
    pub party: String,
    pub reference: String,
    pub bill_date: TallyDate,
    pub due_date: TallyDate,
    pub direction: ExposureDirection,
    pub amount: ExactDecimal,
    pub bill_age_days: Option<u32>,
    pub due_age_days: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartyWorkingPaperRow {
    pub party: String,
    pub receivable_bills: ExactDecimal,
    pub payable_bills: ExactDecimal,
    pub receivable_unallocated: ExactDecimal,
    pub payable_unallocated: ExactDecimal,
    pub receivable_total: ExactDecimal,
    pub payable_total: ExactDecimal,
    pub outstanding_total: ExactDecimal,
    pub oldest_bill_age_days: Option<u32>,
    pub oldest_due_age_days: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkingPaperControls {
    pub receivable_bills: ExactDecimal,
    pub payable_bills: ExactDecimal,
    pub receivable_unallocated: ExactDecimal,
    pub payable_unallocated: ExactDecimal,
    pub receivable_total: ExactDecimal,
    pub payable_total: ExactDecimal,
    pub outstanding_total: ExactDecimal,
    pub bill_date_ageing: DirectionalAgeingControls,
    pub due_date_ageing: DirectionalAgeingControls,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgeingBucketControls {
    pub date_not_reached: ExactDecimal,
    pub days_0_30: ExactDecimal,
    pub days_31_60: ExactDecimal,
    pub days_61_90: ExactDecimal,
    pub days_90_plus: ExactDecimal,
}

impl Default for AgeingBucketControls {
    fn default() -> Self {
        Self {
            date_not_reached: ExactDecimal::zero(),
            days_0_30: ExactDecimal::zero(),
            days_31_60: ExactDecimal::zero(),
            days_61_90: ExactDecimal::zero(),
            days_90_plus: ExactDecimal::zero(),
        }
    }
}

impl AgeingBucketControls {
    fn total(&self) -> Result<ExactDecimal, OutstandingsWorkingPaperError> {
        self.date_not_reached
            .checked_add(&self.days_0_30)
            .and_then(|value| value.checked_add(&self.days_31_60))
            .and_then(|value| value.checked_add(&self.days_61_90))
            .and_then(|value| value.checked_add(&self.days_90_plus))
            .map_err(|_| OutstandingsWorkingPaperError::ArithmeticOverflow)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DirectionalAgeingControls {
    pub receivable: AgeingBucketControls,
    pub payable: AgeingBucketControls,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutstandingsWorkingPaper {
    company: String,
    company_guid: String,
    as_of: TallyDate,
    currency_assertion: OutstandingsCurrencyAssertion,
    synced_at_unix_ms: i64,
    source_bytes: usize,
    source_ageing_anchor: OutstandingsAgeingAnchor,
    parties: Vec<PartyWorkingPaperRow>,
    bills: Vec<DualAgeBillRow>,
    controls: WorkingPaperControls,
}

impl OutstandingsWorkingPaper {
    pub(crate) fn company(&self) -> &str {
        &self.company
    }

    pub(crate) fn as_of(&self) -> &TallyDate {
        &self.as_of
    }

    pub(super) fn company_guid(&self) -> &str {
        &self.company_guid
    }

    pub(super) const fn currency_assertion(&self) -> OutstandingsCurrencyAssertion {
        self.currency_assertion
    }

    pub(super) const fn synced_at_unix_ms(&self) -> i64 {
        self.synced_at_unix_ms
    }

    pub(super) const fn source_bytes(&self) -> usize {
        self.source_bytes
    }

    pub(super) const fn source_ageing_anchor(&self) -> OutstandingsAgeingAnchor {
        self.source_ageing_anchor
    }

    pub(super) fn parties(&self) -> &[PartyWorkingPaperRow] {
        &self.parties
    }

    pub(super) fn bills(&self) -> &[DualAgeBillRow] {
        &self.bills
    }

    pub(super) fn controls(&self) -> &WorkingPaperControls {
        &self.controls
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum OutstandingsWorkingPaperError {
    #[error("working-paper company identity is missing")]
    MissingCompanyIdentity,
    #[error("working-paper party identity is missing")]
    MissingPartyIdentity,
    #[error("working-paper date is invalid ({0})")]
    InvalidDate(String),
    #[error("working-paper amount is negative")]
    NegativeAmount,
    #[error("working-paper source contains a zero-value exposure row")]
    ZeroExposureRow,
    #[error("working-paper source repeats an unallocated party")]
    DuplicateUnallocatedParty,
    #[error("working-paper selected ageing does not match its source dates")]
    SourceAgeMismatch,
    #[error("working-paper exact arithmetic overflowed")]
    ArithmeticOverflow,
    #[error("working-paper {0} control does not reconcile")]
    ControlMismatch(&'static str),
}

struct PartyAccumulator {
    receivable_bills: ExactDecimal,
    payable_bills: ExactDecimal,
    receivable_unallocated: ExactDecimal,
    payable_unallocated: ExactDecimal,
    oldest_bill_age_days: Option<u32>,
    oldest_due_age_days: Option<u32>,
}

impl Default for PartyAccumulator {
    fn default() -> Self {
        Self {
            receivable_bills: ExactDecimal::zero(),
            payable_bills: ExactDecimal::zero(),
            receivable_unallocated: ExactDecimal::zero(),
            payable_unallocated: ExactDecimal::zero(),
            oldest_bill_age_days: None,
            oldest_due_age_days: None,
        }
    }
}

/// Builds exact party and global controls from the complete source vectors.
/// Receivable and payable magnitudes remain separate and are never netted.
pub fn build_outstandings_working_paper(
    source: OutstandingsWorkingPaperSource,
) -> Result<OutstandingsWorkingPaper, OutstandingsWorkingPaperError> {
    if source.company.trim().is_empty() || source.company_guid.trim().is_empty() {
        return Err(OutstandingsWorkingPaperError::MissingCompanyIdentity);
    }
    require_non_negative(&source.receivable_bill_total)?;
    require_non_negative(&source.payable_bill_total)?;
    require_non_negative(&source.unallocated_total)?;

    let as_of = parse_date(&source.as_of_yyyymmdd)?;
    let mut parties = BTreeMap::<String, PartyAccumulator>::new();
    let mut bills = Vec::with_capacity(source.open_bills.len());
    let mut receivable_bills = ExactDecimal::zero();
    let mut payable_bills = ExactDecimal::zero();
    let mut bill_date_ageing = DirectionalAgeingControls::default();
    let mut due_date_ageing = DirectionalAgeingControls::default();

    for row in source.open_bills {
        if row.party.trim().is_empty() {
            return Err(OutstandingsWorkingPaperError::MissingPartyIdentity);
        }
        require_non_negative(&row.amount)?;
        require_non_zero_row(&row.amount)?;
        let bill_date = parse_date(&row.bill_date)?;
        let due_date = parse_date(&row.due_date)?;
        let bill_age_days = age_on_or_before(&bill_date, &as_of)?;
        let due_age_days = age_on_or_before(&due_date, &as_of)?;
        let selected_age_days = match source.source_ageing_anchor {
            OutstandingsAgeingAnchor::BillDate => bill_age_days,
            OutstandingsAgeingAnchor::DueDate => due_age_days,
        };
        if row.age_days != selected_age_days {
            return Err(OutstandingsWorkingPaperError::SourceAgeMismatch);
        }
        let party = parties.entry(row.party.clone()).or_default();
        party.oldest_bill_age_days = max_age(party.oldest_bill_age_days, bill_age_days);
        party.oldest_due_age_days = max_age(party.oldest_due_age_days, due_age_days);
        match row.kind {
            ExposureDirection::Receivable => {
                checked_add_assign(&mut receivable_bills, &row.amount)?;
                checked_add_assign(&mut party.receivable_bills, &row.amount)?;
                add_ageing_amount(&mut bill_date_ageing.receivable, bill_age_days, &row.amount)?;
                add_ageing_amount(&mut due_date_ageing.receivable, due_age_days, &row.amount)?;
            }
            ExposureDirection::Payable => {
                checked_add_assign(&mut payable_bills, &row.amount)?;
                checked_add_assign(&mut party.payable_bills, &row.amount)?;
                add_ageing_amount(&mut bill_date_ageing.payable, bill_age_days, &row.amount)?;
                add_ageing_amount(&mut due_date_ageing.payable, due_age_days, &row.amount)?;
            }
        }
        bills.push(DualAgeBillRow {
            party: row.party,
            reference: row.reference,
            bill_date,
            due_date,
            direction: row.kind,
            amount: row.amount,
            bill_age_days,
            due_age_days,
        });
    }

    require_equal(
        &receivable_bills,
        &source.receivable_bill_total,
        "receivable bill",
    )?;
    require_equal(&payable_bills, &source.payable_bill_total, "payable bill")?;
    require_equal(
        &bill_date_ageing.receivable.total()?,
        &receivable_bills,
        "bill-date receivable ageing",
    )?;
    require_equal(
        &bill_date_ageing.payable.total()?,
        &payable_bills,
        "bill-date payable ageing",
    )?;
    require_equal(
        &due_date_ageing.receivable.total()?,
        &receivable_bills,
        "due-date receivable ageing",
    )?;
    require_equal(
        &due_date_ageing.payable.total()?,
        &payable_bills,
        "due-date payable ageing",
    )?;

    let mut receivable_unallocated = ExactDecimal::zero();
    let mut payable_unallocated = ExactDecimal::zero();
    let mut unallocated_parties = BTreeSet::new();
    for row in source.unallocated_by_party {
        if row.party.trim().is_empty() {
            return Err(OutstandingsWorkingPaperError::MissingPartyIdentity);
        }
        if !unallocated_parties.insert(row.party.clone()) {
            return Err(OutstandingsWorkingPaperError::DuplicateUnallocatedParty);
        }
        require_non_negative(&row.amount)?;
        require_non_zero_row(&row.amount)?;
        let party = parties.entry(row.party).or_default();
        match row.direction {
            ExposureDirection::Receivable => {
                checked_add_assign(&mut receivable_unallocated, &row.amount)?;
                checked_add_assign(&mut party.receivable_unallocated, &row.amount)?;
            }
            ExposureDirection::Payable => {
                checked_add_assign(&mut payable_unallocated, &row.amount)?;
                checked_add_assign(&mut party.payable_unallocated, &row.amount)?;
            }
        }
    }
    let computed_unallocated = receivable_unallocated
        .checked_add(&payable_unallocated)
        .map_err(|_| OutstandingsWorkingPaperError::ArithmeticOverflow)?;
    require_equal(
        &computed_unallocated,
        &source.unallocated_total,
        "unallocated",
    )?;

    let mut party_rows = parties
        .into_iter()
        .map(|(party, totals)| {
            let receivable_total = totals
                .receivable_bills
                .checked_add(&totals.receivable_unallocated)
                .map_err(|_| OutstandingsWorkingPaperError::ArithmeticOverflow)?;
            let payable_total = totals
                .payable_bills
                .checked_add(&totals.payable_unallocated)
                .map_err(|_| OutstandingsWorkingPaperError::ArithmeticOverflow)?;
            let outstanding_total = receivable_total
                .checked_add(&payable_total)
                .map_err(|_| OutstandingsWorkingPaperError::ArithmeticOverflow)?;
            Ok(PartyWorkingPaperRow {
                party,
                receivable_bills: totals.receivable_bills,
                payable_bills: totals.payable_bills,
                receivable_unallocated: totals.receivable_unallocated,
                payable_unallocated: totals.payable_unallocated,
                receivable_total,
                payable_total,
                outstanding_total,
                oldest_bill_age_days: totals.oldest_bill_age_days,
                oldest_due_age_days: totals.oldest_due_age_days,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    party_rows.sort_by(|left, right| {
        right
            .outstanding_total
            .cmp_magnitude(&left.outstanding_total)
            .then_with(|| left.party.cmp(&right.party))
    });
    bills.sort_by(|left, right| {
        left.party
            .cmp(&right.party)
            .then_with(|| right.due_age_days.cmp(&left.due_age_days))
            .then_with(|| left.reference.cmp(&right.reference))
    });

    let receivable_total = receivable_bills
        .checked_add(&receivable_unallocated)
        .map_err(|_| OutstandingsWorkingPaperError::ArithmeticOverflow)?;
    let payable_total = payable_bills
        .checked_add(&payable_unallocated)
        .map_err(|_| OutstandingsWorkingPaperError::ArithmeticOverflow)?;
    let outstanding_total = receivable_total
        .checked_add(&payable_total)
        .map_err(|_| OutstandingsWorkingPaperError::ArithmeticOverflow)?;

    Ok(OutstandingsWorkingPaper {
        company: source.company,
        company_guid: source.company_guid,
        as_of,
        currency_assertion: source.currency_assertion,
        synced_at_unix_ms: source.synced_at_unix_ms,
        source_bytes: source.source_bytes,
        source_ageing_anchor: source.source_ageing_anchor,
        parties: party_rows,
        bills,
        controls: WorkingPaperControls {
            receivable_bills,
            payable_bills,
            receivable_unallocated,
            payable_unallocated,
            receivable_total,
            payable_total,
            outstanding_total,
            bill_date_ageing,
            due_date_ageing,
        },
    })
}

fn parse_date(value: &str) -> Result<TallyDate, OutstandingsWorkingPaperError> {
    TallyDate::parse(value)
        .map_err(|_| OutstandingsWorkingPaperError::InvalidDate(value.to_string()))
}

fn age_on_or_before(
    date: &TallyDate,
    as_of: &TallyDate,
) -> Result<Option<u32>, OutstandingsWorkingPaperError> {
    if date > as_of {
        return Ok(None);
    }
    bridge_tally_protocol::native_outstandings::age_in_days(date, as_of)
        .map(Some)
        .map_err(|_| OutstandingsWorkingPaperError::InvalidDate(date.as_str().to_string()))
}

fn max_age(left: Option<u32>, right: Option<u32>) -> Option<u32> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (Some(age), None) | (None, Some(age)) => Some(age),
        (None, None) => None,
    }
}

fn require_non_negative(value: &ExactDecimal) -> Result<(), OutstandingsWorkingPaperError> {
    if value.is_negative() {
        Err(OutstandingsWorkingPaperError::NegativeAmount)
    } else {
        Ok(())
    }
}

fn require_non_zero_row(value: &ExactDecimal) -> Result<(), OutstandingsWorkingPaperError> {
    if value.is_zero() {
        Err(OutstandingsWorkingPaperError::ZeroExposureRow)
    } else {
        Ok(())
    }
}

fn checked_add_assign(
    total: &mut ExactDecimal,
    amount: &ExactDecimal,
) -> Result<(), OutstandingsWorkingPaperError> {
    *total = total
        .checked_add(amount)
        .map_err(|_| OutstandingsWorkingPaperError::ArithmeticOverflow)?;
    Ok(())
}

fn add_ageing_amount(
    controls: &mut AgeingBucketControls,
    age_days: Option<u32>,
    amount: &ExactDecimal,
) -> Result<(), OutstandingsWorkingPaperError> {
    let bucket = match age_days {
        None => &mut controls.date_not_reached,
        Some(0..=30) => &mut controls.days_0_30,
        Some(31..=60) => &mut controls.days_31_60,
        Some(61..=90) => &mut controls.days_61_90,
        Some(_) => &mut controls.days_90_plus,
    };
    checked_add_assign(bucket, amount)
}

fn require_equal(
    actual: &ExactDecimal,
    expected: &ExactDecimal,
    label: &'static str,
) -> Result<(), OutstandingsWorkingPaperError> {
    if actual
        .checked_subtract(expected)
        .map_err(|_| OutstandingsWorkingPaperError::ArithmeticOverflow)?
        .is_zero()
    {
        Ok(())
    } else {
        Err(OutstandingsWorkingPaperError::ControlMismatch(label))
    }
}

#[cfg(test)]
#[path = "outstandings_working_paper_tests.rs"]
mod tests;
