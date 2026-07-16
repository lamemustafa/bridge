use crate::db::tally_incremental::IncrementalFoundationEvidence;
use crate::db::tally_mirror::{
    company_profile_correlation_key, selected_read_scope_commitment_sha256, CapabilityItemInput,
    CapabilityKind as MirrorCapabilityKind, CapabilitySnapshotInput,
    CapabilityState as MirrorCapabilityState, Confidence, FreshnessState,
    LocalReconciliationMismatch, ProofSummary, RedactedProofExport, ReviewedSetupInput,
    SelectedReadObservationCommitmentMaterial, SelectedReadObservationInput,
    SelectedReadScopeCommitmentMaterial, SelectedReadScopeInput, SourceIdentityInput,
    TallyMirrorRepository,
};
use crate::gst::{GstDraftRequest, GstReturnDraft};
use crate::sync::coordinator::{SnapshotCoordinator, SnapshotJobStatus};
use crate::sync::reconciliation::ExternalReferenceCatalog;
use crate::sync::snapshot::{
    capability_profile_sha256, AdaptiveWindowPolicy, PlannedWindow, SnapshotPlan,
    SqliteSnapshotStateStore,
};
use crate::tally::runtime::TallyRuntimeControlError;
use crate::tally::validators::{
    normalize_company_guid, validate_company_name, validate_date_range,
};
use crate::tally::{
    company_source_identity, source_lineage, CachedProbeReservation, ConnectionStatus, EndpointKey,
    RuntimeTallyConnector, SelectedReadObservation, SelectedReadScopeEvidence, TallyCompany,
    TallyConfig, TallyLedger, TallyRuntime, TallySessionSnapshot, TallyTelemetryPreviewExport,
    TallyVoucher, SELECTED_LEDGER_QUERY_PROFILE_ID, SELECTED_VOUCHER_QUERY_PROFILE_ID,
};
use bridge_tally_core::{
    CapabilityEvidence, CapabilityFeatureId, CapabilityPackId, CapabilityState,
    CompanyRef as CoreCompanyRef, EvidenceConfidence, ReadWindow, RequestContext, TallyConnector,
    TransportId, CORE_ACCOUNTING_SCHEMA_VERSION,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::State;
use zeroize::Zeroizing;

const MAX_DSC_PIN_BYTES: usize = 128;

#[derive(Debug, Serialize)]
pub struct TallyCommandError {
    pub code: &'static str,
    pub category: &'static str,
    pub message: String,
    pub retry: &'static str,
    pub local_state_changed: bool,
    pub tally_state_may_have_changed: bool,
    pub remediation: &'static str,
}

fn tally_command_error(
    code: &'static str,
    category: &'static str,
    message: impl Into<String>,
    retry: &'static str,
    local_state_changed: bool,
    remediation: &'static str,
) -> TallyCommandError {
    TallyCommandError {
        code,
        category,
        message: message.into(),
        retry,
        local_state_changed,
        tally_state_may_have_changed: false,
        remediation,
    }
}

fn tally_runtime_command_error(error: anyhow::Error) -> TallyCommandError {
    if let Some(control) = error.downcast_ref::<TallyRuntimeControlError>() {
        return match control {
            TallyRuntimeControlError::Cancelled => tally_command_error(
                "request_cancelled",
                "Operation",
                "The read-only Tally request was cancelled.",
                "safe",
                true,
                "Refresh the scoped run or runtime status before starting another request.",
            ),
            TallyRuntimeControlError::QueueDeadline => tally_command_error(
                "tally_runtime_temporarily_unavailable",
                "Operation",
                "The local Tally request queue deadline was exceeded.",
                "safe",
                true,
                "Refresh runtime status before retrying; the failed queue operation was recorded in local runtime health.",
            ),
            TallyRuntimeControlError::CircuitCooldown
            | TallyRuntimeControlError::HalfOpenProbeInFlight
            | TallyRuntimeControlError::EndpointSessionCapacity => tally_command_error(
                "tally_runtime_temporarily_unavailable",
                "Operation",
                "The local Tally request runtime is temporarily unavailable.",
                "safe",
                false,
                "Wait for active requests or the circuit retry time, then refresh runtime status.",
            ),
        };
    }
    let lower = error.to_string().to_ascii_lowercase();
    let (code, category, message, retry, local_state_changed, remediation) = if lower
        .contains("cancel")
    {
        (
            "request_cancelled",
            "Operation",
            "The read-only Tally request was cancelled.",
            "safe",
            true,
            "Refresh the scoped run or runtime status before starting another request.",
        )
    } else if lower.contains("host")
        || lower.contains("port")
        || lower.contains("loopback")
        || lower.contains("endpoint") && lower.contains("invalid")
    {
        (
            "endpoint_configuration_invalid",
            "Endpoint configuration",
            "The local Tally endpoint configuration is invalid.",
            "after_change",
            false,
            "Use localhost or a loopback IP and a port from 1 to 65535, then probe again.",
        )
    } else if lower.contains("parse")
        || lower.contains("xml")
        || lower.contains("decode")
        || lower.contains("schema")
        || lower.contains("response exceeded")
    {
        (
            "response_validation_failed",
            "Response validation",
            "The Tally response did not satisfy Bridge's bounded protocol contract.",
            "after_change",
            true,
            "Keep the result unverified and inspect redacted diagnostics before retrying.",
        )
    } else if lower.contains("company") {
        (
            "tally_company_context_failed",
            "Tally application",
            "Tally did not confirm the selected company context.",
            "after_change",
            true,
            "Load the intended company in Tally, probe again, and reselect its observed identity.",
        )
    } else if lower.contains("queue deadline") {
        (
            "tally_runtime_temporarily_unavailable",
            "Operation",
            "The local Tally request queue deadline was exceeded.",
            "safe",
            true,
            "Refresh runtime status before retrying; the failed queue operation was recorded in local runtime health.",
        )
    } else if lower.contains("capacity")
        || lower.contains("circuit")
        || lower.contains("registry")
        || lower.contains("cache")
    {
        (
            "tally_runtime_temporarily_unavailable",
            "Operation",
            "The local Tally request runtime is temporarily unavailable.",
            "safe",
            false,
            "Wait for active requests or the circuit retry time, then refresh runtime status.",
        )
    } else {
        (
            "endpoint_unreachable",
            "Endpoint configuration",
            "The local Tally endpoint could not complete the read-only request.",
            "after_change",
            true,
            "Confirm Tally is running with the XML server enabled, then probe the loopback endpoint again.",
        )
    };
    TallyCommandError {
        code,
        category,
        message: message.to_string(),
        retry,
        local_state_changed,
        tally_state_may_have_changed: false,
        remediation,
    }
}

#[tauri::command]
pub async fn check_tally_connection(
    config: TallyConfig,
    runtime: State<'_, TallyRuntime>,
) -> Result<ConnectionStatus, TallyCommandError> {
    runtime
        .check_connection(config)
        .await
        .map_err(tally_runtime_command_error)
}

#[tauri::command]
pub async fn probe_tally(
    config: TallyConfig,
    runtime: State<'_, TallyRuntime>,
) -> Result<PersistedTallyProbeResult, TallyCommandError> {
    let canonical_origin = EndpointKey::from_config(&config)
        .map(|endpoint| endpoint.as_str().to_string())
        .map_err(|_| {
            tally_command_error(
                "endpoint_configuration_invalid",
                "Endpoint configuration",
                "Tally endpoint validation failed",
                "after_change",
                false,
                "Use localhost or a loopback IP and a port from 1 to 65535, then probe again.",
            )
        })?;
    let (review_id, observed_at_unix_ms, probe) = runtime
        .probe_with_observation(config)
        .await
        .map_err(tally_runtime_command_error)?;
    let profile_sha256 = capability_profile_sha256(&probe.profile).map_err(|_| {
        tally_command_error(
            "capability_profile_commitment_failed",
            "Operation",
            "The observed Capability Passport could not be committed for review.",
            "safe",
            false,
            "Probe again before selecting and saving a company scope.",
        )
    })?;
    let review_commitment_sha256 = reviewed_probe_commitment_sha256(
        &review_id,
        &canonical_origin,
        observed_at_unix_ms,
        &probe,
    )
    .map_err(|_| {
        tally_command_error(
            "reviewed_probe_commitment_failed",
            "Operation",
            "The exact endpoint, Passport, and company scope could not be committed for review.",
            "safe",
            false,
            "Probe again before selecting and saving a company scope.",
        )
    })?;
    let mut companies = Vec::with_capacity(probe.companies.len());
    for company in probe.companies {
        let identity_confidence = if company
            .guid
            .as_deref()
            .is_some_and(|guid| !guid.trim().is_empty())
        {
            "observed"
        } else {
            "unknown"
        };
        let correlation_key = company
            .guid
            .as_deref()
            .map(|guid| company_profile_correlation_key(&canonical_origin, guid));
        companies.push(PersistedTallyCompany {
            name: company.name,
            guid: company.guid,
            mirror_company_id: None,
            correlation_key,
            identity_confidence,
        });
    }
    Ok(PersistedTallyProbeResult {
        review_id,
        canonical_origin,
        observed_at_unix_ms,
        connection: probe.connection,
        companies,
        profile: probe.profile,
        selected_read_scope: probe.selected_read_scope,
        profile_sha256,
        review_commitment_sha256,
        passport_snapshot_id: None,
    })
}

#[derive(Debug, Deserialize)]
pub struct QualifySelectedReadsRequest {
    pub config: TallyConfig,
    pub expected_review_id: String,
    pub expected_review_commitment_sha256: String,
    pub selected_company_guid: String,
    pub voucher_from_yyyymmdd: String,
    pub voucher_to_yyyymmdd: String,
}

#[derive(Debug, Serialize)]
pub struct SelectedReadQualificationResult {
    pub review_id: String,
    pub observed_at_unix_ms: i64,
    pub profile: bridge_tally_core::CapabilityProfile,
    pub profile_sha256: String,
    pub review_commitment_sha256: String,
    pub selected_read_scope: SelectedReadScopeEvidence,
    pub no_writes_attempted: bool,
    pub raw_records_retained: bool,
    pub completeness_claimed: bool,
}

#[tauri::command]
pub async fn qualify_selected_tally_reads(
    request: QualifySelectedReadsRequest,
    runtime: State<'_, TallyRuntime>,
) -> Result<SelectedReadQualificationResult, TallyCommandError> {
    validate_date_range(&request.voucher_from_yyyymmdd, &request.voucher_to_yyyymmdd).map_err(
        |message| {
            tally_command_error(
                "selected_read_window_invalid",
                "Endpoint configuration",
                message,
                "after_change",
                false,
                "Choose a valid inclusive voucher window and qualify again.",
            )
        },
    )?;
    let from_date = chrono::NaiveDate::parse_from_str(&request.voucher_from_yyyymmdd, "%Y%m%d")
        .map_err(|_| selected_read_window_too_large_error())?;
    let to_date = chrono::NaiveDate::parse_from_str(&request.voucher_to_yyyymmdd, "%Y%m%d")
        .map_err(|_| selected_read_window_too_large_error())?;
    if (to_date - from_date).num_days() > 30 {
        return Err(selected_read_window_too_large_error());
    }
    let canonical_origin = EndpointKey::from_config(&request.config)
        .map(|endpoint| endpoint.as_str().to_string())
        .map_err(|_| {
            tally_command_error(
                "endpoint_configuration_invalid",
                "Endpoint configuration",
                "Tally endpoint validation failed",
                "after_change",
                false,
                "Use localhost or a loopback IP and a valid port, then probe again.",
            )
        })?;
    let mut reservation = runtime
        .reserve_cached_probe_fresh(
            &request.config,
            &request.expected_review_id,
            SETUP_PROBE_MAX_AGE_MS,
        )
        .map_err(tally_runtime_command_error)?
        .ok_or_else(reviewed_probe_expired_error)?;
    let parent_observed_at_unix_ms = reservation.observed_at_unix_ms();
    let mut probe = reservation.result().clone();

    let parent_commitment = match reviewed_probe_commitment_sha256(
        &request.expected_review_id,
        &canonical_origin,
        parent_observed_at_unix_ms,
        &probe,
    ) {
        Ok(commitment) => commitment,
        Err(_) => return Err(reviewed_probe_changed_error()),
    };
    if parent_commitment != request.expected_review_commitment_sha256 {
        return Err(reviewed_probe_changed_error());
    }
    let selected_guid = match normalize_company_guid(&request.selected_company_guid) {
        Ok(guid) => guid,
        Err(_) => {
            return Err(tally_command_error(
                "stable_company_identity_required",
                "Tally application",
                "The selected company does not have a safe observed GUID.",
                "after_change",
                false,
                "Select one GUID-bearing company from the current probe.",
            ));
        }
    };
    let matching_companies = probe
        .companies
        .iter()
        .filter(|company| {
            company
                .guid
                .as_deref()
                .is_some_and(|guid| guid.eq_ignore_ascii_case(&selected_guid))
        })
        .cloned()
        .collect::<Vec<_>>();
    let [company] = matching_companies.as_slice() else {
        return Err(tally_command_error(
            "reviewed_company_scope_changed",
            "Tally application",
            "The selected company is absent or ambiguous in the reviewed probe.",
            "safe",
            false,
            "Probe again and select one company from the replacement result.",
        ));
    };
    let Some(observed_guid) = company.guid.clone() else {
        return Err(reviewed_probe_changed_error());
    };

    let ledger_result = runtime
        .qualify_selected_ledgers(
            request.config.clone(),
            &reservation,
            company.name.clone(),
            observed_guid.clone(),
        )
        .await;
    let ledger_result = match ledger_result {
        Err(error) if selected_read_cancelled(&error) => {
            return Err(tally_runtime_command_error(error));
        }
        result => result,
    };
    if ledger_result
        .as_ref()
        .is_err_and(selected_read_identity_failure)
    {
        consume_selected_read_reservation(&mut reservation)?;
        return Err(selected_read_company_context_error());
    }
    let ledger_observation = selected_read_observation(
        "selected_ledger_read",
        ledger_result,
        false,
        "selected_ledger_read_empty_observed",
        "selected_ledger_read_non_empty_observed",
    );
    let voucher_observation = if ledger_observation.state == CapabilityState::Supported {
        let result = runtime
            .qualify_selected_vouchers(
                request.config.clone(),
                &reservation,
                company.name.clone(),
                observed_guid.clone(),
                request.voucher_from_yyyymmdd.clone(),
                request.voucher_to_yyyymmdd.clone(),
            )
            .await;
        let result = match result {
            Err(error) if selected_read_cancelled(&error) => {
                return Err(tally_runtime_command_error(error));
            }
            result => result,
        };
        if result.as_ref().is_err_and(selected_read_identity_failure) {
            consume_selected_read_reservation(&mut reservation)?;
            return Err(selected_read_company_context_error());
        }
        selected_read_observation(
            "selected_voucher_window_read",
            result,
            true,
            "selected_voucher_window_empty_observed",
            "selected_voucher_window_non_empty_observed",
        )
    } else {
        crate::tally::connection::SelectedReadCapabilityObservation {
            capability_key: "selected_voucher_window_read",
            state: CapabilityState::Unknown,
            confidence: EvidenceConfidence::Unknown,
            safe_reason_code: "qualification_prerequisite_failed",
            result_bucket: "skipped",
            request_sha256: None,
            decoded_response_sha256: None,
            response_encoding: None,
            company_context_verified: false,
            schema_verified: false,
            record_count_verified: false,
            identity_evidence_state: "unverified",
            date_window_verified: false,
        }
    };
    let observed_at_unix_ms = chrono::Utc::now().timestamp_millis();
    let observations = vec![ledger_observation, voucher_observation];
    let commitment_observations = observations
        .iter()
        .map(|observation| SelectedReadObservationCommitmentMaterial {
            capability_key: observation.capability_key.to_string(),
            state: capability_state_label(observation.state).to_string(),
            confidence: evidence_confidence_label(observation.confidence).to_string(),
            safe_reason_code: observation.safe_reason_code.to_string(),
            result_bucket: observation.result_bucket.to_string(),
            request_sha256: observation.request_sha256.clone(),
            decoded_response_sha256: observation.decoded_response_sha256.clone(),
            response_encoding: observation.response_encoding.map(str::to_string),
            company_context_verified: observation.company_context_verified,
            schema_verified: observation.schema_verified,
            record_count_verified: observation.record_count_verified,
            identity_evidence_state: observation.identity_evidence_state.to_string(),
            date_window_verified: observation.date_window_verified,
        })
        .collect::<Vec<_>>();
    let casefolded_guid = observed_guid.to_ascii_lowercase();
    let scope_commitment_sha256 =
        match selected_read_scope_commitment_sha256(&SelectedReadScopeCommitmentMaterial {
            parent_review_commitment_sha256: parent_commitment.clone(),
            canonical_origin: canonical_origin.clone(),
            company_guid_ascii_casefolded: casefolded_guid.clone(),
            company_name: company.name.clone(),
            ledger_profile_id: SELECTED_LEDGER_QUERY_PROFILE_ID.to_string(),
            voucher_profile_id: SELECTED_VOUCHER_QUERY_PROFILE_ID.to_string(),
            voucher_from_yyyymmdd: request.voucher_from_yyyymmdd.clone(),
            voucher_to_yyyymmdd: request.voucher_to_yyyymmdd.clone(),
            observed_at_unix_ms,
            observations: commitment_observations,
        }) {
            Ok(commitment) => commitment,
            Err(_) => {
                let _ = reservation.consume();
                return Err(selected_read_review_state_uncertain_error());
            }
        };
    for observation in &observations {
        probe.profile.features.insert(
            if observation.capability_key == "selected_ledger_read" {
                CapabilityFeatureId::SelectedLedgerRead
            } else {
                CapabilityFeatureId::SelectedVoucherWindowRead
            },
            CapabilityEvidence {
                state: observation.state,
                confidence: observation.confidence,
                safe_reason_code: Some(observation.safe_reason_code.to_string()),
            },
        );
    }
    probe.profile.profile_version = 3;
    let selected_read_scope = SelectedReadScopeEvidence {
        scope_version: 1,
        ledger_profile_id: SELECTED_LEDGER_QUERY_PROFILE_ID.to_string(),
        voucher_profile_id: SELECTED_VOUCHER_QUERY_PROFILE_ID.to_string(),
        voucher_from_yyyymmdd: request.voucher_from_yyyymmdd.clone(),
        voucher_to_yyyymmdd: request.voucher_to_yyyymmdd.clone(),
        scope_commitment_sha256,
        parent_review_sha256: parent_commitment,
        company_guid_ascii_casefolded: casefolded_guid,
        observations,
    };
    probe.selected_read_scope = Some(selected_read_scope.clone());
    let replacement_review_id = uuid::Uuid::new_v4().to_string();
    let profile_sha256 = match capability_profile_sha256(&probe.profile) {
        Ok(hash) => hash,
        Err(_) => {
            let _ = reservation.consume();
            return Err(selected_read_review_state_uncertain_error());
        }
    };
    let review_commitment_sha256 = match reviewed_probe_commitment_sha256(
        &replacement_review_id,
        &canonical_origin,
        observed_at_unix_ms,
        &probe,
    ) {
        Ok(commitment) => commitment,
        Err(_) => {
            let _ = reservation.consume();
            return Err(selected_read_review_state_uncertain_error());
        }
    };
    let replaced = match reservation.replace(
        replacement_review_id.clone(),
        observed_at_unix_ms,
        probe.clone(),
    ) {
        Ok(replaced) => replaced,
        Err(_) => return Err(selected_read_review_state_uncertain_error()),
    };
    if !replaced {
        return Err(selected_read_review_state_uncertain_error());
    }
    Ok(SelectedReadQualificationResult {
        review_id: replacement_review_id,
        observed_at_unix_ms,
        profile: probe.profile,
        profile_sha256,
        review_commitment_sha256,
        selected_read_scope,
        no_writes_attempted: true,
        raw_records_retained: false,
        completeness_claimed: false,
    })
}

fn selected_read_observation(
    capability_key: &'static str,
    result: anyhow::Result<SelectedReadObservation>,
    date_window: bool,
    empty_reason: &'static str,
    non_empty_reason: &'static str,
) -> crate::tally::connection::SelectedReadCapabilityObservation {
    match result {
        Ok(observed) => crate::tally::connection::SelectedReadCapabilityObservation {
            capability_key,
            state: CapabilityState::Supported,
            confidence: EvidenceConfidence::Observed,
            safe_reason_code: if observed.result_bucket == "empty_observed" {
                empty_reason
            } else {
                non_empty_reason
            },
            result_bucket: observed.result_bucket,
            request_sha256: Some(observed.request_sha256),
            decoded_response_sha256: Some(observed.decoded_response_sha256),
            response_encoding: Some(observed.response_encoding),
            company_context_verified: true,
            schema_verified: true,
            record_count_verified: true,
            identity_evidence_state: if observed.result_bucket == "empty_observed" {
                "not_applicable_empty"
            } else {
                "verified"
            },
            date_window_verified: date_window,
        },
        Err(error) => crate::tally::connection::SelectedReadCapabilityObservation {
            capability_key,
            state: CapabilityState::Unknown,
            confidence: EvidenceConfidence::Observed,
            safe_reason_code: selected_read_failure_reason(&error, date_window),
            result_bucket: "rejected",
            request_sha256: None,
            decoded_response_sha256: None,
            response_encoding: None,
            company_context_verified: false,
            schema_verified: false,
            record_count_verified: false,
            identity_evidence_state: "unverified",
            date_window_verified: false,
        },
    }
}

fn selected_read_identity_failure(error: &anyhow::Error) -> bool {
    let message = error.to_string().to_ascii_lowercase();
    message.contains("company") && (message.contains("context") || message.contains("identity"))
}

fn selected_read_cancelled(error: &anyhow::Error) -> bool {
    matches!(
        error.downcast_ref::<TallyRuntimeControlError>(),
        Some(TallyRuntimeControlError::Cancelled)
    )
}

fn consume_selected_read_reservation(
    reservation: &mut CachedProbeReservation,
) -> Result<(), TallyCommandError> {
    match reservation.consume() {
        Ok(true) => Ok(()),
        Ok(false) | Err(_) => Err(selected_read_review_state_uncertain_error()),
    }
}

fn selected_read_failure_reason(error: &anyhow::Error, voucher: bool) -> &'static str {
    let message = error.to_string();
    if voucher && message.contains("voucher_date_outside_requested_window") {
        "selected_voucher_date_outside_window"
    } else if message.contains("stable") || message.contains("identity") {
        "selected_read_identity_unavailable"
    } else if message.contains("schema") || message.contains("structural") {
        "selected_read_schema_rejected"
    } else {
        "selected_read_transport_or_validation_failed"
    }
}

fn capability_state_label(state: CapabilityState) -> &'static str {
    match state {
        CapabilityState::Supported => "supported",
        CapabilityState::Unsupported => "unsupported",
        CapabilityState::Unknown => "unknown",
        CapabilityState::NotConfigured => "not_configured",
    }
}

fn evidence_confidence_label(confidence: EvidenceConfidence) -> &'static str {
    match confidence {
        EvidenceConfidence::Documented => "documented",
        EvidenceConfidence::Observed => "observed",
        EvidenceConfidence::Inferred => "inferred",
        EvidenceConfidence::Unknown => "unknown",
    }
}

fn selected_read_window_too_large_error() -> TallyCommandError {
    tally_command_error(
        "selected_read_window_invalid",
        "Endpoint configuration",
        "Selected-read qualification is limited to one inclusive 31-day voucher window.",
        "after_change",
        false,
        "Choose a valid window of 31 days or fewer.",
    )
}

fn reviewed_probe_expired_error() -> TallyCommandError {
    tally_command_error(
        "reviewed_probe_expired",
        "Operation",
        "The reviewed Capability Passport is missing, busy, or older than five minutes.",
        "safe",
        false,
        "Probe again and review the exact company scope before qualifying.",
    )
}

fn reviewed_probe_changed_error() -> TallyCommandError {
    tally_command_error(
        "reviewed_probe_changed",
        "Operation",
        "The reviewed Capability Passport no longer matches the cached observation.",
        "safe",
        false,
        "Probe again and review the replacement Passport before qualifying.",
    )
}

fn selected_read_company_context_error() -> TallyCommandError {
    tally_command_error(
        "selected_read_company_context_changed",
        "Tally application",
        "A selected read did not prove the exact reviewed company context.",
        "after_change",
        true,
        "Stop using this review, verify the loaded Tally company, and probe again.",
    )
}

fn selected_read_review_state_uncertain_error() -> TallyCommandError {
    tally_command_error(
        "selected_read_review_state_uncertain",
        "Operation",
        "The read-only qualification finished, but its reviewed state could not be installed.",
        "after_change",
        true,
        "Probe again before qualifying or saving any company scope.",
    )
}

const SETUP_PROBE_MAX_AGE_MS: i64 = 5 * 60 * 1_000;

#[derive(Debug, Deserialize)]
pub struct SaveTallySetupRequest {
    pub config: TallyConfig,
    pub expected_review_id: String,
    pub expected_review_commitment_sha256: String,
    pub selected_company_guid: String,
}

#[derive(Debug, Serialize)]
pub struct SavedTallySetup {
    pub passport_snapshot_id: String,
    pub canonical_origin: String,
    pub observed_at_unix_ms: i64,
    pub company: PersistedTallyCompany,
    pub review_cleanup_warning: Option<&'static str>,
}

#[tauri::command]
pub async fn save_tally_setup(
    request: SaveTallySetupRequest,
    mirror: State<'_, TallyMirrorRepository>,
    runtime: State<'_, TallyRuntime>,
) -> Result<SavedTallySetup, TallyCommandError> {
    let canonical_origin = EndpointKey::from_config(&request.config)
        .map(|endpoint| endpoint.as_str().to_string())
        .map_err(|_| {
            tally_command_error(
                "endpoint_configuration_invalid",
                "Endpoint configuration",
                "Tally endpoint validation failed",
                "after_change",
                false,
                "Use localhost or a loopback IP and a port from 1 to 65535, then probe again.",
            )
        })?;
    let mut reservation = runtime
        .reserve_cached_probe_fresh(
            &request.config,
            &request.expected_review_id,
            SETUP_PROBE_MAX_AGE_MS,
        )
        .map_err(tally_runtime_command_error)?
        .ok_or_else(|| {
            tally_command_error(
                "reviewed_probe_expired",
                "Operation",
                "The reviewed Capability Passport is missing or older than five minutes.",
                "safe",
                false,
                "Probe again, review the exact Passport and company scope, then save.",
            )
        })?;
    let observed_at_unix_ms = reservation.observed_at_unix_ms();
    let probe = reservation.result().clone();
    let save_result: Result<SavedTallySetup, TallyCommandError> = async {
        let actual_review_commitment_sha256 = reviewed_probe_commitment_sha256(
            &request.expected_review_id,
            &canonical_origin,
            observed_at_unix_ms,
            &probe,
        )
        .map_err(
            |_| {
                tally_command_error(
                    "reviewed_probe_commitment_failed",
                    "Operation",
                    "The cached endpoint, Passport, and company scope could not be verified.",
                    "safe",
                    false,
                    "Probe again before selecting and saving a company scope.",
                )
            },
        )?;
        if request.expected_review_commitment_sha256 != actual_review_commitment_sha256 {
            return Err(tally_command_error(
                "reviewed_probe_changed",
                "Operation",
                "The reviewed Capability Passport no longer matches the cached probe.",
                "safe",
                false,
                "Probe again and review the replacement Passport before saving.",
            ));
        }
        let selected_guid = normalize_company_guid(&request.selected_company_guid).map_err(|_| {
            tally_command_error(
                "stable_company_identity_required",
                "Tally application",
                "The selected company does not have an observed stable GUID.",
                "after_change",
                false,
                "Select a GUID-bearing company from the current probe.",
            )
        })?;
        let mut matches = probe.companies.iter().filter(|company| {
            company
                .guid
                .as_deref()
                .is_some_and(|guid| guid.eq_ignore_ascii_case(&selected_guid))
        });
        let company = matches.next().cloned().ok_or_else(|| {
            tally_command_error(
                "reviewed_company_scope_changed",
                "Tally application",
                "The selected company is not present in the reviewed probe.",
                "safe",
                false,
                "Probe again and select a company from the current result.",
            )
        })?;
        if matches.next().is_some() {
            return Err(tally_command_error(
                "company_identity_ambiguous",
                "Tally application",
                "The reviewed probe returned the selected GUID more than once.",
                "not_recommended",
                false,
                "Do not save this scope; inspect the synthetic or source company identities.",
            ));
        }
        if probe.selected_read_scope.as_ref().is_some_and(|scope| {
            !company.guid.as_deref().is_some_and(|guid| {
                guid.to_ascii_lowercase() == scope.company_guid_ascii_casefolded
            })
        }) {
            return Err(tally_command_error(
                "qualified_company_scope_changed",
                "Tally application",
                "The selected company does not match the qualified read scope.",
                "after_change",
                false,
                "Select the qualified company or probe and qualify the replacement company.",
            ));
        }

        let saved = mirror
            .save_reviewed_setup(ReviewedSetupInput {
                review_commitment_sha256: request.expected_review_commitment_sha256.clone(),
                capability: CapabilitySnapshotInput {
                    canonical_origin: canonical_origin.clone(),
                    observed_at_unix_ms,
                    profile_version: probe.profile.profile_version,
                    product: probe.profile.product.clone(),
                    release: probe.profile.release.clone(),
                    mode: probe.profile.mode.clone(),
                    mode_confidence: if probe.profile.mode.is_some() {
                        Confidence::Observed
                    } else {
                        Confidence::Unknown
                    },
                    items: capability_items(&probe.profile),
                },
                company_display_name: company.name.clone(),
                company_identity: SourceIdentityInput {
                    // Persist the spelling observed from Tally, not caller-controlled casing.
                    guid: company.guid.clone(),
                    confidence: Some(Confidence::Observed),
                    ..SourceIdentityInput::default()
                },
                selected_read_scope: probe.selected_read_scope.as_ref().map(|scope| {
                    SelectedReadScopeInput {
                        scope_commitment_sha256: scope.scope_commitment_sha256.clone(),
                        parent_review_sha256: scope.parent_review_sha256.clone(),
                        ledger_profile_id: scope.ledger_profile_id.clone(),
                        voucher_profile_id: scope.voucher_profile_id.clone(),
                        voucher_from_yyyymmdd: scope.voucher_from_yyyymmdd.clone(),
                        voucher_to_yyyymmdd: scope.voucher_to_yyyymmdd.clone(),
                        observed_at_unix_ms,
                        observations: scope
                            .observations
                            .iter()
                            .map(|observation| SelectedReadObservationInput {
                                capability_key: observation.capability_key.to_string(),
                                state: mirror_capability_state(observation.state),
                                confidence: mirror_confidence(observation.confidence),
                                safe_reason_code: observation.safe_reason_code.to_string(),
                                result_bucket: observation.result_bucket.to_string(),
                                request_sha256: observation.request_sha256.clone(),
                                decoded_response_sha256: observation
                                    .decoded_response_sha256
                                    .clone(),
                                response_encoding: observation
                                    .response_encoding
                                    .map(str::to_string),
                                company_context_verified: observation.company_context_verified,
                                schema_verified: observation.schema_verified,
                                record_count_verified: observation.record_count_verified,
                                identity_evidence_state: observation
                                    .identity_evidence_state
                                    .to_string(),
                                date_window_verified: observation.date_window_verified,
                            })
                            .collect(),
                    }
                }),
            })
            .await
            .map_err(|_| {
                tally_command_error(
                    "reviewed_setup_store_failed",
                    "Operation",
                    "The reviewed Passport and selected company scope could not be stored atomically.",
                    "after_change",
                    false,
                    "Verify encrypted storage, then retry this reviewed scope while it is fresh.",
                )
            })?;
        let correlation_key = company
            .guid
            .as_deref()
            .map(|guid| company_profile_correlation_key(&canonical_origin, guid));
        Ok(SavedTallySetup {
            passport_snapshot_id: saved.snapshot.id,
            canonical_origin,
            observed_at_unix_ms,
            company: PersistedTallyCompany {
                name: company.name,
                correlation_key,
                guid: company.guid,
                mirror_company_id: Some(saved.company.id),
                identity_confidence: "observed",
            },
            review_cleanup_warning: None,
        })
    }
    .await;
    let consume = save_result.is_ok();
    let cleanup_succeeded = if consume {
        reservation.consume().unwrap_or(false)
    } else {
        reservation.release().unwrap_or(false)
    };
    reconcile_review_cleanup(save_result, cleanup_succeeded)
}

fn reconcile_review_cleanup(
    save_result: Result<SavedTallySetup, TallyCommandError>,
    cleanup_succeeded: bool,
) -> Result<SavedTallySetup, TallyCommandError> {
    match save_result {
        Ok(mut saved) => {
            if !cleanup_succeeded {
                saved.review_cleanup_warning = Some("review_cache_cleanup_failed_after_save");
            }
            Ok(saved)
        }
        Err(_error) if !cleanup_succeeded => Err(tally_command_error(
            "reviewed_setup_retry_state_uncertain",
            "Operation",
            "The local setup was not stored, and the in-memory review reservation could not be released.",
            "after_change",
            true,
            "Restart Bridge, probe again, review the exact scope, and save again.",
        )),
        Err(error) => Err(error),
    }
}

#[derive(Debug, Serialize)]
pub struct PersistedTallyCompany {
    pub name: String,
    pub guid: Option<String>,
    pub mirror_company_id: Option<String>,
    pub correlation_key: Option<String>,
    pub identity_confidence: &'static str,
}

#[derive(Debug, Serialize)]
pub struct PersistedTallyProbeResult {
    pub review_id: String,
    pub canonical_origin: String,
    pub observed_at_unix_ms: i64,
    pub connection: ConnectionStatus,
    pub companies: Vec<PersistedTallyCompany>,
    pub profile: bridge_tally_core::CapabilityProfile,
    pub selected_read_scope: Option<SelectedReadScopeEvidence>,
    pub profile_sha256: String,
    pub review_commitment_sha256: String,
    pub passport_snapshot_id: Option<String>,
}

#[derive(Serialize)]
struct ReviewedProbeCommitment<'a> {
    schema: &'static str,
    review_id: &'a str,
    canonical_origin: &'a str,
    observed_at_unix_ms: i64,
    connection: &'a ConnectionStatus,
    companies: &'a [TallyCompany],
    profile: &'a bridge_tally_core::CapabilityProfile,
}

fn reviewed_probe_commitment_sha256(
    review_id: &str,
    canonical_origin: &str,
    observed_at_unix_ms: i64,
    probe: &crate::tally::TallyProbeResult,
) -> Result<String, serde_json::Error> {
    let bytes = serde_json::to_vec(&ReviewedProbeCommitment {
        schema: "bridge.tally.reviewed-setup-probe/1",
        review_id,
        canonical_origin,
        observed_at_unix_ms,
        connection: &probe.connection,
        companies: &probe.companies,
        profile: &probe.profile,
    })?;
    Ok(Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn capability_items(profile: &bridge_tally_core::CapabilityProfile) -> Vec<CapabilityItemInput> {
    let mut items = Vec::new();
    for (transport, evidence) in &profile.transports {
        items.push(CapabilityItemInput {
            kind: MirrorCapabilityKind::Transport,
            key: transport_key(*transport).to_string(),
            state: mirror_capability_state(evidence.state),
            confidence: mirror_confidence(evidence.confidence),
            safe_reason_code: evidence.safe_reason_code.clone(),
        });
    }
    for (pack, evidence) in &profile.packs {
        items.push(CapabilityItemInput {
            kind: MirrorCapabilityKind::Pack,
            key: pack_key(*pack).to_string(),
            state: mirror_capability_state(evidence.state),
            confidence: mirror_confidence(evidence.confidence),
            safe_reason_code: evidence.safe_reason_code.clone(),
        });
    }
    for (feature, evidence) in &profile.features {
        items.push(CapabilityItemInput {
            kind: MirrorCapabilityKind::Feature,
            key: feature_key(*feature).to_string(),
            state: mirror_capability_state(evidence.state),
            confidence: mirror_confidence(evidence.confidence),
            safe_reason_code: evidence.safe_reason_code.clone(),
        });
    }
    items
}

#[tauri::command]
pub async fn tally_persisted_company_profiles(
    mirror: State<'_, TallyMirrorRepository>,
) -> Result<crate::db::tally_mirror::PersistedCompanyProfilePage, String> {
    mirror
        .persisted_company_profiles()
        .await
        .map_err(|_| "persisted_tally_company_profiles_unavailable".to_string())
}

#[derive(Debug, Deserialize)]
pub struct TallyMirrorExplorerRequest {
    pub mirror_company_id: String,
    pub pack_id: String,
    pub offset: u32,
    pub limit: u32,
}

#[tauri::command]
pub async fn tally_mirror_explorer_page(
    request: TallyMirrorExplorerRequest,
    mirror: State<'_, TallyMirrorRepository>,
) -> Result<crate::db::tally_mirror::MirrorExplorerPage, String> {
    mirror
        .mirror_explorer_page(
            &request.mirror_company_id,
            &request.pack_id,
            request.offset,
            request.limit,
        )
        .await
        .map_err(|_| "tally_mirror_explorer_unavailable".to_string())
}

#[derive(Debug, Deserialize)]
pub struct TallyEvidenceRequest {
    pub mirror_company_id: String,
}

#[derive(Debug, Serialize)]
pub struct TallyFreshnessEvidence {
    pub state: &'static str,
    pub verified_at_unix_ms: Option<i64>,
    pub age_seconds: Option<i64>,
    pub checkpoint_present: bool,
    pub proof_present: bool,
}

#[derive(Debug, Serialize)]
pub struct TallyEvidenceResponse {
    pub latest_proofs: Vec<ProofSummary>,
    pub latest_reconciliation_mismatches: Vec<LocalReconciliationMismatch>,
    pub core_accounting_freshness: TallyFreshnessEvidence,
    pub incremental: IncrementalFoundationEvidence,
}

#[tauri::command]
pub async fn tally_sync_evidence(
    request: TallyEvidenceRequest,
    mirror: State<'_, TallyMirrorRepository>,
) -> Result<TallyEvidenceResponse, String> {
    if request.mirror_company_id.trim().is_empty() {
        return Err("Select a company with an observed stable identity".to_string());
    }
    mirror
        .snapshot_source_pin(&request.mirror_company_id)
        .await
        .map_err(|_| "The selected encrypted Tally company pin is unavailable".to_string())?;
    let freshness = mirror
        .freshness(
            &request.mirror_company_id,
            "core_accounting",
            chrono::Utc::now().timestamp_millis(),
        )
        .await
        .map_err(|_| "Encrypted Tally freshness evidence could not be read".to_string())?;
    let latest_proofs = mirror
        .latest_proofs(&request.mirror_company_id, 20)
        .await
        .map_err(|_| "Encrypted Tally proof evidence could not be read".to_string())?;
    let latest_reconciliation_mismatches = match latest_proofs.first() {
        Some(proof) => mirror
            .local_reconciliation_mismatches(
                &request.mirror_company_id,
                &proof.selection_token,
                chrono::Utc::now().timestamp_millis(),
            )
            .await
            .map_err(|_| {
                "The latest proof lacks a valid durable reconciliation receipt".to_string()
            })?,
        None => Vec::new(),
    };
    let incremental = mirror
        .incremental_foundation_evidence(&request.mirror_company_id)
        .await
        .map_err(|_| "Encrypted incremental evidence could not be read".to_string())?;
    let state = match freshness.state {
        FreshnessState::Fresh => "fresh",
        FreshnessState::Stale => "stale",
        FreshnessState::NeverVerified => "never_verified",
    };
    Ok(TallyEvidenceResponse {
        latest_proofs,
        latest_reconciliation_mismatches,
        incremental,
        core_accounting_freshness: TallyFreshnessEvidence {
            state,
            verified_at_unix_ms: freshness.verified_at_unix_ms,
            age_seconds: freshness.age_seconds,
            checkpoint_present: freshness.checkpoint_token.is_some(),
            proof_present: freshness.proof_id.is_some(),
        },
    })
}

#[derive(Debug, Deserialize)]
pub struct RedactedProofExportRequest {
    pub mirror_company_id: String,
    pub proof_id: String,
}

#[tauri::command]
pub async fn preview_tally_redacted_proof(
    request: RedactedProofExportRequest,
    mirror: State<'_, TallyMirrorRepository>,
) -> Result<RedactedProofExport, String> {
    if request.mirror_company_id.trim().is_empty() || request.proof_id.trim().is_empty() {
        return Err("Select a proof for an observed Tally company".to_string());
    }
    mirror
        .snapshot_source_pin(&request.mirror_company_id)
        .await
        .map_err(|_| "The selected encrypted Tally company pin is unavailable".to_string())?;
    mirror
        .redacted_proof_export(
            &request.mirror_company_id,
            &request.proof_id,
            chrono::Utc::now().timestamp_millis(),
        )
        .await
        .map_err(|_| {
            "The proof failed local integrity validation and cannot be exported".to_string()
        })
}

#[derive(Debug, Deserialize)]
pub struct StartCoreSnapshotRequest {
    pub config: TallyConfig,
    pub mirror_company_id: String,
    pub from: String,
    pub to: String,
}

fn first_calendar_day_canary_window(
    requested_from_yyyymmdd: &str,
) -> Result<PlannedWindow, String> {
    let first_day = chrono::NaiveDate::parse_from_str(requested_from_yyyymmdd, "%Y%m%d")
        .map_err(|_| "The requested snapshot start date is invalid".to_string())?;
    let first_day_yyyymmdd = first_day.format("%Y%m%d").to_string();
    if first_day_yyyymmdd != requested_from_yyyymmdd {
        return Err("The requested snapshot start date is invalid".to_string());
    }
    Ok(PlannedWindow::deterministic(
        CapabilityPackId::CoreAccounting,
        ReadWindow {
            from_yyyymmdd: first_day_yyyymmdd.clone(),
            to_yyyymmdd: first_day_yyyymmdd,
        },
    ))
}

#[tauri::command]
pub async fn start_tally_core_snapshot(
    request: StartCoreSnapshotRequest,
    mirror: State<'_, TallyMirrorRepository>,
    runtime: State<'_, TallyRuntime>,
    coordinator: State<'_, SnapshotCoordinator>,
) -> Result<SnapshotJobStatus, String> {
    validate_date_range(&request.from, &request.to)?;
    let pin = mirror
        .snapshot_source_pin(&request.mirror_company_id)
        .await
        .map_err(|_| "The selected encrypted Tally company pin is unavailable".to_string())?;
    validate_company_name(&pin.display_name)?;
    let request_origin = EndpointKey::from_config(&request.config)
        .map_err(|_| "Tally endpoint validation failed".to_string())?;
    if request_origin.as_str() != pin.canonical_origin {
        return Err("The selected company pin belongs to a different Tally endpoint".to_string());
    }

    let lineage = source_lineage(&request.config).map_err(|_| "Tally source lineage is invalid")?;
    let company = CoreCompanyRef {
        identity: company_source_identity(&lineage, &pin.company_guid),
        display_name: pin.display_name,
    };
    let run_id = uuid::Uuid::new_v4().to_string();
    let capability_canary_window = first_calendar_day_canary_window(&request.from)?;
    let planned = PlannedWindow::deterministic(
        CapabilityPackId::CoreAccounting,
        ReadWindow {
            from_yyyymmdd: request.from,
            to_yyyymmdd: request.to,
        },
    );
    let context = RequestContext {
        run_id: run_id.clone(),
        company: company.clone(),
        pack: CapabilityPackId::CoreAccounting,
        schema_version: CORE_ACCOUNTING_SCHEMA_VERSION,
        window: capability_canary_window.range.clone(),
        query_profile: capability_canary_window.query_profile.clone(),
        filters_sha256: capability_canary_window.filters_sha256.clone(),
    };
    let connector = RuntimeTallyConnector::new(
        runtime.inner().clone(),
        request.config,
        company.clone(),
        context,
    )
    .map_err(|_| "The Core Accounting snapshot profile is invalid".to_string())?;

    // Persist only the profile produced by the exact canary used for this run. A prior generic
    // endpoint probe intentionally cannot authorize a pack snapshot.
    let canary = connector
        .probe()
        .await
        .map_err(|_| "The read-only Core Accounting canary could not complete".to_string())?;
    if !canary.reachable
        || !canary
            .profile
            .transports
            .get(&TransportId::XmlHttp)
            .is_some_and(|evidence| {
                evidence.state == CapabilityState::Supported
                    && evidence.confidence == bridge_tally_core::EvidenceConfidence::Observed
            })
        || !canary
            .profile
            .packs
            .get(&CapabilityPackId::CoreAccounting)
            .is_some_and(|evidence| {
                evidence.state == CapabilityState::Supported
                    && evidence.confidence == bridge_tally_core::EvidenceConfidence::Observed
            })
    {
        return Err(
            "Core Accounting remains unverified for this company, release, and query profile"
                .to_string(),
        );
    }
    let observed_at_unix_ms = chrono::Utc::now().timestamp_millis();
    let mut items = Vec::new();
    for (transport, evidence) in &canary.profile.transports {
        items.push(CapabilityItemInput {
            kind: MirrorCapabilityKind::Transport,
            key: transport_key(*transport).to_string(),
            state: mirror_capability_state(evidence.state),
            confidence: mirror_confidence(evidence.confidence),
            safe_reason_code: evidence.safe_reason_code.clone(),
        });
    }
    for (pack, evidence) in &canary.profile.packs {
        items.push(CapabilityItemInput {
            kind: MirrorCapabilityKind::Pack,
            key: pack_key(*pack).to_string(),
            state: mirror_capability_state(evidence.state),
            confidence: mirror_confidence(evidence.confidence),
            safe_reason_code: evidence.safe_reason_code.clone(),
        });
    }
    let snapshot = mirror
        .save_capability_snapshot(CapabilitySnapshotInput {
            canonical_origin: pin.canonical_origin,
            observed_at_unix_ms,
            profile_version: canary.profile.profile_version,
            product: canary.profile.product.clone(),
            release: canary.profile.release.clone(),
            mode: canary.profile.mode.clone(),
            mode_confidence: if canary.profile.mode.is_some() {
                Confidence::Observed
            } else {
                Confidence::Unknown
            },
            items,
        })
        .await
        .map_err(|_| "The read-only canary passed, but its encrypted evidence was not stored")?;

    let capability_profile_sha256 = capability_profile_sha256(&canary.profile)
        .map_err(|_| "The capability profile could not be bound to the snapshot plan")?;
    let plan = SnapshotPlan {
        resume_key: format!("snapshot:{run_id}"),
        run_id,
        capability_snapshot_id: snapshot.id,
        mirror_company_id: pin.company_id,
        company,
        pack: CapabilityPackId::CoreAccounting,
        pack_schema_version: CORE_ACCOUNTING_SCHEMA_VERSION,
        capability_profile_version: canary.profile.profile_version,
        capability_profile_sha256,
        source_product: canary.profile.product,
        source_transport: "xml_http".to_string(),
        source_release: canary.profile.release,
        source_mode: canary.profile.mode,
        external_references: ExternalReferenceCatalog::Unavailable,
        adaptive_window_policy: Some(AdaptiveWindowPolicy::bounded_default()),
        capability_canary_window: Some(capability_canary_window),
        windows: vec![planned],
        started_at_unix_ms: observed_at_unix_ms,
        freshness_target_seconds: 86_400,
    };
    coordinator
        .start(plan, connector, mirror.inner().clone())
        .await
        .map_err(str::to_string)
}

#[tauri::command]
pub async fn tally_snapshot_status(
    run_id: String,
    mirror: State<'_, TallyMirrorRepository>,
    coordinator: State<'_, SnapshotCoordinator>,
) -> Result<SnapshotJobStatus, String> {
    coordinator
        .status(&run_id, mirror.inner())
        .await
        .map_err(str::to_string)
}

#[tauri::command]
pub async fn tally_recent_snapshot_runs(
    mirror: State<'_, TallyMirrorRepository>,
    coordinator: State<'_, SnapshotCoordinator>,
) -> Result<Vec<SnapshotJobStatus>, String> {
    coordinator
        .recent(mirror.inner(), 20)
        .await
        .map_err(str::to_string)
}

#[derive(Debug, Deserialize)]
pub struct ResumeCoreSnapshotRequest {
    pub config: TallyConfig,
    pub run_id: String,
}

#[tauri::command]
pub async fn resume_tally_core_snapshot(
    request: ResumeCoreSnapshotRequest,
    mirror: State<'_, TallyMirrorRepository>,
    runtime: State<'_, TallyRuntime>,
    coordinator: State<'_, SnapshotCoordinator>,
) -> Result<SnapshotJobStatus, String> {
    let store = SqliteSnapshotStateStore::new(mirror.pool_clone());
    store
        .migrate()
        .await
        .map_err(|_| "Restart-safe snapshot recovery is not installed".to_string())?;
    let state = store
        .load_by_run_id(&request.run_id)
        .await
        .map_err(|_| "The encrypted snapshot recovery state is invalid".to_string())?
        .ok_or_else(|| "The snapshot recovery state was not found".to_string())?;
    if state.progress.phase.is_terminal() {
        return Err("A terminal snapshot cannot be resumed".to_string());
    }
    let plan = state
        .recoverable_plan()
        .map_err(|_| "This snapshot predates restart-safe recovery or its plan is invalid")?;
    if plan.pack != CapabilityPackId::CoreAccounting
        || plan.pack_schema_version != CORE_ACCOUNTING_SCHEMA_VERSION
        || plan.source_transport != "xml_http"
    {
        return Err("The stored snapshot profile is not resumable by this build".to_string());
    }

    let pin = mirror
        .snapshot_source_pin(&plan.mirror_company_id)
        .await
        .map_err(|_| "The encrypted company pin for this snapshot is unavailable".to_string())?;
    validate_company_name(&pin.display_name)?;
    let request_origin = EndpointKey::from_config(&request.config)
        .map_err(|_| "Tally endpoint validation failed".to_string())?;
    let lineage = source_lineage(&request.config).map_err(|_| "Tally source lineage is invalid")?;
    let observed_company = CoreCompanyRef {
        identity: company_source_identity(&lineage, &pin.company_guid),
        display_name: pin.display_name.clone(),
    };
    if request_origin.as_str() != pin.canonical_origin
        || plan.mirror_company_id != pin.company_id
        || plan.company != observed_company
    {
        return Err(
            "The current endpoint or encrypted company pin does not match the immutable snapshot plan"
                .to_string(),
        );
    }
    if !mirror
        .capability_snapshot_matches_plan(
            &plan.capability_snapshot_id,
            &plan.mirror_company_id,
            plan.capability_profile_version,
            &plan.source_product,
            plan.source_release.as_deref(),
            plan.source_mode.as_deref(),
        )
        .await
        .map_err(|_| "The stored capability evidence could not be validated".to_string())?
    {
        return Err(
            "The stored capability evidence is not bound to the pinned company endpoint"
                .to_string(),
        );
    }

    let canary_window = plan
        .capability_canary_window
        .clone()
        .ok_or_else(|| "The stored snapshot plan contains no canary window".to_string())?;
    let context = RequestContext {
        run_id: plan.run_id.clone(),
        company: plan.company.clone(),
        pack: plan.pack,
        schema_version: plan.pack_schema_version,
        window: canary_window.range,
        query_profile: canary_window.query_profile,
        filters_sha256: canary_window.filters_sha256,
    };
    let connector = RuntimeTallyConnector::new(
        runtime.inner().clone(),
        request.config,
        plan.company.clone(),
        context,
    )
    .map_err(|_| "The stored Core Accounting snapshot profile is invalid".to_string())?;
    coordinator
        .start(plan, connector, mirror.inner().clone())
        .await
        .map_err(str::to_string)
}

#[tauri::command]
pub fn cancel_tally_snapshot(
    run_id: String,
    coordinator: State<'_, SnapshotCoordinator>,
) -> Result<bool, String> {
    coordinator.cancel(&run_id).map_err(str::to_string)
}

fn mirror_capability_state(state: bridge_tally_core::CapabilityState) -> MirrorCapabilityState {
    match state {
        bridge_tally_core::CapabilityState::Supported => MirrorCapabilityState::Supported,
        bridge_tally_core::CapabilityState::Unsupported => MirrorCapabilityState::Unsupported,
        bridge_tally_core::CapabilityState::Unknown => MirrorCapabilityState::Unknown,
        bridge_tally_core::CapabilityState::NotConfigured => MirrorCapabilityState::NotConfigured,
    }
}

fn mirror_confidence(confidence: bridge_tally_core::EvidenceConfidence) -> Confidence {
    match confidence {
        bridge_tally_core::EvidenceConfidence::Documented => Confidence::Documented,
        bridge_tally_core::EvidenceConfidence::Observed => Confidence::Observed,
        bridge_tally_core::EvidenceConfidence::Inferred => Confidence::Inferred,
        bridge_tally_core::EvidenceConfidence::Unknown => Confidence::Unknown,
    }
}

fn transport_key(transport: bridge_tally_core::TransportId) -> &'static str {
    match transport {
        bridge_tally_core::TransportId::XmlHttp => "xml_http",
        bridge_tally_core::TransportId::JsonEx => "json_ex",
        bridge_tally_core::TransportId::TdlCompanion => "tdl_companion",
        bridge_tally_core::TransportId::Odbc => "odbc",
    }
}

fn pack_key(pack: bridge_tally_core::CapabilityPackId) -> &'static str {
    match pack {
        bridge_tally_core::CapabilityPackId::CoreAccounting => "core_accounting",
        bridge_tally_core::CapabilityPackId::IndiaTax => "india_tax",
        bridge_tally_core::CapabilityPackId::BillsAndPayments => "bills_and_payments",
        bridge_tally_core::CapabilityPackId::Inventory => "inventory",
    }
}

fn feature_key(feature: bridge_tally_core::CapabilityFeatureId) -> &'static str {
    match feature {
        bridge_tally_core::CapabilityFeatureId::EndpointReachability => "endpoint_reachability",
        bridge_tally_core::CapabilityFeatureId::LoadedCompanies => "loaded_companies",
        bridge_tally_core::CapabilityFeatureId::StableCompanyIdentity => "stable_company_identity",
        bridge_tally_core::CapabilityFeatureId::EncodingBehaviour => "encoding_behaviour",
        bridge_tally_core::CapabilityFeatureId::PracticalResponseLimit => {
            "practical_response_limit"
        }
        bridge_tally_core::CapabilityFeatureId::CompanyRead => "company_read",
        bridge_tally_core::CapabilityFeatureId::LedgerRead => "ledger_read",
        bridge_tally_core::CapabilityFeatureId::VoucherRead => "voucher_read",
        bridge_tally_core::CapabilityFeatureId::SelectedLedgerRead => "selected_ledger_read",
        bridge_tally_core::CapabilityFeatureId::SelectedVoucherWindowRead => {
            "selected_voucher_window_read"
        }
        bridge_tally_core::CapabilityFeatureId::Write => "write",
    }
}

#[tauri::command]
pub async fn fetch_tally_companies(
    config: TallyConfig,
    runtime: State<'_, TallyRuntime>,
) -> Result<Vec<TallyCompany>, TallyCommandError> {
    runtime
        .fetch_companies(config)
        .await
        .map_err(tally_runtime_command_error)
}

#[derive(Debug, Deserialize)]
pub struct CompanyRequest {
    pub config: TallyConfig,
    pub company: String,
    pub expected_company_guid: String,
}

#[derive(Debug, Deserialize)]
pub struct VoucherRequest {
    pub config: TallyConfig,
    pub company: String,
    pub expected_company_guid: String,
    pub from: String,
    pub to: String,
}

#[tauri::command]
pub async fn fetch_tally_ledgers(
    request: CompanyRequest,
    runtime: State<'_, TallyRuntime>,
) -> Result<Vec<TallyLedger>, TallyCommandError> {
    validate_company_name(&request.company).map_err(|message| {
        tally_command_error(
            "company_selection_invalid",
            "Tally application",
            message,
            "after_change",
            false,
            "Select the intended GUID-bearing company and repeat the read-only action.",
        )
    })?;
    runtime
        .fetch_ledgers(
            request.config,
            request.company,
            request.expected_company_guid,
        )
        .await
        .map_err(tally_runtime_command_error)
}

#[tauri::command]
pub async fn fetch_tally_vouchers(
    request: VoucherRequest,
    runtime: State<'_, TallyRuntime>,
) -> Result<Vec<TallyVoucher>, TallyCommandError> {
    validate_company_name(&request.company).map_err(|message| {
        tally_command_error(
            "company_selection_invalid",
            "Tally application",
            message,
            "after_change",
            false,
            "Select the intended GUID-bearing company and repeat the read-only action.",
        )
    })?;
    validate_date_range(&request.from, &request.to).map_err(|message| {
        tally_command_error(
            "accounting_period_invalid",
            "Endpoint configuration",
            message,
            "after_change",
            false,
            "Choose a valid accounting period, then repeat the read-only action.",
        )
    })?;
    runtime
        .fetch_vouchers(
            request.config,
            request.company,
            request.expected_company_guid,
            request.from,
            request.to,
        )
        .await
        .map_err(tally_runtime_command_error)
}

#[tauri::command]
pub fn cancel_tally_request(
    request_id: String,
    runtime: State<'_, TallyRuntime>,
) -> Result<bool, TallyCommandError> {
    runtime
        .cancel_request(&request_id)
        .map_err(tally_runtime_command_error)
}

#[tauri::command]
pub fn tally_runtime_snapshots(
    runtime: State<'_, TallyRuntime>,
) -> Result<Vec<TallySessionSnapshot>, TallyCommandError> {
    runtime.snapshots().map_err(tally_runtime_command_error)
}

#[tauri::command]
pub fn tally_telemetry_preview(
    runtime: State<'_, TallyRuntime>,
) -> Result<TallyTelemetryPreviewExport, TallyCommandError> {
    runtime
        .telemetry_preview()
        .map_err(tally_runtime_command_error)
}

#[tauri::command]
pub async fn prepare_gst_return_draft(request: GstDraftRequest) -> Result<GstReturnDraft, String> {
    let _ = request;
    Err("GST return drafting is not implemented; Bridge did not produce a GST result".to_string())
}

async fn run_dsc_probe(
    detect_only: bool,
    pins: Option<Zeroizing<Vec<String>>>,
) -> Result<crate::dsc::ProbeReport, String> {
    tokio::task::spawn_blocking(move || {
        let pins = pins.map(|mut pins| std::mem::take(&mut *pins));
        crate::dsc::run_probe_isolated(detect_only, None, pins, true)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("DSC probe task failed: {error}"))?
}

#[tauri::command]
pub async fn detect_dsc_token() -> Result<crate::dsc::ProbeReport, String> {
    run_dsc_probe(true, None).await
}

#[tauri::command]
pub async fn extract_dsc_certificates(
    pins: Option<Vec<String>>,
) -> Result<crate::dsc::ProbeReport, String> {
    let pins = Zeroizing::new(
        pins.ok_or_else(|| "PIN is required to extract DSC certificates".to_string())?,
    );
    validate_dsc_pins(&pins)?;
    run_dsc_probe(false, Some(pins)).await
}

fn validate_dsc_pins(pins: &[String]) -> Result<(), String> {
    if pins.len() != 1 || pins[0].is_empty() {
        return Err("Provide exactly one non-empty PIN".to_string());
    }
    if pins[0].len() > MAX_DSC_PIN_BYTES || pins[0].chars().any(char::is_control) {
        return Err(
            "DSC PIN must be at most 128 bytes and contain no control characters".to_string(),
        );
    }
    Ok(())
}

#[tauri::command]
pub async fn validate_axal_credentials(
    credentials: crate::axal::AxalCredentials,
) -> Result<crate::axal::AxalSessionResponse, String> {
    crate::axal::establish_credential_session(credentials)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn check_axal_connection_status(
    credential_session_id: String,
) -> Result<crate::axal::ConnectionStatusResponse, String> {
    crate::axal::check_connection_status(&credential_session_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn revoke_axal_credential_session(credential_session_id: String) -> Result<(), String> {
    crate::axal::revoke_credential_session(&credential_session_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn sync_dsc_certificates_to_axal(
    request: crate::axal::DscSyncRequest,
) -> Result<crate::axal::DscSyncResponse, String> {
    crate::axal::sync_dsc_certificates(request)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn scan_document_paths(
    request: crate::documents::ScanDocumentsRequest,
) -> Result<crate::documents::ScanDocumentsResponse, String> {
    crate::documents::scan_documents(request)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn sync_documents_to_axal(
    request: crate::documents::SyncDocumentsRequest,
) -> Result<crate::documents::SyncDocumentsResponse, String> {
    crate::documents::sync_documents(request)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn revoke_document_authorizations(
    selection_ids: Vec<String>,
    scan_session_id: Option<String>,
) -> Result<(), String> {
    crate::documents::revoke_document_authorizations(&selection_ids, scan_session_id.as_deref())
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn select_document_files() -> Result<Vec<crate::documents::SelectedDocumentPath>, String>
{
    tokio::task::spawn_blocking(|| {
        let paths = rfd::FileDialog::new()
            .set_title("Select documents")
            .pick_files()
            .unwrap_or_default();
        crate::documents::authorize_selected_paths(paths).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("File picker failed: {error}"))?
}

#[tauri::command]
pub async fn select_document_folder() -> Result<Vec<crate::documents::SelectedDocumentPath>, String>
{
    tokio::task::spawn_blocking(|| {
        let paths = rfd::FileDialog::new()
            .set_title("Select document folder")
            .pick_folder()
            .into_iter()
            .collect::<Vec<_>>();
        crate::documents::authorize_selected_paths(paths).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("Folder picker failed: {error}"))?
}

#[cfg(test)]
mod tests {
    use super::{
        first_calendar_day_canary_window, reconcile_review_cleanup,
        reviewed_probe_commitment_sha256, selected_read_observation, tally_command_error,
        tally_runtime_command_error, validate_dsc_pins, PersistedTallyCompany, SavedTallySetup,
    };
    use crate::tally::{
        ConnectionStatus, SelectedReadObservation, TallyCompany, TallyProbeResult, TallyProduct,
    };
    use bridge_tally_core::CapabilityProfile;
    use std::collections::BTreeMap;

    #[test]
    fn dsc_pin_input_is_strictly_bounded() {
        assert!(validate_dsc_pins(&["1234".to_string()]).is_ok());
        assert!(validate_dsc_pins(&["".to_string()]).is_err());
        assert!(validate_dsc_pins(&["1\n2".to_string()]).is_err());
        assert!(validate_dsc_pins(&["x".repeat(129)]).is_err());
        assert!(validate_dsc_pins(&["1".to_string(), "2".to_string()]).is_err());
    }

    #[test]
    fn snapshot_capability_canary_is_exactly_the_requested_first_calendar_day() {
        let canary = first_calendar_day_canary_window("20260228").unwrap();
        assert_eq!(canary.range.from_yyyymmdd, "20260228");
        assert_eq!(canary.range.to_yyyymmdd, "20260228");
        assert_eq!(canary.query_profile.as_str(), "core_accounting_v2");

        let same = first_calendar_day_canary_window("20260228").unwrap();
        assert_eq!(same, canary);
        assert!(first_calendar_day_canary_window("20260229").is_err());
        assert!(first_calendar_day_canary_window("2026-02-28").is_err());
    }

    #[test]
    fn tally_runtime_error_serialization_is_stable_and_redacted() {
        let error = tally_runtime_command_error(anyhow::anyhow!(
            "synthetic reqwest failure at http://127.0.0.1:9000/?token=private"
        ));
        let json = serde_json::to_string(&error).expect("serialize safe Tally command error");
        assert_eq!(error.code, "endpoint_unreachable");
        assert_eq!(error.category, "Endpoint configuration");
        assert!(error.local_state_changed);
        assert!(!error.tally_state_may_have_changed);
        assert!(!json.contains("token=private"));
        assert!(!json.contains("reqwest"));

        let invalid_config =
            tally_runtime_command_error(anyhow::anyhow!("Tally port must be between 1 and 65535"));
        assert_eq!(invalid_config.code, "endpoint_configuration_invalid");
        assert!(!invalid_config.local_state_changed);

        let queue_deadline =
            tally_runtime_command_error(anyhow::anyhow!("endpoint queue deadline exceeded"));
        assert_eq!(queue_deadline.code, "tally_runtime_temporarily_unavailable");
        assert!(queue_deadline.local_state_changed);
    }

    #[test]
    fn explicit_tally_error_preserves_atomic_failure_truth() {
        let error = tally_command_error(
            "reviewed_setup_store_failed",
            "Operation",
            "Synthetic reviewed setup was not stored",
            "after_change",
            false,
            "Inspect local encrypted storage.",
        );
        assert_eq!(error.code, "reviewed_setup_store_failed");
        assert!(!error.local_state_changed);
        assert!(!error.tally_state_may_have_changed);
    }

    #[test]
    fn post_commit_review_cleanup_failure_preserves_durable_success_truth() {
        let saved = SavedTallySetup {
            passport_snapshot_id: "snapshot-1".to_string(),
            canonical_origin: "http://127.0.0.1:9000".to_string(),
            observed_at_unix_ms: 1_000,
            company: PersistedTallyCompany {
                name: "Synthetic Company".to_string(),
                guid: Some("synthetic-guid".to_string()),
                mirror_company_id: Some("company-1".to_string()),
                correlation_key: Some("c".repeat(64)),
                identity_confidence: "observed",
            },
            review_cleanup_warning: None,
        };
        let result = reconcile_review_cleanup(Ok(saved), false).expect("save stays successful");
        assert_eq!(
            result.review_cleanup_warning,
            Some("review_cache_cleanup_failed_after_save")
        );

        let failed = reconcile_review_cleanup(
            Err(tally_command_error(
                "reviewed_setup_store_failed",
                "Operation",
                "Synthetic failure",
                "after_change",
                false,
                "Retry.",
            )),
            false,
        )
        .expect_err("failed store plus failed cleanup is explicit");
        assert_eq!(failed.code, "reviewed_setup_retry_state_uncertain");
        assert!(failed.local_state_changed);
    }

    #[test]
    fn reviewed_probe_commitment_binds_time_company_name_and_full_company_list() {
        let probe = |names: &[&str]| TallyProbeResult {
            connection: ConnectionStatus {
                reachable: true,
                compatible: false,
                server_text: "Synthetic status".to_string(),
                product: TallyProduct::Unknown,
                error: None,
            },
            companies: names
                .iter()
                .enumerate()
                .map(|(index, name)| TallyCompany {
                    name: (*name).to_string(),
                    guid: Some(format!("guid-{index}")),
                })
                .collect(),
            profile: CapabilityProfile {
                profile_version: 2,
                product: "Unknown".to_string(),
                release: None,
                mode: None,
                transports: BTreeMap::new(),
                features: BTreeMap::new(),
                packs: BTreeMap::new(),
            },
            selected_read_scope: None,
            passport_snapshot_id: None,
        };
        let first = reviewed_probe_commitment_sha256(
            "review-a",
            "http://127.0.0.1:9000",
            1_000,
            &probe(&["Synthetic A"]),
        )
        .unwrap();
        let renamed = reviewed_probe_commitment_sha256(
            "review-a",
            "http://127.0.0.1:9000",
            1_000,
            &probe(&["Synthetic Renamed"]),
        )
        .unwrap();
        let expanded = reviewed_probe_commitment_sha256(
            "review-a",
            "http://127.0.0.1:9000",
            1_000,
            &probe(&["Synthetic A", "Synthetic B"]),
        )
        .unwrap();
        let later = reviewed_probe_commitment_sha256(
            "review-a",
            "http://127.0.0.1:9000",
            1_001,
            &probe(&["Synthetic A"]),
        )
        .unwrap();
        assert_ne!(first, renamed);
        assert_ne!(first, expanded);
        assert_ne!(first, later);
        let different_review = reviewed_probe_commitment_sha256(
            "review-b",
            "http://127.0.0.1:9000",
            1_000,
            &probe(&["Synthetic A"]),
        )
        .unwrap();
        assert_ne!(first, different_review);
    }

    #[test]
    fn selected_read_observation_distinguishes_empty_identity_evidence() {
        let observation = |bucket| SelectedReadObservation {
            request_sha256: "a".repeat(64),
            decoded_response_sha256: "b".repeat(64),
            response_encoding: "utf8",
            result_bucket: bucket,
        };
        let empty = selected_read_observation(
            "selected_ledger_read",
            Ok(observation("empty_observed")),
            false,
            "selected_ledger_read_empty_observed",
            "selected_ledger_read_non_empty_observed",
        );
        assert_eq!(empty.identity_evidence_state, "not_applicable_empty");
        assert!(empty.record_count_verified);

        let populated = selected_read_observation(
            "selected_ledger_read",
            Ok(observation("non_empty_observed")),
            false,
            "selected_ledger_read_empty_observed",
            "selected_ledger_read_non_empty_observed",
        );
        assert_eq!(populated.identity_evidence_state, "verified");
    }
}
