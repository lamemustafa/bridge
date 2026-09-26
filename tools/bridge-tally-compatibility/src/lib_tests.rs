use super::*;
use ed25519_dalek::{Signer, SigningKey};

const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const COMMIT: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const NOW: i64 = 1_800_000_000_000;

fn profile<T>(value: T) -> ProfileValue<T> {
    ProfileValue {
        value,
        authority: EvidenceAuthority::UserAttestation,
        confidence: EvidenceConfidence::Attested,
    }
}

fn configured<T>(value: T) -> ProfileValue<T> {
    ProfileValue {
        value,
        authority: EvidenceAuthority::BridgeConfiguration,
        confidence: EvidenceConfidence::Attested,
    }
}

fn operation(profile: ReadProfileId) -> OperationEvidence {
    OperationEvidence {
        profile,
        template_sha256: SHA.to_string(),
        outcome: OperationOutcome::Passed,
        application_status: ApplicationStatus::Success,
        encoding: TextEncoding::Utf8,
        response_size: SizeBucket::Bytes1To4096,
        record_count: CountBucket::One,
        safe_reason_code: None,
    }
}

fn receipt_for(surface: &str, product: ProductFamily, mode: TallyMode) -> LiveCompatibilityReceipt {
    LiveCompatibilityReceipt {
        schema_version: LIVE_RECEIPT_SCHEMA_VERSION,
        observed_at_unix_ms: NOW - 10_000,
        bridge_commit_sha: COMMIT.to_string(),
        working_tree_dirty: false,
        compatibility_surface_sha256: surface.to_string(),
        executable_sha256: SHA.to_string(),
        cargo_lock_sha256: SHA.to_string(),
        platform: Platform::Windows,
        architecture: Architecture::X86_64,
        endpoint_family: LoopbackFamily::Ipv4,
        transport: TransportProfile::XmlHttp,
        product: profile(product),
        release: profile("7.1".to_string()),
        mode: profile(mode),
        odbc_state: profile(OdbcState::Disabled),
        locale: profile(LocaleProfile::EnglishIndia),
        dataset_tier: configured(DatasetTier::SyntheticSmall),
        fixture_manifest_sha256: SHA.to_string(),
        fixture_marker_verified: true,
        no_customer_data: profile(true),
        loaded_company_count: CountBucket::One,
        operations: [
            ReadProfileId::XmlCompanyEnumerationV1,
            ReadProfileId::XmlSyntheticFixtureMarkerV1,
            ReadProfileId::XmlLedgerReadV1,
            ReadProfileId::XmlVoucherEmptyRangeV1,
            ReadProfileId::XmlVoucherPopulatedRangeV1,
        ]
        .into_iter()
        .map(operation)
        .collect(),
        authority: LiveReadAuthority::observation_only(),
        receipt_sha256: String::new(),
    }
    .seal()
    .unwrap()
}

fn receipt(surface: &str) -> LiveCompatibilityReceipt {
    receipt_for(surface, ProductFamily::TallyPrime, TallyMode::Education)
}

fn trust(signing: &SigningKey) -> TrustedEvidenceKeys {
    TrustedEvidenceKeys {
        schema_version: TRUST_MANIFEST_SCHEMA_VERSION,
        keys: vec![TrustedEvidenceKey {
            key_id: "release-evidence-1".to_string(),
            public_key_hex: hex::encode(signing.verifying_key().to_bytes()),
            valid_from_unix_ms: NOW - 100_000,
            valid_until_unix_ms: NOW + 100_000,
            revoked_at_unix_ms: None,
        }],
    }
}

fn attestation(
    receipt: &LiveCompatibilityReceipt,
    surface: &CompatibilitySurfaceManifest,
    signing: &SigningKey,
) -> ReviewedEvidenceAttestation {
    let mut value = ReviewedEvidenceAttestation {
        schema_version: ATTESTATION_SCHEMA_VERSION,
        evidence_id: "evidence-1".to_string(),
        receipt_sha256: receipt.receipt_sha256.clone(),
        compatibility_surface_sha256: surface.digest().unwrap(),
        reviewed_at_unix_ms: NOW - 1_000,
        expires_at_unix_ms: NOW + 50_000,
        review_commit_sha: COMMIT.to_string(),
        review_url: "https://github.com/lamemustafa/bridge/pull/1".to_string(),
        key_id: "release-evidence-1".to_string(),
        signature_hex: "00".repeat(64),
    };
    value.signature_hex = hex::encode(signing.sign(&value.signing_bytes().unwrap()).to_bytes());
    value
}

fn unsupported_manifest(required_profile: ReadProfileId) -> SupportClaimsManifest {
    SupportClaimsManifest {
        schema_version: SUPPORT_MANIFEST_SCHEMA_VERSION,
        bridge_commit_sha: COMMIT.to_string(),
        claims: vec![SupportClaim {
            claim_id: "unsupported-exact-scope".to_string(),
            level: ClaimLevel::Unsupported,
            promotion_eligible: false,
            product: ProductFamily::TallyPrime,
            release: "7.1".to_string(),
            mode: TallyMode::Education,
            platform: Platform::Windows,
            architecture: Architecture::X86_64,
            transport: TransportProfile::XmlHttp,
            endpoint_family: LoopbackFamily::Ipv4,
            odbc_state: OdbcState::Disabled,
            company_state: CompanyLoadState::One,
            locale: LocaleProfile::EnglishIndia,
            encoding: TextEncoding::Utf8,
            dataset_tier: DatasetTier::SyntheticSmall,
            fixture_manifest_sha256: Some(SHA.to_string()),
            required_profiles: vec![required_profile],
            max_evidence_age_days: 30,
            evidence_id: Some("evidence-1".to_string()),
        }],
    }
}

fn not_attempted_operation(profile: ReadProfileId) -> OperationEvidence {
    OperationEvidence {
        profile,
        template_sha256: SHA.to_string(),
        outcome: OperationOutcome::NotAttempted,
        application_status: ApplicationStatus::NotApplicable,
        encoding: TextEncoding::Unknown,
        response_size: SizeBucket::Zero,
        record_count: CountBucket::Unknown,
        safe_reason_code: Some("fixture_not_verified".to_string()),
    }
}

#[test]
fn receipt_round_trip_is_bounded_private_and_checksum_bound() {
    let receipt = receipt(SHA);
    let bytes = receipt.to_pretty_json().unwrap();
    assert!(bytes.len() < MAX_ARTIFACT_BYTES);
    assert_eq!(
        LiveCompatibilityReceipt::from_json(&bytes).unwrap(),
        receipt
    );
    let mut tampered = receipt.clone();
    tampered.loaded_company_count = CountBucket::TwoToFive;
    assert_eq!(
        tampered.validate().unwrap_err(),
        invalid("receipt_checksum_mismatch")
    );
    let text = String::from_utf8(bytes).unwrap();
    for forbidden in [
        "company_name",
        "company_guid",
        "endpoint_port",
        "raw_xml",
        "amount",
    ] {
        assert!(!text.contains(forbidden));
    }
}

#[test]
fn edit_log_product_family_has_a_distinct_stable_wire_value() {
    let encoded = serde_json::to_string(&ProductFamily::TallyPrimeEditLog).unwrap();
    assert_eq!(encoded, "\"tally_prime_edit_log\"");
    assert_eq!(
        serde_json::from_str::<ProductFamily>(&encoded).unwrap(),
        ProductFamily::TallyPrimeEditLog
    );
}

#[test]
fn edit_log_education_cannot_reach_a_positive_claim_with_valid_signed_evidence() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("surface.txt"), b"surface").unwrap();
    let surface = CompatibilitySurfaceManifest {
        schema_version: SURFACE_SCHEMA_VERSION,
        files: vec![SurfaceFile {
            path: "surface.txt".to_string(),
            sha256: sha256_file(&temp.path().join("surface.txt")).unwrap(),
        }],
    };
    let receipt = receipt_for(
        &surface.digest().unwrap(),
        ProductFamily::TallyPrimeEditLog,
        TallyMode::Education,
    );
    let signing = SigningKey::from_bytes(&[7_u8; 32]);
    let trust = trust(&signing);
    let attestation = attestation(&receipt, &surface, &signing);
    assert!(receipt.validate().is_ok());
    assert!(attestation.verify(&trust, NOW).is_ok());

    let profiles = receipt
        .operations
        .iter()
        .map(|operation| operation.profile)
        .collect::<Vec<_>>();
    for level in [ClaimLevel::Observed, ClaimLevel::Supported] {
        let manifest = SupportClaimsManifest {
            schema_version: SUPPORT_MANIFEST_SCHEMA_VERSION,
            bridge_commit_sha: COMMIT.to_string(),
            claims: vec![SupportClaim {
                claim_id: "edit-log-education-positive".to_string(),
                level,
                promotion_eligible: true,
                product: ProductFamily::TallyPrimeEditLog,
                release: "7.1".to_string(),
                mode: TallyMode::Education,
                platform: Platform::Windows,
                architecture: Architecture::X86_64,
                transport: TransportProfile::XmlHttp,
                endpoint_family: LoopbackFamily::Ipv4,
                odbc_state: OdbcState::Disabled,
                company_state: CompanyLoadState::One,
                locale: LocaleProfile::EnglishIndia,
                encoding: TextEncoding::Utf8,
                dataset_tier: DatasetTier::SyntheticSmall,
                fixture_manifest_sha256: Some(SHA.to_string()),
                required_profiles: profiles.clone(),
                max_evidence_age_days: 30,
                evidence_id: Some("evidence-1".to_string()),
            }],
        };
        assert_eq!(
            enforce_support_gate(
                &manifest,
                &surface,
                &trust,
                std::slice::from_ref(&receipt),
                std::slice::from_ref(&attestation),
                temp.path(),
                NOW,
            )
            .unwrap_err(),
            invalid("education_positive_claim_forbidden")
        );
    }
}

#[test]
fn receipt_cannot_claim_support_authenticity_or_writes() {
    for mutate in [
        |value: &mut LiveCompatibilityReceipt| value.authority.support_claim_eligible = true,
        |value: &mut LiveCompatibilityReceipt| value.authority.writes_attempted = true,
        |value: &mut LiveCompatibilityReceipt| {
            value.authority.responder_authenticity_established = true
        },
    ] {
        let mut value = receipt(SHA);
        value.receipt_sha256.clear();
        mutate(&mut value);
        assert_eq!(
            value.seal().unwrap_err(),
            invalid("receipt_authority_invalid")
        );
    }
    let mut value = receipt(SHA);
    value.receipt_sha256.clear();
    value.authority.tauri_runtime_observed = true;
    assert_eq!(
        value.seal().unwrap_err(),
        invalid("receipt_authority_invalid")
    );
}

#[test]
fn unsuccessful_attempt_is_receipted_without_live_or_fixture_claims() {
    let not_attempted = |profile| OperationEvidence {
        profile,
        template_sha256: SHA.to_string(),
        outcome: OperationOutcome::NotAttempted,
        application_status: ApplicationStatus::NotApplicable,
        encoding: TextEncoding::Unknown,
        response_size: SizeBucket::Zero,
        record_count: CountBucket::Unknown,
        safe_reason_code: Some("fixture_not_verified".to_string()),
    };
    let mut value = receipt(SHA);
    value.receipt_sha256.clear();
    value.fixture_marker_verified = false;
    value.loaded_company_count = CountBucket::Zero;
    value.authority = LiveReadAuthority::attempt_only();
    value.operations = vec![
        OperationEvidence {
            profile: ReadProfileId::XmlCompanyEnumerationV1,
            template_sha256: SHA.to_string(),
            outcome: OperationOutcome::Failed,
            application_status: ApplicationStatus::NotApplicable,
            encoding: TextEncoding::Unknown,
            response_size: SizeBucket::Zero,
            record_count: CountBucket::Zero,
            safe_reason_code: Some("endpoint_unreachable".to_string()),
        },
        not_attempted(ReadProfileId::XmlSyntheticFixtureMarkerV1),
        not_attempted(ReadProfileId::XmlLedgerReadV1),
        not_attempted(ReadProfileId::XmlVoucherEmptyRangeV1),
        not_attempted(ReadProfileId::XmlVoucherPopulatedRangeV1),
    ];
    let sealed = value.seal().unwrap();
    assert!(!sealed.authority.live_endpoint_response_observed);
    assert!(!sealed.fixture_marker_verified);

    let mut illegal = sealed;
    illegal.receipt_sha256.clear();
    illegal.operations[2].outcome = OperationOutcome::Failed;
    illegal.operations[2].safe_reason_code = Some("read_failed".to_string());
    assert_eq!(
        illegal.seal().unwrap_err(),
        invalid("company_read_without_fixture_marker")
    );
}

#[test]
fn unknown_claims_pass_without_live_or_trusted_evidence() {
    let temp = tempfile::tempdir().unwrap();
    create_required_surface_directories(temp.path());
    fs::write(temp.path().join("surface.txt"), b"surface").unwrap();
    let surface = CompatibilitySurfaceManifest {
        schema_version: SURFACE_SCHEMA_VERSION,
        files: sealed_surface_files(temp.path(), &["surface.txt"]),
    };
    let manifest = SupportClaimsManifest {
        schema_version: SUPPORT_MANIFEST_SCHEMA_VERSION,
        bridge_commit_sha: COMMIT.to_string(),
        claims: vec![SupportClaim {
            claim_id: "tally-prime-7-1-windows-education".to_string(),
            level: ClaimLevel::Unknown,
            promotion_eligible: true,
            product: ProductFamily::TallyPrime,
            release: "7.1".to_string(),
            mode: TallyMode::Education,
            platform: Platform::Windows,
            architecture: Architecture::X86_64,
            transport: TransportProfile::XmlHttp,
            endpoint_family: LoopbackFamily::Ipv4,
            odbc_state: OdbcState::Disabled,
            company_state: CompanyLoadState::One,
            locale: LocaleProfile::EnglishIndia,
            encoding: TextEncoding::Utf8,
            dataset_tier: DatasetTier::SyntheticSmall,
            fixture_manifest_sha256: None,
            required_profiles: Vec::new(),
            max_evidence_age_days: 30,
            evidence_id: None,
        }],
    };
    let report = enforce_support_gate(
        &manifest,
        &surface,
        &TrustedEvidenceKeys {
            schema_version: TRUST_MANIFEST_SCHEMA_VERSION,
            keys: Vec::new(),
        },
        &[],
        &[],
        temp.path(),
        NOW,
    )
    .unwrap();
    assert_eq!(report.unknown_claims, 1);
    assert_eq!(report.evidenced_claims, 0);
}

#[test]
fn positive_claim_requires_fresh_signed_exact_scope_evidence() {
    let temp = tempfile::tempdir().unwrap();
    create_required_surface_directories(temp.path());
    fs::write(temp.path().join("surface.txt"), b"surface").unwrap();
    let surface = CompatibilitySurfaceManifest {
        schema_version: SURFACE_SCHEMA_VERSION,
        files: sealed_surface_files(temp.path(), &["surface.txt"]),
    };
    let receipt = receipt_for(
        &surface.digest().unwrap(),
        ProductFamily::TallyPrime,
        TallyMode::Licensed,
    );
    let signing = SigningKey::from_bytes(&[7_u8; 32]);
    let trust = TrustedEvidenceKeys {
        schema_version: TRUST_MANIFEST_SCHEMA_VERSION,
        keys: vec![TrustedEvidenceKey {
            key_id: "release-evidence-1".to_string(),
            public_key_hex: hex::encode(signing.verifying_key().to_bytes()),
            valid_from_unix_ms: NOW - 100_000,
            valid_until_unix_ms: NOW + 100_000,
            revoked_at_unix_ms: None,
        }],
    };
    let mut attestation = ReviewedEvidenceAttestation {
        schema_version: ATTESTATION_SCHEMA_VERSION,
        evidence_id: "evidence-1".to_string(),
        receipt_sha256: receipt.receipt_sha256.clone(),
        compatibility_surface_sha256: surface.digest().unwrap(),
        reviewed_at_unix_ms: NOW - 1_000,
        expires_at_unix_ms: NOW + 50_000,
        review_commit_sha: COMMIT.to_string(),
        review_url: "https://github.com/lamemustafa/bridge/pull/1".to_string(),
        key_id: "release-evidence-1".to_string(),
        signature_hex: "00".repeat(64),
    };
    attestation.signature_hex = hex::encode(
        signing
            .sign(&attestation.signing_bytes().unwrap())
            .to_bytes(),
    );
    let profiles = receipt
        .operations
        .iter()
        .map(|value| value.profile)
        .collect();
    let manifest = SupportClaimsManifest {
        schema_version: SUPPORT_MANIFEST_SCHEMA_VERSION,
        bridge_commit_sha: COMMIT.to_string(),
        claims: vec![SupportClaim {
            claim_id: "supported-exact-scope".to_string(),
            level: ClaimLevel::Supported,
            promotion_eligible: true,
            product: ProductFamily::TallyPrime,
            release: "7.1".to_string(),
            mode: TallyMode::Licensed,
            platform: Platform::Windows,
            architecture: Architecture::X86_64,
            transport: TransportProfile::XmlHttp,
            endpoint_family: LoopbackFamily::Ipv4,
            odbc_state: OdbcState::Disabled,
            company_state: CompanyLoadState::One,
            locale: LocaleProfile::EnglishIndia,
            encoding: TextEncoding::Utf8,
            dataset_tier: DatasetTier::SyntheticSmall,
            fixture_manifest_sha256: Some(SHA.to_string()),
            required_profiles: profiles,
            max_evidence_age_days: 30,
            evidence_id: Some("evidence-1".to_string()),
        }],
    };
    assert!(enforce_support_gate(
        &manifest,
        &surface,
        &trust,
        std::slice::from_ref(&receipt),
        std::slice::from_ref(&attestation),
        temp.path(),
        NOW,
    )
    .is_ok());

    // The gate binds evidence to the digest it computes from the surface
    // (bridge#760). Evidence made for another surface is refused, whether the
    // attestation names it or only the receipt does.
    let other_surface = "b".repeat(64);
    let mut attested_elsewhere = attestation.clone();
    attested_elsewhere.compatibility_surface_sha256 = other_surface.clone();
    attested_elsewhere.signature_hex = hex::encode(
        signing
            .sign(&attested_elsewhere.signing_bytes().unwrap())
            .to_bytes(),
    );
    assert_eq!(
        enforce_support_gate(
            &manifest,
            &surface,
            &trust,
            std::slice::from_ref(&receipt),
            std::slice::from_ref(&attested_elsewhere),
            temp.path(),
            NOW,
        )
        .unwrap_err(),
        gate("attestation_scope_mismatch")
    );
    let received_elsewhere = receipt_for(
        &other_surface,
        ProductFamily::TallyPrime,
        TallyMode::Licensed,
    );
    let mut attesting_it = attestation.clone();
    attesting_it.receipt_sha256 = received_elsewhere.receipt_sha256.clone();
    attesting_it.signature_hex = hex::encode(
        signing
            .sign(&attesting_it.signing_bytes().unwrap())
            .to_bytes(),
    );
    assert_eq!(
        enforce_support_gate(
            &manifest,
            &surface,
            &trust,
            std::slice::from_ref(&received_elsewhere),
            std::slice::from_ref(&attesting_it),
            temp.path(),
            NOW,
        )
        .unwrap_err(),
        gate("receipt_claim_scope_mismatch")
    );

    let mut review_time_invalid = trust.clone();
    review_time_invalid.keys[0].valid_from_unix_ms = NOW - 500;
    assert_eq!(
        enforce_support_gate(
            &manifest,
            &surface,
            &review_time_invalid,
            std::slice::from_ref(&receipt),
            std::slice::from_ref(&attestation),
            temp.path(),
            NOW,
        )
        .unwrap_err(),
        gate("attestation_review_key_inactive")
    );

    let mut wildcard = manifest.clone();
    wildcard.claims[0].release = "latest".to_string();
    assert_eq!(
        wildcard.validate().unwrap_err(),
        invalid("exact_release_required")
    );

    let mut revoked = trust.clone();
    revoked.keys[0].revoked_at_unix_ms = Some(NOW - 1);
    assert_eq!(
        enforce_support_gate(
            &manifest,
            &surface,
            &revoked,
            &[receipt],
            &[attestation],
            temp.path(),
            NOW,
        )
        .unwrap_err(),
        gate("attestation_key_inactive")
    );
}

#[test]
fn unsupported_claims_remain_disabled_without_a_profile_specific_signature() {
    let temp = tempfile::tempdir().unwrap();
    create_required_surface_directories(temp.path());
    fs::write(temp.path().join("surface.txt"), b"surface").unwrap();
    let surface = CompatibilitySurfaceManifest {
        schema_version: SURFACE_SCHEMA_VERSION,
        files: sealed_surface_files(temp.path(), &["surface.txt"]),
    };
    let signing = SigningKey::from_bytes(&[9_u8; 32]);
    let trust = trust(&signing);

    let mut invented_unsupported = receipt(&surface.digest().unwrap());
    invented_unsupported.receipt_sha256.clear();
    let ledger = invented_unsupported
        .operations
        .iter_mut()
        .find(|operation| operation.profile == ReadProfileId::XmlLedgerReadV1)
        .unwrap();
    ledger.outcome = OperationOutcome::Unsupported;
    ledger.application_status = ApplicationStatus::Failure;
    ledger.safe_reason_code = Some("tally_export_rejected".to_string());
    assert_eq!(
        invented_unsupported.seal().unwrap_err(),
        invalid("unsupported_operation_signature_unavailable")
    );

    let mut later_failure = receipt(&surface.digest().unwrap());
    later_failure.receipt_sha256.clear();
    let ledger = later_failure
        .operations
        .iter_mut()
        .find(|operation| operation.profile == ReadProfileId::XmlLedgerReadV1)
        .unwrap();
    ledger.outcome = OperationOutcome::Failed;
    ledger.application_status = ApplicationStatus::Failure;
    ledger.safe_reason_code = Some("tally_export_rejected".to_string());
    for profile in [
        ReadProfileId::XmlVoucherEmptyRangeV1,
        ReadProfileId::XmlVoucherPopulatedRangeV1,
    ] {
        let operation = later_failure
            .operations
            .iter_mut()
            .find(|operation| operation.profile == profile)
            .unwrap();
        *operation = not_attempted_operation(profile);
    }
    let later_failure = later_failure.seal().unwrap();
    let later_attestation = attestation(&later_failure, &surface, &signing);
    let ledger_manifest = unsupported_manifest(ReadProfileId::XmlLedgerReadV1);
    assert_eq!(
        enforce_support_gate(
            &ledger_manifest,
            &surface,
            &trust,
            std::slice::from_ref(&later_failure),
            std::slice::from_ref(&later_attestation),
            temp.path(),
            NOW,
        )
        .unwrap_err(),
        gate("unsupported_claim_signature_unavailable")
    );

    for (reason, application_status, transport_failure) in [
        (
            "ledger_fixture_or_context_invalid",
            ApplicationStatus::Success,
            false,
        ),
        (
            "ledger_response_malformed",
            ApplicationStatus::Unrecognized,
            false,
        ),
        (
            "transport_connection_reset",
            ApplicationStatus::NotApplicable,
            true,
        ),
    ] {
        let mut non_authoritative = receipt(&surface.digest().unwrap());
        non_authoritative.receipt_sha256.clear();
        let ledger = non_authoritative
            .operations
            .iter_mut()
            .find(|operation| operation.profile == ReadProfileId::XmlLedgerReadV1)
            .unwrap();
        ledger.outcome = OperationOutcome::Failed;
        ledger.application_status = application_status;
        ledger.safe_reason_code = Some(reason.to_string());
        if transport_failure {
            ledger.encoding = TextEncoding::Unknown;
            ledger.response_size = SizeBucket::Zero;
        }
        for profile in [
            ReadProfileId::XmlVoucherEmptyRangeV1,
            ReadProfileId::XmlVoucherPopulatedRangeV1,
        ] {
            let operation = non_authoritative
                .operations
                .iter_mut()
                .find(|operation| operation.profile == profile)
                .unwrap();
            *operation = not_attempted_operation(profile);
        }
        let non_authoritative = non_authoritative.seal().unwrap();
        let non_authoritative_attestation = attestation(&non_authoritative, &surface, &signing);
        assert_eq!(
            enforce_support_gate(
                &ledger_manifest,
                &surface,
                &trust,
                std::slice::from_ref(&non_authoritative),
                std::slice::from_ref(&non_authoritative_attestation),
                temp.path(),
                NOW,
            )
            .unwrap_err(),
            if transport_failure {
                gate("receipt_claim_scope_mismatch")
            } else {
                gate("unsupported_claim_signature_unavailable")
            },
            "non-authoritative failure was accepted: {reason}"
        );
    }

    let mut wrong_fixture = later_failure.clone();
    wrong_fixture.receipt_sha256.clear();
    wrong_fixture.fixture_manifest_sha256 = "c".repeat(64);
    let wrong_fixture = wrong_fixture.seal().unwrap();
    let wrong_attestation = attestation(&wrong_fixture, &surface, &signing);
    assert_eq!(
        enforce_support_gate(
            &ledger_manifest,
            &surface,
            &trust,
            std::slice::from_ref(&wrong_fixture),
            std::slice::from_ref(&wrong_attestation),
            temp.path(),
            NOW,
        )
        .unwrap_err(),
        gate("receipt_claim_scope_mismatch")
    );

    let mut missing_fixture = receipt(&surface.digest().unwrap());
    missing_fixture.receipt_sha256.clear();
    missing_fixture.fixture_marker_verified = false;
    for profile in [
        ReadProfileId::XmlSyntheticFixtureMarkerV1,
        ReadProfileId::XmlLedgerReadV1,
        ReadProfileId::XmlVoucherEmptyRangeV1,
        ReadProfileId::XmlVoucherPopulatedRangeV1,
    ] {
        let operation = missing_fixture
            .operations
            .iter_mut()
            .find(|operation| operation.profile == profile)
            .unwrap();
        *operation = not_attempted_operation(profile);
    }
    let missing_fixture = missing_fixture.seal().unwrap();
    let missing_attestation = attestation(&missing_fixture, &surface, &signing);
    assert_eq!(
        enforce_support_gate(
            &ledger_manifest,
            &surface,
            &trust,
            std::slice::from_ref(&missing_fixture),
            std::slice::from_ref(&missing_attestation),
            temp.path(),
            NOW,
        )
        .unwrap_err(),
        gate("fixture_marker_contract_not_verified")
    );

    let mut marker_failure = receipt(&surface.digest().unwrap());
    marker_failure.receipt_sha256.clear();
    marker_failure.fixture_marker_verified = false;
    let marker = marker_failure
        .operations
        .iter_mut()
        .find(|operation| operation.profile == ReadProfileId::XmlSyntheticFixtureMarkerV1)
        .unwrap();
    marker.outcome = OperationOutcome::Failed;
    marker.application_status = ApplicationStatus::Failure;
    marker.safe_reason_code = Some("synthetic_fixture_unverified".to_string());
    for profile in [
        ReadProfileId::XmlLedgerReadV1,
        ReadProfileId::XmlVoucherEmptyRangeV1,
        ReadProfileId::XmlVoucherPopulatedRangeV1,
    ] {
        let operation = marker_failure
            .operations
            .iter_mut()
            .find(|operation| operation.profile == profile)
            .unwrap();
        *operation = not_attempted_operation(profile);
    }
    let marker_failure = marker_failure.seal().unwrap();
    let marker_attestation = attestation(&marker_failure, &surface, &signing);
    let marker_manifest = unsupported_manifest(ReadProfileId::XmlSyntheticFixtureMarkerV1);
    assert_eq!(
        enforce_support_gate(
            &marker_manifest,
            &surface,
            &trust,
            std::slice::from_ref(&marker_failure),
            std::slice::from_ref(&marker_attestation),
            temp.path(),
            NOW,
        )
        .unwrap_err(),
        gate("fixture_marker_contract_not_verified")
    );
}

#[test]
fn surface_manifest_detects_compatibility_drift() {
    let temp = tempfile::tempdir().unwrap();
    create_required_surface_directories(temp.path());
    fs::write(temp.path().join("surface.txt"), b"before").unwrap();
    let surface = CompatibilitySurfaceManifest {
        schema_version: SURFACE_SCHEMA_VERSION,
        files: sealed_surface_files(temp.path(), &["surface.txt"]),
    };
    surface.validate_files(temp.path()).unwrap();
    fs::write(temp.path().join("surface.txt"), b"after").unwrap();
    assert_eq!(
        surface.validate_files(temp.path()).unwrap_err(),
        invalid("surface_file_changed")
    );
}

#[test]
fn surface_manifest_detects_required_constructor_drift() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("surface.txt"), b"surface").unwrap();
    let surface = sealed_surface(temp.path(), &["surface.txt"]);
    fs::write(
        temp.path().join("src-tauri/src/agent_desktop_journal.rs"),
        b"changed selected-read constructor",
    )
    .unwrap();

    assert_eq!(
        surface.validate_files(temp.path()).unwrap_err(),
        invalid("surface_file_changed")
    );
}

#[test]
fn gate_rejects_an_unpinned_migration_or_report_file() {
    for unpinned_path in [
        "src-tauri/src/db/migrations/9999_unpinned.sql",
        "src-tauri/src/reports/unpinned.rs",
    ] {
        let temp = tempfile::tempdir().unwrap();
        for directory in REQUIRED_SURFACE_DIRECTORIES {
            fs::create_dir_all(temp.path().join(directory)).unwrap();
        }
        fs::write(temp.path().join(unpinned_path), b"unsealed source").unwrap();
        fs::write(temp.path().join("surface.txt"), b"surface").unwrap();
        assert_eq!(
            sealed_surface(temp.path(), &["surface.txt"])
                .validate_files(temp.path())
                .unwrap_err(),
            invalid("surface_required_directory_file_unpinned")
        );
    }
}

#[cfg(windows)]
#[test]
fn normalise_surface_path_uses_forward_slashes() {
    assert_eq!(
        normalise_surface_path(Path::new("src-tauri\\src\\reports\\party_statement.rs")).unwrap(),
        "src-tauri/src/reports/party_statement.rs"
    );
}

#[cfg(unix)]
#[test]
fn gate_rejects_literal_backslash_filename_alongside_slash_path() {
    let temp = tempfile::tempdir().unwrap();
    let reports_directory = temp.path().join("src-tauri/src/reports");
    let slash_path = reports_directory.join("nested/file.rs");
    let literal_backslash_path = reports_directory.join(r"nested\file.rs");
    fs::create_dir_all(slash_path.parent().unwrap()).unwrap();
    fs::write(&slash_path, b"sealed source").unwrap();
    fs::write(&literal_backslash_path, b"unsealed source").unwrap();

    let slash_surface_path = "src-tauri/src/reports/nested/file.rs";
    let literal_backslash_surface_path = r"src-tauri/src/reports/nested\file.rs";
    assert_ne!(
        normalise_surface_path(Path::new(slash_surface_path)).unwrap(),
        normalise_surface_path(Path::new(literal_backslash_surface_path)).unwrap()
    );

    assert_eq!(
        sealed_surface(temp.path(), &[slash_surface_path])
            .validate_files(temp.path())
            .unwrap_err(),
        invalid("surface_required_directory_file_unpinned")
    );
}

#[cfg(unix)]
#[test]
fn gate_rejects_symlinked_required_directory_entries() {
    use std::os::unix::fs::symlink;

    for directory in REQUIRED_SURFACE_DIRECTORIES {
        let temp = tempfile::tempdir().unwrap();
        for required_directory in REQUIRED_SURFACE_DIRECTORIES {
            fs::create_dir_all(temp.path().join(required_directory)).unwrap();
        }
        symlink(
            temp.path().join("outside-the-repository"),
            temp.path().join(directory).join("linked-entry"),
        )
        .unwrap();
        fs::write(temp.path().join("surface.txt"), b"surface").unwrap();

        assert_eq!(
            sealed_surface(temp.path(), &["surface.txt"])
                .validate_files(temp.path())
                .unwrap_err(),
            invalid("surface_required_directory_entry_unsupported")
        );
    }
}

#[test]
fn gate_rejects_a_missing_required_directory() {
    for missing_directory in REQUIRED_SURFACE_DIRECTORIES {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("surface.txt"), b"surface").unwrap();
        let surface = sealed_surface(temp.path(), &["surface.txt"]);
        fs::remove_dir(temp.path().join(missing_directory)).unwrap();

        assert_eq!(
            surface.validate_files(temp.path()).unwrap_err(),
            invalid("surface_required_directory_unavailable")
        );
    }
}

#[test]
fn gate_rejects_a_required_path_that_is_not_a_directory() {
    for file_path in REQUIRED_SURFACE_DIRECTORIES {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("surface.txt"), b"surface").unwrap();
        let surface = sealed_surface(temp.path(), &["surface.txt"]);
        let path = temp.path().join(file_path);
        fs::remove_dir(&path).unwrap();
        fs::write(path, b"not a directory").unwrap();

        assert_eq!(
            surface.validate_files(temp.path()).unwrap_err(),
            invalid("surface_required_directory_not_directory")
        );
    }
}

#[test]
fn real_tree_has_complete_migration_and_report_surface_coverage() {
    let repository_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let surface = CompatibilitySurfaceManifest::from_json(
        &fs::read(repository_root.join("docs/tally/compatibility/compatibility-surface.json"))
            .unwrap(),
    )
    .unwrap();
    surface.validate_files(&repository_root).unwrap();
    // `<=` lets pinned files consume the deliberate reserve without raising
    // the cap: `validate_shape` protects the upper bound by rejecting a
    // surface above `MAX_SURFACE_FILES`, while this assertion protects the
    // policy bound by rejecting an inflated cap with excess headroom.
    assert!(MAX_SURFACE_FILES - surface.files.len() <= RESERVED_SURFACE_FILES);
}

#[test]
fn surface_file_cap_refuses_one_entry_above_the_cap() {
    let oversized = CompatibilitySurfaceManifest {
        schema_version: SURFACE_SCHEMA_VERSION,
        files: (0..MAX_SURFACE_FILES + 1)
            .map(|index| SurfaceFile {
                path: format!("pinned-{index:03}"),
                sha256: "0".repeat(64),
            })
            .collect(),
    };

    assert_eq!(
        oversized.validate().unwrap_err(),
        invalid("surface_file_count_invalid")
    );
}

#[test]
fn gate_rejects_an_unpinned_selected_read_constructor() {
    let temp = tempfile::tempdir().unwrap();
    create_required_surface_directories(temp.path());
    fs::create_dir_all(temp.path().join("src-tauri/src")).unwrap();
    fs::write(
        temp.path().join("src-tauri/src/agent_desktop_journal.rs"),
        b"selected-read constructor",
    )
    .unwrap();
    fs::write(temp.path().join("surface.txt"), b"surface").unwrap();

    assert_eq!(
        CompatibilitySurfaceManifest {
            schema_version: SURFACE_SCHEMA_VERSION,
            files: vec![SurfaceFile {
                path: "surface.txt".to_string(),
                sha256: sha256_file(&temp.path().join("surface.txt")).unwrap(),
            }],
        }
        .validate_files(temp.path())
        .unwrap_err(),
        invalid("surface_required_directory_file_unpinned")
    );
}

#[test]
fn gate_rejects_each_omitted_required_lifecycle_path() {
    for omitted_path in REQUIRED_SURFACE_FILES {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("surface.txt"), b"surface").unwrap();
        let mut surface = sealed_surface(temp.path(), &["surface.txt"]);
        surface.files.retain(|file| file.path != omitted_path);

        assert_eq!(
            surface.validate_files(temp.path()).unwrap_err(),
            invalid("surface_required_directory_file_unpinned"),
            "{omitted_path} must remain pinned"
        );
    }
}

fn sealed_surface(repository_root: &Path, paths: &[&str]) -> CompatibilitySurfaceManifest {
    create_required_surface_directories(repository_root);
    CompatibilitySurfaceManifest {
        schema_version: SURFACE_SCHEMA_VERSION,
        files: sealed_surface_files(repository_root, paths),
    }
}

fn sealed_surface_files(repository_root: &Path, paths: &[&str]) -> Vec<SurfaceFile> {
    let mut files = paths
        .iter()
        .copied()
        .chain(REQUIRED_SURFACE_FILES)
        .map(|path| SurfaceFile {
            path: path.to_string(),
            sha256: sha256_file(&repository_root.join(path)).unwrap(),
        })
        .collect::<Vec<_>>();
    files.sort_by(|left, right| left.path.cmp(&right.path));
    files
}

fn create_required_surface_directories(repository_root: &Path) {
    for directory in REQUIRED_SURFACE_DIRECTORIES {
        fs::create_dir_all(repository_root.join(directory)).unwrap();
    }
    for path in REQUIRED_SURFACE_FILES {
        let path = repository_root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"required selected-read constructor").unwrap();
    }
}

#[test]
fn rehash_of_an_unchanged_surface_reports_zero_changes() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("surface.txt"), b"unchanged").unwrap();
    let surface = sealed_surface(temp.path(), &["surface.txt"]);

    let (rehashed, changed) = surface.rehash_files(temp.path()).unwrap();

    assert_eq!(changed, 0);
    assert_eq!(rehashed, surface);
}

#[test]
fn rehash_detects_a_modified_pinned_file_without_sealing_it() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("surface.txt"), b"before").unwrap();
    let surface = sealed_surface(temp.path(), &["surface.txt"]);
    fs::write(temp.path().join("surface.txt"), b"after").unwrap();

    let (rehashed, changed) = surface.rehash_files(temp.path()).unwrap();

    assert_eq!(changed, 1);
    let rehashed_surface = rehashed
        .files
        .iter()
        .find(|file| file.path == "surface.txt")
        .unwrap();
    let original_surface = surface
        .files
        .iter()
        .find(|file| file.path == "surface.txt")
        .unwrap();
    assert_ne!(rehashed_surface.sha256, original_surface.sha256);
    // With no stored checksum, the rehash is the whole reseal: the result is
    // valid as it stands, and its digest follows the new bytes.
    rehashed.validate().unwrap();
    assert_ne!(rehashed.digest().unwrap(), surface.digest().unwrap());
}

#[test]
fn rehash_fails_closed_when_a_pinned_file_is_missing() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("surface.txt");
    fs::write(&path, b"present").unwrap();
    let surface = sealed_surface(temp.path(), &["surface.txt"]);
    fs::remove_file(path).unwrap();

    assert_eq!(
        surface.rehash_files(temp.path()).unwrap_err(),
        invalid("surface_file_unavailable")
    );
}

#[test]
fn rehash_preserves_surface_entry_count_and_order() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("alpha.txt"), b"alpha").unwrap();
    fs::write(temp.path().join("beta.txt"), b"beta").unwrap();
    let surface = sealed_surface(temp.path(), &["alpha.txt", "beta.txt"]);
    let paths = surface
        .files
        .iter()
        .map(|file| file.path.clone())
        .collect::<Vec<_>>();
    fs::write(temp.path().join("alpha.txt"), b"updated alpha").unwrap();
    fs::write(temp.path().join("beta.txt"), b"updated beta").unwrap();

    let (rehashed, changed) = surface.rehash_files(temp.path()).unwrap();

    assert_eq!(changed, 2);
    assert_eq!(rehashed.files.len(), surface.files.len());
    assert_eq!(
        rehashed
            .files
            .iter()
            .map(|file| file.path.clone())
            .collect::<Vec<_>>(),
        paths
    );
}

#[test]
fn rendered_claim_matrix_is_deterministic_and_drift_checked() {
    let manifest = SupportClaimsManifest {
        schema_version: SUPPORT_MANIFEST_SCHEMA_VERSION,
        bridge_commit_sha: COMMIT.to_string(),
        claims: vec![SupportClaim {
            claim_id: "prime-7-1-windows-education-xml-one-company".to_string(),
            level: ClaimLevel::Unknown,
            promotion_eligible: true,
            product: ProductFamily::TallyPrime,
            release: "7.1".to_string(),
            mode: TallyMode::Education,
            platform: Platform::Windows,
            architecture: Architecture::X86_64,
            transport: TransportProfile::XmlHttp,
            endpoint_family: LoopbackFamily::Ipv4,
            odbc_state: OdbcState::Disabled,
            company_state: CompanyLoadState::One,
            locale: LocaleProfile::EnglishIndia,
            encoding: TextEncoding::Utf8,
            dataset_tier: DatasetTier::SyntheticSmall,
            fixture_manifest_sha256: None,
            required_profiles: Vec::new(),
            max_evidence_age_days: 180,
            evidence_id: None,
        }],
    };
    let rendered = render_claim_matrix(&manifest).unwrap();
    assert!(rendered.contains("`unknown` | `true` | `missing`"));
    let document = format!("# Matrix\n\n{rendered}\n");
    verify_claim_matrix_markdown(&manifest, document.as_bytes()).unwrap();
    assert_eq!(
        verify_claim_matrix_markdown(&manifest, b"# Matrix\n").unwrap_err(),
        invalid("matrix_markdown_drift")
    );

    let mut unsupported = manifest.clone();
    unsupported.claims[0].level = ClaimLevel::Unsupported;
    unsupported.claims[0].promotion_eligible = false;
    unsupported.claims[0].fixture_manifest_sha256 = Some(SHA.to_string());
    unsupported.claims[0].required_profiles = vec![ReadProfileId::XmlCompanyEnumerationV1];
    unsupported.claims[0].evidence_id = Some("observed-failure-1".to_string());
    assert!(unsupported.validate().is_ok());

    let mut non_promotable_positive = unsupported.clone();
    non_promotable_positive.claims[0].level = ClaimLevel::Observed;
    non_promotable_positive.claims[0].mode = TallyMode::Licensed;
    assert_eq!(
        non_promotable_positive.validate().unwrap_err(),
        invalid("positive_claim_not_promotion_eligible")
    );

    let mut mixed_transport = manifest;
    mixed_transport.claims[0].transport = TransportProfile::JsonExShadow;
    mixed_transport.claims[0].promotion_eligible = false;
    mixed_transport.claims[0].required_profiles = vec![ReadProfileId::XmlCompanyEnumerationV1];
    assert_eq!(
        mixed_transport.validate().unwrap_err(),
        invalid("jsonex_claim_contains_xml_profile")
    );
}

/// The committed schema-1 surface and matrix, byte for byte as master held
/// them before bridge#760 (3d2a4b05).
const SURFACE_SCHEMA_1: &str = include_str!("../tests/fixtures/compatibility-surface-schema1.json");
const MATRIX_SCHEMA_1: &str = include_str!("../tests/fixtures/compatibility-matrix-schema1.json");

#[test]
fn the_computed_digest_is_the_checksum_schema_1_stored() {
    // Receipts and attestations bind a surface digest. Schema 1 stored it as
    // `manifest_sha256`; the computed digest over the same pins must equal
    // it, or every earlier binding would silently change meaning.
    let stored: serde_json::Value = serde_json::from_str(SURFACE_SCHEMA_1).unwrap();
    assert_eq!(stored["schema_version"], 1);
    let surface = CompatibilitySurfaceManifest {
        schema_version: SURFACE_SCHEMA_VERSION,
        files: serde_json::from_value(stored["files"].clone()).unwrap(),
    };
    assert_eq!(
        surface.digest().unwrap(),
        stored["manifest_sha256"].as_str().unwrap()
    );
    let matrix: serde_json::Value = serde_json::from_str(MATRIX_SCHEMA_1).unwrap();
    assert_eq!(
        matrix["compatibility_surface_sha256"],
        stored["manifest_sha256"]
    );
}

#[test]
fn a_schema_1_surface_or_matrix_is_refused() {
    assert_eq!(
        CompatibilitySurfaceManifest::from_json(SURFACE_SCHEMA_1.as_bytes()).unwrap_err(),
        invalid("artifact_json_invalid")
    );
    let mut without_checksum: serde_json::Value = serde_json::from_str(SURFACE_SCHEMA_1).unwrap();
    without_checksum
        .as_object_mut()
        .unwrap()
        .remove("manifest_sha256");
    assert_eq!(
        CompatibilitySurfaceManifest::from_json(&serde_json::to_vec(&without_checksum).unwrap())
            .unwrap_err(),
        invalid("surface_schema_unsupported")
    );
    assert_eq!(
        SupportClaimsManifest::from_json(MATRIX_SCHEMA_1.as_bytes()).unwrap_err(),
        invalid("artifact_json_invalid")
    );
    let mut matrix_without_digest: serde_json::Value =
        serde_json::from_str(MATRIX_SCHEMA_1).unwrap();
    matrix_without_digest
        .as_object_mut()
        .unwrap()
        .remove("compatibility_surface_sha256");
    assert_eq!(
        SupportClaimsManifest::from_json(&serde_json::to_vec(&matrix_without_digest).unwrap())
            .unwrap_err(),
        invalid("support_manifest_invalid")
    );
}
