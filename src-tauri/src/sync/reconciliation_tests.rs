use bridge_tally_core::{
    source_count_scope_fingerprint, ExactDecimal, LedgerEntryPolarity, LedgerEntryRecord,
    LedgerRecord, ObservedSourceIdentities, RawSourceSha256, SourceAlterId, SourceRecordId,
    SourceReportedCountEvidence, VoucherRecord, VoucherTypeRecord,
};

use super::*;
use serde_json::json;

fn source_identity() -> SourceIdentity {
    SourceIdentity {
        bridge_source_lineage: "lineage-1".to_string(),
        company_guid: "company-guid".to_string(),
        observed_fingerprint: "fingerprint".to_string(),
    }
}

fn balanced_batch(reverse: bool) -> PackBatch {
    let mut batch = CoreAccountingBatch {
        ledgers: vec![
            LedgerRecord {
                source_id: "ledger-b".to_string(),
                name: "B".to_string(),
                parent_source_id: None,
                opening_balance: None,
            },
            LedgerRecord {
                source_id: "ledger-a".to_string(),
                name: "A".to_string(),
                parent_source_id: None,
                opening_balance: None,
            },
        ],
        voucher_types: vec![VoucherTypeRecord {
            source_id: "sales".to_string(),
            name: "Sales".to_string(),
        }],
        vouchers: vec![VoucherRecord {
            source_id: "voucher-1".to_string(),
            date_yyyymmdd: "20260701".to_string(),
            voucher_type_source_id: "sales".to_string(),
            voucher_number: Some("1".to_string()),
            cancelled: false,
            optional: false,
        }],
        ledger_entries: vec![
            LedgerEntryRecord {
                source_id: "entry-credit".to_string(),
                voucher_source_id: "voucher-1".to_string(),
                ledger_source_id: "ledger-b".to_string(),
                amount: ExactDecimal::parse("-100.00").unwrap(),
                polarity: LedgerEntryPolarity::Debit,
            },
            LedgerEntryRecord {
                source_id: "entry-debit".to_string(),
                voucher_source_id: "voucher-1".to_string(),
                ledger_source_id: "ledger-a".to_string(),
                amount: ExactDecimal::parse("100").unwrap(),
                polarity: LedgerEntryPolarity::Credit,
            },
        ],
        ..CoreAccountingBatch::default()
    };
    if reverse {
        batch.ledgers.reverse();
        batch.ledger_entries.reverse();
    }
    PackBatch::CoreAccounting(batch)
}

fn window() -> ReadWindow {
    ReadWindow {
        from_yyyymmdd: "20260701".to_string(),
        to_yyyymmdd: "20260731".to_string(),
    }
}

fn query_profile() -> CanonicalText {
    CanonicalText::parse("core-accounting-v1").unwrap()
}

fn filters_sha256() -> CanonicalText {
    CanonicalText::parse("a".repeat(64)).unwrap()
}

fn canonicalize_test(
    batch: PackBatch,
    source_counts: Option<Vec<SourceReportedCountEvidence>>,
) -> CanonicalWindow {
    canonicalize_window(
        &CanonicalWindowContext {
            requested_pack: CapabilityPackId::CoreAccounting,
            schema_version: PackSchemaVersion { major: 1, minor: 0 },
            source_identity: &source_identity(),
            query_profile: &query_profile(),
            filters_sha256: &filters_sha256(),
            external_references: &ExternalReferenceCatalog::Unavailable,
            window_id: "window-1",
            requested_window: &window(),
        },
        &CanonicalPackWindow {
            batch,
            source_counts,
            record_evidence: None,
        },
    )
    .unwrap()
}

fn complete_core_counts() -> Vec<SourceReportedCountEvidence> {
    [
        ("group", 0),
        ("ledger", 2),
        ("voucher_type", 1),
        ("voucher", 1),
        ("ledger_entry", 2),
    ]
    .into_iter()
    .map(|(object_type, source_reported_count)| {
        let object_type = CanonicalText::parse(object_type).unwrap();
        let descriptor = SourceCountScopeDescriptor {
            source_identity: source_identity(),
            pack: CapabilityPackId::CoreAccounting,
            pack_schema_version: PackSchemaVersion { major: 1, minor: 0 },
            object_type: object_type.clone(),
            query_profile: query_profile(),
            filters_sha256: filters_sha256(),
            window: None,
        };
        SourceReportedCountEvidence {
            object_type,
            query_profile: query_profile(),
            source_scope_fingerprint: source_count_scope_fingerprint(
                &descriptor,
                SourceCountScope::Complete,
            )
            .unwrap(),
            source_count_scope: SourceCountScope::Complete,
            source_reported_count,
        }
    })
    .collect()
}

fn complete_core_record_evidence() -> Vec<SourceRecordEvidence> {
    [
        ("ledger", "ledger-a", SourceIdentityKind::Guid, '1'),
        ("ledger", "ledger-b", SourceIdentityKind::RemoteId, '2'),
        ("voucher_type", "sales", SourceIdentityKind::Fallback, '3'),
        ("voucher", "voucher-1", SourceIdentityKind::MasterId, '4'),
        (
            "ledger_entry",
            "entry-credit",
            SourceIdentityKind::RemoteId,
            '5',
        ),
        ("ledger_entry", "entry-debit", SourceIdentityKind::Guid, '6'),
    ]
    .into_iter()
    .map(|(object_type, source_id, identity_kind, hash_char)| {
        let source_id = SourceRecordId::parse(source_id).unwrap();
        let mut observed_identities = ObservedSourceIdentities::default();
        match identity_kind {
            SourceIdentityKind::Guid => observed_identities.guid = Some(source_id.clone()),
            SourceIdentityKind::RemoteId => observed_identities.remote_id = Some(source_id.clone()),
            SourceIdentityKind::MasterId => observed_identities.master_id = Some(source_id.clone()),
            SourceIdentityKind::Fallback => {}
        }
        SourceRecordEvidence {
            object_type: CanonicalText::parse(object_type).unwrap(),
            source_id: source_id.clone(),
            identity_kind,
            observed_identities,
            raw_source_sha256: RawSourceSha256::parse(hash_char.to_string().repeat(64)).unwrap(),
            alter_id: (source_id.as_str() == "voucher-1")
                .then(|| SourceAlterId::parse("alter:77").unwrap()),
        }
    })
    .collect()
}

fn canonicalize_typed(
    pack: CapabilityPackId,
    batch: PackBatch,
    external_references: ExternalReferenceCatalog,
) -> CanonicalWindow {
    canonicalize_window(
        &CanonicalWindowContext {
            requested_pack: pack,
            schema_version: PackSchemaVersion { major: 1, minor: 0 },
            source_identity: &source_identity(),
            query_profile: &CanonicalText::parse("typed-pack-v1").unwrap(),
            filters_sha256: &filters_sha256(),
            external_references: &external_references,
            window_id: "window-1",
            requested_window: &window(),
        },
        &CanonicalPackWindow::without_source_count_evidence(batch),
    )
    .unwrap()
}

fn input(evidence: WindowEvidence) -> ReconciliationInput {
    ReconciliationInput {
        batch_id: "batch-1".to_string(),
        run_id: "run-1".to_string(),
        source_identity: source_identity(),
        pack: CapabilityPackId::CoreAccounting,
        pack_schema_version: PackSchemaVersion { major: 1, minor: 0 },
        started_at_unix_ms: 1_000,
        completed_at_unix_ms: 2_000,
        freshness_before: Freshness::NeverVerified,
        freshness_target_seconds: 300,
        planned_window_ids: BTreeSet::from(["window-1".to_string()]),
        completed_windows: BTreeMap::from([("window-1".to_string(), evidence)]),
        end_profile_check: EndProfileCheck::Unavailable,
        source_stability_check: SourceStabilityCheck::Unavailable,
        explicit_gap_codes: BTreeSet::new(),
        warning_codes: BTreeSet::new(),
    }
}

#[test]
fn canonical_hash_is_stable_when_source_order_changes() {
    let first = canonicalize_test(balanced_batch(false), None);
    let second = canonicalize_test(balanced_batch(true), None);
    assert_eq!(
        first.evidence.canonical_sha256,
        second.evidence.canonical_sha256
    );
    assert_eq!(first.observations.len(), 6);
    assert!(first.evidence.mismatches.is_empty());
}

#[test]
fn mirror_input_preserves_identity_kind_raw_hash_and_alter_id() {
    let source_window = CanonicalPackWindow {
        batch: balanced_batch(false),
        source_counts: None,
        record_evidence: Some(complete_core_record_evidence()),
    };
    let canonical = canonicalize_window(
        &CanonicalWindowContext {
            requested_pack: CapabilityPackId::CoreAccounting,
            schema_version: PackSchemaVersion { major: 1, minor: 0 },
            source_identity: &source_identity(),
            query_profile: &query_profile(),
            filters_sha256: &filters_sha256(),
            external_references: &ExternalReferenceCatalog::Unavailable,
            window_id: "window-1",
            requested_window: &window(),
        },
        &source_window,
    )
    .unwrap();
    assert_eq!(
        canonical.evidence.record_provenance_scope,
        ComparisonScope::Complete
    );
    let voucher = canonical
        .observations
        .iter()
        .find(|record| record.object_type == "voucher")
        .unwrap();
    let mirror = voucher.mirror_input("batch-1", 1_000).unwrap();
    assert_eq!(mirror.identity.master_id.as_deref(), Some("voucher-1"));
    assert!(mirror.identity.guid.is_none());
    assert!(mirror.identity.remote_id.is_none());
    assert!(mirror.identity.fallback_fingerprint.is_none());
    assert_eq!(mirror.raw_source_sha256, "4".repeat(64));
    assert_ne!(mirror.raw_source_sha256, voucher.canonical_sha256);
    assert_eq!(mirror.observed_alter_id.as_deref(), Some("alter:77"));
}

#[test]
fn missing_or_mismatched_record_provenance_never_reaches_staging_as_fabricated_data() {
    let missing = canonicalize_test(balanced_batch(false), None);
    assert_eq!(
        missing.evidence.record_provenance_scope,
        ComparisonScope::Unavailable
    );
    assert!(matches!(
        missing.observations[0].mirror_input("batch-1", 1_000),
        Err(ReconciliationError::RecordProvenanceUnavailable)
    ));
    let decision = build_reconciliation(input(missing.evidence)).unwrap();
    assert!(decision
        .mirror_commit
        .parts()
        .gap_codes
        .contains(&"record_provenance_unavailable".to_string()));

    let mut evidence = complete_core_record_evidence();
    evidence[0].object_type = CanonicalText::parse("group").unwrap();
    let result = canonicalize_window(
        &CanonicalWindowContext {
            requested_pack: CapabilityPackId::CoreAccounting,
            schema_version: PackSchemaVersion { major: 1, minor: 0 },
            source_identity: &source_identity(),
            query_profile: &query_profile(),
            filters_sha256: &filters_sha256(),
            external_references: &ExternalReferenceCatalog::Unavailable,
            window_id: "window-1",
            requested_window: &window(),
        },
        &CanonicalPackWindow {
            batch: balanced_batch(false),
            source_counts: None,
            record_evidence: Some(evidence),
        },
    );
    assert!(matches!(
        result,
        Err(ReconciliationError::RecordEvidenceMismatch)
    ));
}

#[test]
fn exact_decimal_reconciliation_detects_imbalance_without_float_math() {
    let mut batch = balanced_batch(false);
    let PackBatch::CoreAccounting(core) = &mut batch else {
        unreachable!()
    };
    core.ledger_entries[0].amount = ExactDecimal::parse("-99.999").unwrap();
    let canonical = canonicalize_test(batch, None);
    assert!(canonical
        .evidence
        .mismatches
        .iter()
        .any(|mismatch| mismatch.safe_reason_code == "voucher_entries_unbalanced"));
    let decision = build_reconciliation(input(canonical.evidence)).unwrap();
    assert_eq!(decision.proof.verification, CoreVerificationState::Partial);
    assert!(decision.mirror_commit.parts().checkpoint_after.is_none());
    assert!(decision
        .mirror_commit
        .parts()
        .gap_codes
        .contains(&"reconciliation_mismatch".to_string()));
}

#[test]
fn tally_polarity_is_independently_reconciled_from_signed_amounts() {
    let mut batch = balanced_batch(false);
    let PackBatch::CoreAccounting(core) = &mut batch else {
        unreachable!()
    };
    // Invert the WHOLE voucher, not one entry of it. Since bridge#392 a
    // single disagreeing entry is contextual polarity, not a mismatch --
    // it is what an ordinary round-off leg looks like and it fired 111
    // times on one real book's financial year. A voucher every entry of
    // which is inverted cannot be explained that way, and that is the
    // shape this test needs: it is asserting that such a mismatch
    // propagates to Partial verification, not that one flipped entry is
    // individually detectable.
    core.ledger_entries[0].polarity = LedgerEntryPolarity::Credit;
    core.ledger_entries[1].polarity = LedgerEntryPolarity::Debit;
    let canonical = canonicalize_test(batch, None);
    assert!(canonical
        .evidence
        .mismatches
        .iter()
        .any(|mismatch| { mismatch.safe_reason_code == "voucher_entry_polarity_mismatch" }));
    let decision = build_reconciliation(input(canonical.evidence)).unwrap();
    assert_eq!(decision.proof.verification, CoreVerificationState::Partial);
    assert!(decision.mirror_commit.parts().checkpoint_after.is_none());
    assert!(decision
        .mirror_commit
        .parts()
        .gap_codes
        .contains(&"reconciliation_mismatch".to_string()));
}

#[test]
fn cancelled_empty_voucher_is_not_a_false_missing_entry_failure() {
    let mut batch = balanced_batch(false);
    let PackBatch::CoreAccounting(core) = &mut batch else {
        unreachable!()
    };
    core.vouchers[0].cancelled = true;
    core.ledger_entries.clear();
    let canonical = canonicalize_test(batch, None);
    assert!(!canonical
        .evidence
        .accounting_gap_codes
        .contains("voucher_entry_applicability_unavailable"));
    assert!(!canonical
        .evidence
        .mismatches
        .iter()
        .any(|mismatch| mismatch.safe_reason_code == "voucher_entries_missing"));
}

#[test]
fn unknown_non_cancelled_empty_voucher_applicability_is_a_proof_gap() {
    let mut batch = balanced_batch(false);
    let PackBatch::CoreAccounting(core) = &mut batch else {
        unreachable!()
    };
    core.ledger_entries.clear();
    let canonical = canonicalize_test(batch, None);
    let decision = build_reconciliation(input(canonical.evidence)).unwrap();
    assert!(decision
        .mirror_commit
        .parts()
        .gap_codes
        .contains(&"voucher_entry_applicability_unavailable".to_string()));
    assert_eq!(decision.proof.verification, CoreVerificationState::Partial);
    assert!(decision.mirror_commit.parts().checkpoint_after.is_none());
}

#[test]
fn zero_amount_polarity_unavailability_prevents_checkpoint() {
    let mut batch = balanced_batch(false);
    let PackBatch::CoreAccounting(core) = &mut batch else {
        unreachable!()
    };
    core.ledger_entries[0].amount = ExactDecimal::parse("-0.00").unwrap();
    core.ledger_entries[1].amount = ExactDecimal::parse("0").unwrap();
    let canonical = canonicalize_test(batch, None);
    let decision = build_reconciliation(input(canonical.evidence)).unwrap();
    assert!(decision
        .mirror_commit
        .parts()
        .gap_codes
        .contains(&"voucher_entry_polarity_unavailable".to_string()));
    assert_eq!(decision.proof.verification, CoreVerificationState::Partial);
    assert!(decision.mirror_commit.parts().checkpoint_after.is_none());
}

#[test]
fn missing_window_and_mismatch_can_never_create_a_checkpoint() {
    let canonical = canonicalize_test(balanced_batch(false), None);
    let mut input = input(canonical.evidence);
    input.planned_window_ids.insert("window-2".to_string());
    let decision = build_reconciliation(input).unwrap();
    assert_eq!(decision.proof.verification, CoreVerificationState::Partial);
    assert_eq!(decision.mirror_commit.parts().checkpoint_after, None);
    assert!(decision
        .mirror_commit
        .parts()
        .gap_codes
        .contains(&"missing_snapshot_window".to_string()));
}

#[test]
fn complete_counts_cannot_claim_verified_without_required_report_tie_out() {
    let canonical = canonicalize_test(balanced_batch(false), Some(complete_core_counts()));
    let first = build_reconciliation(input(canonical.evidence.clone())).unwrap();
    let second = build_reconciliation(input(canonical.evidence)).unwrap();
    assert_eq!(first.proof.verification, CoreVerificationState::Partial);
    assert_eq!(
        first.proof.snapshot_sha256, second.proof.snapshot_sha256,
        "unchanged canonical state must hash identically"
    );
    assert!(first.mirror_commit.parts().checkpoint_after.is_none());
    assert!(first
        .mirror_commit
        .parts()
        .gap_codes
        .contains(&"report_tie_out_unavailable".to_string()));
    assert!(first
        .mirror_commit
        .parts()
        .gap_codes
        .contains(&"source_cut_consistency_unavailable".to_string()));
    assert!(first
        .mirror_commit
        .parts()
        .gap_codes
        .contains(&"capability_profile_drift_check_unavailable".to_string()));
}

#[test]
fn fresh_profile_and_full_reread_evidence_are_distinguished_from_atomicity() {
    let canonical = canonicalize_test(balanced_batch(false), Some(complete_core_counts()));
    let mut evidence = input(canonical.evidence);
    evidence.end_profile_check = EndProfileCheck::Passed;
    evidence.source_stability_check = SourceStabilityCheck::Passed;
    let decision = build_reconciliation(evidence).unwrap();
    let gaps = &decision.mirror_commit.parts().gap_codes;
    assert!(!gaps.contains(&"capability_profile_drift_check_unavailable".to_string()));
    assert!(!gaps.contains(&"source_cut_consistency_unavailable".to_string()));
    assert!(gaps.contains(&"source_cut_atomicity_unavailable".to_string()));
    assert!(decision.mirror_commit.parts().checkpoint_after.is_none());
}

#[test]
fn end_profile_drift_is_a_proof_mismatch_not_cached_as_passed() {
    let canonical = canonicalize_test(balanced_batch(false), Some(complete_core_counts()));
    let mut evidence = input(canonical.evidence);
    evidence.end_profile_check = EndProfileCheck::Mismatch;
    let decision = build_reconciliation(evidence).unwrap();
    assert!(decision
        .mirror_commit
        .parts()
        .gap_codes
        .contains(&"capability_profile_changed_during_run".to_string()));
    assert!(decision.mirror_commit.parts().checkpoint_after.is_none());
}

#[test]
fn missing_source_count_and_changed_identity_across_windows_fail_closed() {
    let first = canonicalize_test(balanced_batch(false), None);
    let unavailable = build_reconciliation(input(first.evidence.clone())).unwrap();
    assert_eq!(
        unavailable.proof.verification,
        CoreVerificationState::Partial
    );
    assert!(unavailable
        .mirror_commit
        .parts()
        .gap_codes
        .contains(&"source_count_unavailable".to_string()));
    assert!(unavailable.mirror_commit.parts().checkpoint_after.is_none());

    let mut second_evidence = first.evidence.clone();
    second_evidence.window_id = "window-2".to_string();
    second_evidence.from_yyyymmdd = "20260801".to_string();
    second_evidence.to_yyyymmdd = "20260831".to_string();
    let identity = second_evidence
        .canonical_records
        .keys()
        .next()
        .expect("at least one record")
        .clone();
    second_evidence
        .canonical_records
        .insert(identity, "f".repeat(64));
    let mut changed = input(first.evidence);
    changed.planned_window_ids.insert("window-2".to_string());
    changed
        .completed_windows
        .insert("window-2".to_string(), second_evidence);
    let decision = build_reconciliation(changed).unwrap();
    assert!(decision
        .mirror_commit
        .parts()
        .gap_codes
        .contains(&"source_changed_during_snapshot".to_string()));
    assert!(decision.mirror_commit.parts().checkpoint_after.is_none());
}

#[test]
fn complete_count_once_covers_global_scope_without_per_window_repetition() {
    let first = canonicalize_test(balanced_batch(false), Some(complete_core_counts()));
    let second_window = ReadWindow {
        from_yyyymmdd: "20260801".to_string(),
        to_yyyymmdd: "20260831".to_string(),
    };
    let second = canonicalize_window(
        &CanonicalWindowContext {
            requested_pack: CapabilityPackId::CoreAccounting,
            schema_version: PackSchemaVersion { major: 1, minor: 0 },
            source_identity: &source_identity(),
            query_profile: &query_profile(),
            filters_sha256: &filters_sha256(),
            external_references: &ExternalReferenceCatalog::Unavailable,
            window_id: "window-2",
            requested_window: &second_window,
        },
        &CanonicalPackWindow::without_source_count_evidence(PackBatch::CoreAccounting(
            CoreAccountingBatch::default(),
        )),
    )
    .unwrap();
    let mut reconciliation = input(first.evidence);
    reconciliation
        .planned_window_ids
        .insert("window-2".to_string());
    reconciliation
        .completed_windows
        .insert("window-2".to_string(), second.evidence);
    let decision = build_reconciliation(reconciliation).unwrap();
    assert_eq!(decision.proof.verification, CoreVerificationState::Partial);
    assert!(decision
        .mirror_commit
        .parts()
        .gap_codes
        .contains(&"report_tie_out_unavailable".to_string()));
    assert!(!decision
        .mirror_commit
        .parts()
        .gap_codes
        .iter()
        .any(|code| code.starts_with("source_count_")));
}

#[test]
fn mismatch_drill_down_never_retains_raw_printable_source_ids() {
    let raw_source_id = "CUSTOMER-LEDGER-PRINTABLE-123";
    let mismatch = safe_mismatch(
        "synthetic_reference_missing",
        vec![raw_source_id.to_string()],
    );
    let encoded = serde_json::to_string(&mismatch).unwrap();
    assert!(!encoded.contains(raw_source_id));
    assert_eq!(mismatch.safe_record_ids.len(), 1);
    assert!(mismatch.safe_record_ids[0].starts_with("rid:"));
    assert_eq!(mismatch.safe_record_ids[0].len(), 68);
}

#[test]
fn terminal_proofs_never_advance_a_checkpoint() {
    for kind in [TerminalKind::Failed, TerminalKind::Cancelled] {
        let decision = build_terminal_proof(
            "batch-1".to_string(),
            "run-1".to_string(),
            source_identity(),
            CapabilityPackId::CoreAccounting,
            PackSchemaVersion { major: 1, minor: 0 },
            1_000,
            2_000,
            Freshness::Fresh,
            300,
            kind,
            "window_extract_failed".to_string(),
            BTreeSet::from(["earlier_gap".to_string()]),
            BTreeSet::from([WarningCode::AdaptiveWindowSplit]),
            BTreeMap::from([
                ("locally_staged.accepted".to_string(), 2),
                ("locally_staged.rejected".to_string(), 1),
            ]),
        );
        assert_eq!(decision.mirror_commit.parts().checkpoint_after, None);
        assert_eq!(
            decision.proof.verification,
            CoreVerificationState::Unverified
        );
        assert_eq!(
            decision.mirror_commit.parts().gap_codes,
            vec!["earlier_gap", "window_extract_failed"]
        );
        assert_eq!(
            decision.mirror_commit.parts().warning_codes,
            vec!["adaptive_window_split"]
        );
        assert_eq!(
            decision
                .proof
                .gaps
                .iter()
                .map(|gap| gap.safe_reason_code.as_str())
                .collect::<Vec<_>>(),
            vec!["earlier_gap", "window_extract_failed"]
        );
        assert_eq!(decision.proof.record_counts["locally_staged.accepted"], 2);
        assert_eq!(decision.proof.record_counts["locally_staged.rejected"], 1);
        assert_eq!(
            decision.mirror_commit.parts().record_counts_sha256,
            Some(proof_record_counts_sha256(&decision.proof.record_counts))
        );
    }
}

#[test]
fn terminal_proof_clamps_backward_clock_and_records_the_gap() {
    let decision = build_terminal_proof(
        "batch-clock".to_string(),
        "run-clock".to_string(),
        source_identity(),
        CapabilityPackId::CoreAccounting,
        PackSchemaVersion { major: 1, minor: 0 },
        2_000,
        1_999,
        Freshness::NeverVerified,
        300,
        TerminalKind::Failed,
        "source_outcome_unknown".to_string(),
        BTreeSet::new(),
        BTreeSet::new(),
        BTreeMap::new(),
    );
    assert_eq!(decision.proof.completed_at_unix_ms, Some(2_000));
    assert_eq!(decision.mirror_commit.parts().completed_at_unix_ms, 2_000);
    assert_eq!(
        decision.mirror_commit.parts().gap_codes,
        vec!["local_clock_moved_backwards", "source_outcome_unknown"]
    );
}

#[test]
fn source_count_scope_fingerprint_mismatch_is_rejected_before_staging() {
    let mut counts = complete_core_counts();
    counts[0].source_scope_fingerprint = CanonicalText::parse("b".repeat(64)).unwrap();
    let result = canonicalize_window(
        &CanonicalWindowContext {
            requested_pack: CapabilityPackId::CoreAccounting,
            schema_version: PackSchemaVersion { major: 1, minor: 0 },
            source_identity: &source_identity(),
            query_profile: &query_profile(),
            filters_sha256: &filters_sha256(),
            external_references: &ExternalReferenceCatalog::Unavailable,
            window_id: "window-1",
            requested_window: &window(),
        },
        &CanonicalPackWindow {
            batch: balanced_batch(false),
            source_counts: Some(counts),
            record_evidence: None,
        },
    );
    assert!(matches!(
        result,
        Err(ReconciliationError::SourceCountScopeMismatch)
    ));
}

#[test]
fn typed_packs_preserve_exact_values_and_enforce_reference_integrity() {
    let references = ExternalReferenceCatalog::Complete {
        company_ids: BTreeSet::from(["company-guid".to_string()]),
        voucher_ids: BTreeSet::from(["voucher:1".to_string(), "voucher:2".to_string()]),
        ledger_ids: BTreeSet::from(["ledger:customer".to_string()]),
    };
    let tax = bridge_tally_core::IndiaTaxBatch {
        tax_registrations: vec![serde_json::from_value(json!({
            "source_id": "tax-registration:1",
            "owner_kind": "ledger",
            "owner_source_id": "ledger:customer",
            "registration_type": "regular",
            "gstin": "27ABCDE1234F1Z5"
        }))
        .unwrap()],
        voucher_taxes: vec![serde_json::from_value(json!({
            "source_id": "voucher-tax:1",
            "voucher_source_id": "voucher:1",
            "place_of_supply": "27",
            "assessable_value": "1000.00",
            "tax_component": "igst",
            "tax_rate": "18.00",
            // Deliberately not assessable*rate/100: no rounding/formula profile exists.
            "tax_amount": "179.99"
        }))
        .unwrap()],
    };
    let tax_window = canonicalize_typed(
        CapabilityPackId::IndiaTax,
        PackBatch::IndiaTax(tax),
        references.clone(),
    );
    assert_eq!(tax_window.observations.len(), 2);
    assert!(tax_window.evidence.mismatches.is_empty());
    assert_eq!(
        tax_window.observations[1]
            .exact_decimals
            .get("tax_amount")
            .map(String::as_str),
        Some("179.99")
    );

    let bills: bridge_tally_core::BillsAndPaymentsBatch = serde_json::from_value(json!({
        "parties": [{
            "source_identity": {
                "bridge_source_lineage": "bridge-source:test",
                "company_guid": "company-guid:test",
                "observed_fingerprint": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
            },
            "party_ledger_source_id": "ledger:customer",
            "report_as_of_yyyymmdd": "20260731",
            "direction": "receivable",
            "bill_wise_state": "enabled_observed",
            "allocation_coverage": "observed_complete_scope",
            "outstanding_coverage": "observed_complete_scope",
            "fetch_bracket": "stable_observed",
            "query_profile": "bills-confidence-v1",
            "source_scope_fingerprint": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "source_reported_allocation_count": 1,
            "source_reported_outstanding_count": 1,
            "allocations": [{
                "source_id": "bill:1",
                "identity_basis": "parent_ordinal",
                "origin": {
                    "origin": "voucher",
                    "voucher_source_id": "voucher:1",
                    "party_entry_source_id": "entry:party-1"
                },
                "reference": {
                    "kind": "new_reference",
                    "name": "INV-1",
                    "raw_kind": null
                },
                "bill_date_yyyymmdd": "20260701",
                "effective_date_yyyymmdd": null,
                "due_date_yyyymmdd": "20260731",
                "due_date_evidence": "explicit",
                "amount": "-1180.00",
                "observed_polarity": "debit",
                "currency_basis": {
                    "basis": "company_base",
                    "currency": "company-base"
                }
            }],
            "outstanding": [{
                "source_id": "outstanding:1",
                "identity_basis": "parent_ordinal",
                "origin": {
                    "origin": "voucher",
                    "voucher_source_id": "voucher:1"
                },
                "reference": {
                    "kind": "new_reference",
                    "name": "INV-1",
                    "raw_kind": null
                },
                "bill_date_yyyymmdd": "20260701",
                "effective_date_yyyymmdd": null,
                "due_date_yyyymmdd": "20260731",
                "due_date_evidence": "explicit",
                "opening_amount": "-1180.00",
                "pending_amount": "-1180.00",
                "observed_polarity": "debit",
                "source_reported_overdue_days": 0,
                "currency_basis": {
                    "basis": "company_base",
                    "currency": "company-base"
                }
            }]
        }]
    }))
    .unwrap();
    let bills_window = canonicalize_typed(
        CapabilityPackId::BillsAndPayments,
        PackBatch::BillsAndPayments(bills),
        references.clone(),
    );
    assert_eq!(bills_window.observations.len(), 3);
    assert!(bills_window.evidence.mismatches.is_empty());

    let inventory: bridge_tally_core::InventoryBatch = serde_json::from_value(json!({
        "stock_items": [{
            "source_id": "item:1",
            "name": "Synthetic Item",
            "base_unit": "nos"
        }],
        "godowns": [{
            "source_id": "godown:1",
            "name": "Synthetic Location"
        }],
        "inventory_entries": [{
            "source_id": "inventory:1",
            "voucher_source_id": "voucher:1",
            "stock_item_source_id": "missing-item",
            "godown_source_id": "godown:1",
            "quantity": "2.000",
            "rate": "500.00",
            "amount": "999.99"
        }]
    }))
    .unwrap();
    let inventory_window = canonicalize_typed(
        CapabilityPackId::Inventory,
        PackBatch::Inventory(inventory),
        references,
    );
    assert!(inventory_window
        .evidence
        .mismatches
        .iter()
        .any(|mismatch| mismatch.safe_reason_code == "stock_item_reference_missing"));
    assert!(!inventory_window
        .evidence
        .mismatches
        .iter()
        .any(|mismatch| mismatch.safe_reason_code.contains("amount")));
}
