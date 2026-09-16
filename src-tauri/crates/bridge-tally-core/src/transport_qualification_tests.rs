use super::*;
use crate::{
    source_count_scope_fingerprint, GroupRecord, LedgerEntryRecord, LedgerRecord,
    ObservedSourceIdentities, RawSourceSha256, SourceRecordEvidence, SourceReportedCountEvidence,
    VoucherRecord, VoucherTypeRecord,
};

fn scope() -> TransportQualificationScope {
    TransportQualificationScope {
        source_identity: SourceIdentity {
            bridge_source_lineage: "tally_local:test".to_string(),
            company_guid: "synthetic-private-company-guid".to_string(),
            observed_fingerprint: "c".repeat(64),
        },
        product: CanonicalText::parse("TallyPrime").unwrap(),
        release: CanonicalText::parse("7.0").unwrap(),
        mode: CanonicalText::parse("Educational").unwrap(),
        pack: CapabilityPackId::CoreAccounting,
        pack_schema_version: crate::CORE_ACCOUNTING_SCHEMA_VERSION,
        window: ReadWindow {
            from_yyyymmdd: "20260401".to_string(),
            to_yyyymmdd: "20260430".to_string(),
        },
        query_profile: CanonicalText::parse("bridge_core_v3").unwrap(),
        filters_sha256: CanonicalText::parse("a".repeat(64)).unwrap(),
        reference_transport: TransportId::XmlHttp,
        candidate_transport: TransportId::JsonEx,
        candidate_request_profile: CanonicalText::parse("jsonex_core_v1").unwrap(),
    }
}

fn metrics(started_at_unix_ms: i64, completed_at_unix_ms: i64) -> TransportReadMetrics {
    TransportReadMetrics {
        started_at_unix_ms,
        completed_at_unix_ms,
        response_bytes: 128,
    }
}

fn evidence(object_type: &str, source_id: &str, raw_hash_byte: char) -> SourceRecordEvidence {
    let source_id = SourceRecordId::parse(source_id).unwrap();
    let fallback = object_type == "ledger_entry";
    SourceRecordEvidence {
        object_type: CanonicalText::parse(object_type).unwrap(),
        source_id: source_id.clone(),
        identity_kind: if fallback {
            SourceIdentityKind::Fallback
        } else {
            SourceIdentityKind::Guid
        },
        observed_identities: if fallback {
            ObservedSourceIdentities::default()
        } else {
            ObservedSourceIdentities {
                guid: Some(source_id.clone()),
                ..Default::default()
            }
        },
        raw_source_sha256: RawSourceSha256::parse(raw_hash_byte.to_string().repeat(64)).unwrap(),
        alter_id: None,
    }
}

fn entry_window(
    scope: &TransportQualificationScope,
    entry_source_id: &str,
    amount: &str,
    raw_hash_byte: char,
) -> CanonicalPackWindow {
    let count = |object_type: &str, source_count_scope: SourceCountScope, value: u64| {
        let descriptor = SourceCountScopeDescriptor {
            source_identity: scope.source_identity.clone(),
            pack: scope.pack,
            pack_schema_version: scope.pack_schema_version,
            object_type: CanonicalText::parse(object_type).unwrap(),
            query_profile: scope.query_profile.clone(),
            filters_sha256: scope.filters_sha256.clone(),
            window: (source_count_scope == SourceCountScope::Window).then(|| scope.window.clone()),
        };
        SourceReportedCountEvidence {
            object_type: descriptor.object_type.clone(),
            query_profile: descriptor.query_profile.clone(),
            source_scope_fingerprint: source_count_scope_fingerprint(
                &descriptor,
                source_count_scope,
            )
            .unwrap(),
            source_count_scope,
            source_reported_count: value,
        }
    };
    CanonicalPackWindow {
        batch: PackBatch::CoreAccounting(CoreAccountingBatch {
            ledgers: vec![LedgerRecord {
                source_id: "ledger:1".to_string(),
                name: "Synthetic Ledger".to_string(),
                parent_source_id: None,
                opening_balance: Some(ExactDecimal::parse("0.00").unwrap()),
            }],
            voucher_types: vec![VoucherTypeRecord {
                source_id: "voucher-type:1".to_string(),
                name: "Synthetic Voucher Type".to_string(),
            }],
            vouchers: vec![VoucherRecord {
                source_id: "voucher:1".to_string(),
                date_yyyymmdd: "20260415".to_string(),
                voucher_type_source_id: "voucher-type:1".to_string(),
                voucher_number: Some("SYN-1".to_string()),
                cancelled: false,
                optional: false,
            }],
            ledger_entries: vec![LedgerEntryRecord {
                source_id: entry_source_id.to_string(),
                voucher_source_id: "voucher:1".to_string(),
                ledger_source_id: "ledger:1".to_string(),
                amount: ExactDecimal::parse(amount).unwrap(),
                polarity: LedgerEntryPolarity::Debit,
            }],
            ..Default::default()
        }),
        source_counts: Some(vec![
            count("group", SourceCountScope::Complete, 0),
            count("ledger", SourceCountScope::Complete, 1),
            count("voucher_type", SourceCountScope::Complete, 1),
            count("voucher", SourceCountScope::Window, 1),
            count("ledger_entry", SourceCountScope::Window, 1),
        ]),
        record_evidence: Some(vec![
            evidence("ledger", "ledger:1", raw_hash_byte),
            evidence("voucher_type", "voucher-type:1", raw_hash_byte),
            evidence("voucher", "voucher:1", raw_hash_byte),
            evidence("ledger_entry", entry_source_id, raw_hash_byte),
        ]),
    }
}

#[test]
fn semantic_match_excludes_transport_raw_hash_and_entry_id_and_normalizes_scale() {
    let scope = scope();
    let reference = entry_window(&scope, "xml-entry-hash", "-001.00", 'a');
    let candidate = entry_window(&scope, "json-entry-hash", "-1.0", 'b');
    let observation = qualify_core_transport_shadow(
        &scope,
        &reference,
        &candidate,
        &reference,
        metrics(10, 20),
        metrics(21, 25),
        metrics(26, 35),
    )
    .unwrap();

    assert_eq!(observation.verdict, TransportParityVerdict::Matched);
    assert_eq!(
        observation.candidate_recommendation,
        CandidateTransportRecommendation::ContinueShadowing
    );
    assert_eq!(
        observation.reason_codes,
        vec![TransportParityReasonCode::SemanticParityObserved]
    );
    assert_eq!(
        observation.reference_semantic_sha256,
        observation.candidate_semantic_sha256
    );
}

#[test]
fn bracketed_semantic_mismatch_recommends_scope_quarantine() {
    let scope = scope();
    let reference = entry_window(&scope, "xml-entry", "-1.00", 'a');
    let candidate = entry_window(&scope, "json-entry", "-2.00", 'b');
    let observation = qualify_core_transport_shadow(
        &scope,
        &reference,
        &candidate,
        &reference,
        metrics(10, 20),
        metrics(21, 25),
        metrics(26, 35),
    )
    .unwrap();

    assert_eq!(observation.verdict, TransportParityVerdict::Mismatched);
    assert_eq!(
        observation.candidate_recommendation,
        CandidateTransportRecommendation::RecommendQuarantine
    );
    assert!(observation
        .reason_codes
        .contains(&TransportParityReasonCode::CanonicalSemanticsMismatch));
}

#[test]
fn drift_or_missing_evidence_never_becomes_a_transport_mismatch() {
    let scope = scope();
    let before = entry_window(&scope, "xml-entry", "-1.00", 'a');
    let candidate = entry_window(&scope, "json-entry", "-2.00", 'b');
    let after = entry_window(&scope, "xml-entry-after", "-3.00", 'c');
    let drift = qualify_core_transport_shadow(
        &scope,
        &before,
        &candidate,
        &after,
        metrics(10, 20),
        metrics(21, 25),
        metrics(26, 35),
    )
    .unwrap();
    assert_eq!(drift.verdict, TransportParityVerdict::Inconclusive);
    assert_eq!(
        drift.source_stability,
        SourceStabilityEvidence::ReferenceBracketMismatch
    );
    assert!(drift
        .reason_codes
        .contains(&TransportParityReasonCode::CanonicalSemanticsMismatch));

    let mut no_counts = before.clone();
    no_counts.source_counts = None;
    let missing = qualify_core_transport_shadow(
        &scope,
        &no_counts,
        &no_counts,
        &no_counts,
        metrics(10, 20),
        metrics(21, 25),
        metrics(26, 35),
    )
    .unwrap();
    assert_eq!(missing.verdict, TransportParityVerdict::Inconclusive);
    assert!(missing
        .reason_codes
        .contains(&TransportParityReasonCode::SourceCountEvidenceUnavailable));

    let mut partial_counts = before.clone();
    partial_counts.source_counts.as_mut().unwrap().pop();
    let partial = qualify_core_transport_shadow(
        &scope,
        &partial_counts,
        &partial_counts,
        &partial_counts,
        metrics(10, 20),
        metrics(21, 25),
        metrics(26, 35),
    )
    .unwrap();
    assert_eq!(partial.verdict, TransportParityVerdict::Inconclusive);
    assert_eq!(
        partial.source_stability,
        SourceStabilityEvidence::EvidenceUnavailable
    );
}

#[test]
fn observation_receipt_does_not_serialize_company_or_record_values() {
    let scope = scope();
    let reference = entry_window(&scope, "xml-private-entry", "-1.00", 'a');
    let candidate = entry_window(&scope, "json-private-entry", "-1.0", 'b');
    let observation = qualify_core_transport_shadow(
        &scope,
        &reference,
        &candidate,
        &reference,
        metrics(10, 20),
        metrics(21, 25),
        metrics(26, 35),
    )
    .unwrap();
    let json = serde_json::to_string(&observation).unwrap();
    for private in [
        "synthetic-private-company-guid",
        "xml-private-entry",
        "json-private-entry",
    ] {
        assert!(!json.contains(private));
    }
}

#[test]
fn invalid_pair_and_metrics_fail_closed() {
    let valid_scope = scope();
    let reference = entry_window(&valid_scope, "xml-entry", "-1.00", 'a');
    let candidate = entry_window(&valid_scope, "json-entry", "-1.00", 'b');
    let mut invalid_scope = valid_scope.clone();
    invalid_scope.candidate_transport = TransportId::Odbc;
    assert!(matches!(
        qualify_core_transport_shadow(
            &invalid_scope,
            &reference,
            &candidate,
            &reference,
            metrics(10, 20),
            metrics(21, 25),
            metrics(26, 35),
        ),
        Err(TallyError::InvalidData { code }) if code == "transport_qualification_pair_invalid"
    ));
    assert!(matches!(
        qualify_core_transport_shadow(
            &valid_scope,
            &reference,
            &candidate,
            &reference,
            TransportReadMetrics {
                started_at_unix_ms: 20,
                completed_at_unix_ms: 10,
                response_bytes: 128,
            },
            metrics(21, 25),
            metrics(26, 35),
        ),
        Err(TallyError::InvalidData { code }) if code == "transport_qualification_metrics_invalid"
    ));

    assert!(matches!(
        qualify_core_transport_shadow(
            &valid_scope,
            &reference,
            &candidate,
            &reference,
            metrics(10, 30),
            metrics(20, 25),
            metrics(31, 40),
        ),
        Err(TallyError::InvalidData { code }) if code == "transport_qualification_bracket_order_invalid"
    ));

    let mut wrong_schema = valid_scope.clone();
    wrong_schema.pack_schema_version.minor += 1;
    assert!(matches!(
        qualify_core_transport_shadow(
            &wrong_schema,
            &reference,
            &candidate,
            &reference,
            metrics(10, 20),
            metrics(21, 25),
            metrics(26, 35),
        ),
        Err(TallyError::InvalidData { code }) if code == "transport_qualification_schema_version_invalid"
    ));

    let mut invalid_scope = valid_scope;
    invalid_scope.window.to_yyyymmdd = "20260230".to_string();
    assert!(qualify_core_transport_shadow(
        &invalid_scope,
        &reference,
        &candidate,
        &reference,
        metrics(10, 20),
        metrics(21, 25),
        metrics(26, 35),
    )
    .is_err());
}

fn group_record(source_id: &str, name: &str) -> GroupRecord {
    GroupRecord {
        source_id: source_id.to_string(),
        name: name.to_string(),
        parent_source_id: None,
    }
}

fn ledger_record(source_id: &str, name: &str) -> LedgerRecord {
    LedgerRecord {
        source_id: source_id.to_string(),
        name: name.to_string(),
        parent_source_id: None,
        opening_balance: None,
    }
}

fn voucher_type_record(source_id: &str, name: &str) -> VoucherTypeRecord {
    VoucherTypeRecord {
        source_id: source_id.to_string(),
        name: name.to_string(),
    }
}

#[test]
fn empty_group_name_is_rejected() {
    let batch = CoreAccountingBatch {
        groups: vec![group_record("group:1", "")],
        ..Default::default()
    };
    assert!(matches!(
        validate_core_reference_integrity(&batch),
        Err(TallyError::InvalidData { code }) if code == "transport_qualification_group_name_empty"
    ));
}

#[test]
fn empty_ledger_name_is_rejected() {
    let batch = CoreAccountingBatch {
        ledgers: vec![ledger_record("ledger:1", "")],
        ..Default::default()
    };
    assert!(matches!(
        validate_core_reference_integrity(&batch),
        Err(TallyError::InvalidData { code }) if code == "transport_qualification_ledger_name_empty"
    ));
}

#[test]
fn empty_voucher_type_name_is_rejected() {
    let batch = CoreAccountingBatch {
        voucher_types: vec![voucher_type_record("voucher-type:1", "")],
        ..Default::default()
    };
    assert!(matches!(
        validate_core_reference_integrity(&batch),
        Err(TallyError::InvalidData { code }) if code == "transport_qualification_voucher_type_name_empty"
    ));
}

#[test]
fn whitespace_only_and_control_character_names_survive_verbatim() {
    let whitespace_name = "   \t  ";
    let control_name = "Ledger\u{7}Name";
    let batch = CoreAccountingBatch {
        groups: vec![group_record("group:1", whitespace_name)],
        ledgers: vec![ledger_record("ledger:1", control_name)],
        voucher_types: vec![voucher_type_record("voucher-type:1", whitespace_name)],
        ..Default::default()
    };

    validate_core_reference_integrity(&batch)
        .expect("whitespace-only and control-character names must not be rejected");
    // The verbatim policy is load-bearing: neither name may be trimmed,
    // normalized, or otherwise rewritten by the boundary check.
    assert_eq!(batch.groups[0].name, whitespace_name);
    assert_eq!(batch.ledgers[0].name, control_name);
    assert_eq!(batch.voucher_types[0].name, whitespace_name);
}

#[test]
fn empty_master_name_is_rejected_before_a_parity_observation_is_produced() {
    // A structurally invalid sample -- both reference and candidate agreeing on
    // an empty ledger name -- must never reach a successful (Matched) parity
    // observation; it must be rejected at the reference-integrity boundary.
    let scope = scope();
    let mut reference = entry_window(&scope, "xml-entry", "-1.00", 'a');
    let mut candidate = entry_window(&scope, "json-entry", "-1.00", 'b');
    if let PackBatch::CoreAccounting(batch) = &mut reference.batch {
        batch.ledgers[0].name = String::new();
    }
    if let PackBatch::CoreAccounting(batch) = &mut candidate.batch {
        batch.ledgers[0].name = String::new();
    }

    let result = qualify_core_transport_shadow(
        &scope,
        &reference,
        &candidate,
        &reference,
        metrics(10, 20),
        metrics(21, 25),
        metrics(26, 35),
    );
    assert!(matches!(
        result,
        Err(TallyError::InvalidData { code }) if code == "transport_qualification_ledger_name_empty"
    ));
}
