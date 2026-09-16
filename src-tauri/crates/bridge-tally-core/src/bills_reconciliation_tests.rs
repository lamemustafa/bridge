use super::*;
use crate::{
    BillAllocationOrigin, BillDueDateEvidence, BillReference, CurrencyBasis, DerivedIdentityBasis,
    ExactDecimal, OutstandingDirection, OutstandingOrigin, TallyDate,
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
