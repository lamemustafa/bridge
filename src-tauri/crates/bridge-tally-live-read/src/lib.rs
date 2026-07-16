use std::{
    fs,
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};

use bridge_tally_compatibility::{
    now_unix_ms, sha256_file, ApplicationStatus, Architecture, CompatibilitySurfaceManifest,
    CountBucket, DatasetTier, EvidenceAuthority, EvidenceConfidence, LiveCompatibilityReceipt,
    LiveReadAuthority, LocaleProfile, LoopbackFamily, OdbcState, OperationEvidence,
    OperationOutcome, Platform, ProductFamily, ProfileValue, ReadProfileId, SizeBucket, TallyMode,
    TextEncoding, TransportProfile, LIVE_RECEIPT_SCHEMA_VERSION, MAX_ARTIFACT_BYTES,
};
use bridge_tally_protocol::{
    export_failure_reason_code, export_status, parse_companies_with_evidence,
    parse_ledger_source_records_with_evidence, parse_voucher_source_records_with_evidence,
    verify_company_context,
    xml_read_profiles::{ReadOnlyProfile, ValidatedCompanyName, ValidatedDateRange},
    ExportEvidence, TallyExportStatus, TallyTextEncoding,
};
use bridge_tally_read_transport::{
    ReadLoopback, ReadOnlyResponse, ReadOnlyTransport, ReadOnlyTransportError,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

#[cfg(feature = "bills-native-outstandings-probe-runner")]
pub mod native_outstandings_qualification;

const CONFIG_SCHEMA_VERSION: u16 = 1;
const FIXTURE_SCHEMA_VERSION: u16 = 1;
const MAX_LOCAL_INPUT_BYTES: usize = 64 * 1024;
const NETWORK_CONSENT_TTL_MS: i64 = 5 * 60 * 1000;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error("live-read qualification failed ({code})")]
pub struct LiveReadError {
    code: &'static str,
}

impl LiveReadError {
    pub fn safe_code(&self) -> &'static str {
        self.code
    }
}

fn error(code: &'static str) -> LiveReadError {
    LiveReadError { code }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct LiveRunConfig {
    schema_version: u16,
    repository_root: PathBuf,
    fixture_manifest: PathBuf,
    endpoint_family: LoopbackFamily,
    port: u16,
    product: ProductFamily,
    release: String,
    mode: TallyMode,
    odbc_state: OdbcState,
    locale: LocaleProfile,
    no_customer_data_attested: bool,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct DateWindow {
    from_yyyymmdd: String,
    to_yyyymmdd: String,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct SyntheticFixtureManifest {
    schema_version: u16,
    fixture_id: String,
    dataset_tier: DatasetTier,
    company_marker: String,
    ledger_sentinel: String,
    voucher_number_sentinel: String,
    empty_voucher_range: DateWindow,
    populated_voucher_range: DateWindow,
    minimum_ledger_count: u64,
    maximum_ledger_count: u64,
    minimum_populated_voucher_count: u64,
    maximum_populated_voucher_count: u64,
}

struct RunMetadata {
    observed_at_unix_ms: i64,
    bridge_commit_sha: String,
    working_tree_dirty: bool,
    compatibility_surface_sha256: String,
    executable_sha256: String,
    cargo_lock_sha256: String,
    fixture_manifest_sha256: String,
}

pub struct LiveRunInputs {
    config: LiveRunConfig,
    fixture: SyntheticFixtureManifest,
    metadata: RunMetadata,
    repository_root: PathBuf,
    challenge_phrase: String,
    consent_binding: String,
    consent_expires_at_unix_ms: i64,
}

pub struct NetworkConsent {
    binding: String,
    expires_at_unix_ms: i64,
}

pub struct LiveReceiptOutput {
    output_path: PathBuf,
    canonical_parent: PathBuf,
}

impl LiveRunInputs {
    pub fn load(config_path: &Path) -> Result<Self, LiveReadError> {
        let config_bytes = read_bounded(config_path, MAX_LOCAL_INPUT_BYTES, "config_unavailable")?;
        let config: LiveRunConfig =
            serde_json::from_slice(&config_bytes).map_err(|_| error("config_invalid"))?;
        validate_config(&config)?;

        let base = config_path.parent().unwrap_or_else(|| Path::new("."));
        let repository_root =
            canonical_join(base, &config.repository_root, "repository_unavailable")?;
        let reviewed_local_root = repository_root
            .join(".bridge-live")
            .canonicalize()
            .map_err(|_| error("local_evidence_root_unavailable"))?;
        let canonical_config = config_path
            .canonicalize()
            .map_err(|_| error("config_unavailable"))?;
        if !canonical_config.starts_with(&reviewed_local_root) {
            return Err(error("config_path_outside_local_evidence_root"));
        }
        let fixture_path = canonical_join(base, &config.fixture_manifest, "fixture_unavailable")?;
        let allowed_fixture_root = repository_root
            .join("docs/tally/compatibility/fixtures")
            .canonicalize()
            .map_err(|_| error("fixture_root_unavailable"))?;
        if !fixture_path.starts_with(&allowed_fixture_root) {
            return Err(error("fixture_path_outside_reviewed_root"));
        }
        let fixture_bytes =
            read_bounded(&fixture_path, MAX_LOCAL_INPUT_BYTES, "fixture_unavailable")?;
        let fixture: SyntheticFixtureManifest =
            serde_json::from_slice(&fixture_bytes).map_err(|_| error("fixture_invalid"))?;
        validate_fixture(&fixture)?;

        let surface_path =
            repository_root.join("docs/tally/compatibility/compatibility-surface.json");
        let surface = CompatibilitySurfaceManifest::from_json(&read_bounded(
            &surface_path,
            MAX_ARTIFACT_BYTES,
            "surface_unavailable",
        )?)
        .map_err(|_| error("surface_invalid"))?;
        surface
            .validate_files(&repository_root)
            .map_err(|_| error("surface_changed"))?;

        let observed_at_unix_ms = now_unix_ms().map_err(|_| error("system_clock_invalid"))?;
        let bridge_commit_sha = git_output(&repository_root, &["rev-parse", "HEAD"])?;
        if !valid_commit(&bridge_commit_sha) {
            return Err(error("bridge_commit_invalid"));
        }
        let working_tree_dirty = !git_output(
            &repository_root,
            &["status", "--porcelain", "--untracked-files=all"],
        )?
        .is_empty();
        let executable = std::env::current_exe().map_err(|_| error("executable_unavailable"))?;
        let metadata = RunMetadata {
            observed_at_unix_ms,
            bridge_commit_sha,
            working_tree_dirty,
            compatibility_surface_sha256: surface.manifest_sha256,
            executable_sha256: sha256_file(&executable)
                .map_err(|_| error("executable_unavailable"))?,
            cargo_lock_sha256: sha256_file(&repository_root.join("src-tauri/Cargo.lock"))
                .map_err(|_| error("cargo_lock_unavailable"))?,
            fixture_manifest_sha256: sha256_hex(&fixture_bytes),
        };
        let consent_expires_at_unix_ms = observed_at_unix_ms + NETWORK_CONSENT_TTL_MS;
        let consent_binding = network_consent_binding(
            &config,
            &fixture.fixture_id,
            &metadata,
            consent_expires_at_unix_ms,
        )?;
        let challenge_phrase = format!("QUALIFY {} {}", fixture.fixture_id, &consent_binding[..16]);
        Ok(Self {
            config,
            fixture,
            metadata,
            repository_root,
            challenge_phrase,
            consent_binding,
            consent_expires_at_unix_ms,
        })
    }

    pub fn challenge_phrase(&self) -> &str {
        &self.challenge_phrase
    }

    pub fn repository_root(&self) -> &Path {
        &self.repository_root
    }

    pub fn no_customer_data_attested(&self) -> bool {
        self.config.no_customer_data_attested
    }

    pub fn validate_receipt_output(
        &self,
        output_path: &Path,
    ) -> Result<LiveReceiptOutput, LiveReadError> {
        if output_path.exists() {
            return Err(error("receipt_output_exists"));
        }
        let parent = output_path.parent().unwrap_or_else(|| Path::new("."));
        let canonical_parent = parent
            .canonicalize()
            .map_err(|_| error("receipt_output_parent_unavailable"))?;
        let reviewed_local_root = self
            .repository_root
            .join(".bridge-live")
            .canonicalize()
            .map_err(|_| error("local_evidence_root_unavailable"))?;
        if canonical_parent != reviewed_local_root
            || output_path.extension().and_then(|value| value.to_str()) != Some("json")
        {
            return Err(error("receipt_output_outside_local_evidence_root"));
        }
        output_path
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| error("receipt_output_invalid"))?;
        Ok(LiveReceiptOutput {
            output_path: output_path.to_path_buf(),
            canonical_parent,
        })
    }

    pub async fn execute(
        self,
        consent: NetworkConsent,
    ) -> Result<LiveCompatibilityReceipt, LiveReadError> {
        verify_network_consent(&self, &consent)?;
        validate_current_surface(
            &self.repository_root,
            &self.metadata.compatibility_surface_sha256,
        )?;
        let transport =
            ReadOnlyTransport::new(read_loopback(self.config.endpoint_family), self.config.port)
                .map_err(|_| error("endpoint_configuration_invalid"))?;
        execute_with_transport(&self.config, &self.fixture, &self.metadata, &transport).await
    }
}

pub fn confirm_network_challenge(
    inputs: &LiveRunInputs,
    typed: &str,
) -> Result<NetworkConsent, LiveReadError> {
    if typed.trim_end_matches(['\r', '\n']) != inputs.challenge_phrase {
        return Err(error("network_consent_mismatch"));
    }
    ensure_not_expired(inputs.consent_expires_at_unix_ms)?;
    Ok(NetworkConsent {
        binding: inputs.consent_binding.clone(),
        expires_at_unix_ms: inputs.consent_expires_at_unix_ms,
    })
}

pub fn receipt_save_phrase(
    receipt: &LiveCompatibilityReceipt,
    output: &LiveReceiptOutput,
) -> Result<String, LiveReadError> {
    output.revalidate()?;
    let output_binding = live_receipt_output_binding(&output.output_path)?;
    Ok(format!(
        "SAVE {} {}",
        &receipt.receipt_sha256[..12],
        &output_binding[..12]
    ))
}

pub fn save_live_receipt_no_replace(
    output: LiveReceiptOutput,
    receipt_bytes: &[u8],
    typed: &str,
) -> Result<(), LiveReadError> {
    output.revalidate()?;
    let receipt =
        LiveCompatibilityReceipt::from_json(receipt_bytes).map_err(|_| error("receipt_invalid"))?;
    let expected_phrase = receipt_save_phrase(&receipt, &output)?;
    save_receipt_no_replace(&output.output_path, receipt_bytes, typed, &expected_phrase)
}

impl LiveReceiptOutput {
    fn revalidate(&self) -> Result<(), LiveReadError> {
        if self.output_path.exists() {
            return Err(error("receipt_output_exists"));
        }
        let parent = self.output_path.parent().unwrap_or_else(|| Path::new("."));
        let current_parent = parent
            .canonicalize()
            .map_err(|_| error("receipt_output_parent_unavailable"))?;
        if current_parent != self.canonical_parent
            || self
                .output_path
                .extension()
                .and_then(|value| value.to_str())
                != Some("json")
        {
            return Err(error("receipt_output_changed_after_validation"));
        }
        Ok(())
    }
}

pub(crate) fn save_receipt_no_replace(
    output_path: &Path,
    receipt_bytes: &[u8],
    typed: &str,
    expected_phrase: &str,
) -> Result<(), LiveReadError> {
    if typed.trim_end_matches(['\r', '\n']) != expected_phrase {
        return Err(error("save_consent_mismatch"));
    }
    if receipt_bytes.is_empty() || receipt_bytes.len() > MAX_ARTIFACT_BYTES {
        return Err(error("receipt_size_invalid"));
    }
    if output_path.exists() {
        return Err(error("receipt_output_exists"));
    }
    let parent = output_path.parent().unwrap_or_else(|| Path::new("."));
    if !parent.is_dir() {
        return Err(error("receipt_output_parent_unavailable"));
    }
    let file_name = output_path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| error("receipt_output_invalid"))?;
    let temporary = parent.join(format!(
        ".{file_name}.{}.{}.tmp",
        std::process::id(),
        now_unix_ms().map_err(|_| error("system_clock_invalid"))?
    ));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|_| error("receipt_temporary_create_failed"))?;
        file.write_all(receipt_bytes)
            .and_then(|_| file.sync_all())
            .map_err(|_| error("receipt_temporary_write_failed"))?;
        fs::hard_link(&temporary, output_path).map_err(|_| error("receipt_save_failed"))?;
        Ok(())
    })();
    let _ = fs::remove_file(&temporary);
    result
}

fn live_receipt_output_binding(output_path: &Path) -> Result<String, LiveReadError> {
    if output_path.extension().and_then(|value| value.to_str()) != Some("json") {
        return Err(error("receipt_output_invalid"));
    }
    let parent = output_path.parent().unwrap_or_else(|| Path::new("."));
    let canonical_parent = parent
        .canonicalize()
        .map_err(|_| error("receipt_output_parent_unavailable"))?;
    if canonical_parent
        .file_name()
        .and_then(|value| value.to_str())
        != Some(".bridge-live")
    {
        return Err(error("receipt_output_outside_local_evidence_root"));
    }
    let file_name = output_path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| error("receipt_output_invalid"))?;
    let mut digest = Sha256::new();
    digest.update(b"bridge.tally.live-read-receipt-output/1\0");
    let parent_text = canonical_parent.to_string_lossy();
    for field in [parent_text.as_bytes(), file_name.as_bytes()] {
        digest.update((field.len() as u64).to_be_bytes());
        digest.update(field);
    }
    Ok(sha256_digest_hex(digest.finalize()))
}

async fn execute_with_transport(
    config: &LiveRunConfig,
    fixture: &SyntheticFixtureManifest,
    metadata: &RunMetadata,
    transport: &ReadOnlyTransport,
) -> Result<LiveCompatibilityReceipt, LiveReadError> {
    let company_name = ValidatedCompanyName::new(fixture.company_marker.clone())
        .map_err(|_| error("fixture_company_marker_invalid"))?;
    let empty_range = ValidatedDateRange::new(
        fixture.empty_voucher_range.from_yyyymmdd.clone(),
        fixture.empty_voucher_range.to_yyyymmdd.clone(),
    )
    .map_err(|_| error("fixture_empty_range_invalid"))?;
    let populated_range = ValidatedDateRange::new(
        fixture.populated_voucher_range.from_yyyymmdd.clone(),
        fixture.populated_voucher_range.to_yyyymmdd.clone(),
    )
    .map_err(|_| error("fixture_populated_range_invalid"))?;

    let mut operations = Vec::with_capacity(5);
    let mut endpoint_response_observed = false;
    let mut loaded_company_count = CountBucket::Unknown;
    let company_profile = ReadOnlyProfile::CompanyListV1;
    let company_response = match transport.send(company_profile).await {
        Ok(response) => {
            endpoint_response_observed = true;
            response
        }
        Err(transport_error) => {
            operations.push(transport_failure(
                ReadProfileId::XmlCompanyEnumerationV1,
                company_profile.template_sha256(),
                &transport_error,
            ));
            operations.push(not_attempted(
                ReadProfileId::XmlSyntheticFixtureMarkerV1,
                company_profile.template_sha256(),
                "company_enumeration_not_passed",
            ));
            append_company_reads_not_attempted(&mut operations, &company_name, &empty_range);
            return seal_receipt(
                config,
                fixture,
                metadata,
                operations,
                false,
                endpoint_response_observed,
                loaded_company_count,
            );
        }
    };

    let companies = match parse_companies_with_evidence(company_response.text()) {
        Ok(parsed) => {
            loaded_company_count = count_bucket(parsed.records.len());
            operations.push(passed_response(
                ReadProfileId::XmlCompanyEnumerationV1,
                company_profile.template_sha256(),
                &company_response,
                parsed.records.len(),
            ));
            parsed.records
        }
        Err(_) => {
            operations.push(response_failure(
                ReadProfileId::XmlCompanyEnumerationV1,
                company_profile.template_sha256(),
                &company_response,
                application_status(company_response.text()),
                "company_response_invalid",
            ));
            operations.push(not_attempted(
                ReadProfileId::XmlSyntheticFixtureMarkerV1,
                company_profile.template_sha256(),
                "company_enumeration_not_passed",
            ));
            append_company_reads_not_attempted(&mut operations, &company_name, &empty_range);
            return seal_receipt(
                config,
                fixture,
                metadata,
                operations,
                false,
                endpoint_response_observed,
                loaded_company_count,
            );
        }
    };

    let matching: Vec<_> = companies
        .iter()
        .filter(|company| company.name == fixture.company_marker)
        .collect();
    let selected_guid = if matching.len() == 1 {
        matching[0]
            .guid
            .as_deref()
            .filter(|guid| !guid.trim().is_empty())
            .filter(|guid| {
                companies
                    .iter()
                    .filter(|company| company.guid.as_deref() == Some(*guid))
                    .count()
                    == 1
            })
    } else {
        None
    };
    let Some(selected_guid) = selected_guid else {
        operations.push(derived_failure(
            ReadProfileId::XmlSyntheticFixtureMarkerV1,
            company_profile.template_sha256(),
            &company_response,
            "synthetic_fixture_unverified",
        ));
        append_company_reads_not_attempted(&mut operations, &company_name, &empty_range);
        return seal_receipt(
            config,
            fixture,
            metadata,
            operations,
            false,
            endpoint_response_observed,
            loaded_company_count,
        );
    };
    operations.push(passed_response(
        ReadProfileId::XmlSyntheticFixtureMarkerV1,
        company_profile.template_sha256(),
        &company_response,
        1,
    ));

    let ledger_profile = ReadOnlyProfile::LedgersV1 {
        company: &company_name,
    };
    let ledger_response = match transport.send(ledger_profile).await {
        Ok(response) => response,
        Err(transport_error) => {
            operations.push(transport_failure(
                ReadProfileId::XmlLedgerReadV1,
                ledger_profile.template_sha256(),
                &transport_error,
            ));
            append_vouchers_not_attempted(&mut operations, &company_name, &empty_range);
            return seal_receipt(
                config,
                fixture,
                metadata,
                operations,
                false,
                endpoint_response_observed,
                loaded_company_count,
            );
        }
    };
    let ledger_count = match parse_ledger_source_records_with_evidence(ledger_response.text()) {
        Ok(parsed)
            if context_matches(&parsed.evidence, &fixture.company_marker, selected_guid)
                && parsed.records.len() as u64 >= fixture.minimum_ledger_count
                && parsed.records.len() as u64 <= fixture.maximum_ledger_count
                && parsed
                    .records
                    .iter()
                    .filter(|record| record.record.name == fixture.ledger_sentinel)
                    .count()
                    == 1 =>
        {
            parsed.records.len()
        }
        _ => {
            operations.push(response_failure(
                ReadProfileId::XmlLedgerReadV1,
                ledger_profile.template_sha256(),
                &ledger_response,
                application_status(ledger_response.text()),
                "ledger_fixture_or_context_invalid",
            ));
            append_vouchers_not_attempted(&mut operations, &company_name, &empty_range);
            return seal_receipt(
                config,
                fixture,
                metadata,
                operations,
                false,
                endpoint_response_observed,
                loaded_company_count,
            );
        }
    };
    operations.push(passed_response(
        ReadProfileId::XmlLedgerReadV1,
        ledger_profile.template_sha256(),
        &ledger_response,
        ledger_count,
    ));

    let empty_profile = ReadOnlyProfile::VouchersV2 {
        company: &company_name,
        range: &empty_range,
    };
    let empty_response = match transport.send(empty_profile).await {
        Ok(response) => response,
        Err(transport_error) => {
            operations.push(transport_failure(
                ReadProfileId::XmlVoucherEmptyRangeV1,
                empty_profile.template_sha256(),
                &transport_error,
            ));
            operations.push(not_attempted(
                ReadProfileId::XmlVoucherPopulatedRangeV1,
                empty_profile.template_sha256(),
                "empty_voucher_profile_not_passed",
            ));
            return seal_receipt(
                config,
                fixture,
                metadata,
                operations,
                false,
                endpoint_response_observed,
                loaded_company_count,
            );
        }
    };
    match parse_voucher_source_records_with_evidence(empty_response.text()) {
        Ok(parsed)
            if parsed.records.is_empty()
                && context_matches(&parsed.evidence, &fixture.company_marker, selected_guid) =>
        {
            operations.push(passed_response(
                ReadProfileId::XmlVoucherEmptyRangeV1,
                empty_profile.template_sha256(),
                &empty_response,
                0,
            ));
        }
        _ => {
            operations.push(response_failure(
                ReadProfileId::XmlVoucherEmptyRangeV1,
                empty_profile.template_sha256(),
                &empty_response,
                application_status(empty_response.text()),
                "empty_voucher_fixture_or_context_invalid",
            ));
            operations.push(not_attempted(
                ReadProfileId::XmlVoucherPopulatedRangeV1,
                empty_profile.template_sha256(),
                "empty_voucher_profile_not_passed",
            ));
            return seal_receipt(
                config,
                fixture,
                metadata,
                operations,
                false,
                endpoint_response_observed,
                loaded_company_count,
            );
        }
    }

    let populated_profile = ReadOnlyProfile::VouchersV2 {
        company: &company_name,
        range: &populated_range,
    };
    let populated_response = match transport.send(populated_profile).await {
        Ok(response) => response,
        Err(transport_error) => {
            operations.push(transport_failure(
                ReadProfileId::XmlVoucherPopulatedRangeV1,
                populated_profile.template_sha256(),
                &transport_error,
            ));
            return seal_receipt(
                config,
                fixture,
                metadata,
                operations,
                false,
                endpoint_response_observed,
                loaded_company_count,
            );
        }
    };
    match parse_voucher_source_records_with_evidence(populated_response.text()) {
        Ok(parsed)
            if context_matches(&parsed.evidence, &fixture.company_marker, selected_guid)
                && parsed.records.len() as u64 >= fixture.minimum_populated_voucher_count
                && parsed.records.len() as u64 <= fixture.maximum_populated_voucher_count
                && parsed
                    .records
                    .iter()
                    .filter(|record| {
                        record.record.voucher_number.as_deref()
                            == Some(fixture.voucher_number_sentinel.as_str())
                    })
                    .count()
                    == 1
                && parsed.records.iter().all(|record| {
                    record.record.date.as_deref().is_some_and(|date| {
                        date >= populated_range.from_yyyymmdd()
                            && date <= populated_range.to_yyyymmdd()
                    })
                }) =>
        {
            operations.push(passed_response(
                ReadProfileId::XmlVoucherPopulatedRangeV1,
                populated_profile.template_sha256(),
                &populated_response,
                parsed.records.len(),
            ));
        }
        _ => operations.push(response_failure(
            ReadProfileId::XmlVoucherPopulatedRangeV1,
            populated_profile.template_sha256(),
            &populated_response,
            application_status(populated_response.text()),
            "populated_voucher_fixture_range_or_context_invalid",
        )),
    }

    let fixture_marker_verified = operations.len() == 5
        && operations
            .iter()
            .all(|operation| operation.outcome == OperationOutcome::Passed);
    seal_receipt(
        config,
        fixture,
        metadata,
        operations,
        fixture_marker_verified,
        endpoint_response_observed,
        loaded_company_count,
    )
}

fn seal_receipt(
    config: &LiveRunConfig,
    fixture: &SyntheticFixtureManifest,
    metadata: &RunMetadata,
    operations: Vec<OperationEvidence>,
    fixture_marker_verified: bool,
    endpoint_response_observed: bool,
    loaded_company_count: CountBucket,
) -> Result<LiveCompatibilityReceipt, LiveReadError> {
    LiveCompatibilityReceipt {
        schema_version: LIVE_RECEIPT_SCHEMA_VERSION,
        observed_at_unix_ms: metadata.observed_at_unix_ms,
        bridge_commit_sha: metadata.bridge_commit_sha.clone(),
        working_tree_dirty: metadata.working_tree_dirty,
        compatibility_surface_sha256: metadata.compatibility_surface_sha256.clone(),
        executable_sha256: metadata.executable_sha256.clone(),
        cargo_lock_sha256: metadata.cargo_lock_sha256.clone(),
        platform: current_platform(),
        architecture: current_architecture(),
        endpoint_family: config.endpoint_family,
        transport: TransportProfile::XmlHttp,
        product: product_profile(config.product),
        release: release_profile(&config.release),
        mode: mode_profile(config.mode),
        odbc_state: odbc_profile(config.odbc_state),
        locale: locale_profile(config.locale),
        dataset_tier: configured(fixture.dataset_tier),
        fixture_manifest_sha256: metadata.fixture_manifest_sha256.clone(),
        fixture_marker_verified,
        no_customer_data: attested(config.no_customer_data_attested),
        loaded_company_count,
        operations,
        authority: if endpoint_response_observed {
            LiveReadAuthority::observation_only()
        } else {
            LiveReadAuthority::attempt_only()
        },
        receipt_sha256: String::new(),
    }
    .seal()
    .map_err(|_| error("receipt_invalid"))
}

fn attested<T>(value: T) -> ProfileValue<T> {
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

fn unknown<T>(value: T) -> ProfileValue<T> {
    ProfileValue {
        value,
        authority: EvidenceAuthority::Unknown,
        confidence: EvidenceConfidence::Unknown,
    }
}

fn product_profile(value: ProductFamily) -> ProfileValue<ProductFamily> {
    if value == ProductFamily::Unknown {
        unknown(value)
    } else {
        attested(value)
    }
}

fn release_profile(value: &str) -> ProfileValue<String> {
    if value == "unknown" {
        unknown(value.to_string())
    } else {
        attested(value.to_string())
    }
}

fn mode_profile(value: TallyMode) -> ProfileValue<TallyMode> {
    if value == TallyMode::Unknown {
        unknown(value)
    } else {
        attested(value)
    }
}

fn odbc_profile(value: OdbcState) -> ProfileValue<OdbcState> {
    if value == OdbcState::Unknown {
        unknown(value)
    } else {
        attested(value)
    }
}

fn locale_profile(value: LocaleProfile) -> ProfileValue<LocaleProfile> {
    if value == LocaleProfile::Unknown {
        unknown(value)
    } else {
        attested(value)
    }
}

fn append_company_reads_not_attempted(
    operations: &mut Vec<OperationEvidence>,
    company: &ValidatedCompanyName,
    range: &ValidatedDateRange,
) {
    let ledger = ReadOnlyProfile::LedgersV1 { company };
    operations.push(not_attempted(
        ReadProfileId::XmlLedgerReadV1,
        ledger.template_sha256(),
        "synthetic_fixture_unverified",
    ));
    append_vouchers_not_attempted(operations, company, range);
}

fn append_vouchers_not_attempted(
    operations: &mut Vec<OperationEvidence>,
    company: &ValidatedCompanyName,
    range: &ValidatedDateRange,
) {
    let vouchers = ReadOnlyProfile::VouchersV2 { company, range };
    operations.push(not_attempted(
        ReadProfileId::XmlVoucherEmptyRangeV1,
        vouchers.template_sha256(),
        "prior_read_profile_not_passed",
    ));
    operations.push(not_attempted(
        ReadProfileId::XmlVoucherPopulatedRangeV1,
        vouchers.template_sha256(),
        "prior_read_profile_not_passed",
    ));
}

fn passed_response(
    profile: ReadProfileId,
    template_sha256: String,
    response: &ReadOnlyResponse,
    records: usize,
) -> OperationEvidence {
    OperationEvidence {
        profile,
        template_sha256,
        outcome: OperationOutcome::Passed,
        application_status: ApplicationStatus::Success,
        encoding: encoding(response.encoding()),
        response_size: size_bucket(response.encoded_bytes()),
        record_count: count_bucket(records),
        safe_reason_code: None,
    }
}

fn derived_failure(
    profile: ReadProfileId,
    template_sha256: String,
    response: &ReadOnlyResponse,
    reason: &'static str,
) -> OperationEvidence {
    response_failure(
        profile,
        template_sha256,
        response,
        ApplicationStatus::Success,
        reason,
    )
}

fn response_failure(
    profile: ReadProfileId,
    template_sha256: String,
    response: &ReadOnlyResponse,
    application_status: ApplicationStatus,
    reason: &'static str,
) -> OperationEvidence {
    OperationEvidence {
        profile,
        template_sha256,
        outcome: OperationOutcome::Failed,
        application_status,
        encoding: encoding(response.encoding()),
        response_size: size_bucket(response.encoded_bytes()),
        record_count: CountBucket::Unknown,
        safe_reason_code: Some(reason.to_string()),
    }
}

fn transport_failure(
    profile: ReadProfileId,
    template_sha256: String,
    transport_error: &ReadOnlyTransportError,
) -> OperationEvidence {
    OperationEvidence {
        profile,
        template_sha256,
        outcome: OperationOutcome::Failed,
        application_status: ApplicationStatus::NotApplicable,
        encoding: TextEncoding::Unknown,
        response_size: SizeBucket::Zero,
        record_count: CountBucket::Unknown,
        safe_reason_code: Some(transport_error.safe_code().to_string()),
    }
}

fn not_attempted(
    profile: ReadProfileId,
    template_sha256: String,
    reason: &'static str,
) -> OperationEvidence {
    OperationEvidence {
        profile,
        template_sha256,
        outcome: OperationOutcome::NotAttempted,
        application_status: ApplicationStatus::NotApplicable,
        encoding: TextEncoding::Unknown,
        response_size: SizeBucket::Zero,
        record_count: CountBucket::Unknown,
        safe_reason_code: Some(reason.to_string()),
    }
}

fn application_status(xml: &str) -> ApplicationStatus {
    match export_status(xml) {
        Ok(TallyExportStatus::Success) => ApplicationStatus::Success,
        Ok(TallyExportStatus::Failure) => {
            let _ = export_failure_reason_code(xml);
            ApplicationStatus::Failure
        }
        Err(_) => ApplicationStatus::Unrecognized,
    }
}

fn context_matches(evidence: &ExportEvidence, expected_name: &str, expected_guid: &str) -> bool {
    verify_company_context(evidence, expected_guid).is_ok()
        && evidence
            .company_context
            .as_ref()
            .and_then(|context| context.name.as_deref())
            == Some(expected_name)
}

fn encoding(value: TallyTextEncoding) -> TextEncoding {
    match value {
        TallyTextEncoding::Utf8 => TextEncoding::Utf8,
        TallyTextEncoding::Utf8Bom => TextEncoding::Utf8Bom,
        TallyTextEncoding::Utf16LeBom => TextEncoding::Utf16Le,
        TallyTextEncoding::Utf16BeBom => TextEncoding::Utf16Be,
    }
}

fn size_bucket(bytes: usize) -> SizeBucket {
    match bytes {
        0 => SizeBucket::Zero,
        1..=4096 => SizeBucket::Bytes1To4096,
        4097..=65536 => SizeBucket::Bytes4097To65536,
        65537..=1_048_576 => SizeBucket::Bytes65537To1048576,
        _ => SizeBucket::Over1048576,
    }
}

fn count_bucket(records: usize) -> CountBucket {
    match records {
        0 => CountBucket::Zero,
        1 => CountBucket::One,
        2..=5 => CountBucket::TwoToFive,
        6..=20 => CountBucket::SixToTwenty,
        _ => CountBucket::OverTwenty,
    }
}

fn validate_config(config: &LiveRunConfig) -> Result<(), LiveReadError> {
    if config.schema_version != CONFIG_SCHEMA_VERSION
        || config.port == 0
        || config.product == ProductFamily::Unknown
        || !valid_release(&config.release)
        || config.mode == TallyMode::Unknown
        || config.odbc_state == OdbcState::Unknown
        || config.locale == LocaleProfile::Unknown
        || !config.no_customer_data_attested
        || config.repository_root.as_os_str().is_empty()
        || config.fixture_manifest.as_os_str().is_empty()
    {
        return Err(error("config_invalid"));
    }
    Ok(())
}

fn validate_fixture(fixture: &SyntheticFixtureManifest) -> Result<(), LiveReadError> {
    if fixture.schema_version != FIXTURE_SCHEMA_VERSION
        || !valid_slug(&fixture.fixture_id)
        || fixture.dataset_tier == DatasetTier::Unknown
        || !fixture.company_marker.starts_with("BRIDGE-PR14-SYNTHETIC-")
        || fixture.company_marker.len() < 48
        || fixture.company_marker.len() > 128
        || fixture.company_marker.chars().any(char::is_control)
        || !valid_fixture_sentinel(&fixture.ledger_sentinel, "BRIDGE-LEDGER-")
        || !valid_fixture_sentinel(&fixture.voucher_number_sentinel, "BRIDGE-VOUCHER-")
        || fixture.minimum_ledger_count == 0
        || fixture.minimum_ledger_count > fixture.maximum_ledger_count
        || fixture.minimum_populated_voucher_count == 0
        || fixture.minimum_populated_voucher_count > fixture.maximum_populated_voucher_count
    {
        return Err(error("fixture_invalid"));
    }
    ValidatedCompanyName::new(fixture.company_marker.clone())
        .map_err(|_| error("fixture_invalid"))?;
    let empty = ValidatedDateRange::new(
        fixture.empty_voucher_range.from_yyyymmdd.clone(),
        fixture.empty_voucher_range.to_yyyymmdd.clone(),
    )
    .map_err(|_| error("fixture_invalid"))?;
    let populated = ValidatedDateRange::new(
        fixture.populated_voucher_range.from_yyyymmdd.clone(),
        fixture.populated_voucher_range.to_yyyymmdd.clone(),
    )
    .map_err(|_| error("fixture_invalid"))?;
    if ranges_overlap(&empty, &populated) {
        return Err(error("fixture_ranges_overlap"));
    }
    Ok(())
}

fn valid_fixture_sentinel(value: &str, prefix: &str) -> bool {
    value.starts_with(prefix)
        && value.len() >= prefix.len() + 36
        && value.len() <= 128
        && !value.chars().any(char::is_control)
}

fn ranges_overlap(left: &ValidatedDateRange, right: &ValidatedDateRange) -> bool {
    left.from_yyyymmdd() <= right.to_yyyymmdd() && right.from_yyyymmdd() <= left.to_yyyymmdd()
}

fn valid_release(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.trim() == value
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
        && !matches!(value.to_ascii_lowercase().as_str(), "unknown" | "latest")
        && !value.to_ascii_lowercase().ends_with(".x")
}

fn valid_slug(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 96
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
        })
}

fn canonical_join(base: &Path, value: &Path, code: &'static str) -> Result<PathBuf, LiveReadError> {
    let joined = if value.is_absolute() {
        value.to_path_buf()
    } else {
        base.join(value)
    };
    joined.canonicalize().map_err(|_| error(code))
}

fn read_bounded(path: &Path, maximum: usize, code: &'static str) -> Result<Vec<u8>, LiveReadError> {
    let metadata = fs::metadata(path).map_err(|_| error(code))?;
    if metadata.len() == 0 || metadata.len() > maximum as u64 {
        return Err(error("local_input_size_invalid"));
    }
    fs::read(path).map_err(|_| error(code))
}

fn git_output(repository_root: &Path, arguments: &[&str]) -> Result<String, LiveReadError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repository_root)
        .args(arguments)
        .output()
        .map_err(|_| error("git_unavailable"))?;
    if !output.status.success() || output.stdout.len() > MAX_LOCAL_INPUT_BYTES {
        return Err(error("git_query_failed"));
    }
    String::from_utf8(output.stdout)
        .map(|value| value.trim().to_string())
        .map_err(|_| error("git_query_failed"))
}

fn valid_commit(value: &str) -> bool {
    matches!(value.len(), 40 | 64)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn network_consent_binding(
    config: &LiveRunConfig,
    fixture_id: &str,
    metadata: &RunMetadata,
    expires_at_unix_ms: i64,
) -> Result<String, LiveReadError> {
    let config_bytes = serde_json::to_vec(config).map_err(|_| error("config_commitment_failed"))?;
    let mut nonce = [0_u8; 32];
    getrandom::fill(&mut nonce).map_err(|_| error("secure_random_unavailable"))?;
    let mut digest = Sha256::new();
    digest.update(b"bridge.tally.live-read-consent/2\0");
    for field in [
        config_bytes.as_slice(),
        fixture_id.as_bytes(),
        metadata.bridge_commit_sha.as_bytes(),
        metadata.compatibility_surface_sha256.as_bytes(),
        metadata.executable_sha256.as_bytes(),
        metadata.cargo_lock_sha256.as_bytes(),
        metadata.fixture_manifest_sha256.as_bytes(),
    ] {
        digest.update((field.len() as u64).to_be_bytes());
        digest.update(field);
    }
    digest.update([u8::from(metadata.working_tree_dirty)]);
    digest.update(metadata.observed_at_unix_ms.to_be_bytes());
    digest.update(expires_at_unix_ms.to_be_bytes());
    digest.update(nonce);
    Ok(sha256_digest_hex(digest.finalize()))
}

fn verify_network_consent(
    inputs: &LiveRunInputs,
    consent: &NetworkConsent,
) -> Result<(), LiveReadError> {
    ensure_not_expired(consent.expires_at_unix_ms)?;
    if !inputs.config.no_customer_data_attested
        || consent.binding != inputs.consent_binding
        || consent.expires_at_unix_ms != inputs.consent_expires_at_unix_ms
    {
        return Err(error("network_consent_binding_mismatch"));
    }
    Ok(())
}

fn ensure_not_expired(expires_at_unix_ms: i64) -> Result<(), LiveReadError> {
    let now = now_unix_ms().map_err(|_| error("system_clock_invalid"))?;
    if now > expires_at_unix_ms {
        return Err(error("network_consent_expired"));
    }
    Ok(())
}

fn validate_current_surface(
    repository_root: &Path,
    expected_manifest_sha256: &str,
) -> Result<(), LiveReadError> {
    let path = repository_root.join("docs/tally/compatibility/compatibility-surface.json");
    let surface = CompatibilitySurfaceManifest::from_json(&read_bounded(
        &path,
        MAX_ARTIFACT_BYTES,
        "surface_unavailable",
    )?)
    .map_err(|_| error("surface_invalid"))?;
    if surface.manifest_sha256 != expected_manifest_sha256 {
        return Err(error("surface_changed_after_consent"));
    }
    surface
        .validate_files(repository_root)
        .map_err(|_| error("surface_changed_after_consent"))
}

fn sha256_hex(bytes: &[u8]) -> String {
    sha256_digest_hex(Sha256::digest(bytes))
}

fn sha256_digest_hex(bytes: impl AsRef<[u8]>) -> String {
    let mut output = String::with_capacity(64);
    for byte in bytes.as_ref() {
        use std::fmt::Write as _;
        write!(&mut output, "{byte:02x}").expect("writing to a String cannot fail");
    }
    output
}

fn read_loopback(family: LoopbackFamily) -> ReadLoopback {
    match family {
        LoopbackFamily::LocalhostAlias => ReadLoopback::LocalhostAlias,
        LoopbackFamily::Ipv4 => ReadLoopback::Ipv4,
        LoopbackFamily::Ipv6 => ReadLoopback::Ipv6,
    }
}

fn current_platform() -> Platform {
    if cfg!(target_os = "windows") {
        Platform::Windows
    } else if cfg!(target_os = "macos") {
        Platform::Macos
    } else {
        Platform::Linux
    }
}

fn current_architecture() -> Architecture {
    match std::env::consts::ARCH {
        "x86_64" => Architecture::X86_64,
        "aarch64" => Architecture::Aarch64,
        _ => Architecture::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tally_protocol_simulator::{Fixture, ScenarioPlan, Simulator};

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
            company_marker: "BRIDGE-PR14-SYNTHETIC-019f605f-e6cf-77b2-ac95-31722887a911"
                .to_string(),
            ledger_sentinel: "BRIDGE-LEDGER-019f605f-e6cf-77b2-ac95-31722887a911".to_string(),
            voucher_number_sentinel: "BRIDGE-VOUCHER-019f605f-e6cf-77b2-ac95-31722887a911"
                .to_string(),
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
            let simulator = Simulator::spawn(ScenarioPlan::new(Fixture::EmptyExport)).unwrap();
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
}
