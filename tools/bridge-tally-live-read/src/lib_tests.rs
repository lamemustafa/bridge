use super::*;
use tally_protocol_simulator::{Fixture, ScenarioPlan, Simulator, WireEncoding};

const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn config(port: u16) -> LiveRunConfig {
    LiveRunConfig {
        schema_version: CONFIG_SCHEMA_VERSION,
        repository_root: PathBuf::from("."),
        fixture_manifest: PathBuf::from("fixture.json"),
        endpoint_family: LoopbackFamily::Ipv4,
        port,
        product: ProductFamily::TallyPrime,
        release: "7.1".to_string(),
        mode: TallyMode::Education,
        odbc_state: OdbcState::Disabled,
        locale: LocaleProfile::EnglishIndia,
        no_customer_data_attested: true,
    }
}

fn fixture() -> SyntheticFixtureManifest {
    SyntheticFixtureManifest {
        schema_version: FIXTURE_SCHEMA_VERSION,
        fixture_id: "education-small-v1".to_string(),
        dataset_tier: DatasetTier::SyntheticSmall,
        company_marker: "BRIDGE-PR14-SYNTHETIC-019f605f-e6cf-77b2-ac95-31722887a911".to_string(),
        ledger_sentinel: "BRIDGE-LEDGER-019f605f-e6cf-77b2-ac95-31722887a911".to_string(),
        voucher_number_sentinel: "BRIDGE-VOUCHER-019f605f-e6cf-77b2-ac95-31722887a911".to_string(),
        empty_voucher_range: DateWindow {
            from_yyyymmdd: "20260403".to_string(),
            to_yyyymmdd: "20260403".to_string(),
        },
        populated_voucher_range: DateWindow {
            from_yyyymmdd: "20260401".to_string(),
            to_yyyymmdd: "20260402".to_string(),
        },
        minimum_ledger_count: 1,
        maximum_ledger_count: 100,
        minimum_populated_voucher_count: 1,
        maximum_populated_voucher_count: 20,
    }
}

fn metadata() -> RunMetadata {
    RunMetadata {
        observed_at_unix_ms: 1_800_000_000_000,
        bridge_commit_sha: "b".repeat(40),
        working_tree_dirty: true,
        compatibility_surface_sha256: SHA.to_string(),
        executable_sha256: SHA.to_string(),
        cargo_lock_sha256: SHA.to_string(),
        fixture_manifest_sha256: SHA.to_string(),
    }
}

fn receipt() -> LiveCompatibilityReceipt {
    let company = ValidatedCompanyName::new(fixture().company_marker).unwrap();
    let range = ValidatedDateRange::new("20260403", "20260403").unwrap();
    let profile = ReadOnlyProfile::CompanyListV1;
    let mut operations = vec![not_attempted(
        ReadProfileId::XmlCompanyEnumerationV1,
        profile.template_sha256(),
        "endpoint_not_queried",
    )];
    operations.push(not_attempted(
        ReadProfileId::XmlSyntheticFixtureMarkerV1,
        profile.template_sha256(),
        "endpoint_not_queried",
    ));
    append_company_reads_not_attempted(&mut operations, &company, &range);
    seal_receipt(
        &config(9001),
        &fixture(),
        &metadata(),
        operations,
        false,
        false,
        CountBucket::Unknown,
    )
    .unwrap()
}

fn inputs(port: u16, binding: &str, expires_at_unix_ms: i64) -> LiveRunInputs {
    LiveRunInputs {
        config: config(port),
        fixture: fixture(),
        metadata: metadata(),
        repository_root: PathBuf::from("."),
        challenge_phrase: "QUALIFY education-small-v1 test".to_string(),
        consent_binding: binding.to_string(),
        consent_expires_at_unix_ms: expires_at_unix_ms,
    }
}

#[test]
fn fixture_requires_reviewed_sentinels_bounded_counts_and_disjoint_ranges() {
    assert!(validate_fixture(&fixture()).is_ok());
    let mut invalid = fixture();
    invalid.company_marker = "synthetic".to_string();
    assert_eq!(validate_fixture(&invalid), Err(error("fixture_invalid")));
    let mut overlap = fixture();
    overlap.empty_voucher_range = overlap.populated_voucher_range.clone();
    assert_eq!(
        validate_fixture(&overlap),
        Err(error("fixture_ranges_overlap"))
    );
    let mut invalid_bounds = fixture();
    invalid_bounds.maximum_ledger_count = 0;
    assert_eq!(
        validate_fixture(&invalid_bounds),
        Err(error("fixture_invalid"))
    );
}

#[test]
fn dataset_tier_comes_from_the_reviewed_fixture_not_the_local_profile() {
    let mut fixture = fixture();
    fixture.dataset_tier = DatasetTier::SyntheticLarge;
    assert!(validate_fixture(&fixture).is_ok());
    assert_eq!(fixture.dataset_tier, DatasetTier::SyntheticLarge);
}

#[test]
fn unknown_profile_fields_are_rejected_before_network_and_cannot_be_laundered() {
    let mut config = config(9001);
    config.product = ProductFamily::Unknown;
    config.release = "unknown".to_string();
    config.mode = TallyMode::Unknown;
    config.odbc_state = OdbcState::Unknown;
    config.locale = LocaleProfile::Unknown;
    assert_eq!(validate_config(&config), Err(error("config_invalid")));
    let company = ValidatedCompanyName::new(fixture().company_marker).unwrap();
    let range = ValidatedDateRange::new("20260403", "20260403").unwrap();
    let profile = ReadOnlyProfile::CompanyListV1;
    let mut operations = vec![not_attempted(
        ReadProfileId::XmlCompanyEnumerationV1,
        profile.template_sha256(),
        "endpoint_not_queried",
    )];
    operations.push(not_attempted(
        ReadProfileId::XmlSyntheticFixtureMarkerV1,
        profile.template_sha256(),
        "endpoint_not_queried",
    ));
    append_company_reads_not_attempted(&mut operations, &company, &range);
    let receipt = seal_receipt(
        &config,
        &fixture(),
        &metadata(),
        operations,
        false,
        false,
        CountBucket::Unknown,
    )
    .unwrap();
    assert_eq!(receipt.product.authority, EvidenceAuthority::Unknown);
    assert_eq!(receipt.release.confidence, EvidenceConfidence::Unknown);
    assert_eq!(receipt.mode.authority, EvidenceAuthority::Unknown);
}

#[tokio::test]
async fn empty_company_response_stops_after_one_request_and_retains_no_marker() {
    let mut last_failure = "simulator_not_started";
    for _ in 0..5 {
        let simulator = Simulator::spawn(
            ScenarioPlan::new(Fixture::EmptyExport).with_encoding(WireEncoding::Utf16Le),
        )
        .unwrap();
        let config = config(simulator.address().port());
        let transport =
            ReadOnlyTransport::new(ReadLoopback::Ipv4, simulator.address().port()).unwrap();
        let receipt = execute_with_transport(&config, &fixture(), &metadata(), &transport)
            .await
            .unwrap();
        let observed = match simulator.finish() {
            Ok(observed) if observed.method == "POST" && observed.request_processed => observed,
            Ok(_) => {
                last_failure = "simulator_request_not_processed";
                continue;
            }
            Err(_) => {
                last_failure = "simulator_request_unavailable";
                continue;
            }
        };
        assert_eq!(receipt.operations.len(), 5);
        assert_eq!(receipt.operations[0].safe_reason_code.as_deref(), None);
        assert_eq!(receipt.operations[0].outcome, OperationOutcome::Passed);
        assert_eq!(receipt.operations[1].outcome, OperationOutcome::Failed);
        assert!(receipt.operations[2..]
            .iter()
            .all(|operation| operation.outcome == OperationOutcome::NotAttempted));
        assert!(!receipt.fixture_marker_verified);
        assert_eq!(observed.path, "/");
        let text = String::from_utf8(receipt.to_pretty_json().unwrap()).unwrap();
        assert!(!text.contains(&fixture().company_marker));
        return;
    }
    panic!("loopback simulator remained unstable: {last_failure}");
}

#[test]
fn false_customer_attestation_rejects_before_any_post() {
    let simulator = Simulator::spawn(ScenarioPlan::new(Fixture::EmptyExport)).unwrap();
    let mut value = config(simulator.address().port());
    value.no_customer_data_attested = false;
    assert_eq!(validate_config(&value), Err(error("config_invalid")));
    simulator.cancel();
    let observed = simulator.finish().unwrap();
    assert!(!observed.request_processed);
    assert!(observed.cancelled);
}

#[test]
fn consent_is_expiring_and_bound_to_one_loaded_run() {
    let future = now_unix_ms().unwrap() + NETWORK_CONSENT_TTL_MS;
    let run_a = inputs(9000, "a", future);
    let run_b = inputs(9001, "b", future);
    let token_a = confirm_network_challenge(&run_a, "QUALIFY education-small-v1 test").unwrap();
    assert_eq!(
        verify_network_consent(&run_b, &token_a),
        Err(error("network_consent_binding_mismatch"))
    );
    assert_eq!(
        confirm_network_challenge(&run_a, "wrong").err().unwrap(),
        error("network_consent_mismatch")
    );

    let expired = inputs(9000, "expired", 1);
    assert_eq!(
        confirm_network_challenge(&expired, expired.challenge_phrase())
            .err()
            .unwrap(),
        error("network_consent_expired")
    );
}

#[test]
fn live_receipt_paths_are_local_json_and_path_bound() {
    let directory = std::env::temp_dir().join(format!(
        "bridge-live-read-path-test-{}-{}",
        std::process::id(),
        now_unix_ms().unwrap()
    ));
    let local = directory.join(".bridge-live");
    fs::create_dir_all(&local).unwrap();
    let first = local.join("first.json");
    let second = local.join("second.json");
    assert_ne!(
        live_receipt_output_binding(&first).unwrap(),
        live_receipt_output_binding(&second).unwrap()
    );
    assert_eq!(
        live_receipt_output_binding(&directory.join("outside.json")),
        Err(error("receipt_output_outside_local_evidence_root"))
    );
    assert_eq!(
        live_receipt_output_binding(&local.join("receipt.txt")),
        Err(error("receipt_output_invalid"))
    );
    fs::remove_dir(local).unwrap();
    fs::remove_dir(directory).unwrap();
}

#[test]
fn save_requires_exact_receipt_bound_confirmation_and_never_overwrites() {
    let directory = std::env::temp_dir().join(format!(
        "bridge-live-read-save-test-{}-{}",
        std::process::id(),
        now_unix_ms().unwrap()
    ));
    fs::create_dir(&directory).unwrap();
    let output = directory.join("receipt.json");
    assert_eq!(
        save_receipt_no_replace(&output, b"{}", "wrong", "SAVE abc"),
        Err(error("save_consent_mismatch"))
    );
    save_receipt_no_replace(&output, b"{}", "SAVE abc", "SAVE abc").unwrap();
    assert_eq!(fs::read(&output).unwrap(), b"{}");
    assert_eq!(
        save_receipt_no_replace(&output, b"new", "SAVE abc", "SAVE abc"),
        Err(error("receipt_output_exists"))
    );
    fs::remove_file(output).unwrap();
    fs::remove_dir(directory).unwrap();
}

#[test]
fn public_save_consumes_a_repository_bound_target_and_rechecks_no_overwrite() {
    let directory = std::env::temp_dir().join(format!(
        "bridge-live-read-public-save-test-{}-{}",
        std::process::id(),
        now_unix_ms().unwrap()
    ));
    let local = directory.join(".bridge-live");
    fs::create_dir_all(&local).unwrap();
    let output = local.join("receipt.json");
    let mut run = inputs(
        9001,
        "binding",
        now_unix_ms().unwrap() + NETWORK_CONSENT_TTL_MS,
    );
    run.repository_root = directory.clone();
    let target = run.validate_receipt_output(&output).unwrap();
    let overwrite_attempt = run.validate_receipt_output(&output).unwrap();
    let receipt = receipt();
    let bytes = receipt.to_pretty_json().unwrap();
    let phrase = receipt_save_phrase(&receipt, &target).unwrap();
    save_live_receipt_no_replace(target, &bytes, &phrase).unwrap();
    assert_eq!(
        save_live_receipt_no_replace(overwrite_attempt, &bytes, &phrase),
        Err(error("receipt_output_exists"))
    );
    fs::remove_file(output).unwrap();
    fs::remove_dir(local).unwrap();
    fs::remove_dir(directory).unwrap();
}
