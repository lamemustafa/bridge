use bridge_tally_write::{
    authorize_fixture_canary, authorize_synthetic_write, fixture_canary_ledger_mutation,
    prepare_fixture_canary_ledger_import, prepare_ledger_import, preview_ledger_import,
    FixtureCanaryAuthorization, FixtureCanaryAuthorizationRequest, IdempotencyRegistry,
    LedgerMutation, LedgerState, PreparedLedgerImport, QualificationError, SourceLineage,
    SyntheticCompany, WriteAuthorizationRequest, WriteCapability, WriteOutcome,
    FIXTURE_CANARY_MAPPING_VERSION, MAX_LEDGER_WRITE_BATCH,
};

const COMPANY_GUID: &str = "00000000-0000-4000-8000-000000000001";
const REMOTE_ID: &str = "bridge-synthetic-ledger-001";
const HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const PROFILE: &str = "bridge.tally.ledger-write-readback/1";

fn company() -> SyntheticCompany {
    SyntheticCompany::new("BRIDGE SYNTHETIC BOOK", COMPANY_GUID).unwrap()
}

fn lineage(index: usize) -> SourceLineage {
    SourceLineage::new("synthetic-source", format!("record-{index}"), "version-1").unwrap()
}

fn state(name: &str, balance: &str) -> LedgerState {
    LedgerState::new(
        name,
        Some("BRIDGE SYNTHETIC GROUP".to_owned()),
        Some("29ABCDE1234F1Z5".to_owned()),
        Some(balance.to_owned()),
    )
    .unwrap()
}

fn create(remote_id: impl Into<String>, after: LedgerState, index: usize) -> LedgerMutation {
    LedgerMutation::create(remote_id, after, lineage(index)).unwrap()
}

fn alter(remote_id: impl Into<String>, before: LedgerState, after: LedgerState) -> LedgerMutation {
    LedgerMutation::alter(remote_id, before, after, lineage(1)).unwrap()
}

fn authorization(
    key: &str,
    company: &SyntheticCompany,
    mutations: &[LedgerMutation],
) -> bridge_tally_write::WriteAuthorization {
    let preview = preview_ledger_import(company, mutations, "mapping-v1").unwrap();
    authorize_synthetic_write(WriteAuthorizationRequest {
        explicit_opt_in: true,
        synthetic_company_confirmed: true,
        company_guid: COMPANY_GUID.to_owned(),
        capability: WriteCapability::Observed,
        backup_guidance_acknowledged: true,
        approval_evidence_sha256: HASH.to_owned(),
        approved_wire_sha256: preview.wire_digest().as_hex().to_owned(),
        approved_intended_state_sha256: preview.intended_state_digest().as_hex().to_owned(),
        approved_identity_query_sha256: preview.identity_query_digest().as_hex().to_owned(),
        idempotency_key: key.to_owned(),
        outbox_id: format!("outbox-{key}"),
        mapping_version: "mapping-v1".to_owned(),
    })
    .unwrap()
}

fn prepare(
    key: &str,
    mutations: Vec<LedgerMutation>,
    registry: &mut IdempotencyRegistry,
) -> Result<PreparedLedgerImport, QualificationError> {
    let company = company();
    let authorization = authorization(key, &company, &mutations);
    prepare_ledger_import(company, mutations, authorization, registry)
}

fn fixture_authorization(
    company: &SyntheticCompany,
    reservation_id: &str,
    idempotency_key: &str,
) -> FixtureCanaryAuthorization {
    let mutation = fixture_canary_ledger_mutation().unwrap();
    let preview =
        preview_ledger_import(company, &[mutation], FIXTURE_CANARY_MAPPING_VERSION).unwrap();
    authorize_fixture_canary(FixtureCanaryAuthorizationRequest {
        explicit_opt_in: true,
        synthetic_company_confirmed: true,
        company_guid: COMPANY_GUID.to_owned(),
        backup_guidance_acknowledged: true,
        review_commitment_sha256: HASH.to_owned(),
        reservation_id: reservation_id.to_owned(),
        reservation_payload_sha256: HASH.to_owned(),
        approved_wire_sha256: preview.wire_digest().as_hex().to_owned(),
        approved_intended_state_sha256: preview.intended_state_digest().as_hex().to_owned(),
        approved_identity_query_sha256: preview.identity_query_digest().as_hex().to_owned(),
        idempotency_key: idempotency_key.to_owned(),
    })
    .unwrap()
}

fn export(
    company_guid: &str,
    schema: &str,
    query_digest: &str,
    ledgers: &str,
    count: usize,
) -> String {
    format!(
        r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><COMPANYCONTEXT SCHEMA="{schema}" OBJECTTYPE="LEDGER" NAME="BRIDGE SYNTHETIC BOOK" GUID="{company_guid}" RECORDCOUNT="{count}" QUERYIDENTITYSETSHA256="{query_digest}"/>{ledgers}</BODY></ENVELOPE>"#
    )
}

fn ledger(remote_id: &str, name: &str, balance: &str) -> String {
    format!(
        r#"<LEDGER REMOTEID="{remote_id}" NAME="{name}"><PARENT>BRIDGE SYNTHETIC GROUP</PARENT><PARTYGSTIN>29ABCDE1234F1Z5</PARTYGSTIN><OPENINGBALANCE>{balance}</OPENINGBALANCE></LEDGER>"#
    )
}

fn receipt(created: u64, altered: u64, deleted: u64) -> String {
    format!(
        "<RESPONSE><CREATED>{created}</CREATED><ALTERED>{altered}</ALTERED><DELETED>{deleted}</DELETED><IGNORED>0</IGNORED><ERRORS>0</ERRORS><CANCELLED>0</CANCELLED><EXCEPTIONS>0</EXCEPTIONS></RESPONSE>"
    )
}

#[test]
fn authorization_gates_are_mandatory() {
    let base = WriteAuthorizationRequest {
        explicit_opt_in: true,
        synthetic_company_confirmed: true,
        company_guid: COMPANY_GUID.to_owned(),
        capability: WriteCapability::Observed,
        backup_guidance_acknowledged: true,
        approval_evidence_sha256: HASH.to_owned(),
        approved_wire_sha256: HASH.to_owned(),
        approved_intended_state_sha256: HASH.to_owned(),
        approved_identity_query_sha256: HASH.to_owned(),
        idempotency_key: "key".to_owned(),
        outbox_id: "outbox".to_owned(),
        mapping_version: "mapping-v1".to_owned(),
    };
    let mut request = base.clone();
    request.explicit_opt_in = false;
    assert_eq!(
        authorize_synthetic_write(request).unwrap_err(),
        QualificationError::ExplicitOptInRequired
    );
    let mut request = base.clone();
    request.synthetic_company_confirmed = false;
    assert_eq!(
        authorize_synthetic_write(request).unwrap_err(),
        QualificationError::SyntheticCompanyRequired
    );
    let mut request = base.clone();
    request.capability = WriteCapability::Documented;
    assert_eq!(
        authorize_synthetic_write(request).unwrap_err(),
        QualificationError::ObservedCapabilityRequired
    );
    let mut request = base;
    request.backup_guidance_acknowledged = false;
    assert_eq!(
        authorize_synthetic_write(request).unwrap_err(),
        QualificationError::BackupAcknowledgementRequired
    );
}

#[test]
fn authorization_requests_redact_fixture_and_commitment_values_in_debug_output() {
    let synthetic_company = company();
    let generic_mutation = create(REMOTE_ID, state("BRIDGE LEDGER", "10.00"), 1);
    let generic_preview =
        preview_ledger_import(&synthetic_company, &[generic_mutation], "mapping-v1")
            .expect("preview generic authorization request");
    let generic = WriteAuthorizationRequest {
        explicit_opt_in: true,
        synthetic_company_confirmed: true,
        company_guid: COMPANY_GUID.to_owned(),
        capability: WriteCapability::Observed,
        backup_guidance_acknowledged: true,
        approval_evidence_sha256: HASH.to_owned(),
        approved_wire_sha256: generic_preview.wire_digest().as_hex().to_owned(),
        approved_intended_state_sha256: generic_preview.intended_state_digest().as_hex().to_owned(),
        approved_identity_query_sha256: generic_preview.identity_query_digest().as_hex().to_owned(),
        idempotency_key: "fixture-sensitive-key".to_owned(),
        outbox_id: "fixture-sensitive-outbox".to_owned(),
        mapping_version: "mapping-v1".to_owned(),
    };
    let fixture = FixtureCanaryAuthorizationRequest {
        explicit_opt_in: true,
        synthetic_company_confirmed: true,
        company_guid: COMPANY_GUID.to_owned(),
        backup_guidance_acknowledged: true,
        review_commitment_sha256: HASH.to_owned(),
        reservation_id: "fixture-sensitive-reservation".to_owned(),
        reservation_payload_sha256: HASH.to_owned(),
        approved_wire_sha256: HASH.to_owned(),
        approved_intended_state_sha256: HASH.to_owned(),
        approved_identity_query_sha256: HASH.to_owned(),
        idempotency_key: "fixture-sensitive-key".to_owned(),
    };
    let generic_debug = format!("{generic:?}");
    let fixture_debug = format!("{fixture:?}");
    for secret in [
        COMPANY_GUID,
        HASH,
        "fixture-sensitive-key",
        "fixture-sensitive-outbox",
        "fixture-sensitive-reservation",
    ] {
        assert!(!generic_debug.contains(secret));
        assert!(!fixture_debug.contains(secret));
    }
}

#[test]
fn fixture_canary_is_fixed_reservation_bound_and_dispatch_ineligible() {
    let synthetic_company = company();
    let first = fixture_canary_ledger_mutation().expect("construct fixed canary");
    let second = fixture_canary_ledger_mutation().expect("construct fixed canary replay");
    let first_preview =
        preview_ledger_import(&synthetic_company, &[first], FIXTURE_CANARY_MAPPING_VERSION)
            .unwrap();
    let second_preview = preview_ledger_import(
        &synthetic_company,
        &[second],
        FIXTURE_CANARY_MAPPING_VERSION,
    )
    .unwrap();
    assert_eq!(
        first_preview.wire_digest().as_hex(),
        second_preview.wire_digest().as_hex()
    );
    assert_eq!(
        first_preview.intended_state_digest().as_hex(),
        second_preview.intended_state_digest().as_hex()
    );

    let authorization =
        fixture_authorization(&synthetic_company, "fixture-reservation-1", "fixture-key-1");
    assert_eq!(authorization.reservation_id(), "fixture-reservation-1");
    assert_eq!(authorization.reservation_payload_sha256(), HASH);
    assert_eq!(authorization.review_commitment_sha256(), HASH);
    let mut registry = IdempotencyRegistry::default();
    let prepared =
        prepare_fixture_canary_ledger_import(synthetic_company, authorization, &mut registry)
            .expect("prepare exact fixture canary");
    assert!(!prepared.dispatch_eligible());

    let mut mismatched_request = FixtureCanaryAuthorizationRequest {
        explicit_opt_in: true,
        synthetic_company_confirmed: true,
        company_guid: COMPANY_GUID.to_owned(),
        backup_guidance_acknowledged: true,
        review_commitment_sha256: HASH.to_owned(),
        reservation_id: "fixture-reservation-2".to_owned(),
        reservation_payload_sha256: HASH.to_owned(),
        approved_wire_sha256: HASH.to_owned(),
        approved_intended_state_sha256: HASH.to_owned(),
        approved_identity_query_sha256: HASH.to_owned(),
        idempotency_key: "fixture-key-2".to_owned(),
    };
    assert_eq!(
        prepare_fixture_canary_ledger_import(
            company(),
            authorize_fixture_canary(mismatched_request.clone()).unwrap(),
            &mut IdempotencyRegistry::default(),
        )
        .unwrap_err(),
        QualificationError::ApprovalMismatch
    );
    mismatched_request.explicit_opt_in = false;
    assert_eq!(
        authorize_fixture_canary(mismatched_request).unwrap_err(),
        QualificationError::ExplicitOptInRequired
    );
}

#[test]
fn approval_must_bind_the_exact_preview_commitments() {
    let company = company();
    let mutations = vec![create(REMOTE_ID, state("BRIDGE LEDGER", "10.00"), 1)];
    let preview = preview_ledger_import(&company, &mutations, "mapping-v1").unwrap();
    let authorization = authorize_synthetic_write(WriteAuthorizationRequest {
        explicit_opt_in: true,
        synthetic_company_confirmed: true,
        company_guid: COMPANY_GUID.to_owned(),
        capability: WriteCapability::Observed,
        backup_guidance_acknowledged: true,
        approval_evidence_sha256: HASH.to_owned(),
        approved_wire_sha256: HASH.to_owned(),
        approved_intended_state_sha256: preview.intended_state_digest().as_hex().to_owned(),
        approved_identity_query_sha256: preview.identity_query_digest().as_hex().to_owned(),
        idempotency_key: "key-mismatched-approval".to_owned(),
        outbox_id: "outbox-mismatched-approval".to_owned(),
        mapping_version: "mapping-v1".to_owned(),
    })
    .unwrap();
    assert_eq!(
        prepare_ledger_import(
            company,
            mutations,
            authorization,
            &mut IdempotencyRegistry::default(),
        )
        .unwrap_err(),
        QualificationError::ApprovalMismatch
    );
}

#[test]
fn alter_approval_binds_the_exact_declared_before_state() {
    let company = company();
    let approved = vec![alter(
        REMOTE_ID,
        state("BRIDGE LEDGER", "10.00"),
        state("BRIDGE LEDGER", "20.00"),
    )];
    let authorization = authorization("key-before-binding", &company, &approved);
    let changed_before = vec![alter(
        REMOTE_ID,
        state("BRIDGE LEDGER", "11.00"),
        state("BRIDGE LEDGER", "20.00"),
    )];
    assert_eq!(
        prepare_ledger_import(
            company,
            changed_before,
            authorization,
            &mut IdempotencyRegistry::default(),
        )
        .unwrap_err(),
        QualificationError::ApprovalMismatch
    );
}

#[test]
fn prepared_import_is_deterministic_and_bytes_are_not_publicly_exposed() {
    let mutation = create(
        "bridge-&-identity",
        LedgerState::new(
            "BRIDGE <SYNTHETIC>",
            Some("BRIDGE SYNTHETIC GROUP".to_owned()),
            None,
            Some("0".to_owned()),
        )
        .unwrap(),
        1,
    );
    let mut first_registry = IdempotencyRegistry::default();
    let first = prepare("key-1", vec![mutation.clone()], &mut first_registry).unwrap();
    let mut second_registry = IdempotencyRegistry::default();
    let second = prepare("key-1", vec![mutation], &mut second_registry).unwrap();
    assert!(!first.dispatch_eligible());
    assert_eq!(first.wire_digest().as_hex(), second.wire_digest().as_hex());
    assert_ne!(
        first.wire_digest().as_hex(),
        first.intended_state_digest().as_hex()
    );
    let query = first.identity_query_digest().as_hex().to_owned();
    first
        .qualify_preflight(&export(COMPANY_GUID, PROFILE, &query, "", 0))
        .unwrap();
}

#[test]
fn exact_company_bound_create_requires_lifecycle_and_exact_counters() {
    let mut registry = IdempotencyRegistry::default();
    let prepared = prepare(
        "key-applied",
        vec![create(REMOTE_ID, state("BRIDGE LEDGER", "10.00"), 1)],
        &mut registry,
    )
    .unwrap();
    let query = prepared.identity_query_digest().as_hex().to_owned();
    let sent = prepared
        .qualify_preflight(&export(COMPANY_GUID, PROFILE, &query, "", 0))
        .unwrap()
        .record_dispatch_attempt("request-1", &mut registry)
        .unwrap();
    let awaiting = sent
        .record_import_receipt(&receipt(1, 0, 0), &mut registry)
        .unwrap();
    let after = ledger(REMOTE_ID, "BRIDGE LEDGER", "10.00");
    let verdict = awaiting
        .verify_readback(
            &export(COMPANY_GUID, PROFILE, &query, &after, 1),
            &mut registry,
        )
        .unwrap();
    assert_eq!(verdict.outcome(), WriteOutcome::ExactApplied);
    assert!(!verdict.auto_retry_allowed());
}

#[test]
fn contradictory_operation_counters_cannot_verify() {
    for (key, counters) in [
        ("wrong-alter", receipt(0, 1, 0)),
        ("wrong-delete", receipt(0, 0, 1)),
    ] {
        let mut registry = IdempotencyRegistry::default();
        let prepared = prepare(
            key,
            vec![create(REMOTE_ID, state("BRIDGE LEDGER", "10.00"), 1)],
            &mut registry,
        )
        .unwrap();
        let query = prepared.identity_query_digest().as_hex().to_owned();
        let awaiting = prepared
            .qualify_preflight(&export(COMPANY_GUID, PROFILE, &query, "", 0))
            .unwrap()
            .record_dispatch_attempt(format!("request-{key}"), &mut registry)
            .unwrap()
            .record_import_receipt(&counters, &mut registry)
            .unwrap();
        let after = ledger(REMOTE_ID, "BRIDGE LEDGER", "10.00");
        assert_eq!(
            awaiting
                .verify_readback(
                    &export(COMPANY_GUID, PROFILE, &query, &after, 1),
                    &mut registry,
                )
                .unwrap()
                .outcome(),
            WriteOutcome::Mismatch
        );
    }
}

#[test]
fn clean_counters_plus_unchanged_alter_state_cannot_verify() {
    let mut registry = IdempotencyRegistry::default();
    let old = ledger(REMOTE_ID, "BRIDGE LEDGER", "10.00");
    let prepared = prepare(
        "key-stale",
        vec![alter(
            REMOTE_ID,
            state("BRIDGE LEDGER", "10.00"),
            state("BRIDGE LEDGER", "20.00"),
        )],
        &mut registry,
    )
    .unwrap();
    let query = prepared.identity_query_digest().as_hex().to_owned();
    let awaiting = prepared
        .qualify_preflight(&export(COMPANY_GUID, PROFILE, &query, &old, 1))
        .unwrap()
        .record_dispatch_attempt("request-stale", &mut registry)
        .unwrap()
        .record_import_receipt(&receipt(0, 1, 0), &mut registry)
        .unwrap();
    assert_eq!(
        awaiting
            .verify_readback(
                &export(COMPANY_GUID, PROFILE, &query, &old, 1),
                &mut registry,
            )
            .unwrap()
            .outcome(),
        WriteOutcome::Mismatch
    );
}

#[test]
fn lost_response_remains_outcome_unknown_even_with_exact_readback() {
    let mut registry = IdempotencyRegistry::default();
    let prepared = prepare(
        "key-unknown",
        vec![create(REMOTE_ID, state("BRIDGE LEDGER", "10.00"), 1)],
        &mut registry,
    )
    .unwrap();
    let query = prepared.identity_query_digest().as_hex().to_owned();
    let unknown = prepared
        .qualify_preflight(&export(COMPANY_GUID, PROFILE, &query, "", 0))
        .unwrap()
        .record_dispatch_attempt("request-unknown", &mut registry)
        .unwrap()
        .record_outcome_unknown(&mut registry)
        .unwrap();
    let after = ledger(REMOTE_ID, "BRIDGE LEDGER", "10.00");
    assert_eq!(
        unknown
            .observe_readback(&export(COMPANY_GUID, PROFILE, &query, &after, 1))
            .unwrap()
            .outcome(),
        WriteOutcome::OutcomeUnknown
    );
    assert_eq!(
        unknown
            .observe_readback(&export(COMPANY_GUID, PROFILE, &query, "", 0))
            .unwrap()
            .outcome(),
        WriteOutcome::OutcomeUnknown
    );
}

#[test]
fn parsed_zero_mutation_receipt_can_prove_exact_not_applied() {
    let mut registry = IdempotencyRegistry::default();
    let prepared = prepare(
        "key-not-applied",
        vec![create(REMOTE_ID, state("BRIDGE LEDGER", "10.00"), 1)],
        &mut registry,
    )
    .unwrap();
    let query = prepared.identity_query_digest().as_hex().to_owned();
    let awaiting = prepared
        .qualify_preflight(&export(COMPANY_GUID, PROFILE, &query, "", 0))
        .unwrap()
        .record_dispatch_attempt("request-not-applied", &mut registry)
        .unwrap()
        .record_import_receipt(&receipt(0, 0, 0), &mut registry)
        .unwrap();
    assert_eq!(
        awaiting
            .verify_readback(&export(COMPANY_GUID, PROFILE, &query, "", 0), &mut registry,)
            .unwrap()
            .outcome(),
        WriteOutcome::ExactNotApplied
    );
}

#[test]
fn wrong_company_profile_identity_or_field_fails_closed_or_mismatches() {
    let mut registry = IdempotencyRegistry::default();
    let prepared = prepare(
        "key-scope",
        vec![create(REMOTE_ID, state("BRIDGE LEDGER", "10.00"), 1)],
        &mut registry,
    )
    .unwrap();
    let query = prepared.identity_query_digest().as_hex().to_owned();
    assert_eq!(
        prepared
            .clone()
            .qualify_preflight(&export(
                "00000000-0000-4000-8000-000000000099",
                PROFILE,
                &query,
                "",
                0,
            ))
            .unwrap_err(),
        QualificationError::InvalidReadback
    );
    assert_eq!(
        prepared
            .clone()
            .qualify_preflight(&export(COMPANY_GUID, PROFILE, HASH, "", 0))
            .unwrap_err(),
        QualificationError::InvalidReadback
    );
    assert_eq!(
        prepared
            .clone()
            .qualify_preflight(&export(
                COMPANY_GUID,
                "bridge.tally.ledgers/1",
                &query,
                "",
                0,
            ))
            .unwrap_err(),
        QualificationError::InvalidReadback
    );
    let sent = prepared
        .qualify_preflight(&export(COMPANY_GUID, PROFILE, &query, "", 0))
        .unwrap()
        .record_dispatch_attempt("request-scope", &mut registry)
        .unwrap();
    let awaiting = sent
        .record_import_receipt(&receipt(1, 0, 0), &mut registry)
        .unwrap();
    let changed = ledger(REMOTE_ID, "BRIDGE LEDGER", "10.01");
    assert_eq!(
        awaiting
            .verify_readback(
                &export(COMPANY_GUID, PROFILE, &query, &changed, 1),
                &mut registry,
            )
            .unwrap()
            .outcome(),
        WriteOutcome::Mismatch
    );
}

#[test]
fn invalid_decimal_gstin_limits_duplicates_noops_and_replays_are_rejected() {
    assert_eq!(
        LedgerState::new("BRIDGE", None, None, Some("NaN".to_owned())).unwrap_err(),
        QualificationError::InvalidField("opening_balance")
    );
    assert_eq!(
        LedgerState::new("BRIDGE", None, Some("not-a-gstin".to_owned()), None).unwrap_err(),
        QualificationError::InvalidField("party_gstin")
    );
    let parentless = LedgerState::new("BRIDGE", None, None, None)
        .expect("a parentless state remains usable as an alter snapshot");
    assert_eq!(
        LedgerMutation::create(REMOTE_ID, parentless, lineage(1)).unwrap_err(),
        QualificationError::CreateParentRequired
    );
    let unchanged = state("BRIDGE", "0");
    assert_eq!(
        LedgerMutation::alter(REMOTE_ID, unchanged.clone(), unchanged.clone(), lineage(1),)
            .unwrap_err(),
        QualificationError::NoOpMutation
    );
    for (before, after, field) in [
        (
            LedgerState::new(
                "BRIDGE",
                Some("BRIDGE SYNTHETIC GROUP".to_owned()),
                None,
                None,
            )
            .unwrap(),
            LedgerState::new("BRIDGE", None, None, None).unwrap(),
            "parent",
        ),
        (
            LedgerState::new("BRIDGE", None, Some("29ABCDE1234F1Z5".to_owned()), None).unwrap(),
            LedgerState::new("BRIDGE", None, None, None).unwrap(),
            "party_gstin",
        ),
        (
            LedgerState::new("BRIDGE", None, None, Some("0".to_owned())).unwrap(),
            LedgerState::new("BRIDGE", None, None, None).unwrap(),
            "opening_balance",
        ),
    ] {
        assert_eq!(
            LedgerMutation::alter(REMOTE_ID, before, after, lineage(1)).unwrap_err(),
            QualificationError::UnsupportedFieldClear(field)
        );
    }
    let duplicate = create(REMOTE_ID, unchanged.clone(), 1);
    let mut registry = IdempotencyRegistry::default();
    assert_eq!(
        preview_ledger_import(&company(), &[duplicate.clone(), duplicate], "mapping-v1",)
            .unwrap_err(),
        QualificationError::DuplicateIdentity
    );
    let too_many: Vec<_> = (0..=MAX_LEDGER_WRITE_BATCH)
        .map(|index| create(format!("bridge-id-{index}"), unchanged.clone(), index))
        .collect();
    assert_eq!(
        preview_ledger_import(&company(), &too_many, "mapping-v1").unwrap_err(),
        QualificationError::InvalidBatchSize
    );
    let mutation = create(REMOTE_ID, unchanged, 2);
    prepare("key-replay", vec![mutation.clone()], &mut registry).unwrap();
    assert_eq!(
        prepare("key-replay", vec![mutation], &mut registry).unwrap_err(),
        QualificationError::DuplicateSubmission
    );
}

#[test]
fn line_error_evidence_is_derived_and_never_debugs_raw_text() {
    let first = bridge_tally_write::parse_import_receipt(
        "<RESPONSE><CREATED>0</CREATED><ALTERED>0</ALTERED><IGNORED>1</IGNORED><ERRORS>1</ERRORS><EXCEPTIONS>0</EXCEPTIONS><LINEERROR>PRIVATE-SYNTHETIC-SENTINEL-A</LINEERROR></RESPONSE>",
    )
    .unwrap();
    let second = bridge_tally_write::parse_import_receipt(
        "<RESPONSE><CREATED>0</CREATED><ALTERED>0</ALTERED><IGNORED>1</IGNORED><ERRORS>1</ERRORS><EXCEPTIONS>0</EXCEPTIONS><LINEERROR>PRIVATE-SYNTHETIC-SENTINEL-B</LINEERROR></RESPONSE>",
    )
    .unwrap();
    assert_ne!(
        first.line_error_digests()[0].as_hex(),
        second.line_error_digests()[0].as_hex()
    );
    let debug = format!("{first:?}");
    assert!(!debug.contains("PRIVATE-SYNTHETIC"));
    assert!(!debug.contains("SENTINEL"));
}

#[test]
fn documented_direct_failure_receipt_retains_redacted_evidence_without_becoming_clean() {
    let receipt = bridge_tally_write::parse_import_receipt(
        "<ENVELOPE><HEADER><STATUS>0</STATUS></HEADER><BODY><DATA><CREATED>0</CREATED><ALTERED>0</ALTERED><DELETED>0</DELETED><IGNORED>0</IGNORED><ERRORS>1</ERRORS><CANCELLED>0</CANCELLED><LINEERROR>PRIVATE-SYNTHETIC-FAILURE</LINEERROR></DATA></BODY></ENVELOPE>",
    )
    .expect("documented failure receipt remains auditable");
    assert_eq!(
        receipt.application_status(),
        bridge_tally_protocol::TallyImportApplicationStatus::Failure
    );
    assert_eq!(receipt.counters().errors, 1);
    assert_eq!(receipt.line_error_digests().len(), 1);
    assert!(!receipt.exceptions_were_reported());
    let debug = format!("{receipt:?}");
    assert!(!debug.contains("PRIVATE-SYNTHETIC-FAILURE"));
}
