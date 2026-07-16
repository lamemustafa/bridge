use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::exact_arithmetic::{
    is_negative_nonzero, magnitude_cmp, numeric_equal, same_nonzero_sign, ExactDecimalAccumulator,
};
use crate::{
    BillAllocationRecord, BillReferenceKind, BillWiseState, BillsAndPaymentsBatch,
    BillsCoverageState, CurrencyBasis, FetchBracketState, LedgerEntryPolarity,
    OutstandingDirection, OutstandingObservation, PartyOutstandingFacts, SourceIdentity,
    SourceRecordId, TallyDate, TallyError,
};

/// Adapter-owned authority. No source response or request echo may promote
/// these flags; a qualifying live receipt for the exact profile is required.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct PartyOutstandingAuthority {
    qualified_scope: Option<PartyOutstandingExpectedScope>,
    allocation_profile_observed: bool,
    outstanding_profile_observed: bool,
    signed_amount_semantics_observed: bool,
    due_date_semantics_observed: bool,
    on_account_aggregate_semantics_observed: bool,
    settled_omission_semantics_observed: bool,
    empty_scope_semantics_observed: bool,
}

impl fmt::Debug for PartyOutstandingAuthority {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PartyOutstandingAuthority")
            .field("qualified_scope", &self.qualified_scope.is_some())
            .field(
                "allocation_profile_observed",
                &self.allocation_profile_observed,
            )
            .field(
                "outstanding_profile_observed",
                &self.outstanding_profile_observed,
            )
            .field(
                "signed_amount_semantics_observed",
                &self.signed_amount_semantics_observed,
            )
            .field(
                "due_date_semantics_observed",
                &self.due_date_semantics_observed,
            )
            .field(
                "on_account_aggregate_semantics_observed",
                &self.on_account_aggregate_semantics_observed,
            )
            .field(
                "settled_omission_semantics_observed",
                &self.settled_omission_semantics_observed,
            )
            .field(
                "empty_scope_semantics_observed",
                &self.empty_scope_semantics_observed,
            )
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartyOutstandingConfidenceState {
    MatchedWithinObservedBracket,
    PartiallySettledMatched,
    OnAccountAggregateMatched,
    Mismatch,
    BillWiseDisabled,
    CoverageIncomplete,
    IncomparableCurrency,
    ProfileUnobserved,
    SourceChangedDuringFetch,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BillComparisonState {
    Matched,
    PartiallySettledMatched,
    OnAccountAggregateMatched,
    Mismatch,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DueState {
    Overdue { days: u32 },
    DueToday,
    NotDue { days: u32 },
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingInterpretation {
    ObservedOpen,
    PartialSettlementObserved,
    AdvanceObserved,
    OnAccountObserved,
    SettledOmissionObserved,
    Unresolved,
}

/// Raw source IDs remain in this non-serializable intermediate. A future UI
/// adapter must replace them with bounded, proof-local aliases before support
/// export, logging, or persistence outside encrypted run state.
#[derive(Clone, PartialEq, Eq)]
pub struct PartyOutstandingConfidenceRow {
    pub state: BillComparisonState,
    pub due_state: DueState,
    pub pending_interpretation: PendingInterpretation,
    pub allocation_source_ids: Vec<SourceRecordId>,
    pub outstanding_source_id: Option<SourceRecordId>,
}

impl fmt::Debug for PartyOutstandingConfidenceRow {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PartyOutstandingConfidenceRow")
            .field("state", &self.state)
            .field("due_state", &self.due_state)
            .field("pending_interpretation", &self.pending_interpretation)
            .field(
                "allocation_source_id_count",
                &self.allocation_source_ids.len(),
            )
            .field(
                "outstanding_source_id_present",
                &self.outstanding_source_id.is_some(),
            )
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartyOutstandingConfidenceAssessment {
    pub state: PartyOutstandingConfidenceState,
    pub compared_reference_count: u64,
    pub matched_reference_count: u64,
    pub mismatch_count: u64,
    pub unavailable_count: u64,
    pub safe_reason_codes: BTreeSet<&'static str>,
    pub rows: Vec<PartyOutstandingConfidenceRow>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct BillKey {
    name: String,
    currency: String,
}

#[derive(Default)]
struct BillGroup<'a> {
    allocations: Vec<&'a BillAllocationRecord>,
    outstanding: Vec<&'a OutstandingObservation>,
}

/// Caller-owned scope that the observation must match exactly before any
/// confidence decision is permitted. This prevents a valid receipt for one
/// company, party, date, direction, or export profile from being reused for
/// another scope.
#[derive(Clone, PartialEq, Eq)]
pub struct PartyOutstandingExpectedScope {
    pub source_identity: SourceIdentity,
    pub party_ledger_source_id: SourceRecordId,
    pub report_as_of_yyyymmdd: TallyDate,
    pub direction: OutstandingDirection,
    pub query_profile: crate::CanonicalText,
    pub source_scope_fingerprint: crate::CanonicalText,
}

impl fmt::Debug for PartyOutstandingExpectedScope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PartyOutstandingExpectedScope")
            .field("source_identity", &"redacted")
            .field("party_ledger_source_id", &"redacted")
            .field("report_as_of_yyyymmdd", &"redacted")
            .field("direction", &self.direction)
            .field("query_profile", &"redacted")
            .field("source_scope_fingerprint", &"redacted")
            .finish()
    }
}

pub fn assess_party_outstanding(
    facts: &PartyOutstandingFacts,
    expected_scope: &PartyOutstandingExpectedScope,
    authority: PartyOutstandingAuthority,
) -> Result<PartyOutstandingConfidenceAssessment, TallyError> {
    BillsAndPaymentsBatch {
        parties: vec![facts.clone()],
    }
    .validate()?;

    if facts.source_identity != expected_scope.source_identity
        || facts.party_ledger_source_id != expected_scope.party_ledger_source_id
        || facts.report_as_of_yyyymmdd != expected_scope.report_as_of_yyyymmdd
        || facts.direction != expected_scope.direction
        || facts.query_profile != expected_scope.query_profile
        || facts.source_scope_fingerprint != expected_scope.source_scope_fingerprint
    {
        return Err(TallyError::InvalidData {
            code: "party_outstanding_scope_mismatch".to_string(),
        });
    }

    if authority.qualified_scope.as_ref() != Some(expected_scope)
        || !authority.allocation_profile_observed
        || !authority.outstanding_profile_observed
        || !authority.signed_amount_semantics_observed
        || !authority.due_date_semantics_observed
    {
        return Ok(terminal(
            PartyOutstandingConfidenceState::ProfileUnobserved,
            "bills_profile_unobserved",
        ));
    }
    match facts.fetch_bracket {
        FetchBracketState::ChangedObserved => {
            return Ok(terminal(
                PartyOutstandingConfidenceState::SourceChangedDuringFetch,
                "bills_source_changed_during_fetch",
            ));
        }
        FetchBracketState::Unavailable => {
            return Ok(terminal(
                PartyOutstandingConfidenceState::Unavailable,
                "bills_fetch_bracket_unavailable",
            ));
        }
        FetchBracketState::StableObserved => {}
    }
    match facts.bill_wise_state {
        BillWiseState::CompanyDisabledObserved | BillWiseState::PartyDisabledObserved => {
            return Ok(terminal(
                PartyOutstandingConfidenceState::BillWiseDisabled,
                "bill_wise_tracking_disabled",
            ));
        }
        BillWiseState::UnsupportedForeignCurrencyLedgerObserved => {
            return Ok(terminal(
                PartyOutstandingConfidenceState::IncomparableCurrency,
                "foreign_currency_party_ledger_billwise_unsupported",
            ));
        }
        BillWiseState::Unknown => {
            return Ok(terminal(
                PartyOutstandingConfidenceState::Unavailable,
                "bill_wise_tracking_state_unknown",
            ));
        }
        BillWiseState::EnabledObserved => {}
    }
    if facts.allocation_coverage != BillsCoverageState::ObservedCompleteScope
        || facts.outstanding_coverage != BillsCoverageState::ObservedCompleteScope
    {
        return Ok(terminal(
            PartyOutstandingConfidenceState::CoverageIncomplete,
            "bills_source_coverage_incomplete",
        ));
    }
    if facts.allocations.is_empty()
        && facts.outstanding.is_empty()
        && !authority.empty_scope_semantics_observed
    {
        return Ok(terminal(
            PartyOutstandingConfidenceState::Unavailable,
            "empty_scope_semantics_unobserved",
        ));
    }

    let mut currencies = BTreeSet::new();
    for currency in facts
        .allocations
        .iter()
        .map(|record| &record.currency_basis)
        .chain(
            facts
                .outstanding
                .iter()
                .map(|record| &record.currency_basis),
        )
    {
        match currency {
            CurrencyBasis::CompanyBase { currency } => {
                currencies.insert(currency.as_str());
            }
            CurrencyBasis::ObservedSource { .. } | CurrencyBasis::Unspecified => {
                return Ok(terminal(
                    PartyOutstandingConfidenceState::IncomparableCurrency,
                    "bills_currency_basis_incomparable",
                ));
            }
        }
    }
    if currencies.len() > 1 {
        return Ok(terminal(
            PartyOutstandingConfidenceState::IncomparableCurrency,
            "bills_currency_scope_mixed",
        ));
    }

    let mut groups = BTreeMap::<BillKey, BillGroup<'_>>::new();
    let mut on_account_allocations = Vec::new();
    let mut on_account_outstanding = Vec::new();
    for allocation in &facts.allocations {
        if !polarity_matches(allocation.amount.as_str(), allocation.observed_polarity) {
            return Ok(terminal(
                PartyOutstandingConfidenceState::Mismatch,
                "bill_allocation_polarity_mismatch",
            ));
        }
        match allocation.reference.kind {
            BillReferenceKind::OnAccount => on_account_allocations.push(allocation),
            BillReferenceKind::Unclassified => {
                return Ok(terminal(
                    PartyOutstandingConfidenceState::Unavailable,
                    "bill_reference_type_unclassified",
                ));
            }
            BillReferenceKind::Advance
            | BillReferenceKind::AgainstReference
            | BillReferenceKind::NewReference => {
                groups
                    .entry(reference_key(
                        allocation
                            .reference
                            .name
                            .as_ref()
                            .expect("validated named bill reference")
                            .as_str(),
                        &allocation.currency_basis,
                    ))
                    .or_default()
                    .allocations
                    .push(allocation);
            }
        }
    }
    for outstanding in &facts.outstanding {
        if !polarity_matches(
            outstanding.pending_amount.as_str(),
            outstanding.observed_polarity,
        ) || !reference_direction_matches(
            outstanding.reference.kind,
            outstanding.pending_amount.as_str(),
            facts.direction,
        ) || outstanding.opening_amount.as_ref().is_some_and(|opening| {
            !reference_direction_matches(
                outstanding.reference.kind,
                opening.as_str(),
                facts.direction,
            )
        }) {
            return Ok(terminal(
                PartyOutstandingConfidenceState::Mismatch,
                "bill_outstanding_direction_or_polarity_mismatch",
            ));
        }
        match outstanding.reference.kind {
            BillReferenceKind::OnAccount => on_account_outstanding.push(outstanding),
            BillReferenceKind::Unclassified => {
                return Ok(terminal(
                    PartyOutstandingConfidenceState::Unavailable,
                    "bill_reference_type_unclassified",
                ));
            }
            BillReferenceKind::Advance
            | BillReferenceKind::AgainstReference
            | BillReferenceKind::NewReference => {
                groups
                    .entry(reference_key(
                        outstanding
                            .reference
                            .name
                            .as_ref()
                            .expect("validated named bill reference")
                            .as_str(),
                        &outstanding.currency_basis,
                    ))
                    .or_default()
                    .outstanding
                    .push(outstanding);
            }
        }
    }

    let mut assessment = PartyOutstandingConfidenceAssessment {
        state: PartyOutstandingConfidenceState::MatchedWithinObservedBracket,
        compared_reference_count: 0,
        matched_reference_count: 0,
        mismatch_count: 0,
        unavailable_count: 0,
        safe_reason_codes: BTreeSet::new(),
        rows: Vec::new(),
    };
    let mut partial = false;
    let mut due_unavailable = false;
    for group in groups.into_values() {
        assessment.compared_reference_count = assessment.compared_reference_count.saturating_add(1);
        let mut allocation_total = ExactDecimalAccumulator::default();
        for allocation in &group.allocations {
            allocation_total.add(allocation.amount.as_str());
        }
        let allocation_source_ids = group
            .allocations
            .iter()
            .map(|record| record.source_id.clone())
            .collect::<Vec<_>>();
        let mut row = PartyOutstandingConfidenceRow {
            state: BillComparisonState::Mismatch,
            due_state: DueState::Unavailable,
            pending_interpretation: PendingInterpretation::Unresolved,
            allocation_source_ids,
            outstanding_source_id: group
                .outstanding
                .first()
                .map(|record| record.source_id.clone()),
        };
        let anchors = group
            .allocations
            .iter()
            .filter(|allocation| {
                matches!(
                    allocation.reference.kind,
                    BillReferenceKind::NewReference | BillReferenceKind::Advance
                )
            })
            .collect::<Vec<_>>();
        if anchors.len() != 1
            || !reference_direction_matches(
                anchors[0].reference.kind,
                anchors[0].amount.as_str(),
                facts.direction,
            )
            || group
                .outstanding
                .first()
                .is_some_and(|outstanding| outstanding.reference.kind != anchors[0].reference.kind)
        {
            assessment.mismatch_count = assessment.mismatch_count.saturating_add(1);
            assessment
                .safe_reason_codes
                .insert("bill_reference_kind_composition_invalid");
            assessment.rows.push(row);
            continue;
        }
        if group.outstanding.len() > 1 {
            assessment.mismatch_count = assessment.mismatch_count.saturating_add(1);
            assessment
                .safe_reason_codes
                .insert("duplicate_outstanding_reference");
            assessment.rows.push(row);
            continue;
        }
        match group.outstanding.first().copied() {
            Some(outstanding) if allocation_total.equals(outstanding.pending_amount.as_str()) => {
                row.due_state = due_state(outstanding, &facts.report_as_of_yyyymmdd);
                if matches!(row.due_state, DueState::Unavailable) {
                    due_unavailable = true;
                    assessment.unavailable_count = assessment.unavailable_count.saturating_add(1);
                    assessment
                        .safe_reason_codes
                        .insert("bill_due_date_unavailable");
                }
                if source_overdue_days_mismatch(outstanding, row.due_state) {
                    row.state = BillComparisonState::Mismatch;
                    assessment.mismatch_count = assessment.mismatch_count.saturating_add(1);
                    assessment
                        .safe_reason_codes
                        .insert("source_overdue_days_mismatch");
                } else {
                    let is_partial = outstanding.opening_amount.as_ref().is_some_and(|opening| {
                        same_nonzero_sign(opening.as_str(), outstanding.pending_amount.as_str())
                            && magnitude_cmp(outstanding.pending_amount.as_str(), opening.as_str())
                                == Ordering::Less
                    });
                    row.state = if is_partial {
                        partial = true;
                        BillComparisonState::PartiallySettledMatched
                    } else {
                        BillComparisonState::Matched
                    };
                    row.pending_interpretation =
                        if is_partial {
                            PendingInterpretation::PartialSettlementObserved
                        } else if group.allocations.iter().any(|allocation| {
                            allocation.reference.kind == BillReferenceKind::Advance
                        }) {
                            PendingInterpretation::AdvanceObserved
                        } else {
                            PendingInterpretation::ObservedOpen
                        };
                    assessment.matched_reference_count =
                        assessment.matched_reference_count.saturating_add(1);
                }
            }
            None if allocation_total.is_zero() && authority.settled_omission_semantics_observed => {
                row.state = BillComparisonState::Matched;
                row.pending_interpretation = PendingInterpretation::SettledOmissionObserved;
                assessment.matched_reference_count =
                    assessment.matched_reference_count.saturating_add(1);
            }
            None if allocation_total.is_zero() => {
                row.state = BillComparisonState::Unavailable;
                assessment.unavailable_count = assessment.unavailable_count.saturating_add(1);
                assessment
                    .safe_reason_codes
                    .insert("settled_omission_semantics_unobserved");
            }
            Some(_) | None => {
                assessment.mismatch_count = assessment.mismatch_count.saturating_add(1);
                assessment
                    .safe_reason_codes
                    .insert("bill_pending_amount_mismatch");
            }
        }
        assessment.rows.push(row);
    }

    let on_account_present =
        !on_account_allocations.is_empty() || !on_account_outstanding.is_empty();
    let mut on_account_matched = false;
    if on_account_present {
        assessment.compared_reference_count = assessment.compared_reference_count.saturating_add(1);
        let mut allocation_total = ExactDecimalAccumulator::default();
        let mut outstanding_total = ExactDecimalAccumulator::default();
        for record in &on_account_allocations {
            allocation_total.add(record.amount.as_str());
        }
        for record in &on_account_outstanding {
            outstanding_total.add(record.pending_amount.as_str());
        }
        let mut row = PartyOutstandingConfidenceRow {
            state: BillComparisonState::Unavailable,
            due_state: DueState::Unavailable,
            pending_interpretation: PendingInterpretation::OnAccountObserved,
            allocation_source_ids: on_account_allocations
                .iter()
                .map(|record| record.source_id.clone())
                .collect(),
            outstanding_source_id: if on_account_outstanding.len() == 1 {
                Some(on_account_outstanding[0].source_id.clone())
            } else {
                None
            },
        };
        if !authority.on_account_aggregate_semantics_observed {
            assessment.unavailable_count = assessment.unavailable_count.saturating_add(1);
            assessment
                .safe_reason_codes
                .insert("on_account_aggregate_semantics_unobserved");
        } else if on_account_allocations.is_empty() || on_account_outstanding.is_empty() {
            if on_account_outstanding.is_empty()
                && allocation_total.is_zero()
                && authority.settled_omission_semantics_observed
            {
                row.state = BillComparisonState::OnAccountAggregateMatched;
                on_account_matched = true;
                assessment.matched_reference_count =
                    assessment.matched_reference_count.saturating_add(1);
            } else {
                assessment.unavailable_count = assessment.unavailable_count.saturating_add(1);
                assessment
                    .safe_reason_codes
                    .insert("on_account_comparison_side_missing");
            }
        } else if allocation_total == outstanding_total {
            row.state = BillComparisonState::OnAccountAggregateMatched;
            on_account_matched = true;
            assessment.matched_reference_count =
                assessment.matched_reference_count.saturating_add(1);
        } else {
            row.state = BillComparisonState::Mismatch;
            assessment.mismatch_count = assessment.mismatch_count.saturating_add(1);
            assessment
                .safe_reason_codes
                .insert("on_account_pending_amount_mismatch");
        }
        assessment.rows.push(row);
    }

    assessment.state = if assessment.mismatch_count > 0 {
        PartyOutstandingConfidenceState::Mismatch
    } else if assessment.unavailable_count > 0 || due_unavailable {
        PartyOutstandingConfidenceState::Unavailable
    } else if partial {
        PartyOutstandingConfidenceState::PartiallySettledMatched
    } else if on_account_matched {
        PartyOutstandingConfidenceState::OnAccountAggregateMatched
    } else {
        if assessment.rows.is_empty() {
            assessment
                .safe_reason_codes
                .insert("proven_empty_observation_scope");
        }
        PartyOutstandingConfidenceState::MatchedWithinObservedBracket
    };
    Ok(assessment)
}

fn terminal(
    state: PartyOutstandingConfidenceState,
    reason: &'static str,
) -> PartyOutstandingConfidenceAssessment {
    PartyOutstandingConfidenceAssessment {
        state,
        compared_reference_count: 0,
        matched_reference_count: 0,
        mismatch_count: u64::from(state == PartyOutstandingConfidenceState::Mismatch),
        unavailable_count: u64::from(matches!(
            state,
            PartyOutstandingConfidenceState::CoverageIncomplete
                | PartyOutstandingConfidenceState::IncomparableCurrency
                | PartyOutstandingConfidenceState::ProfileUnobserved
                | PartyOutstandingConfidenceState::SourceChangedDuringFetch
                | PartyOutstandingConfidenceState::Unavailable
        )),
        safe_reason_codes: BTreeSet::from([reason]),
        rows: Vec::new(),
    }
}

fn reference_key(name: &str, currency_basis: &CurrencyBasis) -> BillKey {
    let currency = match currency_basis {
        CurrencyBasis::CompanyBase { currency } => currency.as_str(),
        CurrencyBasis::ObservedSource { .. } | CurrencyBasis::Unspecified => {
            unreachable!("currency comparability was established")
        }
    };
    BillKey {
        name: name.to_string(),
        currency: currency.to_string(),
    }
}

fn polarity_matches(amount: &str, polarity: Option<LedgerEntryPolarity>) -> bool {
    if numeric_equal(amount, "0") {
        return polarity.is_some();
    }
    matches!(
        (polarity, is_negative_nonzero(amount)),
        (Some(LedgerEntryPolarity::Debit), true) | (Some(LedgerEntryPolarity::Credit), false)
    )
}

fn direction_matches(amount: &str, direction: OutstandingDirection) -> bool {
    if numeric_equal(amount, "0") {
        return false;
    }
    match direction {
        OutstandingDirection::Receivable => is_negative_nonzero(amount),
        OutstandingDirection::Payable => !is_negative_nonzero(amount),
    }
}

fn reference_direction_matches(
    kind: BillReferenceKind,
    amount: &str,
    direction: OutstandingDirection,
) -> bool {
    match kind {
        BillReferenceKind::NewReference | BillReferenceKind::OnAccount => {
            direction_matches(amount, direction)
        }
        BillReferenceKind::Advance => match direction {
            OutstandingDirection::Receivable => {
                !numeric_equal(amount, "0") && !is_negative_nonzero(amount)
            }
            OutstandingDirection::Payable => is_negative_nonzero(amount),
        },
        BillReferenceKind::AgainstReference | BillReferenceKind::Unclassified => false,
    }
}

fn due_state(outstanding: &OutstandingObservation, as_of: &TallyDate) -> DueState {
    let Some(due_date) = outstanding.due_date_yyyymmdd.as_ref() else {
        return DueState::Unavailable;
    };
    let due = date_ordinal(due_date.as_str());
    let observed = date_ordinal(as_of.as_str());
    match observed.cmp(&due) {
        Ordering::Greater => DueState::Overdue {
            days: u32::try_from(observed - due).unwrap_or(u32::MAX),
        },
        Ordering::Equal => DueState::DueToday,
        Ordering::Less => DueState::NotDue {
            days: u32::try_from(due - observed).unwrap_or(u32::MAX),
        },
    }
}

fn source_overdue_days_mismatch(outstanding: &OutstandingObservation, due_state: DueState) -> bool {
    let Some(source_days) = outstanding.source_reported_overdue_days else {
        return false;
    };
    match due_state {
        DueState::Overdue { days } => source_days != days,
        DueState::DueToday | DueState::NotDue { .. } => source_days != 0,
        DueState::Unavailable => false,
    }
}

fn date_ordinal(value: &str) -> i64 {
    let year = value[0..4]
        .parse::<i64>()
        .expect("validated TallyDate year");
    let month = value[4..6]
        .parse::<i64>()
        .expect("validated TallyDate month");
    let day = value[6..8].parse::<i64>().expect("validated TallyDate day");
    let year = year - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let shifted_month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        BillAllocationOrigin, BillDueDateEvidence, BillReference, CurrencyBasis,
        DerivedIdentityBasis, ExactDecimal, OutstandingDirection, OutstandingOrigin, TallyDate,
    };

    fn id(value: &str) -> SourceRecordId {
        SourceRecordId::parse(value).unwrap()
    }

    fn text(value: &str) -> crate::CanonicalText {
        crate::CanonicalText::parse(value).unwrap()
    }

    fn reference(kind: BillReferenceKind, name: Option<&str>) -> BillReference {
        BillReference {
            kind,
            name: name.map(text),
            raw_kind: None,
        }
    }

    fn currency() -> CurrencyBasis {
        CurrencyBasis::CompanyBase {
            currency: text("company-base"),
        }
    }

    fn allocation(
        id_value: &str,
        kind: BillReferenceKind,
        name: Option<&str>,
        amount: &str,
        origin: BillAllocationOrigin,
    ) -> BillAllocationRecord {
        BillAllocationRecord {
            source_id: id(id_value),
            identity_basis: DerivedIdentityBasis::ParentOrdinal,
            origin,
            reference: reference(kind, name),
            bill_date_yyyymmdd: Some(TallyDate::parse("20260701").unwrap()),
            effective_date_yyyymmdd: None,
            due_date_yyyymmdd: Some(TallyDate::parse("20260731").unwrap()),
            due_date_evidence: BillDueDateEvidence::Explicit,
            amount: ExactDecimal::parse(amount).unwrap(),
            observed_polarity: Some(if amount.starts_with('-') {
                LedgerEntryPolarity::Debit
            } else {
                LedgerEntryPolarity::Credit
            }),
            currency_basis: currency(),
        }
    }

    fn outstanding(
        id_value: &str,
        kind: BillReferenceKind,
        name: Option<&str>,
        opening: Option<&str>,
        pending: &str,
    ) -> OutstandingObservation {
        OutstandingObservation {
            source_id: id(id_value),
            identity_basis: DerivedIdentityBasis::ParentOrdinal,
            origin: OutstandingOrigin::Voucher {
                voucher_source_id: Some(id("voucher:1")),
            },
            reference: reference(kind, name),
            bill_date_yyyymmdd: Some(TallyDate::parse("20260701").unwrap()),
            effective_date_yyyymmdd: None,
            due_date_yyyymmdd: Some(TallyDate::parse("20260731").unwrap()),
            due_date_evidence: BillDueDateEvidence::Explicit,
            opening_amount: opening.map(|value| ExactDecimal::parse(value).unwrap()),
            pending_amount: ExactDecimal::parse(pending).unwrap(),
            observed_polarity: Some(if pending.starts_with('-') {
                LedgerEntryPolarity::Debit
            } else {
                LedgerEntryPolarity::Credit
            }),
            source_reported_overdue_days: Some(1),
            currency_basis: currency(),
        }
    }

    fn facts(
        allocations: Vec<BillAllocationRecord>,
        outstanding: Vec<OutstandingObservation>,
    ) -> PartyOutstandingFacts {
        PartyOutstandingFacts {
            source_identity: SourceIdentity {
                bridge_source_lineage: "bridge-source:test".to_string(),
                company_guid: "company-guid:test".to_string(),
                observed_fingerprint: "b".repeat(64),
            },
            party_ledger_source_id: id("ledger:party"),
            report_as_of_yyyymmdd: TallyDate::parse("20260801").unwrap(),
            direction: OutstandingDirection::Receivable,
            bill_wise_state: BillWiseState::EnabledObserved,
            allocation_coverage: BillsCoverageState::ObservedCompleteScope,
            outstanding_coverage: BillsCoverageState::ObservedCompleteScope,
            fetch_bracket: FetchBracketState::StableObserved,
            query_profile: text("bills-confidence-v1"),
            source_scope_fingerprint: text(&"a".repeat(64)),
            source_reported_allocation_count: allocations.len() as u64,
            source_reported_outstanding_count: outstanding.len() as u64,
            allocations,
            outstanding,
        }
    }

    fn authority(facts: &PartyOutstandingFacts) -> PartyOutstandingAuthority {
        PartyOutstandingAuthority {
            qualified_scope: Some(expected_scope(facts)),
            allocation_profile_observed: true,
            outstanding_profile_observed: true,
            signed_amount_semantics_observed: true,
            due_date_semantics_observed: true,
            on_account_aggregate_semantics_observed: true,
            settled_omission_semantics_observed: false,
            empty_scope_semantics_observed: false,
        }
    }

    fn expected_scope(facts: &PartyOutstandingFacts) -> PartyOutstandingExpectedScope {
        PartyOutstandingExpectedScope {
            source_identity: facts.source_identity.clone(),
            party_ledger_source_id: facts.party_ledger_source_id.clone(),
            report_as_of_yyyymmdd: facts.report_as_of_yyyymmdd.clone(),
            direction: facts.direction,
            query_profile: facts.query_profile.clone(),
            source_scope_fingerprint: facts.source_scope_fingerprint.clone(),
        }
    }

    fn assess(
        facts: &PartyOutstandingFacts,
        authority: PartyOutstandingAuthority,
    ) -> Result<PartyOutstandingConfidenceAssessment, TallyError> {
        assess_party_outstanding(facts, &expected_scope(facts), authority)
    }

    #[test]
    fn profile_unobserved_and_disabled_never_become_empty_or_matched() {
        let unobserved = facts(Vec::new(), Vec::new());
        assert_eq!(
            assess(&unobserved, PartyOutstandingAuthority::default())
                .unwrap()
                .state,
            PartyOutstandingConfidenceState::ProfileUnobserved
        );
        let mut disabled = unobserved;
        disabled.bill_wise_state = BillWiseState::PartyDisabledObserved;
        assert_eq!(
            assess(&disabled, authority(&disabled)).unwrap().state,
            PartyOutstandingConfidenceState::BillWiseDisabled
        );

        let facts = facts(Vec::new(), Vec::new());
        let unavailable = assess(&facts, authority(&facts)).unwrap();
        assert_eq!(
            unavailable.state,
            PartyOutstandingConfidenceState::Unavailable
        );
        assert!(unavailable
            .safe_reason_codes
            .contains("empty_scope_semantics_unobserved"));

        let mut observed_empty_authority = authority(&facts);
        observed_empty_authority.empty_scope_semantics_observed = true;
        let proven_empty = assess(&facts, observed_empty_authority).unwrap();
        assert_eq!(
            proven_empty.state,
            PartyOutstandingConfidenceState::MatchedWithinObservedBracket
        );
        assert!(proven_empty
            .safe_reason_codes
            .contains("proven_empty_observation_scope"));
    }

    #[test]
    fn opening_allocation_and_partial_settlement_match_exactly() {
        let facts = facts(
            vec![
                allocation(
                    "allocation:opening",
                    BillReferenceKind::NewReference,
                    Some("INV-1"),
                    "-1000.00",
                    BillAllocationOrigin::LedgerOpening,
                ),
                allocation(
                    "allocation:receipt-1",
                    BillReferenceKind::AgainstReference,
                    Some("INV-1"),
                    "300.0",
                    BillAllocationOrigin::Voucher {
                        voucher_source_id: id("voucher:receipt-1"),
                        party_entry_source_id: id("entry:receipt-1"),
                    },
                ),
                allocation(
                    "allocation:receipt-2",
                    BillReferenceKind::AgainstReference,
                    Some("INV-1"),
                    "200",
                    BillAllocationOrigin::Voucher {
                        voucher_source_id: id("voucher:receipt-2"),
                        party_entry_source_id: id("entry:receipt-2"),
                    },
                ),
            ],
            vec![outstanding(
                "outstanding:1",
                BillReferenceKind::NewReference,
                Some("INV-1"),
                Some("-1000"),
                "-500.000",
            )],
        );
        let result = assess(&facts, authority(&facts)).unwrap();
        assert_eq!(
            result.state,
            PartyOutstandingConfidenceState::PartiallySettledMatched
        );
        assert_eq!(result.matched_reference_count, 1);
        assert_eq!(result.rows[0].due_state, DueState::Overdue { days: 1 });
    }

    #[test]
    fn party_scoped_reference_and_on_account_aggregate_do_not_invent_bill_links() {
        let matched_facts = facts(
            vec![allocation(
                "allocation:on-account",
                BillReferenceKind::OnAccount,
                None,
                "-50",
                BillAllocationOrigin::Voucher {
                    voucher_source_id: id("voucher:on-account"),
                    party_entry_source_id: id("entry:on-account"),
                },
            )],
            vec![outstanding(
                "outstanding:on-account",
                BillReferenceKind::OnAccount,
                None,
                None,
                "-50.00",
            )],
        );
        let result = assess(&matched_facts, authority(&matched_facts)).unwrap();
        assert_eq!(
            result.state,
            PartyOutstandingConfidenceState::OnAccountAggregateMatched
        );
        assert!(result.rows[0].outstanding_source_id.is_some());
        let debug = format!("{:?}", result.rows[0]);
        assert!(!debug.contains("allocation:on-account"));
        assert!(!debug.contains("outstanding:on-account"));

        let omitted = facts(
            vec![
                allocation(
                    "allocation:on-account-debit",
                    BillReferenceKind::OnAccount,
                    None,
                    "-50",
                    BillAllocationOrigin::LedgerOpening,
                ),
                allocation(
                    "allocation:on-account-credit",
                    BillReferenceKind::OnAccount,
                    None,
                    "50",
                    BillAllocationOrigin::Voucher {
                        voucher_source_id: id("voucher:on-account-credit"),
                        party_entry_source_id: id("entry:on-account-credit"),
                    },
                ),
            ],
            Vec::new(),
        );
        let result = assess(&omitted, authority(&omitted)).unwrap();
        assert_eq!(result.state, PartyOutstandingConfidenceState::Unavailable);
        assert!(result
            .safe_reason_codes
            .contains("on_account_comparison_side_missing"));
    }

    #[test]
    fn exact_mismatch_currency_drift_and_missing_due_date_fail_closed() {
        let base = facts(
            vec![allocation(
                "allocation:1",
                BillReferenceKind::NewReference,
                Some("INV-1"),
                "-100",
                BillAllocationOrigin::Voucher {
                    voucher_source_id: id("voucher:1"),
                    party_entry_source_id: id("entry:1"),
                },
            )],
            vec![outstanding(
                "outstanding:1",
                BillReferenceKind::NewReference,
                Some("INV-1"),
                Some("-100"),
                "-99.999",
            )],
        );
        assert_eq!(
            assess(&base, authority(&base)).unwrap().state,
            PartyOutstandingConfidenceState::Mismatch
        );

        let mut wrong_direction = base.clone();
        wrong_direction.allocations[0].amount = ExactDecimal::parse("100").unwrap();
        wrong_direction.allocations[0].observed_polarity = Some(LedgerEntryPolarity::Credit);
        wrong_direction.outstanding[0].opening_amount = Some(ExactDecimal::parse("100").unwrap());
        wrong_direction.outstanding[0].pending_amount = ExactDecimal::parse("100").unwrap();
        wrong_direction.outstanding[0].observed_polarity = Some(LedgerEntryPolarity::Credit);
        assert_eq!(
            assess(&wrong_direction, authority(&wrong_direction))
                .unwrap()
                .state,
            PartyOutstandingConfidenceState::Mismatch
        );

        let mut ambiguous_reference = base.clone();
        ambiguous_reference.allocations.push(allocation(
            "allocation:advance-conflict",
            BillReferenceKind::Advance,
            Some("INV-1"),
            "-1",
            BillAllocationOrigin::LedgerOpening,
        ));
        ambiguous_reference.source_reported_allocation_count = 2;
        let result = assess(&ambiguous_reference, authority(&ambiguous_reference)).unwrap();
        assert_eq!(result.state, PartyOutstandingConfidenceState::Mismatch);
        assert!(result
            .safe_reason_codes
            .contains("bill_reference_kind_composition_invalid"));

        let mut due_unobserved = authority(&base);
        due_unobserved.due_date_semantics_observed = false;
        assert_eq!(
            assess(&base, due_unobserved).unwrap().state,
            PartyOutstandingConfidenceState::ProfileUnobserved
        );

        let mut foreign = base.clone();
        foreign.allocations[0].currency_basis = CurrencyBasis::ObservedSource {
            currency: text("USD"),
        };
        assert_eq!(
            assess(&foreign, authority(&foreign)).unwrap().state,
            PartyOutstandingConfidenceState::IncomparableCurrency
        );

        let mut drifted = base.clone();
        drifted.fetch_bracket = FetchBracketState::ChangedObserved;
        assert_eq!(
            assess(&drifted, authority(&drifted)).unwrap().state,
            PartyOutstandingConfidenceState::SourceChangedDuringFetch
        );

        let mut missing_due = base;
        missing_due.outstanding[0].pending_amount = ExactDecimal::parse("-100").unwrap();
        missing_due.outstanding[0].due_date_yyyymmdd = None;
        missing_due.outstanding[0].due_date_evidence = BillDueDateEvidence::Unavailable;
        missing_due.outstanding[0].source_reported_overdue_days = None;
        assert_eq!(
            assess(&missing_due, authority(&missing_due)).unwrap().state,
            PartyOutstandingConfidenceState::Unavailable
        );
    }

    #[test]
    fn zero_net_omission_is_not_settled_without_observed_semantics() {
        let correct_settlement = facts(
            vec![
                allocation(
                    "allocation:new",
                    BillReferenceKind::NewReference,
                    Some("INV-1"),
                    "-100",
                    BillAllocationOrigin::Voucher {
                        voucher_source_id: id("voucher:new"),
                        party_entry_source_id: id("entry:new"),
                    },
                ),
                allocation(
                    "allocation:settle",
                    BillReferenceKind::AgainstReference,
                    Some("INV-1"),
                    "100",
                    BillAllocationOrigin::Voucher {
                        voucher_source_id: id("voucher:settle"),
                        party_entry_source_id: id("entry:settle"),
                    },
                ),
            ],
            Vec::new(),
        );
        let result = assess(&correct_settlement, authority(&correct_settlement)).unwrap();
        assert_eq!(result.state, PartyOutstandingConfidenceState::Unavailable);
        assert!(result
            .safe_reason_codes
            .contains("settled_omission_semantics_unobserved"));

        let mut observed_omission = authority(&correct_settlement);
        observed_omission.settled_omission_semantics_observed = true;
        assert_eq!(
            assess(&correct_settlement, observed_omission)
                .unwrap()
                .state,
            PartyOutstandingConfidenceState::MatchedWithinObservedBracket
        );

        let wrong_sign = facts(
            vec![
                allocation(
                    "allocation:wrong-new",
                    BillReferenceKind::NewReference,
                    Some("INV-WRONG"),
                    "100",
                    BillAllocationOrigin::LedgerOpening,
                ),
                allocation(
                    "allocation:wrong-settle",
                    BillReferenceKind::AgainstReference,
                    Some("INV-WRONG"),
                    "-100",
                    BillAllocationOrigin::Voucher {
                        voucher_source_id: id("voucher:wrong-settle"),
                        party_entry_source_id: id("entry:wrong-settle"),
                    },
                ),
            ],
            Vec::new(),
        );
        let mut observed_omission = authority(&wrong_sign);
        observed_omission.settled_omission_semantics_observed = true;
        assert_eq!(
            assess(&wrong_sign, observed_omission).unwrap().state,
            PartyOutstandingConfidenceState::Mismatch
        );

        let zero_anchor = facts(
            vec![allocation(
                "allocation:zero-new",
                BillReferenceKind::NewReference,
                Some("INV-ZERO"),
                "0",
                BillAllocationOrigin::LedgerOpening,
            )],
            Vec::new(),
        );
        let mut observed_omission = authority(&zero_anchor);
        observed_omission.settled_omission_semantics_observed = true;
        assert_eq!(
            assess(&zero_anchor, observed_omission).unwrap().state,
            PartyOutstandingConfidenceState::Mismatch
        );
    }

    #[test]
    fn receipt_scope_must_match_caller_expectations_exactly() {
        let facts = facts(Vec::new(), Vec::new());
        let debug = format!("{:?} {:?}", expected_scope(&facts), authority(&facts));
        assert!(!debug.contains("company-guid:test"));
        assert!(!debug.contains("ledger:party"));
        assert!(!debug.contains(&"a".repeat(64)));
        let mut expected = expected_scope(&facts);
        expected.source_identity.company_guid = "company-guid:other".to_string();
        let error = assess_party_outstanding(&facts, &expected, authority(&facts)).unwrap_err();
        assert!(matches!(
            error,
            TallyError::InvalidData { code } if code == "party_outstanding_scope_mismatch"
        ));

        let mut expected = expected_scope(&facts);
        expected.report_as_of_yyyymmdd = TallyDate::parse("20260802").unwrap();
        assert!(assess_party_outstanding(&facts, &expected, authority(&facts)).is_err());

        let mut expected = expected_scope(&facts);
        expected.query_profile = text("different-profile");
        assert!(assess_party_outstanding(&facts, &expected, authority(&facts)).is_err());

        let authority_for_first_scope = authority(&facts);
        let mut other_scope = facts.clone();
        other_scope.source_identity.company_guid = "company-guid:other".to_string();
        assert_eq!(
            assess(&other_scope, authority_for_first_scope)
                .unwrap()
                .state,
            PartyOutstandingConfidenceState::ProfileUnobserved
        );
    }
}
