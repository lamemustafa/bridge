use crate::db::tally_incremental::IncrementalFoundationEvidence;
use crate::db::tally_mirror::{
    company_profile_correlation_key, CapabilityItemInput, CapabilityKind as MirrorCapabilityKind,
    CapabilitySnapshotInput, CapabilityState as MirrorCapabilityState, Confidence, FreshnessState,
    LocalReconciliationMismatch, ProofSummary, RedactedProofExport, ReviewedSetupInput,
    SelectedReadObservationInput, SelectedReadScopeInput, SourceIdentityInput,
    WriteFixtureEnrollmentInput, WriteFixtureEnrollmentStatus,
};
use crate::gst::{GstDraftRequest, GstReturnDraft};
use crate::reports::bulk_party_statement::{
    bulk_party_statement_party_count, write_bulk_party_statements_with_ageing_anchor,
    BulkPartyStatementRequest, PartyStatementDestinationApprovals,
};
use crate::reports::outstandings_working_paper::build_outstandings_working_paper;
use crate::reports::outstandings_working_paper_store::{
    source_from_complete_result, PartyStatementSourceStore, WorkingPaperExportStore,
    WorkingPaperExportStoreError,
};
use crate::reports::outstandings_working_paper_xlsx::render_outstandings_working_paper_xlsx;
use crate::reports::party_ledger_master::build_party_ledger_master_workbook;
use crate::reports::party_ledger_master_xlsx::render_party_ledger_master_xlsx;
use crate::reports::party_statement::{
    build_party_statement_with_ageing_anchor, PartyStatementError,
};
use crate::reports::party_statement_pdf::render_party_statement_pdf;
use crate::reports::party_statement_xlsx::render_party_statement_xlsx;
use crate::sync::coordinator::{SnapshotCoordinator, SnapshotJobStatus};
use crate::sync::reconciliation::ExternalReferenceCatalog;
use crate::sync::snapshot::{
    capability_profile_sha256, AdaptiveWindowPolicy, PlannedWindow, SnapshotPlan,
    SqliteSnapshotStateStore,
};
use crate::tally::connection::{PairedReadValidationError, PartyLedgerMasterSourceValidationError};
use crate::tally::runtime::TallyRuntimeControlError;
use crate::tally::validators::{
    normalize_company_guid, validate_company_name, validate_date_range,
};
pub use crate::tally::VerifiedCompanyIdentity;
use crate::tally::{
    company_source_identity, core_snapshot_start_authorized, source_lineage, ConnectionStatus,
    EndpointKey, OutstandingsCurrencyAssertion, OutstandingsLoadResult, RuntimeTallyConnector,
    SelectedReadScopeEvidence, TallyCompany, TallyConfig, TallyRuntime, TallySessionSnapshot,
    TallyTelemetryPreviewExport, VerifiedCompanyIdentityError,
};
use bridge_tally_core::{
    CapabilityFeatureId, CapabilityPackId, CapabilityState, CompanyRef as CoreCompanyRef,
    ReadWindow, RequestContext, TallyConnector, TallyDate, TransportId,
    CORE_ACCOUNTING_SCHEMA_VERSION,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::State;

#[path = "commands_trial_balance.rs"]
pub(crate) mod trial_balance;

pub mod all_clients;

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

fn desktop_journal_command_error(
    error: crate::agent::desktop_journal::DesktopJournalError,
) -> TallyCommandError {
    tally_command_error(
        error.code,
        "Journal review",
        error.message,
        "after_change",
        false,
        error.remediation,
    )
}

#[cfg(test)]
#[path = "commands_native_ledger_tests.rs"]
mod native_ledger_tests;

fn tally_runtime_command_error(error: anyhow::Error) -> TallyCommandError {
    if error.chain().any(|cause| {
        cause
            .downcast_ref::<PartyLedgerMasterSourceValidationError>()
            .is_some()
            || cause.downcast_ref::<PairedReadValidationError>().is_some()
            || cause
                .downcast_ref::<crate::tally::runtime::OpeningBoundaryObservationError>()
                .is_some()
            || cause
                .downcast_ref::<crate::tally::runtime::NativeLedgerIdentityAdmissionError>()
                .is_some()
    }) {
        return tally_command_error(
            "response_validation_failed",
            "Response validation",
            "The Tally response did not meet the party/ledger export validation contract, so Bridge withheld the unverified result.",
            "after_change",
            true,
            "Keep the result unverified and inspect redacted diagnostics before retrying.",
        );
    }
    if let Some(control) = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<TallyRuntimeControlError>())
    {
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
    } else if lower.contains("base currency changed between the currency read and the master read")
    {
        (
            "company_base_currency_changed",
            "Currency admission",
            "The selected Tally company changed while Bridge was establishing the workbook currency.",
            "after_change",
            true,
            "Refresh the selected Tally company and retry the export only after its books stop changing.",
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

/// Adds report context only after the shared runtime mapper has removed
/// transport and internal details from the operator-facing text.
fn party_ledger_master_runtime_command_error(error: anyhow::Error) -> TallyCommandError {
    // Sized before the master request was sent (#637): not a validation
    // failure, and retrying the unchanged export cannot help.
    if error.chain().any(|cause| {
        matches!(
            cause.downcast_ref::<PartyLedgerMasterSourceValidationError>(),
            Some(PartyLedgerMasterSourceValidationError::TooLarge { .. })
        )
    }) {
        return tally_command_error(
            "ledger_masters_too_large",
            "Response size",
            "Bridge withheld the party/ledger master: this company's master-alteration mark puts the compliance read over the size Bridge will request, because a read of that size has left Tally unable to answer. The mark is an upper bound on ledgers (stock items, units and every other master count too), so a company with fewer ledgers may be refused. No ledger was requested.",
            "after_change",
            false,
            "Do not retry the unchanged export: it refuses again. A precise ledger count is pending (bridge#668).",
        );
    }
    let mut mapped = tally_runtime_command_error(error);
    mapped.message = format!(
        "Bridge withheld the party/ledger master: {}",
        mapped.message
    );
    mapped
}

fn party_ledger_master_currency_admission_error(reason: &'static str) -> TallyCommandError {
    // Several Currency-master rows do not identify which row is the company's base
    // currency. See TALLY_PROTOCOL_REFERENCE.md §9.10a.1.
    let (message, remediation) = match reason {
        "company_base_currency_undetermined" => (
            "Tally defines multiple Currency masters, so Bridge could not establish the selected company's base currency from this read.",
            "Do not retry the unchanged export: no operator confirmation can make this read safe. Bridge needs a read that establishes one INR base currency before it can label the workbook.",
        ),
        "company_base_currency_not_inr" => (
            "The selected Tally company does not use INR as its base currency.",
            "Do not retry the unchanged export: select a company whose established base currency is INR. A confirmation cannot change an unsupported base currency.",
        ),
        "company_currency_probe_failed" => (
            "Bridge could not establish one INR base currency for the selected Tally company.",
            "Do not retry until Tally can return a valid base-currency read for this selected company.",
        ),
        _ => (
            "Bridge could not establish the selected Tally company's currency.",
            "Do not retry until Bridge can establish the selected company's base currency from Tally.",
        ),
    };
    tally_command_error(
        reason,
        "Currency admission",
        format!(
            "Bridge withheld the party/ledger master: {message} The workbook cannot label the monetary figures safely."
        ),
        "after_change",
        true,
        remediation,
    )
}

fn party_ledger_master_local_export_error(
    code: &'static str,
    message: &'static str,
    remediation: &'static str,
) -> TallyCommandError {
    tally_command_error(
        code,
        "Operation",
        message,
        "after_change",
        false,
        remediation,
    )
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
            && company
                .company_number
                .as_deref()
                .is_some_and(|number| !number.trim().is_empty())
            && company
                .books_from
                .as_deref()
                .is_some_and(|books_from| !books_from.trim().is_empty())
        {
            "observed"
        } else {
            "unknown"
        };
        let correlation_key = company
            .guid
            .as_deref()
            .zip(company.company_number.as_deref())
            .zip(company.books_from.as_deref())
            .map(|((guid, company_number), books_from)| {
                company_profile_correlation_key(
                    &canonical_origin,
                    guid,
                    company_number,
                    &company.name,
                    books_from,
                )
            });
        companies.push(PersistedTallyCompany {
            name: company.name,
            guid: company.guid,
            company_number: company.company_number,
            books_from_yyyymmdd: company.books_from,
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

const SETUP_PROBE_MAX_AGE_MS: i64 = 5 * 60 * 1_000;

#[derive(Debug, Deserialize)]
pub struct SaveTallySetupRequest {
    pub config: TallyConfig,
    pub expected_review_id: String,
    pub expected_review_commitment_sha256: String,
    pub selected_company: SelectedCompanyIdentity,
}

/// Exact Company-collection tuple selected from the reviewed probe. Every
/// field is subsequently matched against the cached probe before persistence.
#[derive(Debug, Deserialize)]
pub struct SelectedCompanyIdentity {
    pub display_name: String,
    pub company_guid: String,
    pub company_number: String,
    pub books_from_yyyymmdd: String,
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
    pub selected_company: SelectedCompanyIdentity,
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
        let selected_identity = verify_observed_company_tuple_from_companies(
            &request.selected_company,
            probe.companies.clone(),
        )?;
        let company = probe
            .companies
            .iter()
            .find(|company| selected_identity.matches_observed_company(company))
            .cloned()
            .ok_or_else(reviewed_probe_changed_error)?;
        if probe.selected_read_scope.as_ref().is_some_and(|scope| {
            !company.guid.as_deref().is_some_and(|guid| {
                guid.to_ascii_lowercase() == scope.company_guid_ascii_casefolded
            }) || company.company_number.as_deref() != Some(scope.company_number.as_str())
                || company.books_from.as_deref() != Some(scope.books_from_yyyymmdd.as_str())
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
                    license_tier: probe.profile.license_tier,
                    mode: probe.profile.mode.clone(),
                    mode_confidence: if probe.profile.mode.is_some() {
                        Confidence::Observed
                    } else {
                        Confidence::Unknown
                    },
                    items: capability_items(&probe.profile),
                },
                company_display_name: selected_identity.display_name().to_string(),
                company_identity: SourceIdentityInput {
                    // Persist the spelling observed from Tally, not caller-controlled casing.
                    guid: Some(selected_identity.company_guid().to_string()),
                    confidence: Some(Confidence::Observed),
                    ..SourceIdentityInput::default()
                },
                company_number: request.selected_company.company_number.clone(),
                books_from_yyyymmdd: request.selected_company.books_from_yyyymmdd.clone(),
                selected_read_scope: probe.selected_read_scope.as_ref().map(|scope| {
                    SelectedReadScopeInput {
                        scope_commitment_sha256: scope.scope_commitment_sha256.clone(),
                        parent_review_sha256: scope.parent_review_sha256.clone(),
                        ledger_profile_id: scope.ledger_profile_id.clone(),
                        voucher_profile_id: scope.voucher_profile_id.clone(),
                        voucher_from_yyyymmdd: scope.voucher_from_yyyymmdd.clone(),
                        voucher_to_yyyymmdd: scope.voucher_to_yyyymmdd.clone(),
                        company_number: scope.company_number.clone(),
                        books_from_yyyymmdd: scope.books_from_yyyymmdd.clone(),
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
        let correlation_key = company.guid.as_deref().zip(company.company_number.as_deref()).zip(company.books_from.as_deref()).map(
            |((guid, company_number), books_from)| company_profile_correlation_key(
                &canonical_origin,
                guid,
                company_number,
                &company.name,
                books_from,
            ),
        );
        Ok(SavedTallySetup {
            passport_snapshot_id: saved.snapshot.id,
            canonical_origin,
            observed_at_unix_ms,
            company: PersistedTallyCompany {
                name: company.name,
                correlation_key,
                guid: company.guid,
                company_number: company.company_number,
                books_from_yyyymmdd: company.books_from,
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
        let selected_identity = verify_observed_company_tuple_from_companies(
            &request.selected_company,
            probe.companies.clone(),
        )?;
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
        if pin.canonical_origin != canonical_origin
            || !pin.company_guid.eq_ignore_ascii_case(selected_identity.company_guid())
            || pin.display_name != selected_identity.display_name()
            || pin.company_number != request.selected_company.company_number
            || pin.books_from_yyyymmdd != request.selected_company.books_from_yyyymmdd
        {
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
    pub company_number: Option<String>,
    pub books_from_yyyymmdd: Option<String>,
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
        identity: company_source_identity(
            &lineage,
            &pin.company_guid,
            &pin.company_number,
            &pin.display_name,
            &pin.books_from_yyyymmdd,
        ),
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
            license_tier: canary.profile.license_tier,
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
        identity: company_source_identity(
            &lineage,
            &pin.company_guid,
            &pin.company_number,
            &pin.display_name,
            &pin.books_from_yyyymmdd,
        ),
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
    pub selected_company: SelectedCompanyIdentity,
}

#[derive(Debug, Deserialize)]
pub struct OutstandingsRequest {
    pub config: TallyConfig,
    pub selected_company: SelectedCompanyIdentity,
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

#[derive(Debug, Serialize)]
pub struct FetchOutstandingsResponse {
    #[serde(flatten)]
    pub result: OutstandingsLoadResult,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_paper_export_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_paper_unavailable_reason_code: Option<&'static str>,
    /// The handle party statements are exported by (bridge#551). Present only
    /// when the read completed with a source the working paper could also use
    /// (see `working_paper_unavailable_reason_code` for why not) and the
    /// statement store held it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub party_statement_source_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SelectedLedgerEntriesRequest {
    pub config: TallyConfig,
    pub selected_company: SelectedCompanyIdentity,
    pub ledger: String,
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub offset: usize,
    #[serde(default = "selected_ledger_entries_default_limit")]
    pub limit: usize,
}

fn selected_ledger_entries_default_limit() -> usize {
    100
}

pub(crate) async fn verify_observed_company_tuple(
    runtime: &TallyRuntime,
    config: &TallyConfig,
    selected: &SelectedCompanyIdentity,
) -> Result<VerifiedCompanyIdentity, TallyCommandError> {
    let companies = runtime
        .fetch_companies(config.clone())
        .await
        .map_err(tally_runtime_command_error)?;
    verify_observed_company_tuple_from_companies(selected, companies)
}

fn verify_observed_company_tuple_from_companies(
    selected: &SelectedCompanyIdentity,
    companies: Vec<TallyCompany>,
) -> Result<VerifiedCompanyIdentity, TallyCommandError> {
    validate_company_name(&selected.display_name).map_err(|message| {
        tally_command_error(
            "company_selection_invalid",
            "Tally application",
            message,
            "after_change",
            false,
            "Select a company with an observed name, number, GUID, and book opening date.",
        )
    })?;
    let selected_guid = normalize_company_guid(&selected.company_guid).map_err(|_| {
        tally_command_error(
            "company_selection_invalid",
            "Tally application",
            "The selected company GUID is invalid.",
            "after_change",
            false,
            "Probe again and choose an observed company identity.",
        )
    })?;
    if !crate::tally::validators::is_valid_company_number(&selected.company_number)
        || TallyDate::parse(selected.books_from_yyyymmdd.clone()).is_err()
    {
        return Err(tally_command_error(
            "company_selection_invalid",
            "Tally application",
            "The selected company tuple is incomplete or invalid.",
            "after_change",
            false,
            "Probe again and choose an observed company identity.",
        ));
    }
    VerifiedCompanyIdentity::from_observed_companies(
        selected.display_name.clone(),
        selected_guid,
        selected.company_number.clone(),
        selected.books_from_yyyymmdd.clone(),
        &companies,
    )
    .map_err(|error| match error {
        VerifiedCompanyIdentityError::InvalidCompanyNumber => tally_command_error(
            "company_selection_invalid",
            "Tally application",
            "The observed company number is invalid.",
            "after_change",
            false,
            "Probe again and choose a valid observed company identity.",
        ),
        VerifiedCompanyIdentityError::InvalidBooksFrom => tally_command_error(
            "company_books_from_invalid",
            "Tally application",
            "Tally returned an invalid books-from calendar date for the selected company.",
            "not_recommended",
            false,
            "Check the company's books-from date in Tally, then probe again.",
        ),
        VerifiedCompanyIdentityError::Missing => tally_command_error(
            "reviewed_company_scope_changed",
            "Tally application",
            "The selected company tuple is no longer present in Tally.",
            "safe",
            false,
            "Probe again and explicitly select the intended book.",
        ),
        VerifiedCompanyIdentityError::DuplicateTuple => tally_command_error(
            "company_identity_ambiguous",
            "Tally application",
            "Tally returned the selected complete company tuple more than once.",
            "not_recommended",
            false,
            "Do not read or enroll this scope. Probe again after resolving the duplicate company tuple.",
        ),
        VerifiedCompanyIdentityError::DisplayScopeAmbiguous => tally_command_error(
            "company_identity_display_scope_ambiguous",
            "Tally application",
            "Tally returned a same-GUID company whose display name cannot safely scope the selected book.",
            "not_recommended",
            false,
            "Do not read or enroll this scope. Rename one of the presentation-equivalent Tally books, then probe again.",
        ),
    })
}

/// Reads and writes one columnar party/ledger master workbook. The workbook
/// is built only from the runtime's verified, paired source; this command
/// accepts no ledger row or amount from the webview.
#[tauri::command]
pub async fn export_party_ledger_master(
    app: tauri::AppHandle,
    request: CompanyRequest,
    runtime: State<'_, TallyRuntime>,
) -> Result<String, TallyCommandError> {
    let identity =
        verify_observed_company_tuple(&runtime, &request.config, &request.selected_company).await?;
    let currency_read = runtime
        .detect_party_ledger_master_currency(request.config.clone(), &identity)
        .await
        .map_err(party_ledger_master_runtime_command_error)?;
    let currency_assertion =
        establish_inr_currency(currency_read.currency_count(), currency_read.is_inr())
            .map_err(party_ledger_master_currency_admission_error)?;
    let currency_assertion = currency_read.bind_party_ledger_master_assertion(currency_assertion);
    let source = runtime
        .fetch_party_ledger_master_source(request.config, &identity, currency_assertion)
        .await
        .map_err(party_ledger_master_runtime_command_error)?;
    let workbook = build_party_ledger_master_workbook(source)
        .map_err(|_| {
            party_ledger_master_local_export_error(
                "party_ledger_master_source_invalid",
                "Bridge withheld the party/ledger master because its verified source could not be represented safely.",
                "Refresh the selected Tally company and retry the export after reviewing its runtime status.",
            )
        })?;
    let bytes = render_party_ledger_master_xlsx(&workbook).map_err(|_| {
        party_ledger_master_local_export_error(
            "party_ledger_master_render_failed",
            "Bridge could not build the party/ledger master workbook safely.",
            "Retry the export after reviewing local application status.",
        )
    })?;
    let mut slug = statement_filename_slug(workbook.source().company.as_str());
    slug.truncate(150);
    save_report_download_bytes(
        &app,
        &format!(
            "party-ledger-master-{slug}-{}.xlsx",
            workbook.source().to.as_str()
        ),
        &bytes,
    )
    .map_err(|_| {
        party_ledger_master_local_export_error(
            "party_ledger_master_save_failed",
            "Bridge could not save the party/ledger master workbook.",
            "Verify local Downloads-folder access, then retry the export.",
        )
    })
}

/// Returns selected-ledger voucher entries using the MCP's captured-source
/// selection pipeline. Pagination limits this response only: the shared
/// operation validates the complete requested window before filtering.
#[tauri::command]
pub async fn fetch_selected_ledger_entries(
    request: SelectedLedgerEntriesRequest,
    runtime: State<'_, TallyRuntime>,
) -> Result<serde_json::Value, TallyCommandError> {
    if request.limit == 0 || request.limit > 500 {
        return Err(tally_command_error(
            "selected_ledger_entries_limit_invalid",
            "Operation",
            "Choose between 1 and 500 entries per displayed page.",
            "after_change",
            false,
            "Adjust the display page size, then try again.",
        ));
    }
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
    let identity =
        verify_observed_company_tuple(&runtime, &request.config, &request.selected_company).await?;
    crate::agent::desktop_selected_vouchers(
        &runtime,
        request.config,
        identity,
        crate::agent::DesktopSelectedVoucherRequest {
            ledger: request.ledger,
            from: request.from,
            to: request.to,
            offset: request.offset,
            limit: request.limit,
        },
    )
    .await
    .map_err(|code| {
        tally_command_error(
            "selected_ledger_entries_refused",
            "Tally application",
            "Bridge withheld this ledger investigation because its source could not be verified.",
            "after_change",
            false,
            match code.as_str() {
                "ledger_not_found" | "ledger_ambiguous" => {
                    "Choose a ledger from the verified company and try again."
                }
                _ => "Refresh the selected company and try a narrower date range.",
            },
        )
    })
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
    working_paper_exports: State<'_, WorkingPaperExportStore>,
    party_statement_sources: State<'_, PartyStatementSourceStore>,
) -> Result<FetchOutstandingsResponse, TallyCommandError> {
    read_screen_outstandings(
        request,
        &runtime,
        &working_paper_exports,
        &party_statement_sources,
    )
    .await
}

/// The body of [`fetch_tally_outstandings`], over plain references so that a
/// test can drive the command's own sequence against a scripted Tally
/// (bridge#604: the command must not read outstandings without its own
/// currency check).
pub(crate) async fn read_screen_outstandings(
    request: OutstandingsRequest,
    runtime: &TallyRuntime,
    working_paper_exports: &WorkingPaperExportStore,
    party_statement_sources: &PartyStatementSourceStore,
) -> Result<FetchOutstandingsResponse, TallyCommandError> {
    let as_of = requested_outstandings_as_of(request.as_of_yyyymmdd)?;
    let canonical_origin = EndpointKey::from_config(&request.config)
        .map(|endpoint| endpoint.as_str().to_string())
        .map_err(tally_runtime_command_error)?;
    let working_paper_company_key = company_profile_correlation_key(
        &canonical_origin,
        &request.selected_company.company_guid,
        &request.selected_company.company_number,
        &request.selected_company.display_name,
        &request.selected_company.books_from_yyyymmdd,
    );
    let identity =
        verify_observed_company_tuple(runtime, &request.config, &request.selected_company).await?;
    let result = runtime
        .fetch_operator_outstandings(
            request.config,
            &identity,
            as_of,
            request.currency_assertion,
            request.ageing_anchor,
        )
        .await
        .map_err(tally_runtime_command_error)?;
    let (working_paper_source, source_unavailable_reason_code) =
        match source_from_complete_result(&result, identity.company_guid()) {
            Ok(Some(source)) => (Some(source), None),
            Ok(None) if matches!(&result, OutstandingsLoadResult::Complete { .. }) => {
                (None, Some("working_paper_complete_source_unavailable"))
            }
            Ok(None) => (None, None),
            Err(_) => (None, Some("working_paper_resource_limit")),
        };
    // A successful refresh supersedes every older working-paper approval and
    // statement source for this company even when the new result cannot
    // issue one. Each store replaces under its own lock, so a stale snapshot
    // never survives beside the result that displaced it in the webview. Both
    // stores hold the one source.
    let working_paper_source = working_paper_source.map(std::sync::Arc::new);
    let party_statement_source_id = party_statement_sources
        .replace_for_company(&working_paper_company_key, working_paper_source.clone())
        .ok()
        .flatten();
    let (working_paper_export_id, working_paper_unavailable_reason_code) =
        match working_paper_exports
            .replace_shared_for_company(&working_paper_company_key, working_paper_source)
        {
            Ok(export_id) => (export_id, source_unavailable_reason_code),
            Err(_) => (None, Some("working_paper_export_store_unavailable")),
        };
    Ok(FetchOutstandingsResponse {
        result,
        working_paper_export_id,
        working_paper_unavailable_reason_code,
        party_statement_source_id,
    })
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
    pub selected_company: SelectedCompanyIdentity,
}

#[derive(Debug, Serialize)]
pub struct CompanyOutstandingsEntry {
    pub company: String,
    pub company_guid: String,
    pub company_number: String,
    pub books_from_yyyymmdd: String,
    pub canonical_origin: String,
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
        Err(CompanySweepFailure::CompanyVerification(error)) => {
            crate::tally::OutstandingsPartialReason::code(error.code)
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

pub(crate) enum CompanySweepFailure {
    ReasonCode(&'static str),
    /// A company-list transport/protocol failure is not evidence that the
    /// operator selected the wrong tuple. Keep the command's typed reason so
    /// the all-company view can direct the operator to the actual failure.
    CompanyVerification(TallyCommandError),
    OutstandingsRead,
}

fn company_sweep_currency_preflight_failure(
    currency_count: usize,
    is_inr: bool,
) -> Option<&'static str> {
    establish_inr_currency(currency_count, is_inr).err()
}

/// The one INR admission rule used by both the existing outstandings sweep and
/// the party/ledger workbook boundary. A workbook can obtain this typed value
/// only after `detect_base_currency` has read Tally's own Currency masters.
fn establish_inr_currency(
    currency_count: usize,
    is_inr: bool,
) -> Result<OutstandingsCurrencyAssertion, &'static str> {
    if currency_count > 1 {
        return Err("company_base_currency_undetermined");
    }
    if currency_count == 1 && !is_inr {
        return Err("company_base_currency_not_inr");
    }
    if currency_count == 1 {
        return Ok(OutstandingsCurrencyAssertion::Inr);
    }
    Err("company_currency_probe_failed")
}

/// One company of the sweep: its own currency read, the INR admission, then
/// outstandings under that read, whose single master's NAME each ledger's own
/// currency is compared with (bridge#551).
pub(crate) async fn sweep_company_outstandings(
    runtime: &TallyRuntime,
    config: &TallyConfig,
    identity: &VerifiedCompanyIdentity,
    as_of: &TallyDate,
    currency_assertion: OutstandingsCurrencyAssertion,
    ageing_anchor: crate::tally::OutstandingsAgeingAnchor,
) -> Result<OutstandingsLoadResult, CompanySweepFailure> {
    let Ok(currency) = runtime
        .detect_base_currency_with_extent(config.clone(), identity)
        .await
    else {
        return Err(CompanySweepFailure::ReasonCode(
            "company_currency_probe_failed",
        ));
    };
    if let Some(reason_code) =
        company_sweep_currency_preflight_failure(currency.currency_count(), currency.is_inr())
    {
        return Err(CompanySweepFailure::ReasonCode(reason_code));
    }
    runtime
        .fetch_outstandings_under_currency_read(
            config.clone(),
            identity,
            as_of.clone(),
            currency,
            currency_assertion,
            ageing_anchor,
        )
        .await
        .map_err(|_| CompanySweepFailure::OutstandingsRead)
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
    let canonical_origin = EndpointKey::from_config(&request.config)
        .map(|endpoint| endpoint.as_str().to_string())
        .map_err(tally_runtime_command_error)?;
    let as_of = requested_outstandings_as_of(request.as_of_yyyymmdd)?;

    let mut entries = Vec::with_capacity(request.companies.len());
    for entry in request.companies {
        let selected = entry.selected_company;
        let result = match verify_observed_company_tuple(&runtime, &request.config, &selected).await
        {
            Err(error) => Err(CompanySweepFailure::CompanyVerification(error)),
            Ok(identity) => {
                sweep_company_outstandings(
                    &runtime,
                    &request.config,
                    &identity,
                    &as_of,
                    request.currency_assertion,
                    request.ageing_anchor,
                )
                .await
            }
        };
        entries.push(CompanyOutstandingsEntry {
            company: selected.display_name,
            company_guid: selected.company_guid,
            company_number: selected.company_number,
            books_from_yyyymmdd: selected.books_from_yyyymmdd,
            canonical_origin: canonical_origin.clone(),
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
pub async fn desktop_pick_journal_for_review(
    config: TallyConfig,
    runtime: State<'_, TallyRuntime>,
) -> Result<Option<crate::agent::desktop_journal::DesktopJournalReview>, TallyCommandError> {
    crate::agent::desktop_journal::pick_for_review(config, runtime.inner().clone())
        .await
        .map_err(desktop_journal_command_error)
}

#[tauri::command]
pub async fn desktop_post_reviewed_journal(
    request: crate::agent::desktop_journal::DesktopJournalDescriptorRequest,
    runtime: State<'_, TallyRuntime>,
) -> Result<crate::agent::desktop_journal::DesktopJournalActionResponse, TallyCommandError> {
    crate::agent::desktop_journal::post_reviewed(request, runtime.inner().clone())
        .await
        .map_err(desktop_journal_command_error)
}

#[tauri::command]
pub async fn desktop_reconcile_reviewed_journal(
    request: crate::agent::desktop_journal::DesktopJournalDescriptorRequest,
    runtime: State<'_, TallyRuntime>,
) -> Result<crate::agent::desktop_journal::DesktopJournalActionResponse, TallyCommandError> {
    crate::agent::desktop_journal::reconcile_reviewed(request, runtime.inner().clone())
        .await
        .map_err(desktop_journal_command_error)
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
#[path = "commands_tests.rs"]
mod tests;

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

/// A party statement names its source by the handle
/// `fetch_tally_outstandings` issued with the completed read; the company,
/// as-of date, ageing anchor and every row come from the server-held source
/// (bridge#551). A request carrying rows is refused.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExportPartyStatementRequest {
    pub source_id: String,
    pub party: String,
    /// XLSX remains the default for callers that predate the PDF option.
    #[serde(default)]
    pub format: PartyStatementFormat,
}

#[derive(Debug, Deserialize)]
pub struct ExportOutstandingsWorkingPaperRequest {
    /// Opaque, one-use binding to source rows retained by Rust after the
    /// completed native read. The webview never supplies financial rows.
    pub export_id: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PartyStatementFormat {
    #[default]
    Xlsx,
    Pdf,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExportBulkPartyStatementsRequest {
    /// The server-held statement source, as for [`ExportPartyStatementRequest`].
    pub source_id: String,
    pub format: PartyStatementFormat,
    /// Returned by the native folder picker. The command verifies it against
    /// that picker result, then still checks it exists and is a directory
    /// before any statement name is joined to it.
    pub destination: String,
    /// Opaque, single-use proof returned with the native picker destination.
    /// A renderer cannot mint this proof for an arbitrary local path.
    pub approval_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreviewBulkPartyStatementsRequest {
    /// The same server-held source the export command will consume. This
    /// command performs no I/O or Tally read; it only makes the pending scope
    /// explicit.
    pub source_id: String,
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
    party_statement_sources: State<'_, PartyStatementSourceStore>,
) -> Result<BulkPartyStatementsPreview, String> {
    let source = party_statement_source(&party_statement_sources, &request.source_id)?;
    Ok(BulkPartyStatementsPreview {
        party_count: bulk_party_statement_party_count(
            &source.open_bills,
            &source.unallocated_by_party,
        ),
    })
}

/// The server-held source a statement request names, or the operator-facing
/// reason it is gone (bridge#551).
pub(crate) fn party_statement_source(
    sources: &PartyStatementSourceStore,
    source_id: &str,
) -> Result<
    std::sync::Arc<crate::reports::outstandings_working_paper::OutstandingsWorkingPaperSource>,
    String,
> {
    sources.get(source_id).map_err(|error| match error {
        WorkingPaperExportStoreError::Unavailable => {
            "Bridge could not reach the statement source. Refresh outstandings and try again."
                .to_string()
        }
        _ => "This outstandings result is no longer available for statements. Refresh \
              outstandings and try again."
            .to_string(),
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
    party_statement_sources: State<'_, PartyStatementSourceStore>,
) -> Result<
    crate::reports::bulk_party_statement::BulkPartyStatementResult,
    BulkPartyStatementExportError,
> {
    export_bulk_party_statements_at_selected_destination(
        request,
        &approvals,
        &party_statement_sources,
    )
}

fn export_bulk_party_statements_at_selected_destination(
    request: ExportBulkPartyStatementsRequest,
    approvals: &PartyStatementDestinationApprovals,
    party_statement_sources: &PartyStatementSourceStore,
) -> Result<
    crate::reports::bulk_party_statement::BulkPartyStatementResult,
    BulkPartyStatementExportError,
> {
    let source = party_statement_source(party_statement_sources, &request.source_id)
        .map_err(BulkPartyStatementExportError::Existing)?;
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
                company: &source.company,
                as_of_yyyymmdd: &source.as_of_yyyymmdd,
                format: "xlsx",
                open_bills: &source.open_bills,
                unallocated_by_party: &source.unallocated_by_party,
                ageing_anchor: source.source_ageing_anchor,
                render: |statement: &crate::reports::party_statement::PartyStatement| {
                    render_party_statement_xlsx(statement).map_err(|error| error.to_string())
                },
            })
        }
        PartyStatementFormat::Pdf => {
            write_bulk_party_statements_with_ageing_anchor(BulkPartyStatementRequest {
                destination: &approved_destination,
                company: &source.company,
                as_of_yyyymmdd: &source.as_of_yyyymmdd,
                format: "pdf",
                open_bills: &source.open_bills,
                unallocated_by_party: &source.unallocated_by_party,
                ageing_anchor: source.source_ageing_anchor,
                render: |statement: &crate::reports::party_statement::PartyStatement| {
                    render_party_statement_pdf(statement).map_err(|error| error.to_string())
                },
            })
        }
    };
    result.map_err(BulkPartyStatementExportError::Existing)
}

/// Builds the all-party, dual-ageing working paper from one completed native
/// result and writes it to the user's Downloads folder. No Tally request is
/// issued here.
///
/// Mirrors `save_report_download` exactly: Tauri's own path resolver, the
/// same filename-traversal guard, and the full written path returned so the
/// UI can say where the file went instead of leaving the operator to guess.
#[tauri::command]
pub async fn export_outstandings_working_paper(
    app: tauri::AppHandle,
    request: ExportOutstandingsWorkingPaperRequest,
    working_paper_exports: State<'_, WorkingPaperExportStore>,
) -> Result<String, String> {
    let source = working_paper_exports
        .take(&request.export_id)
        .map_err(|error| format!("Bridge withheld the working paper: {error}"))?;
    let paper = build_outstandings_working_paper(source)
        .map_err(|error| format!("Bridge withheld the working paper: {error}"))?;
    let bytes = render_outstandings_working_paper_xlsx(&paper)
        .map_err(|error| format!("Bridge could not build the working paper: {error}"))?;
    let mut slug = statement_filename_slug(paper.company());
    slug.truncate(150);
    save_report_download_bytes(
        &app,
        &format!(
            "outstandings-working-paper-{slug}-{}.xlsx",
            paper.as_of().as_str()
        ),
        &bytes,
    )
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
    party_statement_sources: State<'_, PartyStatementSourceStore>,
) -> Result<String, String> {
    use tauri::Manager as _;

    let source = party_statement_source(&party_statement_sources, &request.source_id)?;
    let statement = build_party_statement_with_ageing_anchor(
        &source.company,
        &source.as_of_yyyymmdd,
        &request.party,
        &source.open_bills,
        &source.unallocated_by_party,
        source.source_ageing_anchor,
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
                    "Bridge could not create the statement export: {error}"
                ))
            }
        };
        if let Err(error) = std::io::Write::write_all(&mut file, bytes) {
            drop(file);
            let _ = std::fs::remove_file(&path);
            return Err(format!(
                "Bridge could not finish writing the statement export: {error}"
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
#[path = "commands_statement_export_tests.rs"]
mod statement_export_tests;

#[cfg(test)]
#[path = "commands_party_statement_export_tests.rs"]
mod party_statement_export_tests;

#[derive(Debug, Deserialize)]
pub struct BaseCurrencyRequest {
    pub config: TallyConfig,
    pub selected_company: SelectedCompanyIdentity,
}

/// Establishes a company's base currency from Tally.
#[tauri::command]
pub async fn detect_tally_base_currency(
    request: BaseCurrencyRequest,
    runtime: State<'_, TallyRuntime>,
) -> Result<bridge_tally_protocol::native_outstandings::CompanyCurrency, TallyCommandError> {
    let identity =
        verify_observed_company_tuple(&runtime, &request.config, &request.selected_company).await?;
    runtime
        .detect_base_currency(request.config, &identity)
        .await
        .map_err(tally_runtime_command_error)
}
