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
mod tests {
    use super::*;

    fn decimal(value: &str) -> ExactDecimal {
        ExactDecimal::parse(value).expect("synthetic exact decimal")
    }

    fn bill(
        party: &str,
        reference: &str,
        bill_date: &str,
        due_date: &str,
        amount: &str,
        kind: ExposureDirection,
    ) -> OpenBillRow {
        OpenBillRow {
            party: party.to_string(),
            reference: reference.to_string(),
            bill_date: bill_date.to_string(),
            due_date: due_date.to_string(),
            amount: decimal(amount),
            age_days: None,
            kind,
        }
    }

    fn source() -> OutstandingsWorkingPaperSource {
        let mut source = OutstandingsWorkingPaperSource {
            company: "Synthetic Books".to_string(),
            company_guid: "synthetic-guid".to_string(),
            as_of_yyyymmdd: "20260825".to_string(),
            currency_assertion: OutstandingsCurrencyAssertion::Inr,
            synced_at_unix_ms: 1_777_000_000_000,
            source_bytes: 1_024,
            source_ageing_anchor: OutstandingsAgeingAnchor::DueDate,
            receivable_bill_total: decimal("125.00"),
            payable_bill_total: decimal("40.00"),
            unallocated_total: decimal("17.00"),
            open_bills: vec![
                bill(
                    "A Party",
                    "A-1",
                    "20260501",
                    "20260601",
                    "125.00",
                    ExposureDirection::Receivable,
                ),
                bill(
                    "A Party",
                    "P-1",
                    "20260801",
                    "20260901",
                    "40.00",
                    ExposureDirection::Payable,
                ),
            ],
            unallocated_by_party: vec![
                UnallocatedParty {
                    party: "A Party".to_string(),
                    amount: decimal("7.00"),
                    direction: ExposureDirection::Receivable,
                },
                UnallocatedParty {
                    party: "Unallocated Only".to_string(),
                    amount: decimal("10.00"),
                    direction: ExposureDirection::Payable,
                },
            ],
        };
        source.open_bills[0].age_days = Some(85);
        source
    }

    #[test]
    fn builds_dual_ages_and_keeps_directions_separate() {
        let paper = build_outstandings_working_paper(source()).expect("paper builds");
        let first = paper
            .bills
            .iter()
            .find(|row| row.reference == "A-1")
            .expect("first bill");
        assert_eq!(first.bill_age_days, Some(116));
        assert_eq!(first.due_age_days, Some(85));
        let future_due = paper
            .bills
            .iter()
            .find(|row| row.reference == "P-1")
            .expect("future-due bill");
        assert_eq!(future_due.bill_age_days, Some(24));
        assert_eq!(future_due.due_age_days, None);

        let mixed = paper
            .parties
            .iter()
            .find(|row| row.party == "A Party")
            .expect("mixed party");
        assert_eq!(mixed.receivable_total.as_str(), "132");
        assert_eq!(mixed.payable_total.as_str(), "40");
        assert_eq!(mixed.outstanding_total.as_str(), "172");
        let unallocated_only = paper
            .parties
            .iter()
            .find(|row| row.party == "Unallocated Only")
            .expect("unallocated-only party");
        assert_eq!(unallocated_only.oldest_bill_age_days, None);
        assert_eq!(unallocated_only.oldest_due_age_days, None);
        assert_eq!(
            paper
                .controls
                .bill_date_ageing
                .receivable
                .days_90_plus
                .as_str(),
            "125"
        );
        assert_eq!(
            paper
                .controls
                .due_date_ageing
                .receivable
                .days_61_90
                .as_str(),
            "125"
        );
        assert_eq!(
            paper
                .controls
                .due_date_ageing
                .payable
                .date_not_reached
                .as_str(),
            "40"
        );
        assert_eq!(paper.controls.receivable_total.as_str(), "132");
        assert_eq!(paper.controls.payable_total.as_str(), "50");
        assert_eq!(paper.controls.outstanding_total.as_str(), "182");
    }

    #[test]
    fn rejects_malformed_date_and_control_mismatches() {
        let mut malformed = source();
        malformed.open_bills[0].bill_date = "20260230".to_string();
        assert!(matches!(
            build_outstandings_working_paper(malformed),
            Err(OutstandingsWorkingPaperError::InvalidDate(_))
        ));

        let mut bill_mismatch = source();
        bill_mismatch.receivable_bill_total = decimal("124.99");
        assert_eq!(
            build_outstandings_working_paper(bill_mismatch),
            Err(OutstandingsWorkingPaperError::ControlMismatch(
                "receivable bill"
            ))
        );

        let mut residual_mismatch = source();
        residual_mismatch.unallocated_total = decimal("16.99");
        assert_eq!(
            build_outstandings_working_paper(residual_mismatch),
            Err(OutstandingsWorkingPaperError::ControlMismatch(
                "unallocated"
            ))
        );
    }

    #[test]
    fn rejects_negative_magnitudes() {
        let mut negative = source();
        negative.open_bills[0].amount = decimal("-1");
        assert_eq!(
            build_outstandings_working_paper(negative),
            Err(OutstandingsWorkingPaperError::NegativeAmount)
        );
    }

    #[test]
    fn rejects_zero_value_source_rows() {
        let mut zero = source();
        zero.open_bills[0].amount = ExactDecimal::zero();
        assert_eq!(
            build_outstandings_working_paper(zero),
            Err(OutstandingsWorkingPaperError::ZeroExposureRow)
        );
    }

    #[test]
    fn rejects_exact_arithmetic_overflow() {
        let mut overflow = source();
        let huge = decimal(&"9".repeat(bridge_tally_core::MAX_EXACT_DECIMAL_BYTES));
        overflow.open_bills = vec![
            bill(
                "Overflow Party",
                "BIG",
                "20260801",
                "20260801",
                huge.as_str(),
                ExposureDirection::Receivable,
            ),
            bill(
                "Overflow Party",
                "ONE",
                "20260801",
                "20260801",
                "1",
                ExposureDirection::Receivable,
            ),
        ];
        for row in &mut overflow.open_bills {
            row.age_days = Some(24);
        }
        overflow.payable_bill_total = ExactDecimal::zero();
        overflow.unallocated_total = ExactDecimal::zero();
        overflow.unallocated_by_party.clear();
        assert_eq!(
            build_outstandings_working_paper(overflow),
            Err(OutstandingsWorkingPaperError::ArithmeticOverflow)
        );
    }

    #[test]
    fn rejects_duplicate_residuals_and_selected_age_disagreement() {
        let mut duplicate = source();
        duplicate
            .unallocated_by_party
            .push(duplicate.unallocated_by_party[0].clone());
        duplicate.unallocated_total = decimal("24.00");
        assert_eq!(
            build_outstandings_working_paper(duplicate),
            Err(OutstandingsWorkingPaperError::DuplicateUnallocatedParty)
        );

        let mut wrong_age = source();
        wrong_age.open_bills[0].age_days = Some(84);
        assert_eq!(
            build_outstandings_working_paper(wrong_age),
            Err(OutstandingsWorkingPaperError::SourceAgeMismatch)
        );
    }

    #[test]
    fn bill_date_source_anchor_is_cross_checked_independently() {
        let mut bill_anchor = source();
        bill_anchor.source_ageing_anchor = OutstandingsAgeingAnchor::BillDate;
        bill_anchor.open_bills[0].age_days = Some(116);
        bill_anchor.open_bills[1].age_days = Some(24);
        assert!(build_outstandings_working_paper(bill_anchor).is_ok());
    }

    #[test]
    fn exact_age_boundaries_are_preserved() {
        let as_of = TallyDate::parse("20260825").expect("synthetic as-of");
        for (date, expected) in [
            ("20260726", 30),
            ("20260725", 31),
            ("20260626", 60),
            ("20260625", 61),
            ("20260527", 90),
            ("20260526", 91),
        ] {
            assert_eq!(
                age_on_or_before(&TallyDate::parse(date).expect("synthetic date"), &as_of),
                Ok(Some(expected))
            );
        }
    }
}
