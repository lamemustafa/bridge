//! Qualification-only controller for the dormant native Ledger Outstandings candidate.
//!
//! This module is available only behind a non-default binary feature. It can
//! produce an observation receipt, but it has no runtime, parser, mirror, or
//! compatibility-support authority.

use std::{
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use bridge_tally_compatibility::{
    bills_native_outstandings_probe_receipt::{
        BillsNativeOutstandingsProbeReceiptV0, ByteRepeatability, CandidateAttemptId,
        CandidateAttemptOutcome, IdentityBracketState, IdentitySnapshotId,
        ProbeAttestationAuthority, ProbeAttestedEnvironmentV0, ProbeCandidateAttemptV0,
        ProbeCommitmentsV0, ProbeEndpointV0, ProbeIdentitySnapshotV0, ProbeInitialPreflightV0,
        ProbeReceiptAuthorityV0, ProbeUiObservationV0, UiObservationPosition, UserAttestedUiChange,
        BILLS_NATIVE_OUTSTANDINGS_PROBE_RECEIPT_MAX_BYTES,
        BILLS_NATIVE_OUTSTANDINGS_PROBE_RECEIPT_SCHEMA_VERSION,
        BILLS_NATIVE_OUTSTANDINGS_PROFILE_ID, BILLS_NATIVE_OUTSTANDINGS_REPORT_ID,
    },
    now_unix_ms, sha256_file, ApplicationStatus, CompatibilitySurfaceManifest, DatasetTier,
    LocaleProfile, LoopbackFamily, ProductFamily, TallyMode, MAX_ARTIFACT_BYTES,
};
use bridge_tally_protocol::{
    bills_native_outstandings_probe::{
        NativeLedgerOutstandingsProbeScope, SealedNativeLedgerOutstandingsProbe,
        ValidatedProbeCompanyName, ValidatedProbeLedgerName, ValidatedProbeToDate,
    },
    export_status, parse_companies_with_evidence, parse_ledger_source_records_with_evidence,
    verify_company_context,
    xml_read_profiles::{ReadOnlyProfile, ValidatedCompanyName},
    ParsedSourceIdentityKind, TallyExportStatus,
};
use bridge_tally_read_transport::{
    QualificationOnlyNativeOutstandingsResponse, QualificationOnlyNativeOutstandingsTransport,
    ReadOnlyTransport, ReadOnlyTransportError,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;

use super::{
    canonical_join, current_architecture, current_platform, encoding, git_output, read_bounded,
    read_loopback, save_receipt_no_replace, sha256_hex, valid_commit, valid_release, valid_slug,
    MAX_LOCAL_INPUT_BYTES,
};

const CONFIG_SCHEMA_VERSION: u16 = 1;
const FIXTURE_SCHEMA_VERSION: u16 = 1;
const UI_SCHEMA_VERSION: u16 = 1;
const CONSENT_TTL_MS: i64 = 5 * 60 * 1000;
const UI_BEFORE_MAX_AGE_MS: i64 = 15 * 60 * 1000;
const UI_PROJECTION_MAX_ROWS: usize = 256;
const UI_PROJECTION_TEXT_MAX_CHARS: usize = 512;
const COMPANY_IDENTITY_DOMAIN: &[u8] =
    b"bridge.tally.native-ledger-outstandings.company-identity/0\0";
const PARTY_IDENTITY_DOMAIN: &[u8] = b"bridge.tally.native-ledger-outstandings.party-identity/0\0";
const CHALLENGE_DOMAIN: &[u8] = b"bridge.tally.native-ledger-outstandings.consent/0\0";
const EXPECTED_PROJECTION_FIELDS: [&str; 10] = [
    "row_index",
    "row_kind",
    "display_date_text",
    "reference_text",
    "opening_amount_text",
    "pending_amount_text",
    "due_date_text",
    "overdue_text",
    "voucher_details_text",
    "dr_cr_text",
];

#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error("native outstandings qualification failed ({code})")]
pub struct NativeOutstandingsQualificationError {
    code: &'static str,
}

impl NativeOutstandingsQualificationError {
    pub fn safe_code(&self) -> &'static str {
        self.code
    }
}

fn error(code: &'static str) -> NativeOutstandingsQualificationError {
    NativeOutstandingsQualificationError { code }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct NativeProbeConfig {
    schema_version: u16,
    repository_root: PathBuf,
    fixture_manifest: PathBuf,
    scenario_id: String,
    endpoint_family: LoopbackFamily,
    port: u16,
    product: ProductFamily,
    release: String,
    mode: TallyMode,
    locale: LocaleProfile,
    configured_tdl_count: Option<u32>,
    configured_add_on_count: Option<u32>,
    expected_company_identity_sha256: String,
    expected_party_identity_sha256: String,
    identity_registration_id: String,
    identity_registration_reviewed: bool,
    no_customer_data_attested: bool,
    ui_before_observation: PathBuf,
    ui_after_observation: PathBuf,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureCandidate {
    profile_id: String,
    template_sha256: String,
    observation_posture: String,
    request_shape_immutable: bool,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureScope {
    company_marker: String,
    party_marker: String,
    currency: String,
    bill_by_bill_tracking_required: bool,
    customer_or_personal_data_forbidden: bool,
    expected_company_identity_commitment_source: String,
    expected_party_identity_commitment_source: String,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureScenario {
    scenario_id: String,
    to_date: String,
    request_sha256: String,
    scope_sha256: String,
    expected_accounting_facts: Vec<String>,
    inv_001_ui_presence_requirement: UiPresenceRequirement,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum UiPresenceRequirement {
    MustBeObservedPresent,
    MustRecordObservedPresentOrOmitted,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureBudget {
    preflight_posts: u8,
    dispatch_identity_posts: u8,
    candidate_dispatch_posts: u8,
    dispatch_posts: u8,
    maximum_total_posts: u8,
    automatic_retries: u8,
    preflight_order: Vec<String>,
    dispatch_order: Vec<String>,
    dispatch_request: String,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeFixtureManifest {
    schema_version: u16,
    fixture_id: String,
    dataset_tier: DatasetTier,
    candidate: FixtureCandidate,
    synthetic_scope: FixtureScope,
    education_constraints: Value,
    fixture_facts: Vec<Value>,
    scenarios: Vec<FixtureScenario>,
    one_scenario_per_invocation: bool,
    request_budget: FixtureBudget,
    ui_observation_contract: Value,
    authority: Value,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct UiContext {
    company_marker: String,
    party_marker: String,
    to_date: String,
    report_name: String,
    report_mode: String,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct UiVisibleColumns {
    opening: bool,
    pending: bool,
    due: bool,
    overdue: bool,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct UiProjectionRow {
    row_index: u32,
    row_kind: String,
    display_date_text: String,
    reference_text: String,
    opening_amount_text: String,
    pending_amount_text: String,
    due_date_text: String,
    overdue_text: String,
    voucher_details_text: String,
    dr_cr_text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SettledReferenceObservation {
    Present,
    Omitted,
    Unobserved,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct UiObservation {
    schema_version: u16,
    fixture_id: String,
    scenario_id: String,
    phase: String,
    evidence_complete: bool,
    captured_unix_ms: i64,
    screenshot_sha256: String,
    context: UiContext,
    visible_columns: UiVisibleColumns,
    ordered_projection_fields: Vec<String>,
    ordered_projection: Vec<UiProjectionRow>,
    inv_001_settled_reference_observation: SettledReferenceObservation,
    operator_attests_no_tally_interaction_until_after_capture: Option<bool>,
    operator_attests_no_tally_interaction_since_before_capture: Option<bool>,
}

struct ProbeMetadata {
    bridge_commit_sha: String,
    working_tree_dirty: bool,
    compatibility_surface_sha256: String,
    executable_sha256: String,
    cargo_lock_sha256: String,
    fixture_manifest_sha256: String,
}

pub struct LoadedNativeOutstandingsProbe {
    config: NativeProbeConfig,
    fixture: NativeFixtureManifest,
    scenario: FixtureScenario,
    candidate: SealedNativeLedgerOutstandingsProbe,
    metadata: ProbeMetadata,
    repository_root: PathBuf,
    ui_before: UiObservation,
    ui_after_path: PathBuf,
    run_commitment: String,
    preflight_challenge: String,
    preflight_binding: String,
    preflight_expires_at: i64,
}

pub struct PreflightConsent {
    binding: String,
    expires_at: i64,
}
pub struct DispatchConsent {
    binding: String,
    expires_at: i64,
}
pub struct UiAfterConsent {
    binding: String,
    expires_at: i64,
}

pub struct NativeProbeReceiptOutput {
    output_path: PathBuf,
    canonical_parent: PathBuf,
}

pub struct DispatchReadyNativeOutstandingsProbe {
    loaded: LoadedNativeOutstandingsProbe,
    initial_preflight: ProbeInitialPreflightV0,
    dispatch_challenge: String,
    dispatch_binding: String,
    dispatch_expires_at: i64,
}

pub struct PendingNativeOutstandingsProbeReceipt {
    loaded: LoadedNativeOutstandingsProbe,
    initial_preflight: ProbeInitialPreflightV0,
    snapshots: [ProbeIdentitySnapshotV0; 4],
    attempts: [ProbeCandidateAttemptV0; 3],
    byte_repeatability: ByteRepeatability,
    ui_after_challenge: String,
    ui_after_binding: String,
    ui_after_expires_at: i64,
}

impl LoadedNativeOutstandingsProbe {
    pub fn load(config_path: &Path) -> Result<Self, NativeOutstandingsQualificationError> {
        let config_bytes = read_bounded(
            config_path,
            MAX_LOCAL_INPUT_BYTES,
            "native_probe_config_unavailable",
        )
        .map_err(|_| error("native_probe_config_unavailable"))?;
        let config: NativeProbeConfig = serde_json::from_slice(&config_bytes)
            .map_err(|_| error("native_probe_config_invalid"))?;
        validate_config(&config)?;
        let base = config_path.parent().unwrap_or_else(|| Path::new("."));
        let repository_root = canonical_join(
            base,
            &config.repository_root,
            "native_probe_repository_unavailable",
        )
        .map_err(|_| error("native_probe_repository_unavailable"))?;
        let reviewed_local_root = repository_root
            .join(".bridge-live")
            .canonicalize()
            .map_err(|_| error("native_probe_local_root_unavailable"))?;
        let canonical_config = config_path
            .canonicalize()
            .map_err(|_| error("native_probe_config_unavailable"))?;
        if !canonical_config.starts_with(&reviewed_local_root) {
            return Err(error("native_probe_config_outside_local_root"));
        }

        let fixture_path = canonical_join(
            base,
            &config.fixture_manifest,
            "native_probe_fixture_unavailable",
        )
        .map_err(|_| error("native_probe_fixture_unavailable"))?;
        let fixture_root = repository_root
            .join("docs/tally/compatibility/fixtures")
            .canonicalize()
            .map_err(|_| error("native_probe_fixture_root_unavailable"))?;
        if !fixture_path.starts_with(&fixture_root) {
            return Err(error("native_probe_fixture_outside_reviewed_root"));
        }
        let fixture_bytes = read_bounded(
            &fixture_path,
            MAX_LOCAL_INPUT_BYTES,
            "native_probe_fixture_unavailable",
        )
        .map_err(|_| error("native_probe_fixture_unavailable"))?;
        let fixture: NativeFixtureManifest = serde_json::from_slice(&fixture_bytes)
            .map_err(|_| error("native_probe_fixture_invalid"))?;
        let scenario = validate_fixture(&fixture, &config.scenario_id)?.clone();

        let company =
            ValidatedProbeCompanyName::new(fixture.synthetic_scope.company_marker.clone())
                .map_err(|_| error("native_probe_company_marker_invalid"))?;
        let ledger = ValidatedProbeLedgerName::new(fixture.synthetic_scope.party_marker.clone())
            .map_err(|_| error("native_probe_party_marker_invalid"))?;
        let to_date = ValidatedProbeToDate::new(scenario.to_date.clone())
            .map_err(|_| error("native_probe_scenario_date_invalid"))?;
        let candidate = NativeLedgerOutstandingsProbeScope::new(company, ledger, to_date).seal();
        if candidate.template_sha256() != fixture.candidate.template_sha256
            || candidate.request_sha256() != scenario.request_sha256
            || candidate.scope_sha256() != scenario.scope_sha256
        {
            return Err(error("native_probe_candidate_commitment_mismatch"));
        }

        let ui_before_path = canonical_join(
            base,
            &config.ui_before_observation,
            "native_probe_ui_before_unavailable",
        )
        .map_err(|_| error("native_probe_ui_before_unavailable"))?;
        let ui_after_path = canonical_join(
            base,
            &config.ui_after_observation,
            "native_probe_ui_after_unavailable",
        )
        .map_err(|_| error("native_probe_ui_after_unavailable"))?;
        if !ui_before_path.starts_with(&reviewed_local_root)
            || !ui_after_path.starts_with(&reviewed_local_root)
            || ui_before_path == ui_after_path
        {
            return Err(error("native_probe_ui_path_invalid"));
        }
        let ui_before = load_ui(&ui_before_path, "native_probe_ui_before_unavailable")?;

        let surface_path =
            repository_root.join("docs/tally/compatibility/compatibility-surface.json");
        let surface = CompatibilitySurfaceManifest::from_json(
            &read_bounded(
                &surface_path,
                MAX_ARTIFACT_BYTES,
                "native_probe_surface_unavailable",
            )
            .map_err(|_| error("native_probe_surface_unavailable"))?,
        )
        .map_err(|_| error("native_probe_surface_invalid"))?;
        surface
            .validate_files(&repository_root)
            .map_err(|_| error("native_probe_surface_changed"))?;

        let loaded_at_unix_ms = now_unix_ms().map_err(|_| error("native_probe_clock_invalid"))?;
        validate_ui(&ui_before, "before", &fixture, &scenario, loaded_at_unix_ms)?;
        if loaded_at_unix_ms - ui_before.captured_unix_ms > UI_BEFORE_MAX_AGE_MS {
            return Err(error("native_probe_ui_before_stale"));
        }
        let bridge_commit_sha = git_output(&repository_root, &["rev-parse", "HEAD"])
            .map_err(|_| error("native_probe_git_query_failed"))?;
        if !valid_commit(&bridge_commit_sha) {
            return Err(error("native_probe_commit_invalid"));
        }
        let working_tree_dirty = !git_output(
            &repository_root,
            &["status", "--porcelain", "--untracked-files=all"],
        )
        .map_err(|_| error("native_probe_git_query_failed"))?
        .is_empty();
        let executable =
            std::env::current_exe().map_err(|_| error("native_probe_executable_unavailable"))?;
        let metadata = ProbeMetadata {
            bridge_commit_sha,
            working_tree_dirty,
            compatibility_surface_sha256: surface.manifest_sha256,
            executable_sha256: sha256_file(&executable)
                .map_err(|_| error("native_probe_executable_unavailable"))?,
            cargo_lock_sha256: sha256_file(&repository_root.join("src-tauri/Cargo.lock"))
                .map_err(|_| error("native_probe_cargo_lock_unavailable"))?,
            fixture_manifest_sha256: sha256_hex(&fixture_bytes),
        };
        let run_commitment = qualification_run_commitment(
            &config, &fixture, &scenario, &candidate, &metadata, &ui_before,
        )?;
        let preflight_expires_at = loaded_at_unix_ms + CONSENT_TTL_MS;
        let preflight_binding = consent_binding(
            "preflight",
            &config,
            &fixture.fixture_id,
            candidate.request_sha256(),
            &run_commitment,
            preflight_expires_at,
        )?;
        let preflight_challenge = format!(
            "PREFLIGHT {} {}",
            fixture.fixture_id,
            &preflight_binding[..16]
        );
        Ok(Self {
            config,
            fixture,
            scenario,
            candidate,
            metadata,
            repository_root,
            ui_before,
            ui_after_path,
            run_commitment,
            preflight_challenge,
            preflight_binding,
            preflight_expires_at,
        })
    }

    pub fn preflight_challenge(&self) -> &str {
        &self.preflight_challenge
    }

    pub fn validate_receipt_output(
        &self,
        output_path: &Path,
    ) -> Result<NativeProbeReceiptOutput, NativeOutstandingsQualificationError> {
        if output_path.exists() {
            return Err(error("receipt_output_exists"));
        }
        let parent = output_path.parent().unwrap_or_else(|| Path::new("."));
        let canonical_parent = parent
            .canonicalize()
            .map_err(|_| error("receipt_output_parent_unavailable"))?;
        let local_root = self
            .repository_root
            .join(".bridge-live")
            .canonicalize()
            .map_err(|_| error("native_probe_local_root_unavailable"))?;
        if canonical_parent != local_root
            || output_path.extension().and_then(|v| v.to_str()) != Some("json")
        {
            return Err(error("receipt_output_outside_local_root"));
        }
        output_path
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| error("receipt_output_invalid"))?;
        Ok(NativeProbeReceiptOutput {
            output_path: output_path.to_path_buf(),
            canonical_parent,
        })
    }

    pub async fn run_preflight(
        self,
        consent: PreflightConsent,
    ) -> Result<DispatchReadyNativeOutstandingsProbe, NativeOutstandingsQualificationError> {
        verify_consumed_consent(
            &consent.binding,
            consent.expires_at,
            &self.preflight_binding,
            self.preflight_expires_at,
        )?;
        let transport =
            ReadOnlyTransport::new(read_loopback(self.config.endpoint_family), self.config.port)
                .map_err(|_| error("native_probe_endpoint_configuration_invalid"))?;
        let observation = observe_identity(&transport, &self.fixture).await?;
        require_registered_identity(&self.config, &observation)?;
        let initial_preflight = ProbeInitialPreflightV0 {
            observed_at_unix_ms: observation.observed_at_unix_ms,
            company_identity_commitment_sha256: observation.company_identity,
            party_identity_commitment_sha256: observation.party_identity,
            company_response_sha256: observation.company_response,
            party_response_sha256: observation.party_response,
        };
        let expires_at =
            now_unix_ms().map_err(|_| error("native_probe_clock_invalid"))? + CONSENT_TTL_MS;
        let dispatch_evidence = private_commitment(
            "dispatch-evidence",
            &[
                &self.run_commitment,
                &initial_preflight.company_identity_commitment_sha256,
                &initial_preflight.party_identity_commitment_sha256,
                &initial_preflight.company_response_sha256,
                &initial_preflight.party_response_sha256,
            ],
        );
        let dispatch_binding = consent_binding(
            "dispatch",
            &self.config,
            &self.fixture.fixture_id,
            self.candidate.request_sha256(),
            &dispatch_evidence,
            expires_at,
        )?;
        let dispatch_challenge = format!(
            "DISPATCH {} {}",
            self.fixture.fixture_id,
            &dispatch_binding[..16]
        );
        Ok(DispatchReadyNativeOutstandingsProbe {
            loaded: self,
            initial_preflight,
            dispatch_challenge,
            dispatch_binding,
            dispatch_expires_at: expires_at,
        })
    }
}

pub fn confirm_preflight_challenge(
    loaded: &LoadedNativeOutstandingsProbe,
    typed: &str,
) -> Result<PreflightConsent, NativeOutstandingsQualificationError> {
    require_typed(typed, &loaded.preflight_challenge)?;
    ensure_unexpired(loaded.preflight_expires_at)?;
    Ok(PreflightConsent {
        binding: loaded.preflight_binding.clone(),
        expires_at: loaded.preflight_expires_at,
    })
}

impl DispatchReadyNativeOutstandingsProbe {
    pub fn dispatch_challenge(&self) -> &str {
        &self.dispatch_challenge
    }

    pub async fn dispatch(
        self,
        consent: DispatchConsent,
    ) -> Result<PendingNativeOutstandingsProbeReceipt, NativeOutstandingsQualificationError> {
        verify_consumed_consent(
            &consent.binding,
            consent.expires_at,
            &self.dispatch_binding,
            self.dispatch_expires_at,
        )?;
        let loopback = read_loopback(self.loaded.config.endpoint_family);
        let identity_transport = ReadOnlyTransport::new(loopback, self.loaded.config.port)
            .map_err(|_| error("native_probe_endpoint_configuration_invalid"))?;
        let candidate_transport =
            QualificationOnlyNativeOutstandingsTransport::new(loopback, self.loaded.config.port)
                .map_err(|_| error("native_probe_endpoint_configuration_invalid"))?;

        let b0 = observe_identity(&identity_transport, &self.loaded.fixture).await?;
        require_registered_identity(&self.loaded.config, &b0)?;
        let a1 = observe_candidate(
            &candidate_transport,
            &self.loaded.candidate,
            CandidateAttemptId::A1,
        )
        .await?;
        let b1 = observe_identity(&identity_transport, &self.loaded.fixture).await?;
        require_unchanged_identity(&b0, &b1)?;
        let a2 = observe_candidate(
            &candidate_transport,
            &self.loaded.candidate,
            CandidateAttemptId::A2,
        )
        .await?;
        let b2 = observe_identity(&identity_transport, &self.loaded.fixture).await?;
        require_unchanged_identity(&b1, &b2)?;
        let a3 = observe_candidate(
            &candidate_transport,
            &self.loaded.candidate,
            CandidateAttemptId::A3,
        )
        .await?;
        let b3 = observe_identity(&identity_transport, &self.loaded.fixture).await?;
        require_unchanged_identity(&b2, &b3)?;

        let snapshots = [
            snapshot(IdentitySnapshotId::B0, b0),
            snapshot(IdentitySnapshotId::B1, b1),
            snapshot(IdentitySnapshotId::B2, b2),
            snapshot(IdentitySnapshotId::B3, b3),
        ];
        let byte_repeatability = match (&a1.encoded_body, &a2.encoded_body, &a3.encoded_body) {
            (Some(first), Some(second), Some(third)) if first == second && second == third => {
                ByteRepeatability::Identical
            }
            (Some(_), Some(_), Some(_)) => ByteRepeatability::Different,
            _ => ByteRepeatability::NotEstablished,
        };
        let attempts = [a1.attempt, a2.attempt, a3.attempt];
        let attempt_evidence = [
            attempt_fact_commitment(&attempts[0])?,
            attempt_fact_commitment(&attempts[1])?,
            attempt_fact_commitment(&attempts[2])?,
        ];
        let expires_at =
            now_unix_ms().map_err(|_| error("native_probe_clock_invalid"))? + CONSENT_TTL_MS;
        let ui_after_evidence = private_commitment(
            "ui-after-evidence",
            &[
                &self.loaded.run_commitment,
                &self.initial_preflight.company_response_sha256,
                &self.initial_preflight.party_response_sha256,
                &snapshots[3].company_response_sha256,
                &snapshots[3].party_response_sha256,
                &attempt_evidence[0],
                &attempt_evidence[1],
                &attempt_evidence[2],
            ],
        );
        let ui_after_binding = consent_binding(
            "ui-after",
            &self.loaded.config,
            &self.loaded.fixture.fixture_id,
            self.loaded.candidate.request_sha256(),
            &ui_after_evidence,
            expires_at,
        )?;
        let ui_after_challenge = format!(
            "UI-AFTER {} {}",
            self.loaded.fixture.fixture_id,
            &ui_after_binding[..16]
        );
        Ok(PendingNativeOutstandingsProbeReceipt {
            loaded: self.loaded,
            initial_preflight: self.initial_preflight,
            snapshots,
            attempts,
            byte_repeatability,
            ui_after_challenge,
            ui_after_binding,
            ui_after_expires_at: expires_at,
        })
    }
}

pub fn confirm_dispatch_challenge(
    ready: &DispatchReadyNativeOutstandingsProbe,
    typed: &str,
) -> Result<DispatchConsent, NativeOutstandingsQualificationError> {
    require_typed(typed, &ready.dispatch_challenge)?;
    ensure_unexpired(ready.dispatch_expires_at)?;
    Ok(DispatchConsent {
        binding: ready.dispatch_binding.clone(),
        expires_at: ready.dispatch_expires_at,
    })
}

impl PendingNativeOutstandingsProbeReceipt {
    pub fn ui_after_challenge(&self) -> &str {
        &self.ui_after_challenge
    }

    pub fn finalize(
        self,
        consent: UiAfterConsent,
    ) -> Result<BillsNativeOutstandingsProbeReceiptV0, NativeOutstandingsQualificationError> {
        verify_consumed_consent(
            &consent.binding,
            consent.expires_at,
            &self.ui_after_binding,
            self.ui_after_expires_at,
        )?;
        let now = now_unix_ms().map_err(|_| error("native_probe_clock_invalid"))?;
        let ui_after = load_ui(
            &self.loaded.ui_after_path,
            "native_probe_ui_after_unavailable",
        )?;
        validate_ui(
            &ui_after,
            "after",
            &self.loaded.fixture,
            &self.loaded.scenario,
            now,
        )?;
        if ui_after.captured_unix_ms < self.snapshots[3].observed_at_unix_ms {
            return Err(error("native_probe_ui_after_precedes_dispatch"));
        }
        let before_hash = structured_ui_hash(&self.loaded.ui_before)?;
        let after_hash = structured_ui_hash(&ui_after)?;
        if before_hash != after_hash
            || self.loaded.ui_before.screenshot_sha256 == ui_after.screenshot_sha256
        {
            return Err(error("native_probe_ui_observation_changed_or_reused"));
        }
        let ui_before_receipt = receipt_ui(
            &self.loaded.ui_before,
            UiObservationPosition::Before,
            &self.loaded.config,
            &self.initial_preflight,
            &before_hash,
        );
        let ui_after_receipt = receipt_ui(
            &ui_after,
            UiObservationPosition::After,
            &self.loaded.config,
            &self.initial_preflight,
            &after_hash,
        );
        BillsNativeOutstandingsProbeReceiptV0 {
            schema_version: BILLS_NATIVE_OUTSTANDINGS_PROBE_RECEIPT_SCHEMA_VERSION,
            observed_at_unix_ms: now,
            bridge_commit_sha: self.loaded.metadata.bridge_commit_sha,
            working_tree_dirty: self.loaded.metadata.working_tree_dirty,
            compatibility_surface_sha256: self.loaded.metadata.compatibility_surface_sha256,
            executable_sha256: self.loaded.metadata.executable_sha256,
            cargo_lock_sha256: self.loaded.metadata.cargo_lock_sha256,
            platform: current_platform(),
            architecture: current_architecture(),
            endpoint: ProbeEndpointV0 {
                family: self.loaded.config.endpoint_family,
                port: self.loaded.config.port,
                canonical_origin: canonical_origin(
                    self.loaded.config.endpoint_family,
                    self.loaded.config.port,
                ),
            },
            attested_environment: ProbeAttestedEnvironmentV0 {
                authority: ProbeAttestationAuthority::User,
                product: self.loaded.config.product,
                release: self.loaded.config.release,
                mode: self.loaded.config.mode,
                locale: self.loaded.config.locale,
                configured_tdl_count: self.loaded.config.configured_tdl_count.unwrap_or_default(),
                no_customer_data: self.loaded.config.no_customer_data_attested,
            },
            commitments: ProbeCommitmentsV0 {
                fixture_id: self.loaded.fixture.fixture_id,
                fixture_manifest_sha256: self.loaded.metadata.fixture_manifest_sha256,
                profile_id: BILLS_NATIVE_OUTSTANDINGS_PROFILE_ID.to_string(),
                template_sha256: self.loaded.candidate.template_sha256(),
                request_sha256: self.loaded.candidate.request_sha256().to_string(),
                scope_sha256: self.loaded.candidate.scope_sha256().to_string(),
            },
            initial_preflight: self.initial_preflight,
            identity_snapshots: self.snapshots,
            attempts: self.attempts,
            byte_repeatability: self.byte_repeatability,
            ui_before: ui_before_receipt,
            ui_after: ui_after_receipt,
            user_attested_ui_change: UserAttestedUiChange::Unchanged,
            authority: ProbeReceiptAuthorityV0::observation_only(),
            receipt_sha256: String::new(),
        }
        .seal()
        .map_err(|_| error("native_probe_receipt_invalid"))
    }
}

pub fn confirm_ui_after_challenge(
    pending: &PendingNativeOutstandingsProbeReceipt,
    typed: &str,
) -> Result<UiAfterConsent, NativeOutstandingsQualificationError> {
    require_typed(typed, &pending.ui_after_challenge)?;
    ensure_unexpired(pending.ui_after_expires_at)?;
    Ok(UiAfterConsent {
        binding: pending.ui_after_binding.clone(),
        expires_at: pending.ui_after_expires_at,
    })
}

pub fn native_probe_save_phrase(
    receipt: &BillsNativeOutstandingsProbeReceiptV0,
    output: &NativeProbeReceiptOutput,
) -> Result<String, NativeOutstandingsQualificationError> {
    output.revalidate()?;
    let output_binding = native_probe_output_binding(output)?;
    Ok(format!(
        "SAVE-NATIVE-OBSERVATION {} {}",
        &receipt.receipt_sha256[..16],
        &output_binding[..16]
    ))
}

pub fn save_native_probe_receipt_no_replace(
    output: NativeProbeReceiptOutput,
    receipt_bytes: &[u8],
    typed: &str,
) -> Result<(), NativeOutstandingsQualificationError> {
    output.revalidate()?;
    if receipt_bytes.len() > BILLS_NATIVE_OUTSTANDINGS_PROBE_RECEIPT_MAX_BYTES {
        return Err(error("native_probe_receipt_size_invalid"));
    }
    let receipt = BillsNativeOutstandingsProbeReceiptV0::from_json(receipt_bytes)
        .map_err(|_| error("native_probe_receipt_invalid"))?;
    let expected_phrase = native_probe_save_phrase(&receipt, &output)?;
    save_receipt_no_replace(&output.output_path, receipt_bytes, typed, &expected_phrase)
        .map_err(|failure| error(failure.safe_code()))
}

impl NativeProbeReceiptOutput {
    fn revalidate(&self) -> Result<(), NativeOutstandingsQualificationError> {
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

fn native_probe_output_binding(
    output: &NativeProbeReceiptOutput,
) -> Result<String, NativeOutstandingsQualificationError> {
    let file_name = output
        .output_path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| error("receipt_output_invalid"))?;
    let parent = output.canonical_parent.to_string_lossy();
    Ok(identity_commitment(
        b"bridge.tally.native-ledger-outstandings.receipt-output/0\0",
        &[parent.as_bytes(), file_name.as_bytes()],
    ))
}

struct IdentityObservation {
    observed_at_unix_ms: i64,
    company_identity: String,
    party_identity: String,
    company_response: String,
    party_response: String,
}

async fn observe_identity(
    transport: &ReadOnlyTransport,
    fixture: &NativeFixtureManifest,
) -> Result<IdentityObservation, NativeOutstandingsQualificationError> {
    let company_response = transport
        .send(ReadOnlyProfile::CompanyListV1)
        .await
        .map_err(|_| error("native_probe_company_preflight_transport_failed"))?;
    if !matches!(
        export_status(company_response.text()),
        Ok(TallyExportStatus::Success)
    ) {
        return Err(error("native_probe_company_preflight_status_failed"));
    }
    let companies = parse_companies_with_evidence(company_response.text())
        .map_err(|_| error("native_probe_company_preflight_invalid"))?;
    let matches: Vec<_> = companies
        .records
        .iter()
        .filter(|company| company.name == fixture.synthetic_scope.company_marker)
        .collect();
    if matches.len() != 1 {
        return Err(error("native_probe_company_identity_ambiguous"));
    }
    let guid = matches[0]
        .guid
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| error("native_probe_company_identity_missing"))?;
    if companies
        .records
        .iter()
        .filter(|company| company.guid.as_deref() == Some(guid))
        .count()
        != 1
    {
        return Err(error("native_probe_company_identity_duplicate"));
    }
    let company_name = ValidatedCompanyName::new(fixture.synthetic_scope.company_marker.clone())
        .map_err(|_| error("native_probe_company_marker_invalid"))?;
    let party_response = transport
        .send(ReadOnlyProfile::LedgersV1 {
            company: &company_name,
        })
        .await
        .map_err(|_| error("native_probe_party_preflight_transport_failed"))?;
    if !matches!(
        export_status(party_response.text()),
        Ok(TallyExportStatus::Success)
    ) {
        return Err(error("native_probe_party_preflight_status_failed"));
    }
    let ledgers = parse_ledger_source_records_with_evidence(party_response.text())
        .map_err(|_| error("native_probe_party_preflight_invalid"))?;
    if verify_company_context(&ledgers.evidence, guid).is_err()
        || ledgers
            .evidence
            .company_context
            .as_ref()
            .and_then(|context| context.name.as_deref())
            != Some(fixture.synthetic_scope.company_marker.as_str())
    {
        return Err(error("native_probe_party_context_mismatch"));
    }
    let matches: Vec<_> = ledgers
        .records
        .iter()
        .filter(|record| record.record.name == fixture.synthetic_scope.party_marker)
        .collect();
    if matches.len() != 1 {
        return Err(error("native_probe_party_identity_ambiguous"));
    }
    let source_id = matches[0]
        .source_id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| error("native_probe_party_identity_missing"))?;
    let identity_kind = matches[0]
        .identity_kind
        .ok_or_else(|| error("native_probe_party_identity_missing"))?;
    if ledgers
        .records
        .iter()
        .filter(|record| record.source_id.as_deref() == Some(source_id))
        .count()
        != 1
    {
        return Err(error("native_probe_party_identity_duplicate"));
    }
    Ok(IdentityObservation {
        observed_at_unix_ms: now_unix_ms().map_err(|_| error("native_probe_clock_invalid"))?,
        company_identity: identity_commitment(COMPANY_IDENTITY_DOMAIN, &[guid.as_bytes()]),
        party_identity: identity_commitment(
            PARTY_IDENTITY_DOMAIN,
            &[
                identity_kind_label(identity_kind).as_bytes(),
                source_id.as_bytes(),
            ],
        ),
        company_response: sha256_hex(company_response.encoded_body()),
        party_response: sha256_hex(party_response.encoded_body()),
    })
}

struct CandidateObservation {
    attempt: ProbeCandidateAttemptV0,
    encoded_body: Option<Vec<u8>>,
}

async fn observe_candidate(
    transport: &QualificationOnlyNativeOutstandingsTransport,
    candidate: &SealedNativeLedgerOutstandingsProbe,
    attempt_id: CandidateAttemptId,
) -> Result<CandidateObservation, NativeOutstandingsQualificationError> {
    match transport.send_candidate_v0(candidate).await {
        Ok(response) => {
            let attempt = observed_attempt(attempt_id, &response)?;
            Ok(CandidateObservation {
                attempt,
                encoded_body: Some(response.encoded_body().to_vec()),
            })
        }
        Err(failure) => Ok(CandidateObservation {
            attempt: failed_attempt(attempt_id, &failure)?,
            encoded_body: None,
        }),
    }
}

fn observed_attempt(
    attempt_id: CandidateAttemptId,
    response: &QualificationOnlyNativeOutstandingsResponse,
) -> Result<ProbeCandidateAttemptV0, NativeOutstandingsQualificationError> {
    let (application_status, safe_reason_code) = match export_status(response.text()) {
        Ok(TallyExportStatus::Success) => (ApplicationStatus::Success, None),
        Ok(TallyExportStatus::Failure) => (
            ApplicationStatus::Failure,
            Some("tally_application_failure".to_string()),
        ),
        Err(_) => (
            ApplicationStatus::Unrecognized,
            Some("tally_application_status_unrecognized".to_string()),
        ),
    };
    Ok(ProbeCandidateAttemptV0 {
        attempt_id,
        observed_at_unix_ms: now_unix_ms().map_err(|_| error("native_probe_clock_invalid"))?,
        outcome: CandidateAttemptOutcome::ResponseObserved,
        http_status: Some(response.http_status()),
        application_status,
        encoding: encoding(response.encoding()),
        exact_encoded_bytes: response.encoded_body().len() as u64,
        encoded_body_sha256: Some(sha256_hex(response.encoded_body())),
        decoded_text_sha256: Some(sha256_hex(response.text().as_bytes())),
        safe_reason_code,
        bracket_state: IdentityBracketState::Unchanged,
    })
}

fn failed_attempt(
    attempt_id: CandidateAttemptId,
    failure: &ReadOnlyTransportError,
) -> Result<ProbeCandidateAttemptV0, NativeOutstandingsQualificationError> {
    Ok(ProbeCandidateAttemptV0 {
        attempt_id,
        observed_at_unix_ms: now_unix_ms().map_err(|_| error("native_probe_clock_invalid"))?,
        outcome: if failure.http_status().is_some() {
            CandidateAttemptOutcome::HttpRejected
        } else {
            CandidateAttemptOutcome::TransportFailed
        },
        http_status: failure.http_status(),
        application_status: ApplicationStatus::NotApplicable,
        encoding: bridge_tally_compatibility::TextEncoding::Unknown,
        exact_encoded_bytes: 0,
        encoded_body_sha256: None,
        decoded_text_sha256: None,
        safe_reason_code: Some(failure.safe_code().to_string()),
        bracket_state: IdentityBracketState::Unchanged,
    })
}

fn attempt_fact_commitment(
    attempt: &ProbeCandidateAttemptV0,
) -> Result<String, NativeOutstandingsQualificationError> {
    let bytes =
        serde_json::to_vec(attempt).map_err(|_| error("native_probe_attempt_commitment_failed"))?;
    Ok(identity_commitment(
        b"bridge.tally.native-ledger-outstandings.attempt-fact/0\0",
        &[&bytes],
    ))
}

fn snapshot(id: IdentitySnapshotId, value: IdentityObservation) -> ProbeIdentitySnapshotV0 {
    ProbeIdentitySnapshotV0 {
        snapshot_id: id,
        observed_at_unix_ms: value.observed_at_unix_ms,
        company_identity_commitment_sha256: value.company_identity,
        party_identity_commitment_sha256: value.party_identity,
        company_response_sha256: value.company_response,
        party_response_sha256: value.party_response,
    }
}

fn require_registered_identity(
    config: &NativeProbeConfig,
    value: &IdentityObservation,
) -> Result<(), NativeOutstandingsQualificationError> {
    if value.company_identity != config.expected_company_identity_sha256
        || value.party_identity != config.expected_party_identity_sha256
    {
        return Err(error("native_probe_registered_identity_mismatch"));
    }
    Ok(())
}

fn require_unchanged_identity(
    left: &IdentityObservation,
    right: &IdentityObservation,
) -> Result<(), NativeOutstandingsQualificationError> {
    if left.company_identity != right.company_identity
        || left.party_identity != right.party_identity
    {
        return Err(error("native_probe_identity_drift_observed"));
    }
    Ok(())
}

fn validate_config(config: &NativeProbeConfig) -> Result<(), NativeOutstandingsQualificationError> {
    if config.schema_version != CONFIG_SCHEMA_VERSION
        || config.port == 0
        || config.product != ProductFamily::TallyPrime
        || !valid_release(&config.release)
        || config.mode != TallyMode::Education
        || config.locale == LocaleProfile::Unknown
        || config.configured_tdl_count != Some(0)
        || config.configured_add_on_count != Some(0)
        || !config.identity_registration_reviewed
        || !valid_slug(&config.identity_registration_id)
        || config.identity_registration_id == "unregistered"
        || !config.no_customer_data_attested
        || !valid_nonzero_sha256(&config.expected_company_identity_sha256)
        || !valid_nonzero_sha256(&config.expected_party_identity_sha256)
        || config.repository_root.as_os_str().is_empty()
        || config.fixture_manifest.as_os_str().is_empty()
    {
        return Err(error("native_probe_config_invalid"));
    }
    Ok(())
}

fn validate_fixture<'a>(
    fixture: &'a NativeFixtureManifest,
    scenario_id: &str,
) -> Result<&'a FixtureScenario, NativeOutstandingsQualificationError> {
    let expected_preflight = ["company_list_v1", "ledgers_v1"];
    let expected_dispatch = [
        "b0_company_list_v1",
        "b0_ledgers_v1",
        "candidate_1",
        "b1_company_list_v1",
        "b1_ledgers_v1",
        "candidate_2",
        "b2_company_list_v1",
        "b2_ledgers_v1",
        "candidate_3",
        "b3_company_list_v1",
        "b3_ledgers_v1",
    ];
    if fixture.schema_version != FIXTURE_SCHEMA_VERSION || !valid_slug(&fixture.fixture_id)
        || fixture.dataset_tier != DatasetTier::SyntheticSmall
        || fixture.candidate.profile_id != BILLS_NATIVE_OUTSTANDINGS_PROFILE_ID
        || fixture.candidate.observation_posture != "profile_unobserved"
        || !fixture.candidate.request_shape_immutable
        || !valid_nonzero_sha256(&fixture.candidate.template_sha256)
        || !fixture.synthetic_scope.company_marker.starts_with("BRIDGE-PR18-NATIVE-OUTSTANDINGS-COMPANY-")
        || !fixture.synthetic_scope.party_marker.starts_with("BRIDGE-PR18-NATIVE-OUTSTANDINGS-PARTY-")
        || fixture.synthetic_scope.currency != "INR"
        || !fixture.synthetic_scope.bill_by_bill_tracking_required
        || !fixture.synthetic_scope.customer_or_personal_data_forbidden
        || fixture.synthetic_scope.expected_company_identity_commitment_source != "separate_reviewed_local_registration"
        || fixture.synthetic_scope.expected_party_identity_commitment_source != "separate_reviewed_local_registration"
        || fixture.fixture_facts.is_empty() || !fixture.one_scenario_per_invocation
        || fixture.request_budget.preflight_posts != 2
        || fixture.request_budget.dispatch_identity_posts != 8
        || fixture.request_budget.candidate_dispatch_posts != 3
        || fixture.request_budget.dispatch_posts != 11
        || fixture.request_budget.maximum_total_posts != 13
        || fixture.request_budget.automatic_retries != 0
        || fixture.request_budget.preflight_order.iter().map(String::as_str).ne(expected_preflight)
        || fixture.request_budget.dispatch_order.iter().map(String::as_str).ne(expected_dispatch)
        || fixture.request_budget.dispatch_request != "four_identity_brackets_and_three_byte_identical_native_ledger_outstandings_candidate_v0_requests"
        || !fixture.education_constraints.is_object() || !fixture.ui_observation_contract.is_object()
        || !fixture.authority.is_object()
    { return Err(error("native_probe_fixture_invalid")); }
    let matches: Vec<_> = fixture
        .scenarios
        .iter()
        .filter(|scenario| scenario.scenario_id == scenario_id)
        .collect();
    if matches.len() != 1
        || matches[0].expected_accounting_facts.is_empty()
        || !valid_nonzero_sha256(&matches[0].request_sha256)
        || !valid_nonzero_sha256(&matches[0].scope_sha256)
    {
        return Err(error("native_probe_scenario_invalid"));
    }
    Ok(matches[0])
}

fn load_ui(
    path: &Path,
    code: &'static str,
) -> Result<UiObservation, NativeOutstandingsQualificationError> {
    let bytes = read_bounded(path, MAX_LOCAL_INPUT_BYTES, code).map_err(|_| error(code))?;
    serde_json::from_slice(&bytes).map_err(|_| error("native_probe_ui_observation_invalid"))
}

fn validate_ui(
    ui: &UiObservation,
    phase: &str,
    fixture: &NativeFixtureManifest,
    scenario: &FixtureScenario,
    now: i64,
) -> Result<(), NativeOutstandingsQualificationError> {
    let expected_fields: Vec<String> = EXPECTED_PROJECTION_FIELDS
        .iter()
        .map(|v| (*v).to_string())
        .collect();
    let phase_attestation = match phase {
        "before" => {
            ui.operator_attests_no_tally_interaction_until_after_capture == Some(true)
                && ui
                    .operator_attests_no_tally_interaction_since_before_capture
                    .is_none()
        }
        "after" => {
            ui.operator_attests_no_tally_interaction_since_before_capture == Some(true)
                && ui
                    .operator_attests_no_tally_interaction_until_after_capture
                    .is_none()
        }
        _ => false,
    };
    let presence_matches_scenario = match scenario.inv_001_ui_presence_requirement {
        UiPresenceRequirement::MustBeObservedPresent => {
            ui.inv_001_settled_reference_observation == SettledReferenceObservation::Present
        }
        UiPresenceRequirement::MustRecordObservedPresentOrOmitted => matches!(
            ui.inv_001_settled_reference_observation,
            SettledReferenceObservation::Present | SettledReferenceObservation::Omitted
        ),
    };
    if ui.schema_version != UI_SCHEMA_VERSION
        || ui.fixture_id != fixture.fixture_id
        || ui.scenario_id != scenario.scenario_id
        || ui.phase != phase
        || !ui.evidence_complete
        || ui.captured_unix_ms <= 0
        || ui.captured_unix_ms > now
        || !valid_nonzero_sha256(&ui.screenshot_sha256)
        || ui.context.company_marker != fixture.synthetic_scope.company_marker
        || ui.context.party_marker != fixture.synthetic_scope.party_marker
        || ui.context.to_date != scenario.to_date
        || ui.context.report_name != "Ledger Outstandings"
        || ui.context.report_mode != "detailed"
        || !ui.visible_columns.opening
        || !ui.visible_columns.pending
        || !ui.visible_columns.due
        || !ui.visible_columns.overdue
        || ui.ordered_projection_fields != expected_fields
        || !presence_matches_scenario
        || !phase_attestation
    {
        return Err(error("native_probe_ui_observation_incomplete"));
    }
    validate_ui_projection(&ui.ordered_projection)?;
    Ok(())
}

fn validate_ui_projection(
    rows: &[UiProjectionRow],
) -> Result<(), NativeOutstandingsQualificationError> {
    if rows.is_empty() || rows.len() > UI_PROJECTION_MAX_ROWS {
        return Err(error("native_probe_ui_projection_invalid"));
    }
    for (index, row) in rows.iter().enumerate() {
        if row.row_index != index as u32
            || !valid_ui_projection_text(&row.row_kind, false, 64)
            || [
                &row.display_date_text,
                &row.reference_text,
                &row.opening_amount_text,
                &row.pending_amount_text,
                &row.due_date_text,
                &row.overdue_text,
                &row.voucher_details_text,
                &row.dr_cr_text,
            ]
            .into_iter()
            .any(|value| !valid_ui_projection_text(value, true, UI_PROJECTION_TEXT_MAX_CHARS))
        {
            return Err(error("native_probe_ui_projection_invalid"));
        }
    }
    Ok(())
}

fn valid_ui_projection_text(value: &str, allow_empty: bool, maximum_chars: usize) -> bool {
    (allow_empty || !value.is_empty())
        && value.chars().count() <= maximum_chars
        && !value.chars().any(char::is_control)
}

fn structured_ui_hash(ui: &UiObservation) -> Result<String, NativeOutstandingsQualificationError> {
    let value = serde_json::to_vec(&(
        &ui.fixture_id,
        &ui.scenario_id,
        &ui.context,
        &ui.visible_columns,
        &ui.ordered_projection_fields,
        &ui.ordered_projection,
        &ui.inv_001_settled_reference_observation,
    ))
    .map_err(|_| error("native_probe_ui_observation_invalid"))?;
    Ok(sha256_hex(&value))
}

fn receipt_ui(
    ui: &UiObservation,
    position: UiObservationPosition,
    config: &NativeProbeConfig,
    preflight: &ProbeInitialPreflightV0,
    structured_hash: &str,
) -> ProbeUiObservationV0 {
    ProbeUiObservationV0 {
        position,
        observed_at_unix_ms: ui.captured_unix_ms,
        product: config.product,
        release: config.release.clone(),
        mode: config.mode,
        report_id: BILLS_NATIVE_OUTSTANDINGS_REPORT_ID.to_string(),
        opening_column_visible: ui.visible_columns.opening,
        pending_column_visible: ui.visible_columns.pending,
        due_column_visible: ui.visible_columns.due,
        overdue_column_visible: ui.visible_columns.overdue,
        company_identity_commitment_sha256: preflight.company_identity_commitment_sha256.clone(),
        party_identity_commitment_sha256: preflight.party_identity_commitment_sha256.clone(),
        structured_observation_sha256: structured_hash.to_string(),
        screenshot_sha256: ui.screenshot_sha256.clone(),
    }
}

fn consent_binding(
    stage: &str,
    config: &NativeProbeConfig,
    fixture_id: &str,
    request_hash: &str,
    prior_hash: &str,
    expires_at: i64,
) -> Result<String, NativeOutstandingsQualificationError> {
    let mut nonce = [0_u8; 32];
    getrandom::fill(&mut nonce).map_err(|_| error("native_probe_random_unavailable"))?;
    let mut digest = Sha256::new();
    digest.update(CHALLENGE_DOMAIN);
    for field in [
        stage.as_bytes(),
        fixture_id.as_bytes(),
        request_hash.as_bytes(),
        prior_hash.as_bytes(),
        config.expected_company_identity_sha256.as_bytes(),
        config.expected_party_identity_sha256.as_bytes(),
    ] {
        hash_field(&mut digest, field);
    }
    digest.update(config.port.to_be_bytes());
    digest.update(expires_at.to_be_bytes());
    digest.update(nonce);
    Ok(hex_lower(&digest.finalize()))
}

fn qualification_run_commitment(
    config: &NativeProbeConfig,
    fixture: &NativeFixtureManifest,
    scenario: &FixtureScenario,
    candidate: &SealedNativeLedgerOutstandingsProbe,
    metadata: &ProbeMetadata,
    ui_before: &UiObservation,
) -> Result<String, NativeOutstandingsQualificationError> {
    let config_commitment =
        serde_json::to_vec(config).map_err(|_| error("native_probe_config_commitment_failed"))?;
    let ui_commitment = structured_ui_hash(ui_before)?;
    let template_commitment = candidate.template_sha256();
    Ok(private_commitment_bytes(
        "reviewed-run",
        &[
            &config_commitment,
            fixture.fixture_id.as_bytes(),
            scenario.scenario_id.as_bytes(),
            template_commitment.as_bytes(),
            candidate.request_sha256().as_bytes(),
            candidate.scope_sha256().as_bytes(),
            metadata.bridge_commit_sha.as_bytes(),
            metadata.compatibility_surface_sha256.as_bytes(),
            metadata.executable_sha256.as_bytes(),
            metadata.cargo_lock_sha256.as_bytes(),
            metadata.fixture_manifest_sha256.as_bytes(),
            ui_commitment.as_bytes(),
            ui_before.screenshot_sha256.as_bytes(),
        ],
    ))
}

fn private_commitment(stage: &str, fields: &[&str]) -> String {
    let fields: Vec<&[u8]> = fields.iter().map(|field| field.as_bytes()).collect();
    private_commitment_bytes(stage, &fields)
}

fn private_commitment_bytes(stage: &str, fields: &[&[u8]]) -> String {
    let mut digest = Sha256::new();
    digest.update(b"bridge.tally.native-ledger-outstandings.private-commitment/0\0");
    hash_field(&mut digest, stage.as_bytes());
    for field in fields {
        hash_field(&mut digest, field);
    }
    hex_lower(&digest.finalize())
}

fn identity_commitment(domain: &[u8], fields: &[&[u8]]) -> String {
    let mut digest = Sha256::new();
    digest.update(domain);
    for field in fields {
        hash_field(&mut digest, field);
    }
    hex_lower(&digest.finalize())
}

fn hash_field(digest: &mut Sha256, value: &[u8]) {
    digest.update((value.len() as u64).to_be_bytes());
    digest.update(value);
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(&mut output, "{byte:02x}").expect("string write");
    }
    output
}

fn identity_kind_label(kind: ParsedSourceIdentityKind) -> &'static str {
    match kind {
        ParsedSourceIdentityKind::Guid => "guid",
        ParsedSourceIdentityKind::RemoteId => "remote_id",
        ParsedSourceIdentityKind::MasterId => "master_id",
    }
}

fn require_typed(typed: &str, expected: &str) -> Result<(), NativeOutstandingsQualificationError> {
    if typed.trim_end_matches(['\r', '\n']) != expected {
        return Err(error("native_probe_consent_mismatch"));
    }
    Ok(())
}

fn ensure_unexpired(expires_at: i64) -> Result<(), NativeOutstandingsQualificationError> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| error("native_probe_clock_invalid"))?
        .as_millis();
    let now = i64::try_from(now).map_err(|_| error("native_probe_clock_invalid"))?;
    if now > expires_at {
        return Err(error("native_probe_consent_expired"));
    }
    Ok(())
}

fn verify_consumed_consent(
    binding: &str,
    expires_at: i64,
    expected_binding: &str,
    expected_expiry: i64,
) -> Result<(), NativeOutstandingsQualificationError> {
    ensure_unexpired(expires_at)?;
    if binding != expected_binding || expires_at != expected_expiry {
        return Err(error("native_probe_consent_binding_mismatch"));
    }
    Ok(())
}

fn valid_nonzero_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        && value.bytes().any(|byte| byte != b'0')
}

fn canonical_origin(family: LoopbackFamily, port: u16) -> String {
    match family {
        LoopbackFamily::Ipv6 => format!("http://[::1]:{port}"),
        LoopbackFamily::Ipv4 | LoopbackFamily::LocalhostAlias => format!("http://127.0.0.1:{port}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tally_protocol_simulator::{Fixture, ScenarioPlan, SequenceSimulator};

    const COMPANY: &str =
        "BRIDGE-PR18-NATIVE-OUTSTANDINGS-COMPANY-019f605f-e6cf-77b2-ac95-31722887a911";
    const PARTY: &str =
        "BRIDGE-PR18-NATIVE-OUTSTANDINGS-PARTY-019f605f-e6cf-77b2-ac95-31722887a911";
    const COMPANY_GUID: &str = "00000000-0000-4000-8000-000000000001";
    const PARTY_GUID: &str = "00000000-0000-4000-8000-000000000101";

    fn company_xml() -> String {
        format!(
            "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><COMPANYINFO><COMPANYNAMEFIELD>{COMPANY}</COMPANYNAMEFIELD><COMPANYGUIDFIELD>{COMPANY_GUID}</COMPANYGUIDFIELD></COMPANYINFO></BODY></ENVELOPE>"
        )
    }

    fn ledger_xml() -> String {
        format!(
            "<ENVELOPE><HEADER><VERSION>1</VERSION><STATUS>1</STATUS></HEADER><BODY><DATA><COMPANYCONTEXT><SCHEMA>bridge.tally.ledgers/1</SCHEMA><OBJECTTYPE>LEDGER</OBJECTTYPE><NAME>{COMPANY}</NAME><GUID>{COMPANY_GUID}</GUID><RECORDCOUNT>1</RECORDCOUNT></COMPANYCONTEXT><COLLECTION><LEDGER NAME=\"{PARTY}\" GUID=\"{PARTY_GUID}\"><PARENT>Sundry Debtors</PARENT></LEDGER></COLLECTION></DATA></BODY></ENVELOPE>"
        )
    }

    fn candidate() -> SealedNativeLedgerOutstandingsProbe {
        NativeLedgerOutstandingsProbeScope::new(
            ValidatedProbeCompanyName::new(COMPANY).unwrap(),
            ValidatedProbeLedgerName::new(PARTY).unwrap(),
            ValidatedProbeToDate::new("20260731").unwrap(),
        )
        .seal()
    }

    fn fixture(candidate: &SealedNativeLedgerOutstandingsProbe) -> NativeFixtureManifest {
        NativeFixtureManifest {
            schema_version: 1,
            fixture_id: "education-native-outstandings-v0".to_string(),
            dataset_tier: DatasetTier::SyntheticSmall,
            candidate: FixtureCandidate {
                profile_id: BILLS_NATIVE_OUTSTANDINGS_PROFILE_ID.to_string(),
                template_sha256: candidate.template_sha256(),
                observation_posture: "profile_unobserved".to_string(),
                request_shape_immutable: true,
            },
            synthetic_scope: FixtureScope {
                company_marker: COMPANY.to_string(),
                party_marker: PARTY.to_string(),
                currency: "INR".to_string(),
                bill_by_bill_tracking_required: true,
                customer_or_personal_data_forbidden: true,
                expected_company_identity_commitment_source:
                    "separate_reviewed_local_registration".to_string(),
                expected_party_identity_commitment_source:
                    "separate_reviewed_local_registration".to_string(),
            },
            education_constraints: serde_json::json!({}),
            fixture_facts: vec![serde_json::json!({"synthetic": true})],
            scenarios: vec![FixtureScenario {
                scenario_id: "education-to-date-20260731".to_string(),
                to_date: "20260731".to_string(),
                request_sha256: candidate.request_sha256().to_string(),
                scope_sha256: candidate.scope_sha256().to_string(),
                expected_accounting_facts: vec!["synthetic fact".to_string()],
                inv_001_ui_presence_requirement:
                    UiPresenceRequirement::MustRecordObservedPresentOrOmitted,
            }],
            one_scenario_per_invocation: true,
            request_budget: FixtureBudget {
                preflight_posts: 2,
                dispatch_identity_posts: 8,
                candidate_dispatch_posts: 3,
                dispatch_posts: 11,
                maximum_total_posts: 13,
                automatic_retries: 0,
                preflight_order: vec!["company_list_v1".into(), "ledgers_v1".into()],
                dispatch_order: [
                    "b0_company_list_v1",
                    "b0_ledgers_v1",
                    "candidate_1",
                    "b1_company_list_v1",
                    "b1_ledgers_v1",
                    "candidate_2",
                    "b2_company_list_v1",
                    "b2_ledgers_v1",
                    "candidate_3",
                    "b3_company_list_v1",
                    "b3_ledgers_v1",
                ]
                .into_iter()
                .map(str::to_string)
                .collect(),
                dispatch_request: "four_identity_brackets_and_three_byte_identical_native_ledger_outstandings_candidate_v0_requests".into(),
            },
            ui_observation_contract: serde_json::json!({}),
            authority: serde_json::json!({}),
        }
    }

    fn config(port: u16) -> NativeProbeConfig {
        NativeProbeConfig {
            schema_version: 1,
            repository_root: PathBuf::from("."),
            fixture_manifest: PathBuf::from("fixture.json"),
            scenario_id: "education-to-date-20260731".to_string(),
            endpoint_family: LoopbackFamily::Ipv4,
            port,
            product: ProductFamily::TallyPrime,
            release: "1.1.7.0".to_string(),
            mode: TallyMode::Education,
            locale: LocaleProfile::EnglishIndia,
            configured_tdl_count: Some(0),
            configured_add_on_count: Some(0),
            expected_company_identity_sha256: identity_commitment(
                COMPANY_IDENTITY_DOMAIN,
                &[COMPANY_GUID.as_bytes()],
            ),
            expected_party_identity_sha256: identity_commitment(
                PARTY_IDENTITY_DOMAIN,
                &[b"guid", PARTY_GUID.as_bytes()],
            ),
            identity_registration_id: "reviewed-synthetic-registration".to_string(),
            identity_registration_reviewed: true,
            no_customer_data_attested: true,
            ui_before_observation: PathBuf::from("before.json"),
            ui_after_observation: PathBuf::from("after.json"),
        }
    }

    fn loaded(port: u16) -> LoadedNativeOutstandingsProbe {
        let candidate = candidate();
        let fixture = fixture(&candidate);
        let scenario = fixture.scenarios[0].clone();
        let expires = now_unix_ms().unwrap() + CONSENT_TTL_MS;
        LoadedNativeOutstandingsProbe {
            config: config(port),
            fixture,
            scenario,
            candidate,
            metadata: ProbeMetadata {
                bridge_commit_sha: "a".repeat(40),
                working_tree_dirty: true,
                compatibility_surface_sha256: "b".repeat(64),
                executable_sha256: "c".repeat(64),
                cargo_lock_sha256: "d".repeat(64),
                fixture_manifest_sha256: "e".repeat(64),
            },
            repository_root: PathBuf::from("."),
            ui_before: UiObservation {
                schema_version: 1,
                fixture_id: "education-native-outstandings-v0".into(),
                scenario_id: "education-to-date-20260731".into(),
                phase: "before".into(),
                evidence_complete: true,
                captured_unix_ms: now_unix_ms().unwrap(),
                screenshot_sha256: "f".repeat(64),
                context: UiContext {
                    company_marker: COMPANY.into(),
                    party_marker: PARTY.into(),
                    to_date: "20260731".into(),
                    report_name: "Ledger Outstandings".into(),
                    report_mode: "detailed".into(),
                },
                visible_columns: UiVisibleColumns {
                    opening: true,
                    pending: true,
                    due: true,
                    overdue: true,
                },
                ordered_projection_fields: EXPECTED_PROJECTION_FIELDS
                    .into_iter()
                    .map(str::to_string)
                    .collect(),
                ordered_projection: vec![UiProjectionRow {
                    row_index: 0,
                    row_kind: "opening".into(),
                    display_date_text: String::new(),
                    reference_text: "OPEN-001".into(),
                    opening_amount_text: "250.00".into(),
                    pending_amount_text: "250.00".into(),
                    due_date_text: String::new(),
                    overdue_text: String::new(),
                    voucher_details_text: String::new(),
                    dr_cr_text: "Dr".into(),
                }],
                inv_001_settled_reference_observation: SettledReferenceObservation::Present,
                operator_attests_no_tally_interaction_until_after_capture: Some(true),
                operator_attests_no_tally_interaction_since_before_capture: None,
            },
            ui_after_path: PathBuf::from("after.json"),
            run_commitment: "1".repeat(64),
            preflight_challenge: "PREFLIGHT test".into(),
            preflight_binding: "binding".into(),
            preflight_expires_at: expires,
        }
    }

    async fn run_exact_sequence_once() -> Result<(), &'static str> {
        let mut plans = vec![
            ScenarioPlan::new(Fixture::SyntheticXml(company_xml())),
            ScenarioPlan::new(Fixture::SyntheticXml(ledger_xml())),
        ];
        for bracket in 0..4 {
            plans.push(ScenarioPlan::new(Fixture::SyntheticXml(company_xml())));
            plans.push(ScenarioPlan::new(Fixture::SyntheticXml(ledger_xml())));
            if bracket < 3 {
                plans.push(ScenarioPlan::new(Fixture::ExportStatusOne));
            }
        }
        assert_eq!(plans.len(), 13);
        let simulator = SequenceSimulator::spawn(plans).unwrap();
        let loaded = loaded(simulator.address().port());
        let preflight = confirm_preflight_challenge(&loaded, "PREFLIGHT test").unwrap();
        let ready = match loaded.run_preflight(preflight).await {
            Ok(ready) => ready,
            Err(failure) => {
                simulator.cancel();
                let _ = simulator.finish();
                return Err(failure.safe_code());
            }
        };
        let dispatch_phrase = ready.dispatch_challenge().to_string();
        let dispatch = confirm_dispatch_challenge(&ready, &dispatch_phrase).unwrap();
        let pending = match ready.dispatch(dispatch).await {
            Ok(pending) => pending,
            Err(failure) => {
                simulator.cancel();
                let _ = simulator.finish();
                return Err(failure.safe_code());
            }
        };
        assert_eq!(pending.attempts.len(), 3);
        assert!(pending
            .attempts
            .iter()
            .all(|attempt| attempt.application_status == ApplicationStatus::Success));

        let observed = simulator.finish().unwrap();
        let candidate_hash = candidate().request_sha256().to_string();
        if observed.len() != 13
            || !observed.iter().all(|request| request.request_processed)
            || [4_usize, 7, 10]
                .into_iter()
                .any(|index| observed[index].request_body_sha256 != candidate_hash)
        {
            return Err("simulator_request_observation_incomplete");
        }
        Ok(())
    }

    #[tokio::test]
    async fn exact_thirteen_request_sequence_is_bracketed_and_zero_retry() {
        // This retries only a disposable Windows socket harness. Each runner
        // instance still has a fixed 13-request budget and zero request retry.
        let mut last_failure = "simulator_not_started";
        for _ in 0..5 {
            match run_exact_sequence_once().await {
                Ok(()) => return,
                Err(failure) => last_failure = failure,
            }
        }
        panic!("sequence simulator remained unstable: {last_failure}");
    }

    async fn run_candidate_failure_sequence_once() -> Result<(), &'static str> {
        let mut plans = vec![
            ScenarioPlan::new(Fixture::SyntheticXml(company_xml())),
            ScenarioPlan::new(Fixture::SyntheticXml(ledger_xml())),
        ];
        let candidates = [
            ScenarioPlan::new(Fixture::ExportStatusZero),
            ScenarioPlan::new(Fixture::ExportStatusOne).with_http_status(503),
            ScenarioPlan::new(Fixture::ExportStatusMissing),
        ];
        for candidate_plan in candidates {
            plans.push(ScenarioPlan::new(Fixture::SyntheticXml(company_xml())));
            plans.push(ScenarioPlan::new(Fixture::SyntheticXml(ledger_xml())));
            plans.push(candidate_plan);
        }
        plans.push(ScenarioPlan::new(Fixture::SyntheticXml(company_xml())));
        plans.push(ScenarioPlan::new(Fixture::SyntheticXml(ledger_xml())));
        let simulator = SequenceSimulator::spawn(plans).unwrap();
        let loaded = loaded(simulator.address().port());
        let preflight = confirm_preflight_challenge(&loaded, "PREFLIGHT test").unwrap();
        let ready = match loaded.run_preflight(preflight).await {
            Ok(ready) => ready,
            Err(failure) => {
                simulator.cancel();
                let _ = simulator.finish();
                return Err(failure.safe_code());
            }
        };
        let dispatch_phrase = ready.dispatch_challenge().to_string();
        let dispatch = confirm_dispatch_challenge(&ready, &dispatch_phrase).unwrap();
        let pending = match ready.dispatch(dispatch).await {
            Ok(pending) => pending,
            Err(failure) => {
                simulator.cancel();
                let _ = simulator.finish();
                return Err(failure.safe_code());
            }
        };
        assert_eq!(pending.snapshots.len(), 4);
        assert_eq!(
            pending.byte_repeatability,
            ByteRepeatability::NotEstablished
        );
        assert_eq!(
            pending.attempts[0].outcome,
            CandidateAttemptOutcome::ResponseObserved
        );
        assert_eq!(
            pending.attempts[0].application_status,
            ApplicationStatus::Failure
        );
        assert_eq!(
            pending.attempts[0].safe_reason_code.as_deref(),
            Some("tally_application_failure")
        );
        assert_eq!(
            pending.attempts[1].outcome,
            CandidateAttemptOutcome::HttpRejected
        );
        assert_eq!(pending.attempts[1].http_status, Some(503));
        assert_eq!(
            pending.attempts[1].safe_reason_code.as_deref(),
            Some("http_status_failure")
        );
        assert_eq!(
            pending.attempts[2].application_status,
            ApplicationStatus::Unrecognized
        );
        assert_eq!(
            pending.attempts[2].safe_reason_code.as_deref(),
            Some("tally_application_status_unrecognized")
        );

        let observed = simulator.finish().unwrap();
        if observed.len() != 13
            || !observed.iter().all(|request| request.request_processed)
            || [4_usize, 7, 10]
                .into_iter()
                .any(|index| observed[index].request_body_sha256 != candidate().request_sha256())
        {
            return Err("simulator_failure_sequence_incomplete");
        }
        Ok(())
    }

    #[tokio::test]
    async fn candidate_failures_are_receipted_and_each_gets_a_trailing_identity_bracket() {
        let mut last_failure = "simulator_not_started";
        for _ in 0..5 {
            match run_candidate_failure_sequence_once().await {
                Ok(()) => return,
                Err(failure) => last_failure = failure,
            }
        }
        panic!("failure sequence simulator remained unstable: {last_failure}");
    }

    #[test]
    fn customer_data_and_placeholder_identity_configs_fail_closed() {
        let mut value = config(9000);
        value.no_customer_data_attested = false;
        assert_eq!(
            validate_config(&value).unwrap_err().safe_code(),
            "native_probe_config_invalid"
        );
        value.no_customer_data_attested = true;
        value.expected_party_identity_sha256 = "0".repeat(64);
        assert_eq!(
            validate_config(&value).unwrap_err().safe_code(),
            "native_probe_config_invalid"
        );
    }

    #[test]
    fn consent_tokens_are_distinct_consumed_types_and_expire() {
        let loaded = loaded(9000);
        assert!(confirm_preflight_challenge(&loaded, "wrong").is_err());
        assert!(verify_consumed_consent("x", 1, "x", 1).is_err());
        assert_ne!(
            std::any::type_name::<PreflightConsent>(),
            std::any::type_name::<DispatchConsent>()
        );
        assert_ne!(
            std::any::type_name::<DispatchConsent>(),
            std::any::type_name::<UiAfterConsent>()
        );
    }

    #[test]
    fn ui_rows_and_settlement_presence_are_typed_bounded_and_scenario_specific() {
        let run = loaded(9000);
        let now = now_unix_ms().unwrap() + 1;
        validate_ui(&run.ui_before, "before", &run.fixture, &run.scenario, now).unwrap();

        let mut non_contiguous = run.ui_before.clone();
        non_contiguous.ordered_projection[0].row_index = 1;
        assert_eq!(
            validate_ui(&non_contiguous, "before", &run.fixture, &run.scenario, now,)
                .unwrap_err()
                .safe_code(),
            "native_probe_ui_projection_invalid"
        );

        let mut present_required = run.scenario.clone();
        present_required.inv_001_ui_presence_requirement =
            UiPresenceRequirement::MustBeObservedPresent;
        let mut omitted = run.ui_before.clone();
        omitted.inv_001_settled_reference_observation = SettledReferenceObservation::Omitted;
        assert!(validate_ui(&omitted, "before", &run.fixture, &present_required, now,).is_err());

        let mut unknown_row_field = serde_json::to_value(&run.ui_before).unwrap();
        unknown_row_field["ordered_projection"][0]["invented"] = Value::Bool(true);
        assert!(serde_json::from_value::<UiObservation>(unknown_row_field).is_err());

        let mut invented_settlement = serde_json::to_value(&run.ui_before).unwrap();
        invented_settlement["inv_001_settled_reference_observation"] =
            Value::String("looks_settled".into());
        assert!(serde_json::from_value::<UiObservation>(invented_settlement).is_err());
    }

    #[test]
    fn native_receipt_output_target_is_repository_confined_and_path_bound() {
        let directory = std::env::temp_dir().join(format!(
            "bridge-native-probe-output-test-{}-{}",
            std::process::id(),
            now_unix_ms().unwrap()
        ));
        let local = directory.join(".bridge-live");
        std::fs::create_dir_all(&local).unwrap();
        let mut run = loaded(9000);
        run.repository_root = directory.clone();
        let first = run
            .validate_receipt_output(&local.join("first.json"))
            .unwrap();
        let second = run
            .validate_receipt_output(&local.join("second.json"))
            .unwrap();
        assert_ne!(
            native_probe_output_binding(&first).unwrap(),
            native_probe_output_binding(&second).unwrap()
        );
        assert!(run
            .validate_receipt_output(&directory.join("outside.json"))
            .is_err());
        assert!(run
            .validate_receipt_output(&local.join("receipt.txt"))
            .is_err());
        first.revalidate().unwrap();
        std::fs::remove_dir(local).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }
}
