use super::*;
use crate::LiveCompatibilityReceipt;

const SHA_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SHA_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const COMMIT: &str = "cccccccccccccccccccccccccccccccccccccccc";
const T0: i64 = 1_800_000_000_000;

fn snapshot(id: IdentitySnapshotId, time: i64) -> ProbeIdentitySnapshotV0 {
    ProbeIdentitySnapshotV0 {
        snapshot_id: id,
        observed_at_unix_ms: time,
        company_identity_commitment_sha256: SHA_A.to_string(),
        party_identity_commitment_sha256: SHA_A.to_string(),
        company_response_sha256: SHA_A.to_string(),
        party_response_sha256: SHA_A.to_string(),
    }
}

fn attempt(id: CandidateAttemptId, time: i64) -> ProbeCandidateAttemptV0 {
    ProbeCandidateAttemptV0 {
        attempt_id: id,
        observed_at_unix_ms: time,
        outcome: CandidateAttemptOutcome::ResponseObserved,
        http_status: Some(200),
        application_status: ApplicationStatus::Success,
        encoding: TextEncoding::Utf8,
        exact_encoded_bytes: 1024,
        encoded_body_sha256: Some(SHA_A.to_string()),
        decoded_text_sha256: Some(SHA_A.to_string()),
        safe_reason_code: None,
        bracket_state: IdentityBracketState::Unchanged,
    }
}

fn ui(position: UiObservationPosition, time: i64, screenshot: &str) -> ProbeUiObservationV0 {
    ProbeUiObservationV0 {
        position,
        observed_at_unix_ms: time,
        product: ProductFamily::TallyPrime,
        release: "7.1".to_string(),
        mode: TallyMode::Education,
        report_id: BILLS_NATIVE_OUTSTANDINGS_REPORT_ID.to_string(),
        opening_column_visible: true,
        pending_column_visible: true,
        due_column_visible: true,
        overdue_column_visible: true,
        company_identity_commitment_sha256: SHA_A.to_string(),
        party_identity_commitment_sha256: SHA_A.to_string(),
        structured_observation_sha256: SHA_A.to_string(),
        screenshot_sha256: screenshot.to_string(),
    }
}

fn receipt() -> BillsNativeOutstandingsProbeReceiptV0 {
    BillsNativeOutstandingsProbeReceiptV0 {
        schema_version: BILLS_NATIVE_OUTSTANDINGS_PROBE_RECEIPT_SCHEMA_VERSION,
        observed_at_unix_ms: T0 + 10,
        bridge_commit_sha: COMMIT.to_string(),
        working_tree_dirty: true,
        compatibility_surface_sha256: SHA_A.to_string(),
        executable_sha256: SHA_A.to_string(),
        cargo_lock_sha256: SHA_A.to_string(),
        platform: Platform::Windows,
        architecture: Architecture::X86_64,
        endpoint: ProbeEndpointV0 {
            family: LoopbackFamily::Ipv4,
            port: 9000,
            canonical_origin: "http://127.0.0.1:9000".to_string(),
        },
        attested_environment: ProbeAttestedEnvironmentV0 {
            authority: ProbeAttestationAuthority::User,
            product: ProductFamily::TallyPrime,
            release: "7.1".to_string(),
            mode: TallyMode::Education,
            locale: LocaleProfile::EnglishIndia,
            configured_tdl_count: 0,
            no_customer_data: true,
        },
        commitments: ProbeCommitmentsV0 {
            fixture_id: "education-small-v1".to_string(),
            fixture_manifest_sha256: SHA_A.to_string(),
            profile_id: BILLS_NATIVE_OUTSTANDINGS_PROFILE_ID.to_string(),
            template_sha256: SHA_A.to_string(),
            request_sha256: SHA_A.to_string(),
            scope_sha256: SHA_A.to_string(),
        },
        initial_preflight: ProbeInitialPreflightV0 {
            observed_at_unix_ms: T0 + 1,
            company_identity_commitment_sha256: SHA_A.to_string(),
            party_identity_commitment_sha256: SHA_A.to_string(),
            company_response_sha256: SHA_A.to_string(),
            party_response_sha256: SHA_A.to_string(),
        },
        identity_snapshots: [
            snapshot(IdentitySnapshotId::B0, T0 + 2),
            snapshot(IdentitySnapshotId::B1, T0 + 4),
            snapshot(IdentitySnapshotId::B2, T0 + 6),
            snapshot(IdentitySnapshotId::B3, T0 + 8),
        ],
        attempts: [
            attempt(CandidateAttemptId::A1, T0 + 3),
            attempt(CandidateAttemptId::A2, T0 + 5),
            attempt(CandidateAttemptId::A3, T0 + 7),
        ],
        byte_repeatability: ByteRepeatability::Identical,
        ui_before: ui(UiObservationPosition::Before, T0, SHA_A),
        ui_after: ui(UiObservationPosition::After, T0 + 9, SHA_B),
        user_attested_ui_change: UserAttestedUiChange::Unchanged,
        authority: ProbeReceiptAuthorityV0::observation_only(),
        receipt_sha256: String::new(),
    }
}

fn reseal(
    mut value: BillsNativeOutstandingsProbeReceiptV0,
) -> Result<BillsNativeOutstandingsProbeReceiptV0, CompatibilityError> {
    value.receipt_sha256.clear();
    value.seal()
}

#[test]
fn round_trip_is_bounded_redacted_and_structurally_separate() {
    let receipt = receipt().seal().unwrap();
    let bytes = receipt.to_pretty_json().unwrap();
    assert!(bytes.len() < BILLS_NATIVE_OUTSTANDINGS_PROBE_RECEIPT_MAX_BYTES);
    assert_eq!(
        BillsNativeOutstandingsProbeReceiptV0::from_json(&bytes).unwrap(),
        receipt
    );
    assert!(LiveCompatibilityReceipt::from_json(&bytes).is_err());
    let debug = format!("{receipt:?}");
    for forbidden in [SHA_A, SHA_B, COMMIT, "http://127.0.0.1:9000", "7.1"] {
        assert!(!debug.contains(forbidden));
    }
}

#[test]
fn checksum_tampering_and_unknown_json_fields_fail_closed() {
    let receipt = receipt().seal().unwrap();
    let mut tampered = receipt.clone();
    tampered.working_tree_dirty = !tampered.working_tree_dirty;
    assert_eq!(
        tampered.validate(),
        Err(invalid("native_probe_receipt_checksum_mismatch"))
    );

    let mut value = serde_json::to_value(receipt).unwrap();
    value.as_object_mut().unwrap().insert(
        "support_claim_eligible".to_string(),
        serde_json::Value::Bool(true),
    );
    assert!(
        BillsNativeOutstandingsProbeReceiptV0::from_json(&serde_json::to_vec(&value).unwrap())
            .is_err()
    );
}

#[test]
fn ordering_timeline_and_bracket_consistency_are_enforced() {
    let mut value = receipt();
    value.identity_snapshots.swap(0, 1);
    assert!(reseal(value).is_err());

    let mut value = receipt();
    value.attempts.swap(0, 1);
    assert!(reseal(value).is_err());

    let mut value = receipt();
    value.identity_snapshots[1].party_identity_commitment_sha256 = SHA_B.to_string();
    assert!(reseal(value.clone()).is_err());
    value.attempts[0].bracket_state = IdentityBracketState::Changed;
    value.attempts[1].bracket_state = IdentityBracketState::Changed;
    assert!(reseal(value).is_ok());

    let mut value = receipt();
    value.attempts[1].observed_at_unix_ms = T0;
    assert!(reseal(value).is_err());
}

#[test]
fn attempt_fact_shapes_and_repeatability_cannot_be_laundered() {
    let mut value = receipt();
    value.attempts[1].outcome = CandidateAttemptOutcome::TransportFailed;
    value.attempts[1].http_status = None;
    value.attempts[1].application_status = ApplicationStatus::NotApplicable;
    value.attempts[1].encoding = TextEncoding::Unknown;
    value.attempts[1].exact_encoded_bytes = 0;
    value.attempts[1].encoded_body_sha256 = None;
    value.attempts[1].decoded_text_sha256 = None;
    value.attempts[1].safe_reason_code = Some("request_failed".to_string());
    assert!(reseal(value.clone()).is_err());
    value.byte_repeatability = ByteRepeatability::NotEstablished;
    assert!(reseal(value).is_ok());

    let mut value = receipt();
    value.attempts[2].encoded_body_sha256 = Some(SHA_B.to_string());
    value.attempts[2].decoded_text_sha256 = Some(SHA_B.to_string());
    assert!(reseal(value.clone()).is_err());
    value.byte_repeatability = ByteRepeatability::Different;
    assert!(reseal(value).is_ok());

    let mut value = receipt();
    value.attempts[0].exact_encoded_bytes = BILLS_NATIVE_OUTSTANDINGS_RESPONSE_MAX_BYTES + 1;
    assert!(reseal(value).is_err());
}

#[test]
fn environment_ui_and_authority_cannot_promote_the_observation() {
    let mut value = receipt();
    value.attested_environment.no_customer_data = false;
    assert!(reseal(value).is_err());

    let mut value = receipt();
    value.attested_environment.configured_tdl_count = 1;
    assert!(reseal(value).is_err());

    let mut value = receipt();
    value.ui_after.screenshot_sha256 = value.ui_before.screenshot_sha256.clone();
    assert!(reseal(value).is_err());

    let mut value = receipt();
    value.ui_after.structured_observation_sha256 = SHA_B.to_string();
    assert!(reseal(value.clone()).is_err());
    value.user_attested_ui_change = UserAttestedUiChange::Changed;
    assert!(reseal(value).is_ok());

    for mutate in [
        |authority: &mut ProbeReceiptAuthorityV0| authority.support_claim_eligible = true,
        |authority: &mut ProbeReceiptAuthorityV0| authority.response_scope_bound = true,
        |authority: &mut ProbeReceiptAuthorityV0| authority.accounting_semantics_established = true,
        |authority: &mut ProbeReceiptAuthorityV0| authority.raw_response_retained = true,
    ] {
        let mut value = receipt();
        mutate(&mut value.authority);
        assert!(reseal(value).is_err());
    }
}

#[test]
fn endpoint_profile_and_artifact_size_are_strict() {
    let mut value = receipt();
    value.endpoint.canonical_origin = "http://localhost:9000".to_string();
    assert!(reseal(value).is_err());

    let mut value = receipt();
    value.commitments.profile_id = "native_ledger_outstandings_candidate_v1".to_string();
    assert!(reseal(value).is_err());

    assert_eq!(
        BillsNativeOutstandingsProbeReceiptV0::from_json(&vec![
            b' ';
            BILLS_NATIVE_OUTSTANDINGS_PROBE_RECEIPT_MAX_BYTES
                + 1
        ]),
        Err(invalid("native_probe_receipt_size_invalid"))
    );
}
