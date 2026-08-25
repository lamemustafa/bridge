use crate::client_groups;
use crate::db::tally_incremental::IncrementalFoundationEvidence;
use crate::db::tally_mirror::{
    company_profile_correlation_key, selected_read_scope_commitment_sha256, CapabilityItemInput,
    CapabilityKind as MirrorCapabilityKind, CapabilitySnapshotInput,
    CapabilityState as MirrorCapabilityState, Confidence, FreshnessState,
    LocalReconciliationMismatch, ProofSummary, RedactedProofExport, ReviewedSetupInput,
    SelectedReadObservationCommitmentMaterial, SelectedReadObservationInput,
    SelectedReadScopeCommitmentMaterial, SelectedReadScopeInput, SourceIdentityInput,
    WriteFixtureEnrollmentInput, WriteFixtureEnrollmentStatus,
};
use crate::gst::{GstDraftRequest, GstReturnDraft};
use crate::reports::bulk_party_statement::{
    bulk_party_statement_party_count, write_bulk_party_statements_with_ageing_anchor,
    BulkPartyStatementRequest, PartyStatementDestinationApprovals,
};
use crate::reports::party_statement::{
    build_party_statement_with_ageing_anchor, PartyStatementError,
};
use crate::reports::party_statement_pdf::render_party_statement_pdf;
use crate::reports::party_statement_xlsx::render_party_statement_xlsx;
use crate::reports::trial_balance::TrialBalanceExportSummary;
use crate::reports::trial_balance_xlsx::render_trial_balance_xlsx;
use crate::sync::coordinator::{SnapshotCoordinator, SnapshotJobStatus};
use crate::sync::reconciliation::ExternalReferenceCatalog;
use crate::sync::snapshot::{
    capability_profile_sha256, AdaptiveWindowPolicy, PlannedWindow, SnapshotPlan,
    SqliteSnapshotStateStore,
};
use crate::tally::runtime::{TallyRuntimeControlError, TrialBalanceReadError};
use crate::tally::validators::{
    normalize_company_guid, validate_company_name, validate_date_range,
};
use crate::tally::{
    company_source_identity, core_snapshot_start_authorized, source_lineage,
    CachedProbeReservation, ConnectionStatus, EndpointKey, OpenBillRow,
    OutstandingsCurrencyAssertion, OutstandingsLoadResult, RuntimeTallyConnector,
    SelectedReadObservation, SelectedReadScopeEvidence, TallyCompany, TallyConfig, TallyLedger,
    TallyRuntime, TallySessionSnapshot, TallyTelemetryPreviewExport, TallyVoucher,
    UnallocatedParty, SELECTED_LEDGER_QUERY_PROFILE_ID, SELECTED_VOUCHER_QUERY_PROFILE_ID,
};
use bridge_tally_core::{
    CapabilityEvidence, CapabilityFeatureId, CapabilityPackId, CapabilityState,
    CompanyRef as CoreCompanyRef, EvidenceConfidence, ReadWindow, RequestContext, TallyConnector,
    TallyDate, TransportId, CORE_ACCOUNTING_SCHEMA_VERSION,
};
use bridge_tally_protocol::trial_balance::TrialBalanceError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Manager, State};
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

/// Bulk statement exports historically returned a plain message for filesystem
/// failures. Keep that IPC behavior intact while making an unapproved
/// destination distinguishable to callers through the standard error envelope.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum BulkPartyStatementExportError {
    DestinationNotAuthorized(TallyCommandError),
    Existing(String),
}

fn party_statement_destination_not_authorized_error() -> TallyCommandError {
    tally_command_error(
        "statement_destination_not_authorized",
        "Operation",
        "The statement destination was not selected in this Bridge session.",
        "after_change",
        false,
        "Choose the destination folder again, then restart the statement export.",
    )
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
    if let Some(trial_balance) = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<TrialBalanceReadError>())
    {
        return match trial_balance {
            TrialBalanceReadError::AsOfPrecedesBooksFrom => tally_command_error(
                "trial_balance_as_of_precedes_books_from",
                "Financial data",
                "The selected Trial Balance date is before this company’s books start. Choose a date on or after the books start and export again.",
                "after_change",
                true,
                "Choose a date on or after the company books start, then repeat the export.",
            ),
            TrialBalanceReadError::PeriodBoundaryUnsupported => tally_command_error(
                "trial_balance_period_boundary_unsupported",
                "Financial data",
                "This Tally mode supports Trial Balance exports only on day 1, 2, or 31. Choose one of those dates and export again.",
                "after_change",
                true,
                "Choose day 1, 2, or 31 in the As of control, then repeat the export.",
            ),
            TrialBalanceReadError::SnapshotDrifted => tally_command_error(
                "trial_balance_snapshot_drifted",
                "Financial data",
                "Tally returned different Trial Balance snapshots. Keep the company unchanged, wait for a stable moment, and export again.",
                "after_change",
                true,
                "Keep the selected company unchanged and repeat the export after current Tally activity settles.",
            ),
            TrialBalanceReadError::BookChanged => tally_command_error(
                "trial_balance_book_changed",
                "Financial data",
                "The company changed during the Trial Balance read. Keep the book unchanged, wait for a stable moment, and export again.",
                "after_change",
                true,
                "Keep the selected company unchanged and repeat the export after current Tally activity settles.",
            ),
        };
    }
    if error
        .chain()
        .any(|cause| cause.downcast_ref::<TrialBalanceError>().is_some())
    {
        return tally_command_error(
            "trial_balance_response_unverified",
            "Financial data",
            "Tally’s Trial Balance response did not satisfy Bridge’s exact identity and accounting controls. No workbook was created.",
            "after_change",
            true,
            "Keep the selected company unchanged and retain the selected date for support before retrying.",
        );
    }
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
    let deadline_exceeded = error.chain().any(|cause| {
        let message = cause.to_string().to_ascii_lowercase();
        message.contains("request exceeded its deadline")
            || message.contains("request deadline exceeded")
    });
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
    } else if deadline_exceeded {
        (
            "tally_request_deadline_exceeded",
            "Operation",
            "The bounded Tally read exceeded its production deadline.",
            "after_change",
            true,
            "Do not retry the unchanged request. Verify Tally gateway health and change the request shape only after reviewing the measured segment.",
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
    } else if lower.contains("interactive discovery listing limit exceeded") {
        (
            "untrusted_discovery_limit_exceeded",
            "Discovery listing",
            "The unverified local company listing exceeded Bridge's display safety limit.",
            "after_change",
            true,
            "Reduce the locally listed companies or use a strict probe for reviewed company evidence.",
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

/// Produces the typed command error for the encrypted Tally mirror failing to initialise on
/// first use (denied keychain authorisation, or a local disk/storage failure). The mirror was
/// never opened, so no local or Tally state changed; retrying after the operator resolves the
/// underlying keychain/disk issue is safe.
fn mirror_unavailable_command_error(_error: anyhow::Error) -> TallyCommandError {
    tally_command_error(
        "tally_mirror_unavailable",
        "Operation",
        "The encrypted Tally mirror could not be opened. Its operating-system credential may have been denied, or local storage is unavailable.",
        "safe",
        false,
        "Approve the operating-system credential prompt for Bridge, or verify local disk access, then retry.",
    )
}

/// Same failure as [`mirror_unavailable_command_error`], for the handful of mirror-backed
/// commands that report errors as a plain `String` rather than a [`TallyCommandError`].
fn mirror_unavailable_string_error(_error: anyhow::Error) -> String {
    "The encrypted Tally mirror could not be opened. Its operating-system credential may have been denied, or local storage is unavailable.".to_string()
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
    persisted_tally_probe_result(review_id, canonical_origin, observed_at_unix_ms, probe)
}

fn persisted_tally_probe_result(
    review_id: String,
    canonical_origin: String,
    observed_at_unix_ms: i64,
    probe: crate::tally::TallyProbeResult,
) -> Result<PersistedTallyProbeResult, TallyCommandError> {
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

#[derive(Debug, Deserialize)]
pub struct EnrollTallyWriteFixtureRequest {
    pub config: TallyConfig,
    pub expected_review_id: String,
    pub expected_review_commitment_sha256: String,
    pub mirror_company_id: String,
    pub selected_company_guid: String,
    pub disposable_company_attested: bool,
    pub no_customer_data_attested: bool,
    pub backup_guidance_acknowledged: bool,
}

#[derive(Debug, Deserialize)]
pub struct TallyWriteFixtureCompanyRequest {
    pub mirror_company_id: String,
}

#[derive(Debug, Serialize)]
pub struct TallyWriteFixtureEnrollmentResponse {
    #[serde(flatten)]
    pub status: WriteFixtureEnrollmentStatus,
    pub tally_requests_attempted: u8,
    pub tally_writes_attempted: u8,
    pub review_cleanup_warning: Option<&'static str>,
}

#[tauri::command]
pub async fn save_tally_setup(
    request: SaveTallySetupRequest,
    mirror: State<'_, crate::LazyTallyMirror>,
    runtime: State<'_, TallyRuntime>,
) -> Result<SavedTallySetup, TallyCommandError> {
    let mirror = mirror
        .get()
        .await
        .map_err(mirror_unavailable_command_error)?;
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

#[tauri::command]
pub async fn enroll_tally_write_fixture(
    request: EnrollTallyWriteFixtureRequest,
    mirror: State<'_, crate::LazyTallyMirror>,
    runtime: State<'_, TallyRuntime>,
) -> Result<TallyWriteFixtureEnrollmentResponse, TallyCommandError> {
    let mirror = mirror
        .get()
        .await
        .map_err(mirror_unavailable_command_error)?;
    let canonical_origin = EndpointKey::from_config(&request.config)
        .map(|endpoint| endpoint.as_str().to_string())
        .map_err(|_| {
            tally_command_error(
                "endpoint_configuration_invalid",
                "Endpoint configuration",
                "Tally endpoint validation failed",
                "after_change",
                false,
                "Use the reviewed loopback endpoint and probe again.",
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
                "Probe again, review the exact Passport and company scope, then enroll.",
            )
        })?;
    let observed_at_unix_ms = reservation.observed_at_unix_ms();
    let probe = reservation.result().clone();
    let result: Result<TallyWriteFixtureEnrollmentResponse, TallyCommandError> = async {
        if request.expected_review_commitment_sha256
            != reviewed_probe_commitment_sha256(
                &request.expected_review_id, &canonical_origin, observed_at_unix_ms, &probe,
            ).map_err(|_| tally_command_error(
                "reviewed_probe_commitment_failed", "Operation",
                "The cached endpoint, Passport, and company scope could not be verified.",
                "safe", false, "Probe again before enrolling a fixture.",
            ))?
        {
            return Err(tally_command_error(
                "reviewed_probe_changed", "Operation",
                "The reviewed Capability Passport no longer matches the cached probe.",
                "safe", false, "Probe again and review the replacement Passport before enrolling.",
            ));
        }
        let selected_guid = normalize_company_guid(&request.selected_company_guid).map_err(|_| {
            tally_command_error(
                "stable_company_identity_required", "Tally application",
                "The selected company does not have an observed stable GUID.",
                "after_change", false, "Select a GUID-bearing company from the current probe.",
            )
        })?;
        let matching_companies = probe.companies.iter().filter(|company| {
            company.guid.as_deref().is_some_and(|guid| guid.eq_ignore_ascii_case(&selected_guid))
        }).count();
        if matching_companies != 1 {
            return Err(tally_command_error(
                if matching_companies == 0 { "reviewed_company_scope_changed" } else { "company_identity_ambiguous" },
                "Tally application",
                "The selected company identity is not uniquely present in the reviewed probe.",
                "safe", false, "Probe again and select one GUID-bearing company from the current result.",
            ));
        }
        if probe.profile.features.get(&CapabilityFeatureId::Write)
            .is_some_and(|evidence| evidence.state == CapabilityState::Unsupported)
        {
            return Err(tally_command_error(
                "write_capability_unsupported", "Tally application",
                "The reviewed Passport marks Tally write capability unsupported.",
                "safe", false, "Do not enroll this scope for a write canary.",
            ));
        }
        let pin = mirror.snapshot_source_pin(&request.mirror_company_id).await.map_err(|_| {
            tally_command_error(
                "persisted_company_scope_required", "Operation",
                "A persisted observed company pin is required before fixture enrollment.",
                "safe", false, "Save the reviewed company scope, then probe and enroll while it is fresh.",
            )
        })?;
        if pin.canonical_origin != canonical_origin || !pin.company_guid.eq_ignore_ascii_case(&selected_guid) {
            return Err(tally_command_error(
                "persisted_company_scope_changed", "Tally application",
                "The persisted company pin does not match the fresh reviewed company identity.",
                "safe", false, "Probe again and save the selected company scope before enrolling.",
            ));
        }
        let enrollment = mirror.enroll_write_fixture(WriteFixtureEnrollmentInput {
            company_id: request.mirror_company_id.clone(),
            review_commitment_sha256: request.expected_review_commitment_sha256.clone(),
            disposable_company_attested: request.disposable_company_attested,
            no_customer_data_attested: request.no_customer_data_attested,
            backup_guidance_acknowledged: request.backup_guidance_acknowledged,
            enrolled_at_unix_ms: chrono::Utc::now().timestamp_millis(),
        }).await.map_err(|_| tally_command_error(
            "fixture_enrollment_store_failed", "Operation",
            "The local write-fixture enrollment could not be stored.",
            "safe", false, "Verify the three confirmations and local encrypted storage, then retry the fresh review.",
        ))?;
        let status = mirror.write_fixture_enrollment_status(&request.mirror_company_id).await.map_err(|_| {
            tally_command_error("fixture_enrollment_status_unavailable", "Operation", "The local fixture status could not be read after enrollment.", "after_change", true, "Restart Bridge and inspect the local fixture status before any future canary.")
        })?;
        debug_assert!(!enrollment.id.is_empty());
        Ok(TallyWriteFixtureEnrollmentResponse {
            status,
            tally_requests_attempted: 0,
            tally_writes_attempted: 0,
            review_cleanup_warning: None,
        })
    }.await;
    let cleanup_succeeded = if result.is_ok() {
        reservation.consume().unwrap_or(false)
    } else {
        reservation.release().unwrap_or(false)
    };
    match result {
        Ok(mut response) => {
            if !cleanup_succeeded {
                response.review_cleanup_warning = Some("review_cache_cleanup_failed_after_fixture_enrollment");
            }
            Ok(response)
        }
        Err(_) if !cleanup_succeeded => Err(tally_command_error(
            "fixture_enrollment_retry_state_uncertain", "Operation",
            "The local fixture enrollment did not complete cleanly and the reviewed cache could not be released.",
            "after_change", true, "Restart Bridge, probe again, and inspect local fixture status before retrying.",
        )),
        Err(error) => Err(error),
    }
}

#[tauri::command]
pub async fn tally_write_fixture_enrollment_status(
    request: TallyWriteFixtureCompanyRequest,
    mirror: State<'_, crate::LazyTallyMirror>,
) -> Result<WriteFixtureEnrollmentStatus, TallyCommandError> {
    let mirror = mirror
        .get()
        .await
        .map_err(mirror_unavailable_command_error)?;
    mirror
        .write_fixture_enrollment_status(&request.mirror_company_id)
        .await
        .map_err(|_| {
            tally_command_error(
                "fixture_enrollment_status_unavailable",
                "Operation",
                "The local fixture status is unavailable.",
                "safe",
                false,
                "Save a reviewed company scope before checking fixture status.",
            )
        })
}

#[tauri::command]
pub async fn revoke_tally_write_fixture_enrollment(
    request: TallyWriteFixtureCompanyRequest,
    mirror: State<'_, crate::LazyTallyMirror>,
) -> Result<WriteFixtureEnrollmentStatus, TallyCommandError> {
    let mirror = mirror
        .get()
        .await
        .map_err(mirror_unavailable_command_error)?;
    mirror
        .revoke_write_fixture_enrollment(
            &request.mirror_company_id,
            chrono::Utc::now().timestamp_millis(),
        )
        .await
        .map_err(|_| {
            tally_command_error(
                "fixture_enrollment_revoke_failed",
                "Operation",
                "The local fixture enrollment could not be revoked.",
                "safe",
                false,
                "Check the saved company scope and retry; no Tally request was made.",
            )
        })
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
    mirror: State<'_, crate::LazyTallyMirror>,
) -> Result<crate::db::tally_mirror::PersistedCompanyProfilePage, String> {
    let mirror = mirror
        .get()
        .await
        .map_err(mirror_unavailable_string_error)?;
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
    mirror: State<'_, crate::LazyTallyMirror>,
) -> Result<crate::db::tally_mirror::MirrorExplorerPage, String> {
    let mirror = mirror
        .get()
        .await
        .map_err(mirror_unavailable_string_error)?;
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
    mirror: State<'_, crate::LazyTallyMirror>,
) -> Result<TallyEvidenceResponse, String> {
    if request.mirror_company_id.trim().is_empty() {
        return Err("Select a company with an observed stable identity".to_string());
    }
    let mirror = mirror
        .get()
        .await
        .map_err(mirror_unavailable_string_error)?;
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
    mirror: State<'_, crate::LazyTallyMirror>,
) -> Result<RedactedProofExport, String> {
    if request.mirror_company_id.trim().is_empty() || request.proof_id.trim().is_empty() {
        return Err("Select a proof for an observed Tally company".to_string());
    }
    let mirror = mirror
        .get()
        .await
        .map_err(mirror_unavailable_string_error)?;
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
    mirror: State<'_, crate::LazyTallyMirror>,
    runtime: State<'_, TallyRuntime>,
    coordinator: State<'_, SnapshotCoordinator>,
) -> Result<SnapshotJobStatus, String> {
    let mirror = mirror
        .get()
        .await
        .map_err(mirror_unavailable_string_error)?;
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
            .is_some_and(core_snapshot_start_authorized)
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
        .start(plan, connector, mirror.clone())
        .await
        .map_err(str::to_string)
}

#[tauri::command]
pub async fn tally_snapshot_status(
    run_id: String,
    mirror: State<'_, crate::LazyTallyMirror>,
    coordinator: State<'_, SnapshotCoordinator>,
) -> Result<SnapshotJobStatus, String> {
    let mirror = mirror
        .get()
        .await
        .map_err(mirror_unavailable_string_error)?;
    coordinator
        .status(&run_id, mirror)
        .await
        .map_err(str::to_string)
}

#[tauri::command]
pub async fn tally_recent_snapshot_runs(
    mirror: State<'_, crate::LazyTallyMirror>,
    coordinator: State<'_, SnapshotCoordinator>,
) -> Result<Vec<SnapshotJobStatus>, String> {
    let mirror = mirror
        .get()
        .await
        .map_err(mirror_unavailable_string_error)?;
    coordinator.recent(mirror, 20).await.map_err(str::to_string)
}

#[derive(Debug, Deserialize)]
pub struct ResumeCoreSnapshotRequest {
    pub config: TallyConfig,
    pub run_id: String,
}

#[tauri::command]
pub async fn resume_tally_core_snapshot(
    request: ResumeCoreSnapshotRequest,
    mirror: State<'_, crate::LazyTallyMirror>,
    runtime: State<'_, TallyRuntime>,
    coordinator: State<'_, SnapshotCoordinator>,
) -> Result<SnapshotJobStatus, String> {
    let mirror = mirror
        .get()
        .await
        .map_err(mirror_unavailable_string_error)?;
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
        .core_snapshot_resume_evidence_matches_plan(
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
        .start(plan, connector, mirror.clone())
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
        bridge_tally_core::CapabilityFeatureId::ProductAndMode => "product_and_mode",
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
) -> Result<Vec<UntrustedCompanyCandidate>, TallyCommandError> {
    runtime
        .fetch_companies(config)
        .await
        .map(|companies| {
            companies
                .into_iter()
                .map(|company| UntrustedCompanyCandidate { name: company.name })
                .collect()
        })
        .map_err(tally_runtime_command_error)
}

#[derive(Debug, Serialize)]
pub struct UntrustedCompanyCandidate {
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct BootstrapDirectCompanyRequest {
    pub config: TallyConfig,
    pub candidate_name: String,
}

#[tauri::command]
pub async fn bootstrap_direct_tally_company(
    request: BootstrapDirectCompanyRequest,
    runtime: State<'_, TallyRuntime>,
) -> Result<PersistedTallyProbeResult, TallyCommandError> {
    let canonical_origin = EndpointKey::from_config(&request.config)
        .map(|endpoint| endpoint.as_str().to_string())
        .map_err(|_| {
            tally_command_error(
                "endpoint_configuration_invalid",
                "Endpoint configuration",
                "Tally endpoint validation failed",
                "after_change",
                false,
                "Use localhost or a loopback IP and a port from 1 to 65535, then try verification again.",
            )
        })?;
    let (review_id, observed_at_unix_ms, probe) = runtime
        .bootstrap_direct_company_with_observation(request.config, request.candidate_name)
        .await
        .map_err(tally_runtime_command_error)?;
    persisted_tally_probe_result(review_id, canonical_origin, observed_at_unix_ms, probe)
}

#[derive(Debug, Deserialize)]
pub struct CompanyRequest {
    pub config: TallyConfig,
    pub company: String,
    pub expected_company_guid: String,
}

#[derive(Debug, Deserialize)]
pub struct OutstandingsRequest {
    pub config: TallyConfig,
    pub company: String,
    pub expected_company_guid: String,
    pub currency_assertion: OutstandingsCurrencyAssertion,
    /// An explicit operator-selected date when present. Omission preserves
    /// today's date for existing callers and licensed Tally users.
    #[serde(default)]
    pub as_of_yyyymmdd: Option<TallyDate>,
    /// Existing callers predate an operator-visible selector and therefore
    /// retain the established due-date report. New callers must send their
    /// selection so every rendered/exported figure names the same basis.
    #[serde(default)]
    pub ageing_anchor: crate::tally::OutstandingsAgeingAnchor,
}

#[derive(Debug, Deserialize)]
pub struct ExportTrialBalanceRequest {
    pub config: TallyConfig,
    pub company: String,
    pub expected_company_guid: String,
    pub as_of_yyyymmdd: TallyDate,
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
pub async fn fetch_standard_tally_ledger_catalog(
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
        .fetch_standard_ledger_catalog(
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

fn requested_outstandings_as_of(
    explicit_as_of: Option<TallyDate>,
) -> Result<TallyDate, TallyCommandError> {
    explicit_as_of
        .map(Ok)
        .unwrap_or_else(|| TallyDate::parse(chrono::Local::now().format("%Y%m%d").to_string()))
        .map_err(|_| {
            tally_command_error(
                "current_date_invalid",
                "Bridge application",
                "Bridge could not construct today's outstandings date.",
                "after_change",
                false,
                "Check the workstation date and time, then repeat the read-only action.",
            )
        })
}

#[tauri::command]
pub async fn fetch_tally_outstandings(
    request: OutstandingsRequest,
    runtime: State<'_, TallyRuntime>,
) -> Result<OutstandingsLoadResult, TallyCommandError> {
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
    let as_of = requested_outstandings_as_of(request.as_of_yyyymmdd)?;
    runtime
        .fetch_outstandings(
            request.config,
            request.company,
            request.expected_company_guid,
            as_of,
            request.currency_assertion,
            request.ageing_anchor,
        )
        .await
        .map_err(tally_runtime_command_error)
}

/// Reads and saves a GUID-bracketed native, book-to-date Trial Balance.
/// Ledger rows remain Rust-owned; only the resulting path and control summary
/// cross the webview boundary. No Tally write is sent.
#[tauri::command]
pub async fn export_tally_trial_balance(
    app: tauri::AppHandle,
    request: ExportTrialBalanceRequest,
    runtime: State<'_, TallyRuntime>,
) -> Result<TrialBalanceExportSummary, TallyCommandError> {
    use tauri::Manager as _;

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
    let expected_company_guid =
        normalize_company_guid(&request.expected_company_guid).map_err(|_| {
            tally_command_error(
                "company_selection_invalid",
                "Tally application",
                "The selected company GUID is invalid.",
                "after_change",
                false,
                "Select the intended GUID-bearing company and repeat the read-only action.",
            )
        })?;
    let source = runtime
        .fetch_trial_balance_source(
            request.config,
            request.company,
            expected_company_guid,
            request.as_of_yyyymmdd,
        )
        .await
        .map_err(tally_runtime_command_error)?;
    let bytes = render_trial_balance_xlsx(&source).map_err(|_| {
        tally_command_error(
            "trial_balance_render_failed",
            "Financial data",
            "Bridge could not build a Trial Balance whose exact controls and Excel values agree.",
            "after_change",
            false,
            "Keep the book unchanged, then repeat the export. If it fails again, retain the selected period for support.",
        )
    })?;
    let downloads = app
        .path()
        .download_dir()
        .or_else(|_| app.path().home_dir())
        .map_err(|_| {
            tally_command_error(
                "trial_balance_destination_unavailable",
                "Filesystem",
                "Bridge could not locate a folder for the Trial Balance export.",
                "after_change",
                false,
                "Check the workstation Downloads folder and repeat the export.",
            )
        })?;
    let mut company_slug = statement_filename_slug(&source.company);
    company_slug.truncate(100);
    let stem = format!(
        "trial-balance-{company_slug}-{}-to-{}",
        source.from_yyyymmdd, source.to_yyyymmdd
    );
    let path = write_unique_export_file(&downloads, &stem, "xlsx", &bytes, "Trial Balance")
        .map_err(|_| {
            tally_command_error(
                "trial_balance_write_failed",
                "Filesystem",
                "Bridge could not finish writing the Trial Balance export.",
                "after_change",
                false,
                "Check free disk space and Downloads-folder permissions, then repeat the export.",
            )
        })?;
    Ok(TrialBalanceExportSummary {
        path: path.to_string_lossy().into_owned(),
        company: source.company,
        from_yyyymmdd: source.from_yyyymmdd,
        to_yyyymmdd: source.to_yyyymmdd,
        ledger_count: source.trial_balance.rows.len(),
    })
}

/// Reads operator-owned filing labels from ordinary application configuration.
///
/// The helper deliberately degrades to no labels for a missing, empty, corrupt,
/// or unavailable file. It receives no mirror state, so this command cannot
/// initialise SQLCipher or resolve a keychain key.
#[tauri::command]
pub fn load_client_group_labels(app: AppHandle) -> client_groups::ClientGroupLabels {
    let Ok(directory) = app.path().app_config_dir() else {
        return client_groups::ClientGroupLabels::new();
    };
    client_groups::load(&directory)
}

/// Reads the optional all-client sort preference from ordinary application
/// configuration. Like group labels, it never initialises the Tally mirror.
#[tauri::command]
pub fn load_client_sort_preference(app: AppHandle) -> Option<client_groups::ClientSortPreference> {
    let Ok(directory) = app.path().app_config_dir() else {
        return None;
    };
    client_groups::load_sort_preference(&directory)
}

#[derive(Debug, Deserialize)]
pub struct SaveClientGroupLabelRequest {
    pub company_guid: String,
    pub label: String,
}

/// Saves one operator-owned filing label without accessing the Tally mirror.
#[tauri::command]
pub fn save_client_group_label(
    app: AppHandle,
    request: SaveClientGroupLabelRequest,
) -> Result<(), String> {
    if request.company_guid.trim().is_empty() {
        return Err("Bridge could not identify the company for this group label.".to_string());
    }
    let directory = app
        .path()
        .app_config_dir()
        .map_err(|_| "Bridge could not locate its local group-label configuration.".to_string())?;
    client_groups::save_label(&directory, &request.company_guid, &request.label)
        .map_err(|_| "Bridge could not save this group label.".to_string())
}

/// Saves the optional all-client sort preference without accessing the Tally mirror.
#[tauri::command]
pub fn save_client_sort_preference(
    app: AppHandle,
    preference: client_groups::ClientSortPreference,
) -> Result<(), String> {
    let directory = app.path().app_config_dir().map_err(|_| {
        "Bridge could not locate its local client-preference configuration.".to_string()
    })?;
    client_groups::save_sort_preference(&directory, preference)
        .map_err(|_| "Bridge could not save the all-client sort preference.".to_string())
}

#[derive(Debug, Deserialize)]
pub struct AllCompaniesOutstandingsRequest {
    pub config: TallyConfig,
    pub companies: Vec<AllCompaniesEntry>,
    pub currency_assertion: OutstandingsCurrencyAssertion,
    #[serde(default)]
    pub as_of_yyyymmdd: Option<TallyDate>,
    #[serde(default)]
    pub ageing_anchor: crate::tally::OutstandingsAgeingAnchor,
}

#[derive(Debug, Deserialize)]
pub struct AllCompaniesEntry {
    pub company: String,
    pub expected_company_guid: String,
}

#[derive(Debug, Serialize)]
pub struct CompanyOutstandingsEntry {
    pub company: String,
    pub company_guid: String,
    pub result: OutstandingsLoadResult,
}

fn company_sweep_result(
    result: Result<OutstandingsLoadResult, CompanySweepFailure>,
) -> OutstandingsLoadResult {
    let reason = match result {
        Ok(result) => return result,
        Err(CompanySweepFailure::ReasonCode(reason_code)) => {
            crate::tally::OutstandingsPartialReason::code(reason_code)
        }
        Err(CompanySweepFailure::OutstandingsRead) => {
            crate::tally::OutstandingsPartialReason::code("company_outstandings_read_failed")
        }
    };
    OutstandingsLoadResult::Partial {
        reason,
        synced_at_unix_ms: chrono::Utc::now().timestamp_millis(),
    }
}

enum CompanySweepFailure {
    ReasonCode(&'static str),
    OutstandingsRead,
}

fn company_sweep_currency_preflight_failure(
    currency_count: usize,
    is_inr: bool,
) -> Option<&'static str> {
    if currency_count > 1 {
        return Some("company_base_currency_undetermined");
    }
    if currency_count == 1 && !is_inr {
        return Some("company_base_currency_not_inr");
    }
    if currency_count == 1 {
        return None;
    }
    Some("company_currency_probe_failed")
}

/// Reads outstandings for several companies in one action.
///
/// This is the read Tally structurally will not do: it is per-company by
/// design, so a firm holding ten client books has no way to ask one question
/// across them. At roughly 0.35s per company on the native path, ten books
/// answer in about four seconds.
///
/// **Reads run strictly one after another, never concurrently.** Tally's
/// gateway serialises anyway, and the project rule is one live request at a
/// time with a health check between -- issuing these in parallel is the
/// documented way to hang or crash the instance the user is working in.
///
/// A company that fails does not abort the rest: its own typed Partial is
/// recorded and the sweep continues, because one unreadable book must not
/// hide the nine that read cleanly.
#[tauri::command]
pub async fn fetch_tally_outstandings_all_companies(
    request: AllCompaniesOutstandingsRequest,
    runtime: State<'_, TallyRuntime>,
) -> Result<Vec<CompanyOutstandingsEntry>, TallyCommandError> {
    if request.companies.is_empty() {
        return Ok(Vec::new());
    }
    let as_of = requested_outstandings_as_of(request.as_of_yyyymmdd)?;

    let mut entries = Vec::with_capacity(request.companies.len());
    for entry in request.companies {
        let result = if validate_company_name(&entry.company).is_err() {
            Err(CompanySweepFailure::ReasonCode("company_selection_invalid"))
        } else {
            match runtime
                .detect_base_currency(
                    request.config.clone(),
                    entry.company.clone(),
                    entry.expected_company_guid.clone(),
                )
                .await
            {
                Err(_) => Err(CompanySweepFailure::ReasonCode(
                    "company_currency_probe_failed",
                )),
                Ok(currency) => match company_sweep_currency_preflight_failure(
                    currency.currency_count,
                    currency.is_inr,
                ) {
                    Some(reason_code) => Err(CompanySweepFailure::ReasonCode(reason_code)),
                    None => runtime
                        .fetch_outstandings(
                            request.config.clone(),
                            entry.company.clone(),
                            entry.expected_company_guid.clone(),
                            as_of.clone(),
                            request.currency_assertion,
                            request.ageing_anchor,
                        )
                        .await
                        .map_err(|_| CompanySweepFailure::OutstandingsRead),
                },
            }
        };
        entries.push(CompanyOutstandingsEntry {
            company: entry.company,
            company_guid: entry.expected_company_guid,
            result: company_sweep_result(result),
        });
    }
    Ok(entries)
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
        company_sweep_currency_preflight_failure, company_sweep_result,
        first_calendar_day_canary_window, portable_export_file_name, reconcile_review_cleanup,
        reviewed_probe_commitment_sha256, selected_read_observation, tally_command_error,
        tally_runtime_command_error, validate_dsc_pins, write_unique_download, CompanySweepFailure,
        OutstandingsRequest, PersistedTallyCompany, SavedTallySetup,
    };
    // Used only by the `#[cfg(unix)]` non-UTF-8 destination test — an invalid-byte
    // path cannot be constructed portably. The import must carry the same gate as
    // the test, or Windows fails on an unused import under `-D warnings`.
    #[cfg(unix)]
    use super::require_utf8_destination;
    use crate::tally::runtime::TrialBalanceReadError;
    use crate::tally::{
        ConnectionStatus, OutstandingsCurrencyAssertion, OutstandingsLoadResult,
        SelectedReadObservation, TallyCompany, TallyProbeResult, TallyProduct,
    };
    use bridge_tally_core::CapabilityProfile;
    use bridge_tally_protocol::trial_balance::TrialBalanceError;
    use std::collections::BTreeMap;

    /// Regression for the destination-picker leak: `select_party_statement_
    /// destination` used to build the folder's IPC string with
    /// `to_string_lossy()`, which replaces invalid UTF-8 byte sequences with
    /// U+FFFD -- silently turning the folder the operator picked into a
    /// *different* path that likely doesn't exist. This failed before the
    /// fix because the old conversion never returned an `Err` at all: it
    /// always produced a (possibly wrong) string.
    #[cfg(unix)]
    #[test]
    fn require_utf8_destination_rejects_non_utf8_paths_instead_of_rewriting_them() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        // 0xFF is never valid UTF-8 on its own, so this cannot be constructed
        // as a Rust string literal -- it has to come in through the raw byte
        // API a real OS path could actually hand back.
        let invalid_bytes = [b'p', b'i', b'c', b'k', 0xFF, b'e', b'd'];
        let path = std::path::PathBuf::from(OsStr::from_bytes(&invalid_bytes));

        let error =
            require_utf8_destination(path).expect_err("non-UTF-8 folder names must be rejected");

        assert!(
            !error.contains('\u{FFFD}'),
            "must not silently substitute a replacement character: {error}"
        );
        assert!(error.to_lowercase().contains("unicode"));
    }

    #[test]
    fn dsc_pin_input_is_strictly_bounded() {
        assert!(validate_dsc_pins(&["1234".to_string()]).is_ok());
        assert!(validate_dsc_pins(&["".to_string()]).is_err());
        assert!(validate_dsc_pins(&["1\n2".to_string()]).is_err());
        assert!(validate_dsc_pins(&["x".repeat(129)]).is_err());
        assert!(validate_dsc_pins(&["1".to_string(), "2".to_string()]).is_err());
    }

    #[test]
    fn outstandings_accepts_only_an_explicit_inr_currency_assertion() {
        let accepted: OutstandingsRequest = serde_json::from_value(serde_json::json!({
            "config": { "host": "127.0.0.1", "port": 9000 },
            "company": "Synthetic Company",
            "expected_company_guid": "synthetic-guid",
            "currency_assertion": "INR"
        }))
        .expect("INR is the one supported explicit assertion");
        assert_eq!(
            accepted.currency_assertion,
            OutstandingsCurrencyAssertion::Inr
        );

        let rejected = serde_json::from_value::<OutstandingsRequest>(serde_json::json!({
            "config": { "host": "127.0.0.1", "port": 9000 },
            "company": "Synthetic Company",
            "expected_company_guid": "synthetic-guid",
            "currency_assertion": "USD"
        }));
        assert!(
            rejected.is_err(),
            "unsupported currencies must not start a scan"
        );
    }

    #[test]
    fn company_sweep_keeps_probe_and_read_failures_in_band() {
        let outcomes = [
            Ok(OutstandingsLoadResult::Partial {
                reason: crate::tally::OutstandingsPartialReason::code("first_book_partial"),
                synced_at_unix_ms: 1,
            }),
            Err(CompanySweepFailure::ReasonCode(
                "company_currency_probe_failed",
            )),
            Err(CompanySweepFailure::ReasonCode(
                "company_base_currency_undetermined",
            )),
            Err(CompanySweepFailure::ReasonCode(
                "company_outstandings_read_failed",
            )),
            Ok(OutstandingsLoadResult::Partial {
                reason: crate::tally::OutstandingsPartialReason::code("last_book_partial"),
                synced_at_unix_ms: 2,
            }),
        ]
        .into_iter()
        .map(company_sweep_result)
        .collect::<Vec<_>>();

        assert_eq!(
            outcomes.len(),
            5,
            "one bad book must not truncate the sweep"
        );
        assert!(matches!(
            &outcomes[1],
            OutstandingsLoadResult::Partial { reason, .. }
                if reason.reason_code == "company_currency_probe_failed"
        ));
        assert!(matches!(
            &outcomes[2],
            OutstandingsLoadResult::Partial { reason, .. }
                if reason.reason_code == "company_base_currency_undetermined"
        ));
        assert!(matches!(
            &outcomes[3],
            OutstandingsLoadResult::Partial { reason, .. }
                if reason.reason_code == "company_outstandings_read_failed"
        ));
        assert!(matches!(
            &outcomes[4],
            OutstandingsLoadResult::Partial { reason, .. }
                if reason.reason_code == "last_book_partial"
        ));
    }

    #[test]
    fn company_sweep_currency_preflight_names_undetermined_base_currency() {
        assert_eq!(
            company_sweep_currency_preflight_failure(2, false),
            Some("company_base_currency_undetermined"),
            "several currency masters do not identify the company's base currency"
        );
        assert_eq!(
            company_sweep_currency_preflight_failure(2, true),
            Some("company_base_currency_undetermined"),
            "the parser's INR flag is not authoritative when several currency masters exist"
        );
        assert_eq!(
            company_sweep_currency_preflight_failure(1, false),
            Some("company_base_currency_not_inr"),
            "one non-Indian currency identifies an unsupported base currency"
        );
        assert_eq!(company_sweep_currency_preflight_failure(1, true), None);
        assert_eq!(
            company_sweep_currency_preflight_failure(0, false),
            Some("company_currency_probe_failed"),
            "an impossible empty collection remains fail-closed"
        );
    }

    #[test]
    fn company_sweep_preserves_runtime_foreign_currency_partial() {
        let outcome = company_sweep_result(Ok(OutstandingsLoadResult::Partial {
            reason: crate::tally::OutstandingsPartialReason::foreign_currency_ledger_balance(
                "Synthetic FX Debtor".to_string(),
            ),
            synced_at_unix_ms: 1,
        }));

        assert!(matches!(
            outcome,
            OutstandingsLoadResult::Partial { reason, .. }
                if reason.reason_code == "company_foreign_currency_ledger_balance"
                    && reason.foreign_currency_ledger_name.as_deref() == Some("Synthetic FX Debtor")
        ));
    }

    #[test]
    fn export_names_are_portable_and_reserved_devices_are_neutralized() {
        assert_eq!(
            portable_export_file_name("outstandings-A:B?C.csv").unwrap(),
            "outstandings-A-B-C.csv"
        );
        assert_eq!(portable_export_file_name("CON.csv").unwrap(), "_CON.csv");
        assert!(portable_export_file_name("../outside.csv").is_err());
        assert!(portable_export_file_name("nested/report.csv").is_err());
    }

    #[test]
    fn report_downloads_never_overwrite_an_existing_file() {
        let directory = tempfile::tempdir().expect("temporary download directory");
        let first = write_unique_download(directory.path(), "report.csv", b"first").unwrap();
        let second = write_unique_download(directory.path(), "report.csv", b"second").unwrap();

        assert_eq!(first.file_name().unwrap(), "report.csv");
        assert_eq!(second.file_name().unwrap(), "report-2.csv");
        assert_eq!(std::fs::read(first).unwrap(), b"first");
        assert_eq!(std::fs::read(second).unwrap(), b"second");
    }

    #[test]
    fn snapshot_capability_canary_is_exactly_the_requested_first_calendar_day() {
        let canary = first_calendar_day_canary_window("20260228").unwrap();
        assert_eq!(canary.range.from_yyyymmdd, "20260228");
        assert_eq!(canary.range.to_yyyymmdd, "20260228");
        assert_eq!(canary.query_profile.as_str(), "core_accounting_v3");

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

        let deadline = tally_runtime_command_error(
            anyhow::Error::new(bridge_tally_transport::TallyTransportError::RequestTimedOut)
                .context("outstandings second segment read failed for 20251002..20251101"),
        );
        assert_eq!(deadline.code, "tally_request_deadline_exceeded");
        assert_eq!(deadline.retry, "after_change");
        assert!(deadline.remediation.contains("Do not retry"));

        let discovery_limit = tally_runtime_command_error(anyhow::anyhow!(
            "interactive discovery listing limit exceeded: synthetic company response"
        ));
        assert_eq!(discovery_limit.code, "untrusted_discovery_limit_exceeded");
        assert_eq!(discovery_limit.category, "Discovery listing");

        for (source, expected_code) in [
            (
                TrialBalanceReadError::AsOfPrecedesBooksFrom,
                "trial_balance_as_of_precedes_books_from",
            ),
            (
                TrialBalanceReadError::PeriodBoundaryUnsupported,
                "trial_balance_period_boundary_unsupported",
            ),
            (
                TrialBalanceReadError::SnapshotDrifted,
                "trial_balance_snapshot_drifted",
            ),
            (
                TrialBalanceReadError::BookChanged,
                "trial_balance_book_changed",
            ),
        ] {
            let mapped = tally_runtime_command_error(anyhow::Error::new(source));
            assert_eq!(mapped.code, expected_code);
            assert_eq!(mapped.category, "Financial data");
            assert!(mapped.local_state_changed);
            assert!(!mapped.message.contains("endpoint"));
        }

        let unverified_opening = tally_runtime_command_error(anyhow::Error::new(
            TrialBalanceError::InvalidResponse("opening_difference_unverified"),
        ));
        assert_eq!(unverified_opening.code, "trial_balance_response_unverified");
        assert_eq!(unverified_opening.category, "Financial data");
        assert!(!unverified_opening.message.contains("opening_difference"));
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

fn portable_export_file_name(file_name: &str) -> Result<String, String> {
    let trimmed = file_name.trim();
    if trimmed.is_empty()
        || trimmed.len() > 200
        || trimmed.contains('/')
        || trimmed.contains('\\')
        || trimmed.contains("..")
        || trimmed.starts_with('.')
    {
        return Err("Bridge could not build a safe file name for this export.".to_string());
    }

    let mut sanitized = trimmed
        .chars()
        .map(|character| {
            if character.is_control()
                || matches!(character, '<' | '>' | ':' | '"' | '|' | '?' | '*')
            {
                '-'
            } else {
                character
            }
        })
        .collect::<String>();
    while sanitized.ends_with([' ', '.']) {
        sanitized.pop();
    }
    if sanitized.is_empty() {
        return Err("Bridge could not build a safe file name for this export.".to_string());
    }

    let stem = sanitized
        .split_once('.')
        .map_or(sanitized.as_str(), |(stem, _)| stem);
    let upper_stem = stem.to_ascii_uppercase();
    let reserved = matches!(upper_stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || upper_stem.strip_prefix("COM").is_some_and(|suffix| {
            matches!(suffix, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
        })
        || upper_stem.strip_prefix("LPT").is_some_and(|suffix| {
            matches!(suffix, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
        });
    if reserved {
        sanitized.insert(0, '_');
    }
    Ok(sanitized)
}

fn write_unique_download(
    directory: &std::path::Path,
    file_name: &str,
    contents: &[u8],
) -> std::io::Result<std::path::PathBuf> {
    use std::io::Write as _;

    let path = std::path::Path::new(file_name);
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("report");
    let extension = path.extension().and_then(|value| value.to_str());
    for number in 1_u32..=10_000 {
        let candidate_name = if number == 1 {
            file_name.to_string()
        } else if let Some(extension) = extension {
            format!("{stem}-{number}.{extension}")
        } else {
            format!("{stem}-{number}")
        };
        let candidate = directory.join(candidate_name);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(mut file) => {
                if let Err(error) = file.write_all(contents).and_then(|()| file.sync_all()) {
                    drop(file);
                    let _ = std::fs::remove_file(&candidate);
                    return Err(error);
                }
                return Ok(candidate);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "too many exports already use this file name",
    ))
}

/// Writes an exported report to the user's Downloads folder and returns the
/// full path.
///
/// The browser route (`Blob` + an `<a download>` click) silently does nothing
/// inside the Tauri webview -- there is no download handler and the app
/// declares no plugin permissions -- so the button appeared to work and
/// produced no file. Writing from Rust needs no new dependency and no
/// capability grant, and returning the path lets the UI say where it went
/// instead of leaving the user to guess.
#[tauri::command]
pub async fn save_report_download(
    app: tauri::AppHandle,
    file_name: String,
    contents: String,
) -> Result<String, String> {
    save_report_download_bytes(&app, &file_name, contents.as_bytes())
}

/// Shared byte-oriented implementation for every local report export.
///
/// The public command keeps its text-only IPC contract for CSV exports; binary
/// formats use this same checked path after their renderer has produced bytes.
fn save_report_download_bytes(
    app: &tauri::AppHandle,
    file_name: &str,
    contents: &[u8],
) -> Result<String, String> {
    use tauri::Manager as _;
    let file_name = checked_export_file_name(file_name)?;
    // Tauri's own path resolver, so this needs no extra crate and no
    // capability grant.
    let downloads = app
        .path()
        .download_dir()
        .or_else(|_| app.path().home_dir())
        .map_err(|_| "Bridge could not locate a folder to save into.".to_string())?;
    let path = write_unique_download(&downloads, &file_name, contents)
        .map_err(|error| format!("Bridge could not write the export: {error}"))?;
    Ok(path.to_string_lossy().into_owned())
}

fn checked_export_file_name(file_name: &str) -> Result<String, String> {
    portable_export_file_name(file_name)
}

/// Reveals an exported file in the OS file manager.
///
/// Only ever called with a path this process just wrote, and the path is
/// re-checked as an existing file before being handed to the platform tool --
/// so a caller cannot use this to launch an arbitrary target.
#[tauri::command]
pub async fn reveal_exported_file(path: String) -> Result<(), String> {
    let target = std::path::PathBuf::from(&path);
    if !target.is_file() {
        return Err("Bridge could not find that export any more.".to_string());
    }

    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = std::process::Command::new("open");
        command.arg("-R").arg(&target);
        command
    };
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = std::process::Command::new("explorer");
        // `explorer` wants the selector and path as one argument.
        command.arg(format!("/select,{}", target.display()));
        command
    };
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    let mut command = {
        let parent = target.parent().unwrap_or(&target);
        let mut command = std::process::Command::new("xdg-open");
        command.arg(parent);
        command
    };

    command
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("Bridge could not open the folder: {error}"))
}

#[derive(Debug, Deserialize)]
pub struct ExportPartyStatementRequest {
    pub company: String,
    pub as_of_yyyymmdd: String,
    pub party: String,
    /// XLSX remains the default for callers that predate the PDF option.
    #[serde(default)]
    pub format: PartyStatementFormat,
    #[serde(default)]
    pub ageing_anchor: crate::tally::OutstandingsAgeingAnchor,
    /// The `open_bills`/`unallocated_by_party` rows the frontend already
    /// holds from `fetch_tally_outstandings`. This command reads no Tally
    /// endpoint of its own -- `OutstandingsLoadResult::Complete` already
    /// carries every fact a statement needs.
    pub open_bills: Vec<OpenBillRow>,
    pub unallocated_by_party: Vec<UnallocatedParty>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PartyStatementFormat {
    #[default]
    Xlsx,
    Pdf,
}

#[derive(Debug, Deserialize)]
pub struct ExportBulkPartyStatementsRequest {
    pub company: String,
    pub as_of_yyyymmdd: String,
    pub format: PartyStatementFormat,
    #[serde(default)]
    pub ageing_anchor: crate::tally::OutstandingsAgeingAnchor,
    /// Returned by the native folder picker. The command verifies it against
    /// that picker result, then still checks it exists and is a directory
    /// before any statement name is joined to it.
    pub destination: String,
    /// Opaque, single-use proof returned with the native picker destination.
    /// A renderer cannot mint this proof for an arbitrary local path.
    pub approval_id: String,
    /// These are complete statement-source rows from the finished local read,
    /// not the dashboard's display projections.
    pub open_bills: Vec<OpenBillRow>,
    pub unallocated_by_party: Vec<UnallocatedParty>,
}

#[derive(Debug, Deserialize)]
pub struct PreviewBulkPartyStatementsRequest {
    /// The same complete rows the export command will consume. This command
    /// performs no I/O or Tally read; it only makes the pending scope explicit.
    pub open_bills: Vec<OpenBillRow>,
    pub unallocated_by_party: Vec<UnallocatedParty>,
}

#[derive(Debug, Serialize)]
pub struct BulkPartyStatementsPreview {
    pub party_count: usize,
}

/// The picker result the renderer must preserve verbatim until it starts the
/// matching export. The proof is intentionally opaque to the UI.
#[derive(Debug, Serialize)]
pub struct PartyStatementDestinationSelection {
    pub destination: String,
    pub approval_id: String,
}

/// Lets the operator choose where the whole statement batch will be written.
#[tauri::command]
pub async fn select_party_statement_destination(
    approvals: State<'_, PartyStatementDestinationApprovals>,
) -> Result<Option<PartyStatementDestinationSelection>, String> {
    let selected = tokio::task::spawn_blocking(|| {
        rfd::FileDialog::new()
            .set_title("Choose a folder for party statements")
            .pick_folder()
    })
    .await
    .map_err(|_| "Bridge could not open the statement destination picker.".to_string())?;

    let Some(selected) = selected else {
        return Ok(None);
    };
    let destination = require_utf8_destination(selected)?;
    let approval_id = approvals
        .issue(std::path::PathBuf::from(&destination))
        .map_err(|_| {
            "Bridge could not record the statement destination. Choose the folder again."
                .to_string()
        })?;
    Ok(Some(PartyStatementDestinationSelection {
        destination,
        approval_id,
    }))
}

/// Releases a picker approval when the renderer abandons its pending export.
#[tauri::command]
pub async fn revoke_party_statement_destination(
    approval_id: String,
    approvals: State<'_, PartyStatementDestinationApprovals>,
) -> Result<(), String> {
    approvals.revoke(&approval_id).map_err(|_| {
        "Bridge could not release the statement destination. Choose the folder again.".to_string()
    })
}

/// Converts a user-picked folder into the UTF-8 text Bridge's own IPC
/// boundary and file APIs require.
///
/// `to_string_lossy` is deliberately not used here: it silently replaces any
/// byte sequence that isn't valid UTF-8 with U+FFFD, which turns the picked
/// path into a *different* path -- one that likely does not exist. The
/// statement batch would then either fail with a confusing "destination does
/// not exist" error, or land somewhere other than the folder the operator
/// actually chose. Failing closed with a clear message beats guessing.
fn require_utf8_destination(path: std::path::PathBuf) -> Result<String, String> {
    path.into_os_string().into_string().map_err(|_| {
        "Bridge could not use that folder because its name is not valid Unicode text. \
         Choose a different folder, or rename it using standard characters."
            .to_string()
    })
}

/// Counts the unique, non-zero parties that the bulk writer will process.
/// Kept beside the writer's source conversion so the confirmation cannot use
/// a separately implemented frontend approximation of the export scope.
#[tauri::command]
pub async fn preview_bulk_party_statements(
    request: PreviewBulkPartyStatementsRequest,
) -> Result<BulkPartyStatementsPreview, String> {
    Ok(BulkPartyStatementsPreview {
        party_count: bulk_party_statement_party_count(
            &request.open_bills,
            &request.unallocated_by_party,
        ),
    })
}

/// Writes a separate statement for every party in the completed source rows.
/// A failed party remains visible in the returned result and manifest while
/// the remaining parties continue, so an operator cannot mistake a partial
/// batch for a complete send-ready set.
#[tauri::command]
pub async fn export_bulk_party_statements(
    request: ExportBulkPartyStatementsRequest,
    approvals: State<'_, PartyStatementDestinationApprovals>,
) -> Result<
    crate::reports::bulk_party_statement::BulkPartyStatementResult,
    BulkPartyStatementExportError,
> {
    export_bulk_party_statements_at_selected_destination(request, &approvals)
}

fn export_bulk_party_statements_at_selected_destination(
    request: ExportBulkPartyStatementsRequest,
    approvals: &PartyStatementDestinationApprovals,
) -> Result<
    crate::reports::bulk_party_statement::BulkPartyStatementResult,
    BulkPartyStatementExportError,
> {
    let approved_destination = approvals
        .consume(
            &request.approval_id,
            std::path::Path::new(&request.destination),
        )
        .map_err(|_| {
            BulkPartyStatementExportError::DestinationNotAuthorized(
                party_statement_destination_not_authorized_error(),
            )
        })?;

    let result = match request.format {
        PartyStatementFormat::Xlsx => {
            write_bulk_party_statements_with_ageing_anchor(BulkPartyStatementRequest {
                destination: &approved_destination,
                company: &request.company,
                as_of_yyyymmdd: &request.as_of_yyyymmdd,
                format: "xlsx",
                open_bills: &request.open_bills,
                unallocated_by_party: &request.unallocated_by_party,
                ageing_anchor: request.ageing_anchor,
                render: |statement: &crate::reports::party_statement::PartyStatement| {
                    render_party_statement_xlsx(statement).map_err(|error| error.to_string())
                },
            })
        }
        PartyStatementFormat::Pdf => {
            write_bulk_party_statements_with_ageing_anchor(BulkPartyStatementRequest {
                destination: &approved_destination,
                company: &request.company,
                as_of_yyyymmdd: &request.as_of_yyyymmdd,
                format: "pdf",
                open_bills: &request.open_bills,
                unallocated_by_party: &request.unallocated_by_party,
                ageing_anchor: request.ageing_anchor,
                render: |statement: &crate::reports::party_statement::PartyStatement| {
                    render_party_statement_pdf(statement).map_err(|error| error.to_string())
                },
            })
        }
    };
    result.map_err(BulkPartyStatementExportError::Existing)
}

/// Builds one party's aged-bills statement in the requested format and writes
/// it to the user's Downloads folder.
///
/// Mirrors `save_report_download` exactly: Tauri's own path resolver, the
/// same filename-traversal guard, and the full written path returned so the
/// UI can say where the file went instead of leaving the operator to guess.
#[tauri::command]
pub async fn export_party_statement(
    app: tauri::AppHandle,
    request: ExportPartyStatementRequest,
) -> Result<String, String> {
    use tauri::Manager as _;

    let statement = build_party_statement_with_ageing_anchor(
        &request.company,
        &request.as_of_yyyymmdd,
        &request.party,
        &request.open_bills,
        &request.unallocated_by_party,
        request.ageing_anchor,
    )
    .map_err(|error| match error {
        PartyStatementError::PartyNotFound => {
            "Bridge no longer has exposure on record for this party — refresh and try again."
                .to_string()
        }
        PartyStatementError::ArithmeticOverflow => {
            "Bridge could not total this party's statement exactly.".to_string()
        }
    })?;

    let (bytes, extension) = match request.format {
        PartyStatementFormat::Xlsx => (
            render_party_statement_xlsx(&statement)
                .map_err(|error| format!("Bridge could not build the statement: {error}"))?,
            "xlsx",
        ),
        PartyStatementFormat::Pdf => (
            render_party_statement_pdf(&statement)
                .map_err(|error| format!("Bridge could not build the statement: {error}"))?,
            "pdf",
        ),
    };

    let mut slug = statement_filename_slug(&statement.party);
    slug.truncate(150);
    let stem = format!("statement-{slug}-{}", statement.as_of_yyyymmdd);

    let downloads = app
        .path()
        .download_dir()
        .or_else(|_| app.path().home_dir())
        .map_err(|_| "Bridge could not locate a folder to save into.".to_string())?;
    let path = write_unique_statement_file(&downloads, &stem, extension, &bytes)?;
    Ok(path.to_string_lossy().into_owned())
}

/// Writes a new statement filename without ever replacing an earlier export.
/// `create_new` closes the race between checking a candidate and writing it;
/// a repeat export becomes `-2`, `-3`, and so on rather than a silent loss.
fn write_unique_statement_file(
    destination: &std::path::Path,
    stem: &str,
    extension: &str,
    bytes: &[u8],
) -> Result<std::path::PathBuf, String> {
    write_unique_export_file(destination, stem, extension, bytes, "statement")
}

fn write_unique_export_file(
    destination: &std::path::Path,
    stem: &str,
    extension: &str,
    bytes: &[u8],
    export_label: &str,
) -> Result<std::path::PathBuf, String> {
    if stem.is_empty()
        || stem.len() > 190
        || std::path::Path::new(stem).components().count() != 1
        || extension.is_empty()
        || extension.contains('.')
    {
        return Err("Bridge could not build a safe file name for this export.".to_string());
    }
    for sequence in 1..=10_000_u32 {
        let suffix = if sequence == 1 {
            String::new()
        } else {
            format!("-{sequence}")
        };
        let path = destination.join(format!("{stem}{suffix}.{extension}"));
        let mut file = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "Bridge could not create the {export_label} export: {error}"
                ))
            }
        };
        if let Err(error) = std::io::Write::write_all(&mut file, bytes) {
            drop(file);
            let _ = std::fs::remove_file(&path);
            return Err(format!(
                "Bridge could not finish writing the {export_label} export: {error}"
            ));
        }
        return Ok(path);
    }
    Err("Bridge could not find an unused statement filename after 10,000 attempts.".to_string())
}

/// Lower-cases and hyphenates a party name into a filesystem-safe slug,
/// collapsing runs of punctuation/whitespace into a single `-` rather than
/// leaving `statement----------20260808.xlsx`.
fn statement_filename_slug(party: &str) -> String {
    let mut slug = String::with_capacity(party.len());
    let mut previous_was_dash = false;
    for ch in party.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            previous_was_dash = false;
        } else if !previous_was_dash {
            slug.push('-');
            previous_was_dash = true;
        }
    }
    let trimmed = slug.trim_matches('-');
    if trimmed.is_empty() {
        "party".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod statement_export_tests {
    use super::*;

    #[test]
    fn repeated_statement_exports_get_distinct_new_files() {
        let destination = tempfile::tempdir().expect("synthetic destination");
        let first = write_unique_statement_file(
            destination.path(),
            "statement-party-20260809",
            "xlsx",
            b"one",
        )
        .expect("first statement");
        let second = write_unique_statement_file(
            destination.path(),
            "statement-party-20260809",
            "xlsx",
            b"two",
        )
        .expect("second statement");

        assert_eq!(
            first.file_name().and_then(|name| name.to_str()),
            Some("statement-party-20260809.xlsx")
        );
        assert_eq!(
            second.file_name().and_then(|name| name.to_str()),
            Some("statement-party-20260809-2.xlsx")
        );
        assert_eq!(std::fs::read(first).expect("first bytes"), b"one");
        assert_eq!(std::fs::read(second).expect("second bytes"), b"two");
    }
}

#[cfg(test)]
mod party_statement_export_tests {
    use super::*;

    #[test]
    fn utf8_picker_destination_round_trips_to_the_authorized_path() {
        let destination = tempfile::tempdir().expect("temporary destination");
        let selected_path = destination.path().to_path_buf();
        let approvals = PartyStatementDestinationApprovals::default();
        let approval_id = approvals
            .issue(selected_path.clone())
            .expect("approve picker destination");
        let ipc_destination =
            require_utf8_destination(selected_path).expect("temporary destination is valid UTF-8");

        let approved = approvals
            .consume(&approval_id, std::path::Path::new(&ipc_destination))
            .expect("IPC path reconstruction retains the selected destination");

        assert_eq!(approved.path(), std::path::Path::new(&ipc_destination));
    }

    fn bulk_export_request(
        destination: &std::path::Path,
        approval_id: &str,
    ) -> ExportBulkPartyStatementsRequest {
        ExportBulkPartyStatementsRequest {
            company: "Synthetic Books Pvt Ltd".to_string(),
            as_of_yyyymmdd: "20260808".to_string(),
            format: PartyStatementFormat::Xlsx,
            ageing_anchor: crate::tally::OutstandingsAgeingAnchor::DueDate,
            destination: destination.to_string_lossy().into_owned(),
            approval_id: approval_id.to_string(),
            open_bills: vec![OpenBillRow {
                party: "Synthetic Party".to_string(),
                reference: "SYNTHETIC-1".to_string(),
                bill_date: "20260801".to_string(),
                due_date: "20260831".to_string(),
                amount: bridge_tally_core::ExactDecimal::parse("100.00").expect("synthetic amount"),
                age_days: Some(7),
                kind: crate::tally::ExposureDirection::Receivable,
            }],
            unallocated_by_party: Vec::new(),
        }
    }

    #[test]
    fn statement_export_format_defaults_to_xlsx_and_accepts_pdf() {
        let base = serde_json::json!({
            "company": "Synthetic Books Pvt Ltd",
            "as_of_yyyymmdd": "20260808",
            "party": "Synthetic Party",
            "open_bills": [],
            "unallocated_by_party": [],
        });
        let defaulted: ExportPartyStatementRequest = serde_json::from_value(base.clone()).unwrap();
        assert!(matches!(defaulted.format, PartyStatementFormat::Xlsx));
        assert!(matches!(
            defaulted.ageing_anchor,
            crate::tally::OutstandingsAgeingAnchor::DueDate
        ));

        let mut pdf = base;
        pdf["format"] = serde_json::Value::String("pdf".to_string());
        let pdf: ExportPartyStatementRequest = serde_json::from_value(pdf).unwrap();
        assert!(matches!(pdf.format, PartyStatementFormat::Pdf));
    }

    #[test]
    fn bulk_statement_export_defaults_the_ageing_anchor_for_legacy_callers() {
        let request: ExportBulkPartyStatementsRequest = serde_json::from_value(serde_json::json!({
            "company": "Synthetic Books Pvt Ltd",
            "as_of_yyyymmdd": "20260808",
            "format": "xlsx",
            "destination": "/tmp/statements",
            "approval_id": "synthetic-approval",
            "open_bills": [],
            "unallocated_by_party": [],
        }))
        .unwrap();

        assert!(matches!(
            request.ageing_anchor,
            crate::tally::OutstandingsAgeingAnchor::DueDate
        ));
    }

    #[test]
    fn bulk_statement_export_rejects_an_unselected_destination_without_writing() {
        let destination = tempfile::tempdir().expect("unselected synthetic destination");
        let approvals = PartyStatementDestinationApprovals::default();

        let error = export_bulk_party_statements_at_selected_destination(
            bulk_export_request(destination.path(), "not-issued"),
            &approvals,
        )
        .expect_err("an unselected destination must be rejected");

        match error {
            BulkPartyStatementExportError::DestinationNotAuthorized(error) => {
                assert_eq!(error.code, "statement_destination_not_authorized");
                assert_eq!(error.retry, "after_change");
                assert!(!error.local_state_changed);
            }
            BulkPartyStatementExportError::Existing(error) => {
                panic!("expected typed destination error, got {error}");
            }
        }
        assert!(
            std::fs::read_dir(destination.path())
                .expect("destination remains readable")
                .next()
                .is_none(),
            "the rejected batch must not write a statement or manifest"
        );
    }

    #[test]
    fn bulk_statement_export_writes_to_the_destination_approved_by_the_picker() {
        let destination = tempfile::tempdir().expect("selected synthetic destination");
        let approvals = PartyStatementDestinationApprovals::default();
        let approval_id = approvals
            .issue(destination.path().to_path_buf())
            .expect("record picker destination");

        let result = export_bulk_party_statements_at_selected_destination(
            bulk_export_request(destination.path(), &approval_id),
            &approvals,
        )
        .expect("recorded destination is accepted");

        assert_eq!(result.written.len(), 1);
        assert!(std::path::Path::new(&result.manifest_path).is_file());
        assert!(destination
            .path()
            .join(&result.written[0].file_name)
            .is_file());
    }

    #[test]
    fn independent_picker_approvals_export_to_their_own_destinations() {
        let first_destination = tempfile::tempdir().expect("first synthetic destination");
        let second_destination = tempfile::tempdir().expect("second synthetic destination");
        let approvals = PartyStatementDestinationApprovals::default();
        // Both selections exist before either export starts; neither replaces
        // the other as the previous singleton store did.
        let first_approval = approvals
            .issue(first_destination.path().to_path_buf())
            .expect("approve first destination");
        let second_approval = approvals
            .issue(second_destination.path().to_path_buf())
            .expect("approve second destination");

        let first = export_bulk_party_statements_at_selected_destination(
            bulk_export_request(first_destination.path(), &first_approval),
            &approvals,
        )
        .expect("first approved destination writes");
        let second = export_bulk_party_statements_at_selected_destination(
            bulk_export_request(second_destination.path(), &second_approval),
            &approvals,
        )
        .expect("second approved destination writes");

        assert!(std::path::Path::new(&first.manifest_path).starts_with(first_destination.path()));
        assert!(std::path::Path::new(&second.manifest_path).starts_with(second_destination.path()));
        assert!(first_destination
            .path()
            .join(&first.written[0].file_name)
            .is_file());
        assert!(second_destination
            .path()
            .join(&second.written[0].file_name)
            .is_file());
    }

    #[test]
    fn cancelled_picker_leaves_no_approval_that_can_export() {
        let destination = tempfile::tempdir().expect("cancelled synthetic destination");
        let approvals = PartyStatementDestinationApprovals::default();

        let error = export_bulk_party_statements_at_selected_destination(
            bulk_export_request(destination.path(), "no-picker-selection"),
            &approvals,
        )
        .expect_err("a cancelled picker has no approval to consume");

        assert!(matches!(
            error,
            BulkPartyStatementExportError::DestinationNotAuthorized(error)
                if error.code == "statement_destination_not_authorized"
        ));
        assert!(
            std::fs::read_dir(destination.path())
                .expect("destination remains readable")
                .next()
                .is_none(),
            "the cancelled selection must not write a statement or manifest"
        );
    }

    #[test]
    fn approval_destination_mismatch_is_rejected_without_writing() {
        let approved_destination = tempfile::tempdir().expect("approved synthetic destination");
        let requested_destination = tempfile::tempdir().expect("requested synthetic destination");
        let approvals = PartyStatementDestinationApprovals::default();
        let approval_id = approvals
            .issue(approved_destination.path().to_path_buf())
            .expect("approve first destination");

        let error = export_bulk_party_statements_at_selected_destination(
            bulk_export_request(requested_destination.path(), &approval_id),
            &approvals,
        )
        .expect_err("an approval cannot be substituted for another destination");

        assert!(matches!(
            error,
            BulkPartyStatementExportError::DestinationNotAuthorized(error)
                if error.code == "statement_destination_not_authorized"
        ));
        let reuse_error = export_bulk_party_statements_at_selected_destination(
            bulk_export_request(approved_destination.path(), &approval_id),
            &approvals,
        )
        .expect_err("a mismatched approval must be consumed");
        assert!(matches!(
            reuse_error,
            BulkPartyStatementExportError::DestinationNotAuthorized(error)
                if error.code == "statement_destination_not_authorized"
        ));
        assert!(
            std::fs::read_dir(requested_destination.path())
                .expect("requested destination remains readable")
                .next()
                .is_none(),
            "the rejected mismatch must not write a statement or manifest"
        );
        assert!(
            std::fs::read_dir(approved_destination.path())
                .expect("approved destination remains readable")
                .next()
                .is_none(),
            "a consumed mismatch approval must not write after a retry"
        );
    }

    #[test]
    fn bulk_statement_export_keeps_the_existing_deleted_destination_failure() {
        let destination = tempfile::tempdir().expect("selected synthetic destination");
        let destination_path = destination.path().to_path_buf();
        let approvals = PartyStatementDestinationApprovals::default();
        let approval_id = approvals
            .issue(destination_path.clone())
            .expect("record picker destination");
        destination.close().expect("remove selected destination");

        let error = export_bulk_party_statements_at_selected_destination(
            bulk_export_request(&destination_path, &approval_id),
            &approvals,
        )
        .expect_err("a deleted selected destination must fail the existing directory check");

        assert!(matches!(
            error,
            BulkPartyStatementExportError::Existing(message)
                if message == "Bridge could not use that statement destination folder."
        ));
    }

    #[test]
    fn statement_export_rejects_unknown_bill_direction_at_the_ipc_boundary() {
        let request = serde_json::json!({
            "company": "Synthetic Books Pvt Ltd",
            "as_of_yyyymmdd": "20260808",
            "party": "Synthetic Party",
            "open_bills": [{
                "party": "Synthetic Party",
                "reference": "INV-1",
                "bill_date": "20260801",
                "due_date": "20260831",
                "amount": "100.00",
                "age_days": 7,
                "kind": "unknown"
            }],
            "unallocated_by_party": []
        });

        assert!(serde_json::from_value::<ExportPartyStatementRequest>(request).is_err());
    }

    #[test]
    fn local_export_file_names_reject_path_like_and_hidden_values() {
        assert_eq!(
            checked_export_file_name(" statement.csv ").unwrap(),
            "statement.csv"
        );
        for name in [
            "",
            ".hidden.csv",
            "../statement.csv",
            "nested/report.csv",
            "nested\\report.csv",
        ] {
            assert!(
                checked_export_file_name(name).is_err(),
                "{name:?} must be rejected"
            );
        }
    }

    #[test]
    fn statement_party_slug_is_portable_and_nonempty() {
        assert_eq!(statement_filename_slug("  ../Aarav & Sons  "), "aarav-sons");
        assert_eq!(statement_filename_slug("///"), "party");
    }
}

#[derive(Debug, Deserialize)]
pub struct BaseCurrencyRequest {
    pub config: TallyConfig,
    pub company: String,
    pub expected_company_guid: String,
}

/// Establishes a company's base currency from Tally.
#[tauri::command]
pub async fn detect_tally_base_currency(
    request: BaseCurrencyRequest,
    runtime: State<'_, TallyRuntime>,
) -> Result<bridge_tally_protocol::native_outstandings::CompanyCurrency, TallyCommandError> {
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
        .detect_base_currency(
            request.config,
            request.company,
            request.expected_company_guid,
        )
        .await
        .map_err(tally_runtime_command_error)
}
