use std::collections::{BTreeMap, BTreeSet};

use bridge_tally_core::{ExactDecimal, PackSchemaVersion, TallyDate};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{sqlite::SqliteRow, QueryBuilder, Row, Sqlite, SqlitePool, Transaction};
use uuid::Uuid;

use crate::sync::reconciliation::CommitBatchInput;
use crate::tally::core_snapshot_start_authorized_codes;

const MIRROR_MIGRATION_V2: &str = include_str!("migrations/0002_tally_mirror.sql");
const MIRROR_MIGRATION_V3: &str = include_str!("migrations/0003_tally_safe_writes.sql");
const MIRROR_MIGRATION_V4: &str = include_str!("migrations/0004_tally_snapshot_state.sql");
const MIRROR_MIGRATION_V5: &str = include_str!("migrations/0005_tally_snapshot_recovery.sql");
const MIRROR_MIGRATION_V6: &str = include_str!("migrations/0006_tally_incremental_foundation.sql");
const MIRROR_MIGRATION_V7: &str = include_str!("migrations/0007_tally_selected_read_evidence.sql");
const MIRROR_MIGRATION_V8: &str =
    include_str!("migrations/0008_tally_reviewed_setup_consumption.sql");
const MIRROR_MIGRATION_V9: &str = include_str!("migrations/0009_tally_snapshot_window_staging.sql");
const MIRROR_MIGRATION_V10: &str =
    include_str!("migrations/0010_tally_provenance_unavailable_counts.sql");
const MIRROR_MIGRATION_V11: &str =
    include_str!("migrations/0011_tally_proof_record_counts_digest.sql");
const MIRROR_MIGRATION_V12: &str =
    include_str!("migrations/0012_tally_window_terminal_evidence.sql");
const MIRROR_MIGRATION_V13: &str =
    include_str!("migrations/0013_tally_write_fixture_enrollment.sql");
const MIRROR_MIGRATION_V14: &str =
    include_str!("migrations/0014_tally_write_fixture_revocation_sequence.sql");
const MIRROR_MIGRATION_V14_ALREADY_SEQUENCED: &str =
    include_str!("migrations/0014_tally_write_fixture_revocation_sequence_existing.sql");
const MIRROR_MIGRATION_V15: &str =
    include_str!("migrations/0015_tally_write_canary_reservation.sql");
const MIRROR_MIGRATION_V16: &str =
    include_str!("migrations/0016_tally_write_canary_payload_binding.sql");
const MIRROR_MIGRATION_V17: &str =
    include_str!("migrations/0017_tally_write_canary_preflight_attempt.sql");
const MIRROR_MIGRATION_V18: &str =
    include_str!("migrations/0018_tally_write_canary_preflight_evidence.sql");
const MIRROR_MIGRATION_V19: &str =
    include_str!("migrations/0019_tally_write_canary_dispatch_attempt.sql");
const MIRROR_MIGRATION_V20: &str =
    include_str!("migrations/0020_tally_write_canary_final_verdict.sql");
const MIRROR_MIGRATION_V21: &str =
    include_str!("migrations/0021_tally_write_canary_preflight_target_binding.sql");
const MIRROR_MIGRATION_V22: &str =
    include_str!("migrations/0022_tally_bomless_utf16_read_evidence.sql");
const MIRROR_MIGRATION_V23: &str =
    include_str!("migrations/0023_tally_composite_company_identity.sql");
const MIRROR_MIGRATION_V24: &str = include_str!("migrations/0024_tally_selected_read_scope_v2.sql");
const MIRROR_MIGRATION_V25: &str =
    include_str!("migrations/0025_tally_observed_company_identity_constraint.sql");

const MIRROR_MIGRATION_V26: &str =
    include_str!("migrations/0026_tally_capability_license_tier.sql");
const MIRROR_MIGRATION_V27: &str =
    include_str!("migrations/0027_tally_retire_resurrected_guid_index.sql");
const MIRROR_MIGRATION_V28: &str = include_str!("migrations/0028_schedule_iii_grouping_events.sql");

const MAX_WINDOW_STAGE_CHUNK: usize = 256;
const MAX_WINDOW_EVIDENCE_JSON_BYTES: usize = 16 * 1024;
const WINDOW_MEMBERSHIP_DIGEST_PAGE_SIZE: i64 = 512;

pub(crate) const REVIEWED_TALLY_TERMINAL_CODES: &[&str] = &[
    "adaptive_window_limit_reached",
    "application_response_rejected",
    "canary_cache_unavailable",
    "capability_cache_unavailable",
    "capability_probe_required",
    "company_export_invalid",
    "company_identity_mismatch",
    "connector_context_invalid",
    "endpoint_circuit_open",
    "endpoint_invalid",
    "endpoint_queue_deadline_exceeded",
    "fresh_capability_probe_not_supported",
    "group_export_invalid",
    "http_client_initialization_failed",
    "http_status_failure",
    "ledger_export_invalid",
    "local_clock_moved_backwards",
    "minimum_window_response_too_large",
    "period_report_identity_missing",
    "period_report_invalid",
    "period_report_scope_mismatch",
    "query_profile_not_supported",
    "reconciliation_record_budget_exceeded",
    "request_size_limit_exceeded",
    "response_content_encoding_unsupported",
    "response_encoding_invalid",
    "response_read_failed",
    "response_size_limit_exceeded",
    "response_truncated",
    "runtime_capacity_reached",
    "snapshot_checkpoint_changed",
    "transport_policy_invalid",
    "unclassified_tally_error",
    "voucher_export_invalid",
    "voucher_response_size_limit_exceeded",
    "voucher_type_export_invalid",
    "window_membership_replay_conflict",
];

#[derive(Debug, thiserror::Error)]
pub enum MirrorError {
    #[error("mirror database operation failed")]
    Database(#[from] sqlx::Error),
    #[error("invalid mirror input ({0})")]
    InvalidInput(&'static str),
    #[error("the requested mirror entity was not found")]
    NotFound,
    #[error("the observation batch is no longer open")]
    BatchClosed,
    #[error("multiple source identities resolve to different records")]
    IdentityCollision,
    #[error("a fallback identity cannot be upgraded without an explicit audit event")]
    IdentityUpgradeRequiresAudit,
    #[error("the record has already been observed in this batch")]
    DuplicateObservation,
    #[error("the replayed observation conflicts with the record already stored in this batch")]
    ObservationConflict,
    #[error("the snapshot window attempt is no longer open")]
    WindowAttemptClosed,
    #[error("the observation batch still owns open snapshot window attempts")]
    OpenWindowAttempts,
    #[error("the replayed snapshot window membership conflicts with immutable stored content")]
    WindowMembershipConflict,
    #[error("a previously staged snapshot window membership disappeared")]
    WindowMembershipDisappeared,
    #[error("only a complete, gap-free batch can be verified")]
    VerificationInvariant,
    #[error("the mirror checkpoint changed concurrently")]
    ConcurrentCheckpoint,
    #[error("canonical payload serialization failed")]
    Serialization(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confidence {
    Documented,
    Observed,
    Inferred,
    Unknown,
}

impl Confidence {
    fn as_str(self) -> &'static str {
        match self {
            Self::Documented => "documented",
            Self::Observed => "observed",
            Self::Inferred => "inferred",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityKind {
    Transport,
    Pack,
    Feature,
}

impl CapabilityKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Transport => "transport",
            Self::Pack => "pack",
            Self::Feature => "feature",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityState {
    Supported,
    Unsupported,
    Unknown,
    NotConfigured,
}

impl CapabilityState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Supported => "supported",
            Self::Unsupported => "unsupported",
            Self::Unknown => "unknown",
            Self::NotConfigured => "not_configured",
        }
    }
}

#[derive(Debug, Clone)]
pub struct CapabilityItemInput {
    pub kind: CapabilityKind,
    pub key: String,
    pub state: CapabilityState,
    pub confidence: Confidence,
    pub safe_reason_code: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CapabilitySnapshotInput {
    pub canonical_origin: String,
    pub observed_at_unix_ms: i64,
    pub profile_version: u16,
    pub product: String,
    pub release: Option<String>,
    pub license_tier: Option<bridge_tally_core::LicenseTier>,
    pub mode: Option<String>,
    pub mode_confidence: Confidence,
    pub items: Vec<CapabilityItemInput>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilitySnapshotRef {
    pub id: String,
    pub endpoint_id: String,
}

#[derive(Debug, Clone, Default)]
pub struct SourceIdentityInput {
    pub guid: Option<String>,
    pub remote_id: Option<String>,
    pub master_id: Option<String>,
    pub fallback_fingerprint: Option<String>,
    pub confidence: Option<Confidence>,
}

#[derive(Debug, Clone)]
pub struct CompanyInput {
    pub endpoint_id: String,
    pub display_name: String,
    pub identity: SourceIdentityInput,
    pub observed_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompanyRef {
    pub id: String,
    pub display_name: String,
}

#[derive(Debug, Clone)]
pub struct ReviewedSetupInput {
    pub review_commitment_sha256: String,
    pub capability: CapabilitySnapshotInput,
    pub company_display_name: String,
    pub company_identity: SourceIdentityInput,
    pub company_number: String,
    pub books_from_yyyymmdd: String,
    pub selected_read_scope: Option<SelectedReadScopeInput>,
}

#[derive(Debug, Clone)]
pub struct SelectedReadScopeInput {
    pub scope_commitment_sha256: String,
    pub parent_review_sha256: String,
    pub ledger_profile_id: String,
    pub voucher_profile_id: String,
    pub voucher_from_yyyymmdd: String,
    pub voucher_to_yyyymmdd: String,
    pub company_number: String,
    pub books_from_yyyymmdd: String,
    pub observed_at_unix_ms: i64,
    pub observations: Vec<SelectedReadObservationInput>,
}

#[derive(Debug, Clone)]
pub struct SelectedReadObservationInput {
    pub capability_key: String,
    pub state: CapabilityState,
    pub confidence: Confidence,
    pub safe_reason_code: String,
    pub result_bucket: String,
    pub request_sha256: Option<String>,
    pub decoded_response_sha256: Option<String>,
    pub response_encoding: Option<String>,
    pub company_context_verified: bool,
    pub schema_verified: bool,
    pub record_count_verified: bool,
    pub identity_evidence_state: String,
    pub date_window_verified: bool,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct SelectedReadObservationCommitmentMaterial {
    pub capability_key: String,
    pub state: String,
    pub confidence: String,
    pub safe_reason_code: String,
    pub result_bucket: String,
    pub request_sha256: Option<String>,
    pub decoded_response_sha256: Option<String>,
    pub response_encoding: Option<String>,
    pub company_context_verified: bool,
    pub schema_verified: bool,
    pub record_count_verified: bool,
    pub identity_evidence_state: String,
    pub date_window_verified: bool,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct SelectedReadScopeCommitmentMaterial {
    pub parent_review_commitment_sha256: String,
    pub canonical_origin: String,
    pub company_guid_ascii_casefolded: String,
    pub company_name: String,
    pub company_number: String,
    pub books_from_yyyymmdd: String,
    pub ledger_profile_id: String,
    pub voucher_profile_id: String,
    pub voucher_from_yyyymmdd: String,
    pub voucher_to_yyyymmdd: String,
    pub observed_at_unix_ms: i64,
    pub observations: Vec<SelectedReadObservationCommitmentMaterial>,
}

#[derive(Debug, Clone, Serialize)]
struct SelectedReadScopeCommitmentMaterialV1 {
    parent_review_commitment_sha256: String,
    canonical_origin: String,
    company_guid_ascii_casefolded: String,
    ledger_profile_id: String,
    voucher_profile_id: String,
    voucher_from_yyyymmdd: String,
    voucher_to_yyyymmdd: String,
    observed_at_unix_ms: i64,
    observations: Vec<SelectedReadObservationCommitmentMaterial>,
}

#[derive(Serialize)]
struct SelectedReadScopeCommitmentEnvelope<'a> {
    schema: &'static str,
    #[serde(flatten)]
    material: &'a SelectedReadScopeCommitmentMaterial,
    no_writes_attempted: bool,
    raw_records_retained: bool,
    completeness_claimed: bool,
}

#[derive(Serialize)]
struct SelectedReadScopeCommitmentEnvelopeV1<'a> {
    schema: &'static str,
    #[serde(flatten)]
    material: &'a SelectedReadScopeCommitmentMaterialV1,
    no_writes_attempted: bool,
    raw_records_retained: bool,
    completeness_claimed: bool,
}

pub(crate) fn selected_read_scope_commitment_sha256(
    material: &SelectedReadScopeCommitmentMaterial,
) -> Result<String, MirrorError> {
    sha256_json(&SelectedReadScopeCommitmentEnvelope {
        schema: "bridge.tally.selected-read-scope/2",
        material,
        no_writes_attempted: true,
        raw_records_retained: false,
        completeness_claimed: false,
    })
}

fn selected_read_scope_commitment_sha256_v1(
    material: &SelectedReadScopeCommitmentMaterialV1,
) -> Result<String, MirrorError> {
    sha256_json(&SelectedReadScopeCommitmentEnvelopeV1 {
        schema: "bridge.tally.selected-read-scope/1",
        material,
        no_writes_attempted: true,
        raw_records_retained: false,
        completeness_claimed: false,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewedSetupRef {
    pub snapshot: CapabilitySnapshotRef,
    pub company: CompanyRef,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotSourcePin {
    pub company_id: String,
    pub endpoint_id: String,
    pub canonical_origin: String,
    pub display_name: String,
    pub company_guid: String,
    pub company_number: String,
    pub books_from_yyyymmdd: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PersistedCompanyProfile {
    pub name: String,
    pub guid: String,
    pub company_number: String,
    pub books_from_yyyymmdd: String,
    pub guid_observed: bool,
    pub mirror_company_id: String,
    pub correlation_key: String,
    pub identity_confidence: String,
    pub canonical_endpoint: String,
    pub last_observed_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PersistedCompanyProfilePage {
    pub profiles: Vec<PersistedCompanyProfile>,
    pub total_profiles: u64,
    pub limit: u32,
    pub truncated: bool,
}

/// One durable profile that shares a v1 client-label raw GUID. A missing
/// correlation key means the historical row cannot identify a composite book.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientGroupLabelMigrationProfile {
    pub guid: String,
    pub correlation_key: Option<String>,
    pub identity_confidence: String,
}

#[derive(Debug, Clone)]
pub struct WriteFixtureEnrollmentInput {
    pub company_id: String,
    pub review_commitment_sha256: String,
    pub disposable_company_attested: bool,
    pub no_customer_data_attested: bool,
    pub backup_guidance_acknowledged: bool,
    pub enrolled_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WriteFixtureEnrollmentStatus {
    pub fixture_state: &'static str,
    pub enrolled_at_unix_ms: Option<i64>,
    pub revoked_at_unix_ms: Option<i64>,
    pub candidate_gate: &'static str,
    pub write_capability: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteFixtureEnrollmentRef {
    pub id: String,
    pub enrolled_at_unix_ms: i64,
}

#[derive(Debug, Clone)]
pub struct WriteCanaryReservationInput {
    pub company_id: String,
    pub review_commitment_sha256: String,
    pub reserved_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteCanaryReservationRef {
    pub id: String,
    pub enrollment_id: String,
    /// Exact immutable commitment needed only by private coordinators to bind
    /// the fixed canary payload. This reference is never serialized to a UI
    /// or exposed through a Tauri command.
    pub reservation_payload_sha256: String,
    pub reserved_at_unix_ms: i64,
}

#[derive(Debug, Clone)]
pub struct WriteCanaryPayloadBindingInput {
    pub company_id: String,
    pub review_commitment_sha256: String,
    pub reservation_id: String,
    pub reservation_payload_sha256: String,
    pub wire_sha256: String,
    pub intended_state_sha256: String,
    pub identity_query_sha256: String,
    pub bound_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteCanaryPayloadBindingRef {
    pub id: String,
    pub reservation_id: String,
    pub bound_at_unix_ms: i64,
}

/// The complete immutable commitment set a future canary coordinator must
/// re-present immediately before it considers a readback or dispatch step.
/// This lookup never creates a binding and is not, by itself, an authority to
/// send data to Tally.
#[derive(Debug, Clone)]
pub struct ActiveWriteCanaryPayloadBindingInput {
    pub company_id: String,
    pub review_commitment_sha256: String,
    pub reservation_id: String,
    pub reservation_payload_sha256: String,
    pub wire_sha256: String,
    pub intended_state_sha256: String,
    pub identity_query_sha256: String,
}

/// A one-time, durable claim to run the sealed preflight read for the exact
/// canary binding. It is intentionally not an authority to dispatch a write.
#[derive(Debug, Clone)]
pub struct BeginWriteCanaryPreflightInput {
    pub binding: ActiveWriteCanaryPayloadBindingInput,
    pub started_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteCanaryPreflightAttemptRef {
    pub id: String,
    pub payload_binding_id: String,
    pub started_at_unix_ms: i64,
}

/// Digest-only evidence from the sealed canary readback. No raw response,
/// ledger value, or dispatch authority is persisted or returned.
#[derive(Debug, Clone)]
pub struct WriteCanaryPreflightEvidenceInput {
    pub attempt_id: String,
    pub readback_state_sha256: String,
    pub identity_coverage_sha256: String,
    pub canonical_endpoint_sha256: String,
    pub company_identity_sha256: String,
    pub verified_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteCanaryPreflightEvidenceRef {
    pub id: String,
    pub attempt_id: String,
    pub verified_at_unix_ms: i64,
}

/// The complete durable evidence commitment a future dispatch coordinator must
/// re-present. Verifying it remains read-only and cannot issue a Tally import.
#[derive(Debug, Clone)]
pub struct ActiveWriteCanaryPreflightEvidenceInput {
    pub binding: ActiveWriteCanaryPayloadBindingInput,
    pub attempt_id: String,
    pub evidence_id: String,
    pub readback_state_sha256: String,
    pub identity_coverage_sha256: String,
    pub canonical_endpoint_sha256: String,
    pub company_identity_sha256: String,
}

/// A one-time, durable claim that a future reviewed coordinator may consider
/// dispatching the exact synthetic canary. The claim itself cannot send data.
#[derive(Debug, Clone)]
pub struct BeginWriteCanaryDispatchInput {
    pub evidence: ActiveWriteCanaryPreflightEvidenceInput,
    pub claimed_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteCanaryDispatchAttemptRef {
    pub id: String,
    pub evidence_id: String,
    pub claimed_at_unix_ms: i64,
}

/// The complete immutable commitment set a final verdict must re-present.
/// This lookup remains local and cannot construct, send, or retry a Tally
/// import.
#[derive(Debug, Clone)]
pub struct ActiveWriteCanaryDispatchAttemptInput {
    pub evidence: ActiveWriteCanaryPreflightEvidenceInput,
    pub dispatch_attempt_id: String,
    pub claimed_at_unix_ms: i64,
}

/// Digest-only record of an exact semantic observation correlated to one
/// durable dispatch claim. It deliberately stores no raw Tally response,
/// readback, payload, configuration, or transport detail.
#[derive(Debug, Clone)]
pub struct WriteCanaryFinalVerdictInput {
    pub dispatch: ActiveWriteCanaryDispatchAttemptInput,
    pub import_response_sha256: String,
    pub readback_state_sha256: String,
    pub identity_coverage_sha256: String,
    pub recorded_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteCanaryFinalVerdictRef {
    pub id: String,
    pub dispatch_attempt_id: String,
    pub recorded_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MirrorExplorerRecord {
    pub local_alias: String,
    pub object_type: String,
    pub identity_confidence: String,
    pub last_batch_state: String,
    pub tombstoned: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MirrorExplorerPage {
    pub pack_id: String,
    pub offset: u32,
    pub limit: u32,
    pub total_records: u64,
    pub records: Vec<MirrorExplorerRecord>,
}

#[derive(Debug, Clone)]
pub struct BeginBatchInput {
    pub run_id: String,
    pub capability_snapshot_id: String,
    pub company_id: String,
    pub pack_id: String,
    pub pack_schema_major: u16,
    pub pack_schema_minor: u16,
    pub source_transport: String,
    pub source_release: Option<String>,
    pub requested_from_yyyymmdd: Option<String>,
    pub requested_to_yyyymmdd: Option<String>,
    pub started_at_unix_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObservationStatus {
    Accepted,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObserveRecordOutcome {
    Inserted { observation_id: String },
    AlreadyPresentIdentical { observation_id: String },
}

impl ObservationStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Rejected => "rejected",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ObservedRecordInput {
    pub batch_id: String,
    pub object_type: String,
    pub display_name: Option<String>,
    pub identity: SourceIdentityInput,
    pub observed_at_unix_ms: i64,
    pub raw_source_sha256: String,
    pub canonical_sha256: Option<String>,
    pub canonical_payload: Option<Value>,
    pub exact_decimals: BTreeMap<String, String>,
    pub observed_alter_id: Option<String>,
    pub status: ObservationStatus,
    pub safe_rejection_code: Option<String>,
}

struct PreparedObservedRecord {
    canonical_payload_json: Option<String>,
    exact_decimals_json: String,
}

struct ObservedTransactionResult {
    outcome: ObserveRecordOutcome,
    source_record_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotWindowAttemptRef {
    pub attempt_id: String,
    pub batch_id: String,
    pub window_id: String,
    pub attempt_ordinal: u32,
}

#[derive(Debug, Clone)]
pub struct BeginSnapshotWindowAttemptInput {
    pub batch_id: String,
    pub window_id: String,
    pub started_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BeginSnapshotWindowAttemptResult {
    pub attempt: SnapshotWindowAttemptRef,
    pub prior_abandonment: Option<AbandonSnapshotWindowAttemptResult>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AbandonSnapshotWindowAttemptResult {
    pub completed_at_unix_ms: i64,
    pub local_clock_moved_backwards: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotWindowAttemptCleanupResult {
    pub completed_at_floor: Option<i64>,
    pub local_clock_moved_backwards: bool,
}

#[derive(Debug, Clone)]
pub enum SnapshotWindowMembershipInput {
    Observed {
        record_key: String,
        observation: Box<ObservedRecordInput>,
    },
    ProvenanceUnavailable {
        record_key: String,
        canonical_sha256: String,
        canonical_payload: Value,
        exact_decimals: BTreeMap<String, String>,
        safe_reason_code: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StageSnapshotWindowMembershipsResult {
    pub inserted_memberships: u32,
    pub replayed_memberships: u32,
    pub inserted_observations: u32,
    pub replayed_observations: u32,
    pub provenance_unavailable_memberships: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotWindowReceipt {
    pub schema: String,
    pub attempt_id: String,
    pub batch_id: String,
    pub window_id: String,
    pub attempt_ordinal: u32,
    pub member_count: u32,
    pub membership_sha256: String,
    pub evidence: Value,
    pub completed_at_unix_ms: i64,
    pub receipt_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotWindowCompletionResult {
    pub receipt: SnapshotWindowReceipt,
    pub local_clock_moved_backwards: bool,
}

impl std::ops::Deref for SnapshotWindowCompletionResult {
    type Target = SnapshotWindowReceipt;

    fn deref(&self) -> &Self::Target {
        &self.receipt
    }
}

#[derive(Serialize)]
struct SnapshotWindowReceiptMaterial<'a> {
    schema: &'static str,
    attempt_id: &'a str,
    batch_id: &'a str,
    window_id: &'a str,
    attempt_ordinal: u32,
    member_count: u32,
    membership_sha256: &'a str,
    evidence: &'a Value,
    completed_at_unix_ms: i64,
}

#[derive(Serialize)]
struct SnapshotWindowMembershipDigestEntry<'a> {
    record_key: &'a str,
    canonical_sha256: &'a str,
    provenance_state: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunOutcome {
    Completed,
    Failed,
    Cancelled,
    OutcomeUnknown,
}

impl RunOutcome {
    fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::OutcomeUnknown => "outcome_unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationState {
    Verified,
    Partial,
    Unverified,
}

impl VerificationState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Verified => "verified",
            Self::Partial => "partial",
            Self::Unverified => "unverified",
        }
    }

    fn batch_state(self) -> &'static str {
        match self {
            Self::Verified => "verified",
            Self::Partial => "partial",
            Self::Unverified => "failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitResult {
    pub proof_id: String,
    pub proof_sha256: String,
    pub checkpoint_advanced: bool,
    pub facts: CommitReceiptFacts,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommitReceiptFacts {
    pub proof_contract_version: u16,
    pub run_id: String,
    pub batch_id: String,
    pub capability_snapshot_id: String,
    pub company_id: String,
    pub pack_id: String,
    pub outcome: RunOutcome,
    pub verification: VerificationState,
    pub started_at_unix_ms: i64,
    pub completed_at_unix_ms: i64,
    pub accepted_records: i64,
    pub rejected_records: i64,
    pub provenance_unavailable_records: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record_counts_sha256: Option<String>,
    pub snapshot_sha256: Option<String>,
    pub checkpoint_before: Option<String>,
    pub checkpoint_after: Option<String>,
    pub gap_codes: Vec<String>,
    pub warning_codes: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObservationCounts {
    pub accepted_records: i64,
    pub rejected_records: i64,
    pub provenance_unavailable_records: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreshnessState {
    Fresh,
    Stale,
    NeverVerified,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FreshnessStatus {
    pub state: FreshnessState,
    pub verified_at_unix_ms: Option<i64>,
    pub age_seconds: Option<i64>,
    pub checkpoint_token: Option<String>,
    pub proof_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProofSummary {
    pub integrity_state: &'static str,
    pub run_id: String,
    pub selection_token: String,
    pub proof_sha256: String,
    pub pack_id: String,
    pub outcome: String,
    pub verification_state: String,
    pub started_at_unix_ms: i64,
    pub completed_at_unix_ms: Option<i64>,
    pub accepted_records: i64,
    pub rejected_records: i64,
    pub provenance_unavailable_records: i64,
    pub gap_codes: Vec<String>,
    pub warning_codes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RedactedProofExport {
    pub json: String,
    pub payload_sha256: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct LocalReconciliationMismatch {
    pub reason_code: String,
    pub record_aliases: Vec<String>,
}

#[derive(Serialize)]
struct RedactedProofPayload {
    schema: &'static str,
    schema_version: u16,
    exported_at_unix_ms: i64,
    redaction_profile: &'static str,
    subject: RedactedSubject,
    proofs: Vec<RedactedProofEntry>,
    current_status: RedactedCurrentStatus,
}

#[derive(Serialize)]
struct RedactedSubject {
    reference: &'static str,
    identity_disclosed: bool,
}

#[derive(Serialize)]
struct RedactedProofEntry {
    entry_index: u16,
    proof_contract_version: u16,
    pack_id: String,
    pack_schema_version: PackSchemaVersion,
    outcome: String,
    verification_state: String,
    started_at_unix_ms: i64,
    completed_at_unix_ms: i64,
    counts: RedactedCounts,
    gaps: Vec<String>,
    warnings: Vec<String>,
    local_ledger: RedactedLedgerEvidence,
}

#[derive(Serialize)]
struct RedactedCounts {
    provenance_backed_accepted_records: i64,
    provenance_unavailable_records: i64,
    rejected_records: i64,
}

#[derive(Serialize)]
struct RedactedLedgerEvidence {
    chain_validation: &'static str,
}

#[derive(Serialize)]
struct RedactedCurrentStatus {
    freshness_state: &'static str,
    verified_at_unix_ms: Option<i64>,
    checkpoint_present: bool,
}

#[derive(Serialize)]
struct RedactedProofDocument {
    #[serde(flatten)]
    payload: RedactedProofPayload,
    integrity: RedactedIntegrity,
}

#[derive(Serialize)]
struct RedactedIntegrity {
    canonicalization: &'static str,
    hash_algorithm: &'static str,
    domain: &'static str,
    payload_sha256: String,
    signature: Option<String>,
    integrity_claim: &'static str,
    authenticity_claim: &'static str,
}

#[derive(Clone)]
pub struct TallyMirrorRepository {
    pub(super) pool: SqlitePool,
}

impl TallyMirrorRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub(crate) fn pool_clone(&self) -> SqlitePool {
        self.pool.clone()
    }

    /// Returns the complete durable history sharing an existing client-label
    /// raw GUID. It intentionally includes every confidence state and has no
    /// UI pagination limit: suppressing an older split book here could turn an
    /// unsafe migration into an apparently resolved one.
    pub async fn persisted_company_profiles_for_client_group_label_migration(
        &self,
        raw_guids: &[String],
    ) -> Result<Vec<ClientGroupLabelMigrationProfile>, MirrorError> {
        let raw_guids = raw_guids
            .iter()
            .filter_map(|raw_guid| {
                let raw_guid = raw_guid.trim();
                (!raw_guid.is_empty()).then(|| raw_guid.to_ascii_lowercase())
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let mut profiles = Vec::new();
        for raw_guid_chunk in raw_guids.chunks(500) {
            let mut query = QueryBuilder::<Sqlite>::new(
                "SELECT c.display_name, c.company_guid, c.identity_confidence, \
                 c.company_number, c.books_from_yyyymmdd, e.canonical_origin \
                 FROM tally_companies AS c \
                 JOIN tally_endpoints AS e ON e.id = c.endpoint_id \
                 WHERE c.company_guid IS NOT NULL AND TRIM(c.company_guid) <> '' \
                   AND (",
            );
            {
                let mut conditions = query.separated(" OR ");
                for raw_guid in raw_guid_chunk {
                    conditions.push("c.company_guid COLLATE NOCASE = ");
                    conditions.push_bind_unseparated(raw_guid);
                }
            }
            query.push(") ORDER BY c.last_observed_at_unix_ms DESC, c.id ASC");
            let rows = query.build().fetch_all(&self.pool).await?;
            profiles.extend(
                rows.into_iter()
                    .map(client_group_label_migration_profile_from_row)
                    .collect::<Result<Vec<_>, sqlx::Error>>()?,
            );
        }
        Ok(profiles)
    }

    pub async fn persisted_company_profiles(
        &self,
    ) -> Result<PersistedCompanyProfilePage, MirrorError> {
        const LIMIT: u32 = 500;
        let mut transaction = self.pool.begin().await?;
        let total_profiles = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_companies AS c \
             WHERE c.identity_confidence = 'observed' \
               AND c.company_guid IS NOT NULL AND TRIM(c.company_guid) <> '' \
               AND c.company_number IS NOT NULL AND TRIM(c.company_number) <> '' \
               AND c.books_from_yyyymmdd IS NOT NULL AND TRIM(c.books_from_yyyymmdd) <> ''",
        )
        .fetch_one(&mut *transaction)
        .await?;
        let rows = sqlx::query(
            "SELECT c.id, c.display_name, c.company_guid, c.identity_confidence, \
             c.company_number, c.books_from_yyyymmdd, c.last_observed_at_unix_ms, e.canonical_origin \
             FROM tally_companies AS c \
             JOIN tally_endpoints AS e ON e.id = c.endpoint_id \
             WHERE c.identity_confidence = 'observed' \
               AND c.company_guid IS NOT NULL AND TRIM(c.company_guid) <> '' \
               AND c.company_number IS NOT NULL AND TRIM(c.company_number) <> '' \
               AND c.books_from_yyyymmdd IS NOT NULL AND TRIM(c.books_from_yyyymmdd) <> '' \
             ORDER BY c.last_observed_at_unix_ms DESC, c.id ASC LIMIT ?1",
        )
        .bind(i64::from(LIMIT))
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        let profiles = rows
            .into_iter()
            .map(persisted_company_profile_from_row)
            .collect::<Result<Vec<_>, sqlx::Error>>()?;
        let total_profiles = u64::try_from(total_profiles)
            .map_err(|_| MirrorError::InvalidInput("persisted_company_profile_count"))?;
        Ok(PersistedCompanyProfilePage {
            truncated: total_profiles > u64::from(LIMIT),
            profiles,
            total_profiles,
            limit: LIMIT,
        })
    }

    pub async fn mirror_explorer_page(
        &self,
        company_id: &str,
        pack_id: &str,
        offset: u32,
        limit: u32,
    ) -> Result<MirrorExplorerPage, MirrorError> {
        validate_nonempty(company_id, 128, "mirror_explorer_company")?;
        validate_nonempty(pack_id, 64, "mirror_explorer_pack")?;
        if limit == 0 || limit > 100 || offset > 1_000_000 {
            return Err(MirrorError::InvalidInput("mirror_explorer_page"));
        }
        let mut transaction = self.pool.begin().await?;
        let total_records = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_source_records AS record \
             JOIN tally_observation_batches AS batch ON batch.id = record.last_seen_batch_id \
             WHERE record.company_id = ?1 AND batch.company_id = record.company_id \
               AND batch.pack_id = ?2",
        )
        .bind(company_id)
        .bind(pack_id)
        .fetch_one(&mut *transaction)
        .await?;
        let rows = sqlx::query(
            "SELECT record.object_type, record.identity_confidence, \
             record.tombstoned_at_unix_ms, batch.state \
             FROM tally_source_records AS record \
             JOIN tally_observation_batches AS batch ON batch.id = record.last_seen_batch_id \
             WHERE record.company_id = ?1 AND batch.company_id = record.company_id \
               AND batch.pack_id = ?2 \
             ORDER BY record.object_type ASC, record.id ASC LIMIT ?3 OFFSET ?4",
        )
        .bind(company_id)
        .bind(pack_id)
        .bind(i64::from(limit))
        .bind(i64::from(offset))
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        let records = rows
            .into_iter()
            .enumerate()
            .map(|(index, row)| {
                Ok(MirrorExplorerRecord {
                    local_alias: format!("local-record-{}", u64::from(offset) + index as u64 + 1),
                    object_type: row.try_get("object_type")?,
                    identity_confidence: row.try_get("identity_confidence")?,
                    last_batch_state: row.try_get("state")?,
                    tombstoned: row
                        .try_get::<Option<i64>, _>("tombstoned_at_unix_ms")?
                        .is_some(),
                })
            })
            .collect::<Result<Vec<_>, sqlx::Error>>()?;
        Ok(MirrorExplorerPage {
            pack_id: pack_id.to_string(),
            offset,
            limit,
            total_records: u64::try_from(total_records)
                .map_err(|_| MirrorError::InvalidInput("mirror_explorer_count"))?,
            records,
        })
    }

    pub async fn snapshot_source_pin(
        &self,
        company_id: &str,
    ) -> Result<SnapshotSourcePin, MirrorError> {
        // See TALLY_PROTOCOL_REFERENCE.md §9.11b: a year-end child can
        // inherit its parent's GUID, so a persisted pin needs the full
        // observed tuple rather than a GUID-only lookup.
        if company_id.trim().is_empty() {
            return Err(MirrorError::InvalidInput("company_pin"));
        }
        let row = sqlx::query(
            "SELECT c.id AS company_id, c.endpoint_id, e.canonical_origin, c.display_name, \
             c.company_guid, c.company_number, c.books_from_yyyymmdd, c.identity_confidence FROM tally_companies c \
             JOIN tally_endpoints e ON e.id = c.endpoint_id WHERE c.id = ?1",
        )
        .bind(company_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(MirrorError::NotFound)?;
        let identity_confidence: String = row.try_get("identity_confidence")?;
        let company_number: Option<String> = row.try_get("company_number")?;
        let books_from_yyyymmdd: Option<String> = row.try_get("books_from_yyyymmdd")?;
        if identity_confidence != "observed" {
            if company_number.as_deref().is_none_or(str::is_empty)
                || books_from_yyyymmdd.as_deref().is_none_or(str::is_empty)
            {
                return Err(MirrorError::InvalidInput(
                    "company_identity_reverification_required",
                ));
            }
            return Err(MirrorError::InvalidInput("company_identity_not_observed"));
        }
        let company_guid: Option<String> = row.try_get("company_guid")?;
        Ok(SnapshotSourcePin {
            company_id: row.try_get("company_id")?,
            endpoint_id: row.try_get("endpoint_id")?,
            canonical_origin: row.try_get("canonical_origin")?,
            display_name: row.try_get("display_name")?,
            company_guid: company_guid
                .filter(|guid| !guid.trim().is_empty())
                .ok_or(MirrorError::InvalidInput("company_guid_unobserved"))?,
            company_number: company_number
                .filter(|number| !number.trim().is_empty())
                .ok_or(MirrorError::InvalidInput(
                    "company_identity_reverification_required",
                ))?,
            books_from_yyyymmdd: books_from_yyyymmdd
                .filter(|date| !date.trim().is_empty())
                .ok_or(MirrorError::InvalidInput(
                    "company_identity_reverification_required",
                ))?,
        })
    }

    pub async fn enroll_write_fixture(
        &self,
        input: WriteFixtureEnrollmentInput,
    ) -> Result<WriteFixtureEnrollmentRef, MirrorError> {
        validate_nonempty(&input.company_id, 128, "fixture_company_id")?;
        validate_sha256(&input.review_commitment_sha256)?;
        if !input.disposable_company_attested
            || !input.no_customer_data_attested
            || !input.backup_guidance_acknowledged
            || input.enrolled_at_unix_ms <= 0
        {
            return Err(MirrorError::InvalidInput("fixture_attestation"));
        }
        let pin = self.snapshot_source_pin(&input.company_id).await?;
        let payload_sha256 = fixture_enrollment_payload_sha256(&FixtureEnrollmentCommitment {
            schema: "bridge.tally.write-fixture-enrollment/1",
            review_commitment_sha256: &input.review_commitment_sha256,
            company_id: &pin.company_id,
            canonical_origin: &pin.canonical_origin,
            company_guid_ascii_casefolded: &pin.company_guid.to_ascii_lowercase(),
            contract_version: 1,
            disposable_company_attested: true,
            no_customer_data_attested: true,
            backup_guidance_acknowledged: true,
        })?;

        let mut transaction = self.pool.begin().await?;
        if let Some(row) = sqlx::query(
            "SELECT id, enrollment_payload_sha256, enrolled_at_unix_ms \
             FROM tally_write_fixture_enrollments WHERE review_commitment_sha256 = ?1",
        )
        .bind(&input.review_commitment_sha256)
        .fetch_optional(&mut *transaction)
        .await?
        {
            let existing_payload: String = row.try_get("enrollment_payload_sha256")?;
            if existing_payload != payload_sha256 {
                return Err(MirrorError::InvalidInput(
                    "fixture_review_commitment_reused",
                ));
            }
            let result = WriteFixtureEnrollmentRef {
                id: row.try_get("id")?,
                enrolled_at_unix_ms: row.try_get("enrolled_at_unix_ms")?,
            };
            transaction.commit().await?;
            return Ok(result);
        }
        let id = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO tally_write_fixture_enrollments(\
               id, company_id, review_commitment_sha256, enrollment_payload_sha256, \
               contract_version, disposable_company_attested, no_customer_data_attested, \
               backup_guidance_acknowledged, enrolled_at_unix_ms\
             ) VALUES (?1, ?2, ?3, ?4, 1, 1, 1, 1, ?5)",
        )
        .bind(&id)
        .bind(&pin.company_id)
        .bind(&input.review_commitment_sha256)
        .bind(payload_sha256)
        .bind(input.enrolled_at_unix_ms)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(WriteFixtureEnrollmentRef {
            id,
            enrolled_at_unix_ms: input.enrolled_at_unix_ms,
        })
    }

    /// Atomically consumes the one canary slot for an active, reviewed fixture.
    /// A replay with the same reviewed commitment returns the same reservation;
    /// a different commitment can never allocate a second slot.
    pub async fn reserve_write_canary(
        &self,
        input: WriteCanaryReservationInput,
    ) -> Result<WriteCanaryReservationRef, MirrorError> {
        validate_nonempty(&input.company_id, 128, "fixture_company_id")?;
        validate_sha256(&input.review_commitment_sha256)?;
        if input.reserved_at_unix_ms <= 0 {
            return Err(MirrorError::InvalidInput("canary_reserved_at"));
        }

        let mut transaction = self.pool.begin().await?;
        // SQLite starts deferred transactions. Acquire its write lock before
        // observing an enrollment so concurrent callers serialize around the
        // single durable reservation rather than racing into a lock error.
        sqlx::query("UPDATE tally_schema_migrations SET version = version WHERE version = 4")
            .execute(&mut *transaction)
            .await?;
        let enrollment = sqlx::query(
            "SELECT enrollment.id AS enrollment_id, enrollment.enrollment_payload_sha256, \
             company.id AS company_id, endpoint.canonical_origin, company.company_guid \
             FROM tally_write_fixture_enrollments AS enrollment \
             JOIN tally_companies AS company ON company.id = enrollment.company_id \
             JOIN tally_endpoints AS endpoint ON endpoint.id = company.endpoint_id \
             WHERE enrollment.company_id = ?1 \
               AND enrollment.review_commitment_sha256 = ?2 \
               AND company.identity_confidence = 'observed' \
               AND company.company_guid IS NOT NULL \
               AND TRIM(company.company_guid) <> '' \
               AND NOT EXISTS ( \
                 SELECT 1 FROM tally_write_fixture_revocations AS revocation \
                 WHERE revocation.enrollment_id = enrollment.id \
               )",
        )
        .bind(&input.company_id)
        .bind(&input.review_commitment_sha256)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(MirrorError::InvalidInput("fixture_enrollment_not_active"))?;
        let enrollment_id: String = enrollment.try_get("enrollment_id")?;
        let enrollment_payload_sha256: String = enrollment.try_get("enrollment_payload_sha256")?;
        let company_id: String = enrollment.try_get("company_id")?;
        let canonical_origin: String = enrollment.try_get("canonical_origin")?;
        let company_guid: String = enrollment.try_get("company_guid")?;
        let company_guid_ascii_casefolded = company_guid.to_ascii_lowercase();
        let reservation_payload_sha256 =
            canary_reservation_payload_sha256(&CanaryReservationCommitment {
                schema: "bridge.tally.write-canary-reservation/1",
                enrollment_id: &enrollment_id,
                enrollment_payload_sha256: &enrollment_payload_sha256,
                company_id: &company_id,
                canonical_origin: &canonical_origin,
                company_guid_ascii_casefolded: &company_guid_ascii_casefolded,
                review_commitment_sha256: &input.review_commitment_sha256,
                contract_version: 1,
            })?;

        sqlx::query(
            "INSERT INTO tally_write_canary_reservations( \
               id, enrollment_id, reservation_payload_sha256, contract_version, reserved_at_unix_ms \
             ) VALUES (?1, ?2, ?3, 1, ?4) \
             ON CONFLICT(enrollment_id) DO NOTHING",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(&enrollment_id)
        .bind(&reservation_payload_sha256)
        .bind(input.reserved_at_unix_ms)
        .execute(&mut *transaction)
        .await?;
        let reservation = sqlx::query(
            "SELECT id, reservation_payload_sha256, reserved_at_unix_ms \
             FROM tally_write_canary_reservations WHERE enrollment_id = ?1",
        )
        .bind(&enrollment_id)
        .fetch_one(&mut *transaction)
        .await?;
        let existing_payload_sha256: String = reservation.try_get("reservation_payload_sha256")?;
        if existing_payload_sha256 != reservation_payload_sha256 {
            return Err(MirrorError::InvalidInput("canary_slot_already_reserved"));
        }
        let result = WriteCanaryReservationRef {
            id: reservation.try_get("id")?,
            enrollment_id,
            reservation_payload_sha256: existing_payload_sha256,
            reserved_at_unix_ms: reservation.try_get("reserved_at_unix_ms")?,
        };
        transaction.commit().await?;
        Ok(result)
    }

    /// Atomically binds the only reserved canary slot to its exact immutable
    /// wire, intended-state, and readback-query commitments.
    pub async fn bind_write_canary_payload(
        &self,
        input: WriteCanaryPayloadBindingInput,
    ) -> Result<WriteCanaryPayloadBindingRef, MirrorError> {
        validate_nonempty(&input.company_id, 128, "fixture_company_id")?;
        validate_sha256(&input.review_commitment_sha256)?;
        validate_nonempty(&input.reservation_id, 128, "canary_reservation_id")?;
        validate_sha256(&input.reservation_payload_sha256)?;
        validate_sha256(&input.wire_sha256)?;
        validate_sha256(&input.intended_state_sha256)?;
        validate_sha256(&input.identity_query_sha256)?;
        if input.bound_at_unix_ms <= 0 {
            return Err(MirrorError::InvalidInput("canary_payload_bound_at"));
        }

        let mut transaction = self.pool.begin().await?;
        // Serialize the active-enrollment check and binding insert so a
        // revocation cannot race an otherwise valid payload commitment.
        sqlx::query("UPDATE tally_schema_migrations SET version = version WHERE version = 4")
            .execute(&mut *transaction)
            .await?;
        let reservation_exists = sqlx::query_scalar::<_, i64>(
            r#"
                SELECT COUNT(*)
                FROM tally_write_canary_reservations AS reservation
                JOIN tally_write_fixture_enrollments AS enrollment
                  ON enrollment.id = reservation.enrollment_id
                WHERE reservation.id = ?1
                  AND enrollment.company_id = ?2
                  AND enrollment.review_commitment_sha256 = ?3
                  AND reservation.reservation_payload_sha256 = ?4
                  AND NOT EXISTS (
                    SELECT 1
                    FROM tally_write_fixture_revocations AS revocation
                    WHERE revocation.enrollment_id = enrollment.id
                  )
            "#,
        )
        .bind(&input.reservation_id)
        .bind(&input.company_id)
        .bind(&input.review_commitment_sha256)
        .bind(&input.reservation_payload_sha256)
        .fetch_one(&mut *transaction)
        .await?;
        if reservation_exists != 1 {
            return Err(MirrorError::InvalidInput("canary_reservation_not_active"));
        }
        sqlx::query(
            r#"
                INSERT INTO tally_write_canary_payload_bindings(
                  id, reservation_id, wire_sha256, intended_state_sha256, identity_query_sha256,
                  contract_version, bound_at_unix_ms
                ) VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6)
                ON CONFLICT(reservation_id) DO NOTHING
            "#,
        )
        .bind(Uuid::new_v4().to_string())
        .bind(&input.reservation_id)
        .bind(&input.wire_sha256)
        .bind(&input.intended_state_sha256)
        .bind(&input.identity_query_sha256)
        .bind(input.bound_at_unix_ms)
        .execute(&mut *transaction)
        .await?;
        let binding = sqlx::query(
            r#"
                SELECT id, wire_sha256, intended_state_sha256, identity_query_sha256,
                       bound_at_unix_ms
                FROM tally_write_canary_payload_bindings
                WHERE reservation_id = ?1
            "#,
        )
        .bind(&input.reservation_id)
        .fetch_one(&mut *transaction)
        .await?;
        let existing_wire_sha256: String = binding.try_get("wire_sha256")?;
        let existing_intended_state_sha256: String = binding.try_get("intended_state_sha256")?;
        let existing_identity_query_sha256: String = binding.try_get("identity_query_sha256")?;
        if existing_wire_sha256 != input.wire_sha256
            || existing_intended_state_sha256 != input.intended_state_sha256
            || existing_identity_query_sha256 != input.identity_query_sha256
        {
            return Err(MirrorError::InvalidInput("canary_payload_already_bound"));
        }
        let result = WriteCanaryPayloadBindingRef {
            id: binding.try_get("id")?,
            reservation_id: input.reservation_id,
            bound_at_unix_ms: binding.try_get("bound_at_unix_ms")?,
        };
        transaction.commit().await?;
        Ok(result)
    }

    /// Verifies that one active, durable fixture enrollment has already bound
    /// exactly these canary commitments. This deliberately performs no insert
    /// or state transition: a later coordinator must make its own atomic
    /// attempt claim before network activity.
    pub async fn active_write_canary_payload_binding(
        &self,
        input: ActiveWriteCanaryPayloadBindingInput,
    ) -> Result<WriteCanaryPayloadBindingRef, MirrorError> {
        validate_nonempty(&input.company_id, 128, "fixture_company_id")?;
        validate_sha256(&input.review_commitment_sha256)?;
        validate_nonempty(&input.reservation_id, 128, "canary_reservation_id")?;
        validate_sha256(&input.reservation_payload_sha256)?;
        validate_sha256(&input.wire_sha256)?;
        validate_sha256(&input.intended_state_sha256)?;
        validate_sha256(&input.identity_query_sha256)?;

        let binding = sqlx::query(
            r#"
                SELECT binding.id, binding.reservation_id, binding.bound_at_unix_ms
                FROM tally_write_canary_payload_bindings AS binding
                JOIN tally_write_canary_reservations AS reservation
                  ON reservation.id = binding.reservation_id
                JOIN tally_write_fixture_enrollments AS enrollment
                  ON enrollment.id = reservation.enrollment_id
                WHERE binding.reservation_id = ?1
                  AND enrollment.company_id = ?2
                  AND enrollment.review_commitment_sha256 = ?3
                  AND reservation.reservation_payload_sha256 = ?4
                  AND binding.wire_sha256 = ?5
                  AND binding.intended_state_sha256 = ?6
                  AND binding.identity_query_sha256 = ?7
                  AND NOT EXISTS (
                    SELECT 1
                    FROM tally_write_fixture_revocations AS revocation
                    WHERE revocation.enrollment_id = enrollment.id
                  )
            "#,
        )
        .bind(&input.reservation_id)
        .bind(&input.company_id)
        .bind(&input.review_commitment_sha256)
        .bind(&input.reservation_payload_sha256)
        .bind(&input.wire_sha256)
        .bind(&input.intended_state_sha256)
        .bind(&input.identity_query_sha256)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(MirrorError::InvalidInput(
            "canary_payload_binding_not_active",
        ))?;
        Ok(WriteCanaryPayloadBindingRef {
            id: binding.try_get("id")?,
            reservation_id: binding.try_get("reservation_id")?,
            bound_at_unix_ms: binding.try_get("bound_at_unix_ms")?,
        })
    }

    /// Atomically consumes the one available preflight-read attempt for an
    /// active, exact canary binding. The caller receives no write capability;
    /// this records only the durable precondition for a future sealed
    /// readback. A second call is rejected rather than retried automatically.
    pub async fn begin_write_canary_preflight(
        &self,
        input: BeginWriteCanaryPreflightInput,
    ) -> Result<WriteCanaryPreflightAttemptRef, MirrorError> {
        let binding = input.binding;
        validate_nonempty(&binding.company_id, 128, "fixture_company_id")?;
        validate_sha256(&binding.review_commitment_sha256)?;
        validate_nonempty(&binding.reservation_id, 128, "canary_reservation_id")?;
        validate_sha256(&binding.reservation_payload_sha256)?;
        validate_sha256(&binding.wire_sha256)?;
        validate_sha256(&binding.intended_state_sha256)?;
        validate_sha256(&binding.identity_query_sha256)?;
        if input.started_at_unix_ms <= 0 {
            return Err(MirrorError::InvalidInput("canary_preflight_started_at"));
        }

        let mut transaction = self.pool.begin().await?;
        // SQLite starts deferred transactions. Take its write lock before
        // checking revocation and claiming the only preflight attempt so those
        // decisions cannot race one another.
        sqlx::query("UPDATE tally_schema_migrations SET version = version WHERE version = 4")
            .execute(&mut *transaction)
            .await?;
        let payload_binding_id = sqlx::query_scalar::<_, String>(
            r#"
                SELECT binding.id
                FROM tally_write_canary_payload_bindings AS binding
                JOIN tally_write_canary_reservations AS reservation
                  ON reservation.id = binding.reservation_id
                JOIN tally_write_fixture_enrollments AS enrollment
                  ON enrollment.id = reservation.enrollment_id
                WHERE binding.reservation_id = ?1
                  AND enrollment.company_id = ?2
                  AND enrollment.review_commitment_sha256 = ?3
                  AND reservation.reservation_payload_sha256 = ?4
                  AND binding.wire_sha256 = ?5
                  AND binding.intended_state_sha256 = ?6
                  AND binding.identity_query_sha256 = ?7
                  AND NOT EXISTS (
                    SELECT 1
                    FROM tally_write_fixture_revocations AS revocation
                    WHERE revocation.enrollment_id = enrollment.id
                  )
            "#,
        )
        .bind(&binding.reservation_id)
        .bind(&binding.company_id)
        .bind(&binding.review_commitment_sha256)
        .bind(&binding.reservation_payload_sha256)
        .bind(&binding.wire_sha256)
        .bind(&binding.intended_state_sha256)
        .bind(&binding.identity_query_sha256)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(MirrorError::InvalidInput(
            "canary_payload_binding_not_active",
        ))?;
        let attempt_id = Uuid::new_v4().to_string();
        let inserted = sqlx::query(
            "INSERT INTO tally_write_canary_preflight_attempts( \
               id, payload_binding_id, contract_version, started_at_unix_ms \
             ) VALUES (?1, ?2, 1, ?3) ON CONFLICT(payload_binding_id) DO NOTHING",
        )
        .bind(&attempt_id)
        .bind(&payload_binding_id)
        .bind(input.started_at_unix_ms)
        .execute(&mut *transaction)
        .await?
        .rows_affected();
        if inserted != 1 {
            return Err(MirrorError::InvalidInput(
                "canary_preflight_attempt_already_started",
            ));
        }
        transaction.commit().await?;
        Ok(WriteCanaryPreflightAttemptRef {
            id: attempt_id,
            payload_binding_id,
            started_at_unix_ms: input.started_at_unix_ms,
        })
    }

    /// Stores only the digest evidence for a sealed preflight readback after
    /// rechecking that its originating fixture enrollment is still active.
    /// An exact replay is idempotent; changed evidence fails closed.
    pub async fn record_write_canary_preflight_evidence(
        &self,
        input: WriteCanaryPreflightEvidenceInput,
    ) -> Result<WriteCanaryPreflightEvidenceRef, MirrorError> {
        validate_nonempty(&input.attempt_id, 128, "canary_preflight_attempt_id")?;
        validate_sha256(&input.readback_state_sha256)?;
        validate_sha256(&input.identity_coverage_sha256)?;
        validate_sha256(&input.canonical_endpoint_sha256)?;
        validate_sha256(&input.company_identity_sha256)?;
        if input.verified_at_unix_ms <= 0 {
            return Err(MirrorError::InvalidInput("canary_preflight_verified_at"));
        }

        let mut transaction = self.pool.begin().await?;
        sqlx::query("UPDATE tally_schema_migrations SET version = version WHERE version = 4")
            .execute(&mut *transaction)
            .await?;
        let started_at_unix_ms = sqlx::query_scalar::<_, i64>(
            r#"
                SELECT attempt.started_at_unix_ms
                FROM tally_write_canary_preflight_attempts AS attempt
                JOIN tally_write_canary_payload_bindings AS binding
                  ON binding.id = attempt.payload_binding_id
                JOIN tally_write_canary_reservations AS reservation
                  ON reservation.id = binding.reservation_id
                JOIN tally_write_fixture_enrollments AS enrollment
                  ON enrollment.id = reservation.enrollment_id
                WHERE attempt.id = ?1
                  AND NOT EXISTS (
                    SELECT 1
                    FROM tally_write_fixture_revocations AS revocation
                    WHERE revocation.enrollment_id = enrollment.id
                  )
            "#,
        )
        .bind(&input.attempt_id)
        .fetch_optional(&mut *transaction)
        .await?;
        let started_at_unix_ms =
            started_at_unix_ms.ok_or(MirrorError::InvalidInput("canary_preflight_not_active"))?;
        if input.verified_at_unix_ms < started_at_unix_ms {
            return Err(MirrorError::InvalidInput(
                "canary_preflight_evidence_before_attempt",
            ));
        }
        sqlx::query(
            "INSERT INTO tally_write_canary_preflight_evidence( \
               id, attempt_id, readback_state_sha256, identity_coverage_sha256, \
               canonical_endpoint_sha256, company_identity_sha256, contract_version, verified_at_unix_ms \
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?7) ON CONFLICT(attempt_id) DO NOTHING",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(&input.attempt_id)
        .bind(&input.readback_state_sha256)
        .bind(&input.identity_coverage_sha256)
        .bind(&input.canonical_endpoint_sha256)
        .bind(&input.company_identity_sha256)
        .bind(input.verified_at_unix_ms)
        .execute(&mut *transaction)
        .await?;
        let evidence = sqlx::query(
            "SELECT id, readback_state_sha256, identity_coverage_sha256, canonical_endpoint_sha256, \
             company_identity_sha256, verified_at_unix_ms \
             FROM tally_write_canary_preflight_evidence WHERE attempt_id = ?1",
        )
        .bind(&input.attempt_id)
        .fetch_one(&mut *transaction)
        .await?;
        let existing_state_sha256: String = evidence.try_get("readback_state_sha256")?;
        let existing_coverage_sha256: String = evidence.try_get("identity_coverage_sha256")?;
        let existing_endpoint_sha256: String = evidence.try_get("canonical_endpoint_sha256")?;
        let existing_company_sha256: String = evidence.try_get("company_identity_sha256")?;
        if existing_state_sha256 != input.readback_state_sha256
            || existing_coverage_sha256 != input.identity_coverage_sha256
            || existing_endpoint_sha256 != input.canonical_endpoint_sha256
            || existing_company_sha256 != input.company_identity_sha256
        {
            return Err(MirrorError::InvalidInput(
                "canary_preflight_evidence_already_recorded",
            ));
        }
        let result = WriteCanaryPreflightEvidenceRef {
            id: evidence.try_get("id")?,
            attempt_id: input.attempt_id,
            verified_at_unix_ms: evidence.try_get("verified_at_unix_ms")?,
        };
        transaction.commit().await?;
        Ok(result)
    }

    /// Verifies that immutable evidence belongs to the exact active fixture
    /// binding and one-time preflight attempt. This does not create a token or
    /// grant dispatch authority; a later, separately reviewed coordinator
    /// must still perform its own final check immediately before any import.
    pub async fn active_write_canary_preflight_evidence(
        &self,
        input: ActiveWriteCanaryPreflightEvidenceInput,
    ) -> Result<WriteCanaryPreflightEvidenceRef, MirrorError> {
        let binding = input.binding;
        validate_nonempty(&binding.company_id, 128, "fixture_company_id")?;
        validate_sha256(&binding.review_commitment_sha256)?;
        validate_nonempty(&binding.reservation_id, 128, "canary_reservation_id")?;
        validate_sha256(&binding.reservation_payload_sha256)?;
        validate_sha256(&binding.wire_sha256)?;
        validate_sha256(&binding.intended_state_sha256)?;
        validate_sha256(&binding.identity_query_sha256)?;
        validate_nonempty(&input.attempt_id, 128, "canary_preflight_attempt_id")?;
        validate_nonempty(&input.evidence_id, 128, "canary_preflight_evidence_id")?;
        validate_sha256(&input.readback_state_sha256)?;
        validate_sha256(&input.identity_coverage_sha256)?;
        validate_sha256(&input.canonical_endpoint_sha256)?;
        validate_sha256(&input.company_identity_sha256)?;

        let evidence = sqlx::query(
            r#"
                SELECT evidence.id, evidence.attempt_id, evidence.verified_at_unix_ms
                FROM tally_write_canary_preflight_evidence AS evidence
                JOIN tally_write_canary_preflight_attempts AS attempt
                  ON attempt.id = evidence.attempt_id
                JOIN tally_write_canary_payload_bindings AS binding
                  ON binding.id = attempt.payload_binding_id
                JOIN tally_write_canary_reservations AS reservation
                  ON reservation.id = binding.reservation_id
                JOIN tally_write_fixture_enrollments AS enrollment
                  ON enrollment.id = reservation.enrollment_id
                WHERE evidence.id = ?1
                  AND evidence.attempt_id = ?2
                  AND evidence.readback_state_sha256 = ?3
                  AND evidence.identity_coverage_sha256 = ?4
                  AND evidence.canonical_endpoint_sha256 = ?5
                  AND evidence.company_identity_sha256 = ?6
                  AND binding.reservation_id = ?7
                  AND enrollment.company_id = ?8
                  AND enrollment.review_commitment_sha256 = ?9
                  AND reservation.reservation_payload_sha256 = ?10
                  AND binding.wire_sha256 = ?11
                  AND binding.intended_state_sha256 = ?12
                  AND binding.identity_query_sha256 = ?13
                  AND NOT EXISTS (
                    SELECT 1
                    FROM tally_write_fixture_revocations AS revocation
                    WHERE revocation.enrollment_id = enrollment.id
                  )
            "#,
        )
        .bind(&input.evidence_id)
        .bind(&input.attempt_id)
        .bind(&input.readback_state_sha256)
        .bind(&input.identity_coverage_sha256)
        .bind(&input.canonical_endpoint_sha256)
        .bind(&input.company_identity_sha256)
        .bind(&binding.reservation_id)
        .bind(&binding.company_id)
        .bind(&binding.review_commitment_sha256)
        .bind(&binding.reservation_payload_sha256)
        .bind(&binding.wire_sha256)
        .bind(&binding.intended_state_sha256)
        .bind(&binding.identity_query_sha256)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(MirrorError::InvalidInput(
            "canary_preflight_evidence_not_active",
        ))?;
        Ok(WriteCanaryPreflightEvidenceRef {
            id: evidence.try_get("id")?,
            attempt_id: evidence.try_get("attempt_id")?,
            verified_at_unix_ms: evidence.try_get("verified_at_unix_ms")?,
        })
    }

    /// Atomically consumes the sole no-send dispatch claim after rechecking
    /// exact immutable preflight evidence and active enrollment. It creates no
    /// transport capability and a replay fails closed rather than retrying.
    pub async fn begin_write_canary_dispatch_attempt(
        &self,
        input: BeginWriteCanaryDispatchInput,
    ) -> Result<WriteCanaryDispatchAttemptRef, MirrorError> {
        if input.claimed_at_unix_ms <= 0 {
            return Err(MirrorError::InvalidInput("canary_dispatch_claimed_at"));
        }
        let evidence = self
            .active_write_canary_preflight_evidence(input.evidence)
            .await?;
        let mut transaction = self.pool.begin().await?;
        sqlx::query("UPDATE tally_schema_migrations SET version = version WHERE version = 4")
            .execute(&mut *transaction)
            .await?;
        if input.claimed_at_unix_ms < evidence.verified_at_unix_ms {
            return Err(MirrorError::InvalidInput(
                "canary_dispatch_claim_before_evidence",
            ));
        }
        let attempt_id = Uuid::new_v4().to_string();
        let inserted = sqlx::query(
            "INSERT INTO tally_write_canary_dispatch_attempts( \
               id, evidence_id, contract_version, claimed_at_unix_ms \
             ) VALUES (?1, ?2, 1, ?3) ON CONFLICT(evidence_id) DO NOTHING",
        )
        .bind(&attempt_id)
        .bind(&evidence.id)
        .bind(input.claimed_at_unix_ms)
        .execute(&mut *transaction)
        .await?
        .rows_affected();
        if inserted != 1 {
            return Err(MirrorError::InvalidInput(
                "canary_dispatch_attempt_already_claimed",
            ));
        }
        transaction.commit().await?;
        Ok(WriteCanaryDispatchAttemptRef {
            id: attempt_id,
            evidence_id: evidence.id,
            claimed_at_unix_ms: input.claimed_at_unix_ms,
        })
    }

    /// Rechecks that one immutable no-send dispatch claim is tied to the exact
    /// active fixture, payload commitments, and preflight evidence. It is a
    /// local read only; no Tally transport can be reached through this method.
    pub async fn active_write_canary_dispatch_attempt(
        &self,
        input: ActiveWriteCanaryDispatchAttemptInput,
    ) -> Result<WriteCanaryDispatchAttemptRef, MirrorError> {
        validate_nonempty(
            &input.dispatch_attempt_id,
            128,
            "canary_dispatch_attempt_id",
        )?;
        if input.claimed_at_unix_ms <= 0 {
            return Err(MirrorError::InvalidInput("canary_dispatch_claimed_at"));
        }
        let evidence = self
            .active_write_canary_preflight_evidence(input.evidence)
            .await?;
        let dispatch = sqlx::query(
            "SELECT id, evidence_id, claimed_at_unix_ms \
             FROM tally_write_canary_dispatch_attempts \
             WHERE id = ?1 AND evidence_id = ?2 AND claimed_at_unix_ms = ?3",
        )
        .bind(&input.dispatch_attempt_id)
        .bind(&evidence.id)
        .bind(input.claimed_at_unix_ms)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(MirrorError::InvalidInput(
            "canary_dispatch_attempt_not_active",
        ))?;
        Ok(WriteCanaryDispatchAttemptRef {
            id: dispatch.try_get("id")?,
            evidence_id: dispatch.try_get("evidence_id")?,
            claimed_at_unix_ms: dispatch.try_get("claimed_at_unix_ms")?,
        })
    }

    /// Persists exactly one immutable, digest-only final verdict for an active
    /// dispatch claim. Exact replays are safe; any changed observation fails
    /// closed. This is not a transport or import API.
    pub async fn record_write_canary_final_verdict(
        &self,
        input: WriteCanaryFinalVerdictInput,
    ) -> Result<WriteCanaryFinalVerdictRef, MirrorError> {
        validate_sha256(&input.import_response_sha256)?;
        validate_sha256(&input.readback_state_sha256)?;
        validate_sha256(&input.identity_coverage_sha256)?;
        if input.recorded_at_unix_ms <= 0 {
            return Err(MirrorError::InvalidInput(
                "canary_final_verdict_recorded_at",
            ));
        }
        let dispatch = self
            .active_write_canary_dispatch_attempt(input.dispatch)
            .await?;
        if input.recorded_at_unix_ms < dispatch.claimed_at_unix_ms {
            return Err(MirrorError::InvalidInput(
                "canary_final_verdict_before_dispatch_claim",
            ));
        }

        let mut transaction = self.pool.begin().await?;
        sqlx::query("UPDATE tally_schema_migrations SET version = version WHERE version = 4")
            .execute(&mut *transaction)
            .await?;
        sqlx::query(
            "INSERT INTO tally_write_canary_final_verdicts( \
               id, dispatch_attempt_id, import_response_sha256, \
               readback_state_sha256, identity_coverage_sha256, contract_version, \
               recorded_at_unix_ms \
             ) VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6) \
             ON CONFLICT(dispatch_attempt_id) DO NOTHING",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(&dispatch.id)
        .bind(&input.import_response_sha256)
        .bind(&input.readback_state_sha256)
        .bind(&input.identity_coverage_sha256)
        .bind(input.recorded_at_unix_ms)
        .execute(&mut *transaction)
        .await?;
        let verdict = sqlx::query(
            "SELECT id, import_response_sha256, readback_state_sha256, \
             identity_coverage_sha256, recorded_at_unix_ms \
             FROM tally_write_canary_final_verdicts WHERE dispatch_attempt_id = ?1",
        )
        .bind(&dispatch.id)
        .fetch_one(&mut *transaction)
        .await?;
        let existing_import_response_sha256: String = verdict.try_get("import_response_sha256")?;
        let existing_readback_state_sha256: String = verdict.try_get("readback_state_sha256")?;
        let existing_identity_coverage_sha256: String =
            verdict.try_get("identity_coverage_sha256")?;
        if existing_import_response_sha256 != input.import_response_sha256
            || existing_readback_state_sha256 != input.readback_state_sha256
            || existing_identity_coverage_sha256 != input.identity_coverage_sha256
        {
            return Err(MirrorError::InvalidInput(
                "canary_final_verdict_already_recorded",
            ));
        }
        let result = WriteCanaryFinalVerdictRef {
            id: verdict.try_get("id")?,
            dispatch_attempt_id: dispatch.id,
            recorded_at_unix_ms: verdict.try_get("recorded_at_unix_ms")?,
        };
        transaction.commit().await?;
        Ok(result)
    }

    pub async fn write_fixture_enrollment_status(
        &self,
        company_id: &str,
    ) -> Result<WriteFixtureEnrollmentStatus, MirrorError> {
        validate_nonempty(company_id, 128, "fixture_company_id")?;
        let row = sqlx::query(
            "SELECT enrollment.enrolled_at_unix_ms, revocation.revoked_at_unix_ms \
             FROM tally_write_fixture_enrollments AS enrollment \
             LEFT JOIN tally_write_fixture_revocations AS revocation \
               ON revocation.enrollment_id = enrollment.id \
             WHERE enrollment.company_id = ?1 \
             ORDER BY (revocation.enrollment_id IS NULL) DESC, \
                      revocation.event_sequence DESC, \
                      enrollment.enrolled_at_unix_ms DESC, enrollment.id DESC LIMIT 1",
        )
        .bind(company_id)
        .fetch_optional(&self.pool)
        .await?;
        match row {
            None => Ok(WriteFixtureEnrollmentStatus {
                fixture_state: "not_enrolled",
                enrolled_at_unix_ms: None,
                revoked_at_unix_ms: None,
                candidate_gate: "not_enrolled",
                write_capability: "unknown",
            }),
            Some(row) => {
                let revoked_at_unix_ms: Option<i64> = row.try_get("revoked_at_unix_ms")?;
                Ok(WriteFixtureEnrollmentStatus {
                    fixture_state: if revoked_at_unix_ms.is_some() {
                        "revoked"
                    } else {
                        "active"
                    },
                    enrolled_at_unix_ms: Some(row.try_get("enrolled_at_unix_ms")?),
                    revoked_at_unix_ms,
                    candidate_gate: if revoked_at_unix_ms.is_some() {
                        "not_enrolled"
                    } else {
                        "enrolled"
                    },
                    write_capability: "unknown",
                })
            }
        }
    }

    pub async fn revoke_write_fixture_enrollment(
        &self,
        company_id: &str,
        revoked_at_unix_ms: i64,
    ) -> Result<WriteFixtureEnrollmentStatus, MirrorError> {
        validate_nonempty(company_id, 128, "fixture_company_id")?;
        if revoked_at_unix_ms <= 0 {
            return Err(MirrorError::InvalidInput("fixture_revoked_at"));
        }
        let mut transaction = self.pool.begin().await?;
        let row = sqlx::query(
            "SELECT enrollment.id, enrollment.enrollment_payload_sha256, \
                    enrollment.enrolled_at_unix_ms, revocation.revoked_at_unix_ms \
             FROM tally_write_fixture_enrollments AS enrollment \
             LEFT JOIN tally_write_fixture_revocations AS revocation \
               ON revocation.enrollment_id = enrollment.id \
             WHERE enrollment.company_id = ?1 \
             ORDER BY (revocation.enrollment_id IS NULL) DESC, \
                      revocation.event_sequence DESC, \
                      enrollment.enrolled_at_unix_ms DESC, enrollment.id DESC LIMIT 1",
        )
        .bind(company_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(MirrorError::NotFound)?;
        let enrolled_at_unix_ms: i64 = row.try_get("enrolled_at_unix_ms")?;
        let existing_revocation: Option<i64> = row.try_get("revoked_at_unix_ms")?;
        let status = if existing_revocation.is_none() {
            let enrollment_id: String = row.try_get("id")?;
            let enrollment_payload_sha256: String = row.try_get("enrollment_payload_sha256")?;
            let revocation_payload_sha256 = fixture_revocation_payload_sha256(
                &enrollment_id,
                &enrollment_payload_sha256,
                "operator_revoked",
                revoked_at_unix_ms,
            )?;
            sqlx::query(
                "INSERT INTO tally_write_fixture_revocations(\
                   event_sequence, id, enrollment_id, revocation_payload_sha256, safe_reason_code, revoked_at_unix_ms\
                 ) VALUES ((SELECT COALESCE(MAX(event_sequence), 0) + 1 FROM tally_write_fixture_revocations), \
                           ?1, ?2, ?3, 'operator_revoked', ?4)",
            )
            .bind(Uuid::new_v4().to_string())
            .bind(enrollment_id)
            .bind(revocation_payload_sha256)
            .bind(revoked_at_unix_ms)
            .execute(&mut *transaction)
            .await?;
            WriteFixtureEnrollmentStatus {
                fixture_state: "revoked",
                enrolled_at_unix_ms: Some(enrolled_at_unix_ms),
                revoked_at_unix_ms: Some(revoked_at_unix_ms),
                candidate_gate: "not_enrolled",
                write_capability: "unknown",
            }
        } else {
            WriteFixtureEnrollmentStatus {
                fixture_state: "revoked",
                enrolled_at_unix_ms: Some(enrolled_at_unix_ms),
                revoked_at_unix_ms: existing_revocation,
                candidate_gate: "not_enrolled",
                write_capability: "unknown",
            }
        };
        transaction.commit().await?;
        Ok(status)
    }

    /// Validates the encrypted capability receipt used by Core Accounting restart recovery.
    ///
    /// This is deliberately Core-specific. Other packs keep their own `Supported + Observed`
    /// authorization semantics, while Core resumes only from the exact sealed-profile execution
    /// receipt accepted by a fresh start.
    pub async fn core_snapshot_resume_evidence_matches_plan(
        &self,
        snapshot_id: &str,
        company_id: &str,
        profile_version: u16,
        product: &str,
        release: Option<&str>,
        mode: Option<&str>,
    ) -> Result<bool, MirrorError> {
        validate_nonempty(snapshot_id, 128, "capability_snapshot_id")?;
        validate_nonempty(company_id, 128, "company_id")?;
        if profile_version == 0 {
            return Err(MirrorError::InvalidInput("profile_version"));
        }
        validate_nonempty(product, 128, "product")?;
        validate_optional_text(release, 128, "release")?;
        validate_optional_text(mode, 64, "mode")?;
        let evidence = sqlx::query_as::<_, (String, String, Option<String>)>(
            "SELECT pack.capability_state, pack.confidence, pack.safe_reason_code \
             FROM tally_capability_snapshots AS snapshot \
             JOIN tally_companies AS company ON company.endpoint_id = snapshot.endpoint_id \
             JOIN tally_capability_items AS pack ON pack.snapshot_id = snapshot.id \
             WHERE snapshot.id = ?1 AND company.id = ?2 \
               AND snapshot.profile_version = ?3 AND snapshot.product = ?4 \
               AND snapshot.release IS ?5 AND snapshot.mode IS ?6 \
               AND EXISTS (SELECT 1 FROM tally_capability_items AS transport \
                 WHERE transport.snapshot_id = snapshot.id \
                   AND transport.capability_kind = 'transport' \
                   AND transport.capability_key = 'xml_http' \
                   AND transport.capability_state = 'supported' \
                   AND transport.confidence = 'observed') \
               AND pack.capability_kind = 'pack' \
               AND pack.capability_key = 'core_accounting'",
        )
        .bind(snapshot_id)
        .bind(company_id)
        .bind(i64::from(profile_version))
        .bind(product)
        .bind(release)
        .bind(mode)
        .fetch_optional(&self.pool)
        .await?;
        Ok(
            evidence.is_some_and(|(state, confidence, safe_reason_code)| {
                core_snapshot_start_authorized_codes(
                    &state,
                    &confidence,
                    safe_reason_code.as_deref(),
                )
            }),
        )
    }

    /// Retains the ordinary observed-support contract used by non-snapshot capability flows.
    pub async fn capability_snapshot_matches_plan(
        &self,
        snapshot_id: &str,
        company_id: &str,
        profile_version: u16,
        product: &str,
        release: Option<&str>,
        mode: Option<&str>,
    ) -> Result<bool, MirrorError> {
        validate_nonempty(snapshot_id, 128, "capability_snapshot_id")?;
        validate_nonempty(company_id, 128, "company_id")?;
        if profile_version == 0 {
            return Err(MirrorError::InvalidInput("profile_version"));
        }
        validate_nonempty(product, 128, "product")?;
        validate_optional_text(release, 128, "release")?;
        validate_optional_text(mode, 64, "mode")?;
        let matches = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_capability_snapshots AS snapshot \
             JOIN tally_companies AS company ON company.endpoint_id = snapshot.endpoint_id \
             WHERE snapshot.id = ?1 AND company.id = ?2 \
               AND snapshot.profile_version = ?3 AND snapshot.product = ?4 \
               AND snapshot.release IS ?5 AND snapshot.mode IS ?6 \
               AND EXISTS (SELECT 1 FROM tally_capability_items AS transport \
                 WHERE transport.snapshot_id = snapshot.id \
                   AND transport.capability_kind = 'transport' \
                   AND transport.capability_key = 'xml_http' \
                   AND transport.capability_state = 'supported' \
                   AND transport.confidence = 'observed') \
               AND EXISTS (SELECT 1 FROM tally_capability_items AS pack \
                 WHERE pack.snapshot_id = snapshot.id \
                   AND pack.capability_kind = 'pack' \
                   AND pack.capability_key = 'core_accounting' \
                   AND pack.capability_state = 'supported' \
                   AND pack.confidence = 'observed')",
        )
        .bind(snapshot_id)
        .bind(company_id)
        .bind(i64::from(profile_version))
        .bind(product)
        .bind(release)
        .bind(mode)
        .fetch_one(&self.pool)
        .await?;
        Ok(matches == 1)
    }

    pub async fn migrate(&self) -> Result<(), MirrorError> {
        let mut transaction = self.pool.begin().await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS tally_schema_migrations (\
             version INTEGER PRIMARY KEY, description TEXT NOT NULL, applied_at_unix_ms INTEGER NOT NULL)",
        )
        .execute(&mut *transaction)
        .await?;
        let mirror_installed = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_schema_migrations WHERE version = 2",
        )
        .fetch_one(&mut *transaction)
        .await?;
        if mirror_installed == 0 {
            sqlx::raw_sql(MIRROR_MIGRATION_V2)
                .execute(&mut *transaction)
                .await?;
        }
        let safe_writes_installed = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_schema_migrations WHERE version = 3",
        )
        .fetch_one(&mut *transaction)
        .await?;
        if safe_writes_installed == 0 {
            sqlx::raw_sql(MIRROR_MIGRATION_V3)
                .execute(&mut *transaction)
                .await?;
        }
        let snapshot_state_installed = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_schema_migrations WHERE version = 4",
        )
        .fetch_one(&mut *transaction)
        .await?;
        if snapshot_state_installed == 0 {
            sqlx::raw_sql(MIRROR_MIGRATION_V4)
                .execute(&mut *transaction)
                .await?;
        }
        let recovery_installed = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_schema_migrations WHERE version = 5",
        )
        .fetch_one(&mut *transaction)
        .await?;
        if recovery_installed == 0 {
            let duplicate_snapshot_runs = sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM (SELECT run_id FROM tally_snapshot_run_states \
                 GROUP BY run_id HAVING COUNT(*) > 1)",
            )
            .fetch_one(&mut *transaction)
            .await?;
            if duplicate_snapshot_runs != 0 {
                return Err(MirrorError::InvalidInput("snapshot_state_duplicate_run_id"));
            }
            sqlx::raw_sql(MIRROR_MIGRATION_V5)
                .execute(&mut *transaction)
                .await?;
        }
        let incremental_installed = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_schema_migrations WHERE version = 6",
        )
        .fetch_one(&mut *transaction)
        .await?;
        if incremental_installed == 0 {
            sqlx::raw_sql(MIRROR_MIGRATION_V6)
                .execute(&mut *transaction)
                .await?;
        }
        let selected_read_evidence_installed = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_schema_migrations WHERE version = 7",
        )
        .fetch_one(&mut *transaction)
        .await?;
        let composite_company_identity_installed = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_schema_migrations WHERE version = 23",
        )
        .fetch_one(&mut *transaction)
        .await?;
        if selected_read_evidence_installed == 0 && composite_company_identity_installed == 0 {
            let casefold_collisions = sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM (\
                   SELECT endpoint_id, company_guid COLLATE NOCASE \
                   FROM tally_companies WHERE company_guid IS NOT NULL \
                   GROUP BY endpoint_id, company_guid COLLATE NOCASE HAVING COUNT(*) > 1\
                 )",
            )
            .fetch_one(&mut *transaction)
            .await?;
            if casefold_collisions != 0 {
                return Err(MirrorError::InvalidInput("company_guid_casefold_collision"));
            }
            sqlx::raw_sql(MIRROR_MIGRATION_V7)
                .execute(&mut *transaction)
                .await?;
        }
        let reviewed_setup_consumption_installed = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_schema_migrations WHERE version = 8",
        )
        .fetch_one(&mut *transaction)
        .await?;
        if reviewed_setup_consumption_installed == 0 {
            sqlx::raw_sql(MIRROR_MIGRATION_V8)
                .execute(&mut *transaction)
                .await?;
        }
        let window_staging_installed = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_schema_migrations WHERE version = 9",
        )
        .fetch_one(&mut *transaction)
        .await?;
        if window_staging_installed == 0 {
            sqlx::raw_sql(MIRROR_MIGRATION_V9)
                .execute(&mut *transaction)
                .await?;
        }
        let provenance_counts_installed = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_schema_migrations WHERE version = 10",
        )
        .fetch_one(&mut *transaction)
        .await?;
        if provenance_counts_installed == 0 {
            sqlx::raw_sql(MIRROR_MIGRATION_V10)
                .execute(&mut *transaction)
                .await?;
        }
        let proof_record_counts_installed = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_schema_migrations WHERE version = 11",
        )
        .fetch_one(&mut *transaction)
        .await?;
        if proof_record_counts_installed == 0 {
            sqlx::raw_sql(MIRROR_MIGRATION_V11)
                .execute(&mut *transaction)
                .await?;
        }
        let window_abandonment_evidence_installed = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_schema_migrations WHERE version = 12",
        )
        .fetch_one(&mut *transaction)
        .await?;
        if window_abandonment_evidence_installed == 0 {
            sqlx::raw_sql(MIRROR_MIGRATION_V12)
                .execute(&mut *transaction)
                .await?;
        }
        let write_fixture_enrollment_installed = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_schema_migrations WHERE version = 13",
        )
        .fetch_one(&mut *transaction)
        .await?;
        if write_fixture_enrollment_installed == 0 {
            sqlx::raw_sql(MIRROR_MIGRATION_V13)
                .execute(&mut *transaction)
                .await?;
        }
        let write_fixture_revocation_sequence_installed = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_schema_migrations WHERE version = 14",
        )
        .fetch_one(&mut *transaction)
        .await?;
        if write_fixture_revocation_sequence_installed == 0 {
            let event_sequence_exists = sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM pragma_table_info('tally_write_fixture_revocations') \
                 WHERE name = 'event_sequence'",
            )
            .fetch_one(&mut *transaction)
            .await?
                != 0;
            sqlx::raw_sql(if event_sequence_exists {
                MIRROR_MIGRATION_V14_ALREADY_SEQUENCED
            } else {
                MIRROR_MIGRATION_V14
            })
            .execute(&mut *transaction)
            .await?;
        }
        let write_canary_reservation_installed = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_schema_migrations WHERE version = 15",
        )
        .fetch_one(&mut *transaction)
        .await?;
        if write_canary_reservation_installed == 0 {
            sqlx::raw_sql(MIRROR_MIGRATION_V15)
                .execute(&mut *transaction)
                .await?;
        }
        let write_canary_payload_binding_installed = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_schema_migrations WHERE version = 16",
        )
        .fetch_one(&mut *transaction)
        .await?;
        if write_canary_payload_binding_installed == 0 {
            sqlx::raw_sql(MIRROR_MIGRATION_V16)
                .execute(&mut *transaction)
                .await?;
        }
        let write_canary_preflight_attempt_installed = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_schema_migrations WHERE version = 17",
        )
        .fetch_one(&mut *transaction)
        .await?;
        if write_canary_preflight_attempt_installed == 0 {
            sqlx::raw_sql(MIRROR_MIGRATION_V17)
                .execute(&mut *transaction)
                .await?;
        }
        let write_canary_preflight_evidence_installed = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_schema_migrations WHERE version = 18",
        )
        .fetch_one(&mut *transaction)
        .await?;
        if write_canary_preflight_evidence_installed == 0 {
            sqlx::raw_sql(MIRROR_MIGRATION_V18)
                .execute(&mut *transaction)
                .await?;
        }
        let write_canary_dispatch_attempt_installed = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_schema_migrations WHERE version = 19",
        )
        .fetch_one(&mut *transaction)
        .await?;
        if write_canary_dispatch_attempt_installed == 0 {
            sqlx::raw_sql(MIRROR_MIGRATION_V19)
                .execute(&mut *transaction)
                .await?;
        }
        let write_canary_final_verdict_installed = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_schema_migrations WHERE version = 20",
        )
        .fetch_one(&mut *transaction)
        .await?;
        if write_canary_final_verdict_installed == 0 {
            sqlx::raw_sql(MIRROR_MIGRATION_V20)
                .execute(&mut *transaction)
                .await?;
        }
        let write_canary_preflight_target_binding_installed = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_schema_migrations WHERE version = 21",
        )
        .fetch_one(&mut *transaction)
        .await?;
        if write_canary_preflight_target_binding_installed == 0 {
            sqlx::raw_sql(MIRROR_MIGRATION_V21)
                .execute(&mut *transaction)
                .await?;
        }
        let bomless_utf16_read_evidence_installed = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_schema_migrations WHERE version = 22",
        )
        .fetch_one(&mut *transaction)
        .await?;
        if bomless_utf16_read_evidence_installed == 0 {
            let migration_result = sqlx::raw_sql(MIRROR_MIGRATION_V22)
                .execute(&mut *transaction)
                .await;
            // The migration enables legacy ALTER TABLE only to avoid reparsing an
            // unrelated historical trigger whose deferred column reference SQLite
            // otherwise rejects. Restore the connection setting even when the
            // transactional migration fails before its own reset statement.
            let reset_result = sqlx::query("PRAGMA legacy_alter_table = OFF")
                .execute(&mut *transaction)
                .await;
            migration_result?;
            reset_result?;
        }
        let composite_company_identity_installed = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_schema_migrations WHERE version = 23",
        )
        .fetch_one(&mut *transaction)
        .await?;
        if composite_company_identity_installed == 0 {
            // SQLite otherwise reparses unrelated historical triggers while
            // 0023 rebuilds the revocation table and rejects a deferred
            // column reference outside this migration's scope.
            sqlx::query("PRAGMA legacy_alter_table = ON")
                .execute(&mut *transaction)
                .await?;
            let migration_result = sqlx::raw_sql(MIRROR_MIGRATION_V23)
                .execute(&mut *transaction)
                .await;
            let reset_result = sqlx::query("PRAGMA legacy_alter_table = OFF")
                .execute(&mut *transaction)
                .await;
            migration_result?;
            reset_result?;
            retire_precomposite_fixture_enrollments(
                &mut transaction,
                Utc::now().timestamp_millis(),
            )
            .await?;
            sqlx::query(
                "INSERT INTO tally_schema_migrations(version, description, applied_at_unix_ms) \
                 VALUES (23, 'Tally composite company identity requires re-verification of GUID-only pins', ?1)",
            )
            .bind(Utc::now().timestamp_millis())
            .execute(&mut *transaction)
            .await?;
        }
        let selected_read_scope_v2_installed = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_schema_migrations WHERE version = 24",
        )
        .fetch_one(&mut *transaction)
        .await?;
        let observed_company_identity_constraint_installed = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_schema_migrations WHERE version = 25",
        )
        .fetch_one(&mut *transaction)
        .await?;
        if selected_read_scope_v2_installed == 0
            && observed_company_identity_constraint_installed == 0
        {
            sqlx::query("PRAGMA legacy_alter_table = ON")
                .execute(&mut *transaction)
                .await?;
            let migration_result = sqlx::raw_sql(MIRROR_MIGRATION_V24)
                .execute(&mut *transaction)
                .await;
            let reset_result = sqlx::query("PRAGMA legacy_alter_table = OFF")
                .execute(&mut *transaction)
                .await;
            migration_result?;
            reset_result?;
            validate_legacy_v1_selected_read_scope_layouts(&mut transaction).await?;
        }
        if observed_company_identity_constraint_installed == 0 {
            let demoted_fixture_enrollments =
                fixture_enrollments_for_v25_observed_identity_demotions(&mut transaction).await?;
            sqlx::raw_sql(MIRROR_MIGRATION_V25)
                .execute(&mut *transaction)
                .await?;
            write_fixture_identity_reverification_revocations(
                &mut transaction,
                demoted_fixture_enrollments,
                Utc::now().timestamp_millis(),
            )
            .await?;
        }
        let license_tier_installed = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_schema_migrations WHERE version = 26",
        )
        .fetch_one(&mut *transaction)
        .await?;
        if license_tier_installed == 0 {
            sqlx::raw_sql(MIRROR_MIGRATION_V26)
                .execute(&mut *transaction)
                .await?;
        }
        let resurrected_guid_index_retired = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_schema_migrations WHERE version = 27",
        )
        .fetch_one(&mut *transaction)
        .await?;
        if resurrected_guid_index_retired == 0 {
            sqlx::raw_sql(MIRROR_MIGRATION_V27)
                .execute(&mut *transaction)
                .await?;
        } else {
            // A downgraded binary can rerun legacy bootstrap V2 after V27 was
            // recorded. Keep the retired-index invariant on every fixed open.
            sqlx::query("DROP INDEX IF EXISTS uq_tally_companies_guid")
                .execute(&mut *transaction)
                .await?;
        }
        let grouping_events_installed = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_schema_migrations WHERE version = 28",
        )
        .fetch_one(&mut *transaction)
        .await?;
        if grouping_events_installed == 0 {
            sqlx::raw_sql(MIRROR_MIGRATION_V28)
                .execute(&mut *transaction)
                .await?;
        }
        sqlx::query(
            "UPDATE tally_schema_migrations SET applied_at_unix_ms = ?1 \
             WHERE version IN (2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28) AND applied_at_unix_ms = 0",
        )
        .bind(Utc::now().timestamp_millis())
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn save_capability_snapshot(
        &self,
        input: CapabilitySnapshotInput,
    ) -> Result<CapabilitySnapshotRef, MirrorError> {
        validate_capability_snapshot(&input)?;
        let mut transaction = self.pool.begin().await?;
        let snapshot = Self::insert_capability_snapshot(&mut transaction, input).await?;
        transaction.commit().await?;
        Ok(snapshot)
    }

    async fn insert_capability_snapshot(
        transaction: &mut Transaction<'_, Sqlite>,
        input: CapabilitySnapshotInput,
    ) -> Result<CapabilitySnapshotRef, MirrorError> {
        let endpoint_id = match sqlx::query_scalar::<_, String>(
            "SELECT id FROM tally_endpoints WHERE canonical_origin = ?1",
        )
        .bind(&input.canonical_origin)
        .fetch_optional(&mut **transaction)
        .await?
        {
            Some(id) => {
                sqlx::query(
                    "UPDATE tally_endpoints SET last_observed_at_unix_ms = ?1 WHERE id = ?2",
                )
                .bind(input.observed_at_unix_ms)
                .bind(&id)
                .execute(&mut **transaction)
                .await?;
                id
            }
            None => {
                let id = Uuid::new_v4().to_string();
                sqlx::query(
                    "INSERT INTO tally_endpoints(\
                       id, canonical_origin, created_at_unix_ms, last_observed_at_unix_ms\
                     ) VALUES (?1, ?2, ?3, ?3)",
                )
                .bind(&id)
                .bind(&input.canonical_origin)
                .bind(input.observed_at_unix_ms)
                .execute(&mut **transaction)
                .await?;
                id
            }
        };

        let snapshot_id = Uuid::new_v4().to_string();
        let mut insert = QueryBuilder::<Sqlite>::new(
            "INSERT INTO tally_capability_snapshots(\
               id, endpoint_id, observed_at_unix_ms, profile_version, product, release, mode, \
               mode_confidence",
        );
        // Absent observations retain the historical column set and SQL NULL default.
        if input.license_tier.is_some() {
            insert.push(", license_tier");
        }
        insert.push(") VALUES (");
        let mut values = insert.separated(", ");
        values
            .push_bind(&snapshot_id)
            .push_bind(&endpoint_id)
            .push_bind(input.observed_at_unix_ms)
            .push_bind(i64::from(input.profile_version))
            .push_bind(input.product)
            .push_bind(input.release)
            .push_bind(input.mode)
            .push_bind(input.mode_confidence.as_str());
        if let Some(tier) = input.license_tier {
            values.push_bind(license_tier_key(tier));
        }
        insert.push(")").build().execute(&mut **transaction).await?;

        for item in input.items {
            sqlx::query(
                "INSERT INTO tally_capability_items(\
                   snapshot_id, capability_kind, capability_key, capability_state, confidence, \
                   safe_reason_code\
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )
            .bind(&snapshot_id)
            .bind(item.kind.as_str())
            .bind(item.key)
            .bind(item.state.as_str())
            .bind(item.confidence.as_str())
            .bind(item.safe_reason_code)
            .execute(&mut **transaction)
            .await?;
        }
        Ok(CapabilitySnapshotRef {
            id: snapshot_id,
            endpoint_id,
        })
    }

    pub async fn upsert_company(&self, input: CompanyInput) -> Result<CompanyRef, MirrorError> {
        validate_company_input(&input)?;
        let mut transaction = self.pool.begin().await?;
        let company = Self::upsert_company_in_transaction(&mut transaction, input).await?;
        transaction.commit().await?;
        Ok(company)
    }

    async fn upsert_company_in_transaction(
        transaction: &mut Transaction<'_, Sqlite>,
        input: CompanyInput,
    ) -> Result<CompanyRef, MirrorError> {
        let matches = find_identity_matches(
            transaction,
            "tally_companies",
            "endpoint_id",
            &input.endpoint_id,
            None,
            &input.identity,
        )
        .await?;

        let id = match unique_match(matches)? {
            Some(existing) => {
                ensure_no_silent_identity_change(&existing, &input.identity)?;
                let incoming_confidence = generic_company_confidence(&input.identity).as_str();
                let observed_row_refreshed = sqlx::query(
                    "UPDATE tally_companies SET last_observed_at_unix_ms = ?1 \
                     WHERE id = ?2 AND identity_confidence = 'observed'",
                )
                .bind(input.observed_at_unix_ms)
                .bind(&existing.id)
                .execute(&mut **transaction)
                .await?;
                if observed_row_refreshed.rows_affected() == 0 {
                    sqlx::query(
                        "UPDATE tally_companies SET display_name = ?1, last_observed_at_unix_ms = ?2, \
                         identity_confidence = CASE \
                           WHEN ?3 = 'documented' THEN 'documented' \
                           WHEN ?3 = 'inferred' AND identity_confidence = 'unknown' THEN 'inferred' \
                           ELSE identity_confidence END \
                         WHERE id = ?4 AND identity_confidence <> 'observed'",
                    )
                    .bind(&input.display_name)
                    .bind(input.observed_at_unix_ms)
                    .bind(incoming_confidence)
                    .bind(&existing.id)
                    .execute(&mut **transaction)
                    .await?;
                }
                existing.id
            }
            None => {
                let id = Uuid::new_v4().to_string();
                let incoming_confidence = generic_company_confidence(&input.identity).as_str();
                sqlx::query(
                    "INSERT INTO tally_companies(\
                       id, endpoint_id, display_name, company_guid, remote_id, master_id, \
                       fallback_fingerprint, identity_confidence, first_observed_at_unix_ms, \
                       last_observed_at_unix_ms\
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)",
                )
                .bind(&id)
                .bind(&input.endpoint_id)
                .bind(&input.display_name)
                .bind(&input.identity.guid)
                .bind(&input.identity.remote_id)
                .bind(&input.identity.master_id)
                .bind(&input.identity.fallback_fingerprint)
                .bind(incoming_confidence)
                .bind(input.observed_at_unix_ms)
                .execute(&mut **transaction)
                .await?;
                id
            }
        };
        Ok(CompanyRef {
            id,
            display_name: input.display_name,
        })
    }

    /// Persists only a complete, directly observed company tuple. This path is
    /// deliberately separate from the generic source-record identity helper:
    /// a Tally GUID is not unique after a year-end split, so a GUID match must
    /// never select an older company row here.
    async fn upsert_reviewed_company_in_transaction(
        transaction: &mut Transaction<'_, Sqlite>,
        endpoint_id: &str,
        display_name: &str,
        identity: &SourceIdentityInput,
        company_number: &str,
        books_from_yyyymmdd: &str,
        observed_at_unix_ms: i64,
    ) -> Result<CompanyRef, MirrorError> {
        let guid = identity
            .guid
            .as_deref()
            .ok_or(MirrorError::InvalidInput("company_guid_unobserved"))?;
        let existing = sqlx::query(
            "SELECT id FROM tally_companies WHERE endpoint_id = ?1 \
             AND company_number = ?2 AND company_guid = ?3 COLLATE NOCASE \
             AND display_name = ?4 AND books_from_yyyymmdd = ?5",
        )
        .bind(endpoint_id)
        .bind(company_number)
        .bind(guid)
        .bind(display_name)
        .bind(books_from_yyyymmdd)
        .fetch_optional(&mut **transaction)
        .await?;
        let id = if let Some(existing) = existing {
            let id: String = existing.try_get("id")?;
            sqlx::query(
                "UPDATE tally_companies SET last_observed_at_unix_ms = ?1, \
                 identity_confidence = 'observed' WHERE id = ?2",
            )
            .bind(observed_at_unix_ms)
            .bind(&id)
            .execute(&mut **transaction)
            .await?;
            id
        } else {
            let id = Uuid::new_v4().to_string();
            sqlx::query(
                "INSERT INTO tally_companies(\
                   id, endpoint_id, display_name, company_guid, remote_id, master_id, \
                   fallback_fingerprint, identity_confidence, first_observed_at_unix_ms, \
                   last_observed_at_unix_ms, company_number, books_from_yyyymmdd\
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'observed', ?8, ?8, ?9, ?10)",
            )
            .bind(&id)
            .bind(endpoint_id)
            .bind(display_name)
            .bind(guid)
            // These legacy alternate identifiers are not part of the
            // observed company tuple and can themselves collide across books.
            .bind(Option::<String>::None)
            .bind(Option::<String>::None)
            .bind(Option::<String>::None)
            .bind(observed_at_unix_ms)
            .bind(company_number)
            .bind(books_from_yyyymmdd)
            .execute(&mut **transaction)
            .await?;
            id
        };
        Ok(CompanyRef {
            id,
            display_name: display_name.to_string(),
        })
    }

    pub async fn save_reviewed_setup(
        &self,
        input: ReviewedSetupInput,
    ) -> Result<ReviewedSetupRef, MirrorError> {
        validate_sha256(&input.review_commitment_sha256)?;
        validate_capability_snapshot(&input.capability)?;
        validate_nonempty(&input.company_display_name, 512, "display_name")?;
        validate_identity(&input.company_identity)?;
        if let Some(guid) = input.company_identity.guid.as_deref() {
            validate_company_guid(guid)?;
        }
        validate_company_number(&input.company_number)?;
        validate_books_from_yyyymmdd(&input.books_from_yyyymmdd)?;
        validate_selected_read_scope(
            input.selected_read_scope.as_ref(),
            &input.capability,
            &input.company_display_name,
            input.company_identity.guid.as_deref(),
            &input.company_number,
            &input.books_from_yyyymmdd,
        )?;
        let setup_payload_sha256 = reviewed_setup_payload_sha256(&input)?;

        let observed_at_unix_ms = input.capability.observed_at_unix_ms;
        let mut transaction = self.pool.begin().await?;
        if let Some(existing) = sqlx::query(
            "SELECT consumption.setup_payload_sha256, snapshot.id AS snapshot_id, \
                    snapshot.endpoint_id, company.id AS company_id, company.display_name \
             FROM tally_reviewed_setup_consumptions AS consumption \
             JOIN tally_capability_snapshots AS snapshot \
               ON snapshot.id = consumption.capability_snapshot_id \
             JOIN tally_companies AS company ON company.id = consumption.company_id \
             WHERE consumption.review_commitment_sha256 = ?1",
        )
        .bind(&input.review_commitment_sha256)
        .fetch_optional(&mut *transaction)
        .await?
        {
            if existing.get::<String, _>("setup_payload_sha256") != setup_payload_sha256 {
                transaction.rollback().await?;
                return Err(MirrorError::InvalidInput("review_commitment_reused"));
            }
            let reviewed = ReviewedSetupRef {
                snapshot: CapabilitySnapshotRef {
                    id: existing.get("snapshot_id"),
                    endpoint_id: existing.get("endpoint_id"),
                },
                company: CompanyRef {
                    id: existing.get("company_id"),
                    display_name: existing.get("display_name"),
                },
            };
            transaction.rollback().await?;
            return Ok(reviewed);
        }
        let snapshot = Self::insert_capability_snapshot(&mut transaction, input.capability).await?;
        let company = Self::upsert_reviewed_company_in_transaction(
            &mut transaction,
            &snapshot.endpoint_id,
            &input.company_display_name,
            &input.company_identity,
            &input.company_number,
            &input.books_from_yyyymmdd,
            observed_at_unix_ms,
        )
        .await?;
        if let Some(scope) = input.selected_read_scope {
            Self::insert_selected_read_scope(&mut transaction, &snapshot.id, &company.id, scope)
                .await?;
        }
        sqlx::query(
            "INSERT INTO tally_reviewed_setup_consumptions(\
               review_commitment_sha256, setup_payload_sha256, capability_snapshot_id, \
               company_id, consumed_at_unix_ms\
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .bind(input.review_commitment_sha256)
        .bind(setup_payload_sha256)
        .bind(&snapshot.id)
        .bind(&company.id)
        .bind(Utc::now().timestamp_millis())
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(ReviewedSetupRef { snapshot, company })
    }

    async fn insert_selected_read_scope(
        transaction: &mut Transaction<'_, Sqlite>,
        snapshot_id: &str,
        company_id: &str,
        input: SelectedReadScopeInput,
    ) -> Result<(), MirrorError> {
        let scope_id = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO tally_selected_read_scopes(\
               id, capability_snapshot_id, company_id, scope_contract_version, \
               scope_commitment_sha256, parent_review_sha256, ledger_profile_id, \
               voucher_profile_id, voucher_from_yyyymmdd, voucher_to_yyyymmdd, \
               observed_at_unix_ms, completeness_state, no_writes_attempted, \
               raw_records_retained\
             ) VALUES (?1, ?2, ?3, 2, ?4, ?5, ?6, ?7, ?8, ?9, ?10, \
               'not_claimed', 1, 0)",
        )
        .bind(&scope_id)
        .bind(snapshot_id)
        .bind(company_id)
        .bind(input.scope_commitment_sha256)
        .bind(input.parent_review_sha256)
        .bind(input.ledger_profile_id)
        .bind(input.voucher_profile_id)
        .bind(input.voucher_from_yyyymmdd)
        .bind(input.voucher_to_yyyymmdd)
        .bind(input.observed_at_unix_ms)
        .execute(&mut **transaction)
        .await?;

        for observation in input.observations {
            sqlx::query(
                "INSERT INTO tally_selected_read_observations(\
                   scope_id, capability_snapshot_id, capability_kind, capability_key, \
                   capability_state, confidence, safe_reason_code, result_bucket, \
                   request_sha256, decoded_response_sha256, response_encoding, company_context_verified, schema_verified, \
                   record_count_verified, identity_evidence_state, date_window_verified\
                 ) VALUES (?1, ?2, 'feature', ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, \
                   ?12, ?13, ?14, ?15)",
            )
            .bind(&scope_id)
            .bind(snapshot_id)
            .bind(observation.capability_key)
            .bind(observation.state.as_str())
            .bind(observation.confidence.as_str())
            .bind(observation.safe_reason_code)
            .bind(observation.result_bucket)
            .bind(observation.request_sha256)
            .bind(observation.decoded_response_sha256)
            .bind(observation.response_encoding)
            .bind(i64::from(observation.company_context_verified))
            .bind(i64::from(observation.schema_verified))
            .bind(i64::from(observation.record_count_verified))
            .bind(observation.identity_evidence_state)
            .bind(i64::from(observation.date_window_verified))
            .execute(&mut **transaction)
            .await?;
        }
        Ok(())
    }

    pub async fn begin_batch(&self, input: BeginBatchInput) -> Result<String, MirrorError> {
        validate_nonempty(&input.run_id, 128, "run_id")?;
        validate_safe_code(&input.pack_id)?;
        validate_safe_code(&input.source_transport)?;
        validate_optional_text(input.source_release.as_deref(), 128, "source_release")?;
        validate_date_range(
            input.requested_from_yyyymmdd.as_deref(),
            input.requested_to_yyyymmdd.as_deref(),
        )?;

        let mut transaction = self.pool.begin().await?;
        // Serialize the read-before-insert idempotency check. The v2 schema deliberately permits
        // one batch per (run, pack), so v5 must not strengthen this to global run uniqueness.
        sqlx::query("UPDATE tally_schema_migrations SET version = version WHERE version = 4")
            .execute(&mut *transaction)
            .await?;
        let same_endpoint = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_capability_snapshots AS s \
             JOIN tally_companies AS c ON c.endpoint_id = s.endpoint_id \
             WHERE s.id = ?1 AND c.id = ?2",
        )
        .bind(&input.capability_snapshot_id)
        .bind(&input.company_id)
        .fetch_one(&mut *transaction)
        .await?;
        if same_endpoint != 1 {
            return Err(MirrorError::InvalidInput("snapshot_company_endpoint"));
        }

        let existing = sqlx::query(
            "SELECT id, capability_snapshot_id, company_id, pack_id, pack_schema_major, \
               pack_schema_minor, source_transport, source_release, requested_from_yyyymmdd, \
               requested_to_yyyymmdd, started_at_unix_ms, state \
             FROM tally_observation_batches WHERE run_id = ?1 AND pack_id = ?2",
        )
        .bind(&input.run_id)
        .bind(&input.pack_id)
        .fetch_optional(&mut *transaction)
        .await?;
        if let Some(existing) = existing {
            let matches = existing.try_get::<String, _>("capability_snapshot_id")?
                == input.capability_snapshot_id
                && existing.try_get::<String, _>("company_id")? == input.company_id
                && existing.try_get::<String, _>("pack_id")? == input.pack_id
                && existing.try_get::<i64, _>("pack_schema_major")?
                    == i64::from(input.pack_schema_major)
                && existing.try_get::<i64, _>("pack_schema_minor")?
                    == i64::from(input.pack_schema_minor)
                && existing.try_get::<String, _>("source_transport")? == input.source_transport
                && existing.try_get::<Option<String>, _>("source_release")? == input.source_release
                && existing.try_get::<Option<String>, _>("requested_from_yyyymmdd")?
                    == input.requested_from_yyyymmdd
                && existing.try_get::<Option<String>, _>("requested_to_yyyymmdd")?
                    == input.requested_to_yyyymmdd
                && existing.try_get::<i64, _>("started_at_unix_ms")? == input.started_at_unix_ms
                && existing.try_get::<String, _>("state")? == "staging";
            if !matches {
                return Err(MirrorError::InvalidInput("run_batch_mismatch"));
            }
            let id = existing.try_get("id")?;
            transaction.commit().await?;
            return Ok(id);
        }

        let id = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO tally_observation_batches(\
               id, run_id, capability_snapshot_id, company_id, pack_id, pack_schema_major, \
               pack_schema_minor, source_transport, source_release, requested_from_yyyymmdd, \
               requested_to_yyyymmdd, started_at_unix_ms, state\
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 'staging')",
        )
        .bind(&id)
        .bind(&input.run_id)
        .bind(&input.capability_snapshot_id)
        .bind(&input.company_id)
        .bind(&input.pack_id)
        .bind(i64::from(input.pack_schema_major))
        .bind(i64::from(input.pack_schema_minor))
        .bind(&input.source_transport)
        .bind(&input.source_release)
        .bind(&input.requested_from_yyyymmdd)
        .bind(&input.requested_to_yyyymmdd)
        .bind(input.started_at_unix_ms)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(id)
    }

    pub async fn observe_record(&self, input: ObservedRecordInput) -> Result<String, MirrorError> {
        match self.observe_record_idempotent(input).await {
            Ok(ObserveRecordOutcome::Inserted { observation_id }) => Ok(observation_id),
            Ok(ObserveRecordOutcome::AlreadyPresentIdentical { .. })
            | Err(MirrorError::ObservationConflict) => Err(MirrorError::DuplicateObservation),
            Err(error) => Err(error),
        }
    }

    pub async fn observe_record_idempotent(
        &self,
        input: ObservedRecordInput,
    ) -> Result<ObserveRecordOutcome, MirrorError> {
        let prepared = prepare_observed_record(&input)?;

        let mut transaction = self.pool.begin().await?;
        // Acquire SQLite's write lock before the replay check. This keeps the
        // read-before-insert decision exact when multiple workers lose the
        // acknowledgement for the same observation concurrently.
        sqlx::query("UPDATE tally_schema_migrations SET version = version WHERE version = 4")
            .execute(&mut *transaction)
            .await?;
        let result =
            Self::observe_record_in_transaction(&mut transaction, &input, &prepared).await?;
        transaction.commit().await?;
        Ok(result.outcome)
    }

    async fn observe_record_in_transaction(
        transaction: &mut Transaction<'_, Sqlite>,
        input: &ObservedRecordInput,
        prepared: &PreparedObservedRecord,
    ) -> Result<ObservedTransactionResult, MirrorError> {
        let batch =
            sqlx::query("SELECT company_id, state FROM tally_observation_batches WHERE id = ?1")
                .bind(&input.batch_id)
                .fetch_optional(&mut **transaction)
                .await?
                .ok_or(MirrorError::NotFound)?;
        let company_id: String = batch.try_get("company_id")?;
        let state: String = batch.try_get("state")?;
        if state != "staging" {
            return Err(MirrorError::BatchClosed);
        }

        let matches = find_identity_matches(
            transaction,
            "tally_source_records",
            "company_id",
            &company_id,
            Some(&input.object_type),
            &input.identity,
        )
        .await?;

        let source_record_id = match unique_match(matches)? {
            Some(existing) => {
                ensure_no_silent_identity_change(&existing, &input.identity)?;
                let stored = sqlx::query(
                    "SELECT id, raw_source_sha256, canonical_sha256, canonical_payload_json, \
                       exact_decimals_json, observed_alter_id, validation_status, \
                       safe_rejection_code \
                     FROM tally_record_observations \
                     WHERE batch_id = ?1 AND source_record_id = ?2",
                )
                .bind(&input.batch_id)
                .bind(&existing.id)
                .fetch_optional(&mut **transaction)
                .await?;
                if let Some(stored) = stored {
                    let observation_id: String = stored.try_get("id")?;
                    let identical = stored.try_get::<String, _>("raw_source_sha256")?.as_str()
                        == input.raw_source_sha256.as_str()
                        && stored
                            .try_get::<Option<String>, _>("canonical_sha256")?
                            .as_deref()
                            == input.canonical_sha256.as_deref()
                        && stored
                            .try_get::<Option<String>, _>("canonical_payload_json")?
                            .as_deref()
                            == prepared.canonical_payload_json.as_deref()
                        && stored.try_get::<String, _>("exact_decimals_json")?.as_str()
                            == prepared.exact_decimals_json.as_str()
                        && stored
                            .try_get::<Option<String>, _>("observed_alter_id")?
                            .as_deref()
                            == input.observed_alter_id.as_deref()
                        && stored.try_get::<String, _>("validation_status")?.as_str()
                            == input.status.as_str()
                        && stored
                            .try_get::<Option<String>, _>("safe_rejection_code")?
                            .as_deref()
                            == input.safe_rejection_code.as_deref();
                    if !identical {
                        return Err(MirrorError::ObservationConflict);
                    }
                    return Ok(ObservedTransactionResult {
                        outcome: ObserveRecordOutcome::AlreadyPresentIdentical { observation_id },
                        source_record_id: existing.id,
                    });
                }
                sqlx::query(
                    "UPDATE tally_source_records SET display_name = COALESCE(?1, display_name), \
                     last_seen_batch_id = ?2, tombstoned_at_unix_ms = NULL WHERE id = ?3",
                )
                .bind(&input.display_name)
                .bind(&input.batch_id)
                .bind(&existing.id)
                .execute(&mut **transaction)
                .await?;
                existing.id
            }
            None => {
                let id = Uuid::new_v4().to_string();
                sqlx::query(
                    "INSERT INTO tally_source_records(\
                       id, company_id, object_type, display_name, source_guid, remote_id, master_id, \
                       fallback_fingerprint, identity_confidence, first_seen_batch_id, \
                       last_seen_batch_id\
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)",
                )
                .bind(&id)
                .bind(&company_id)
                .bind(&input.object_type)
                .bind(&input.display_name)
                .bind(&input.identity.guid)
                .bind(&input.identity.remote_id)
                .bind(&input.identity.master_id)
                .bind(&input.identity.fallback_fingerprint)
                .bind(identity_confidence(&input.identity).as_str())
                .bind(&input.batch_id)
                .execute(&mut **transaction)
                .await?;
                id
            }
        };

        let observation_id = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO tally_record_observations(\
               id, batch_id, source_record_id, observed_at_unix_ms, raw_source_sha256, \
               canonical_sha256, canonical_payload_json, exact_decimals_json, observed_alter_id, \
               validation_status, safe_rejection_code\
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        )
        .bind(&observation_id)
        .bind(&input.batch_id)
        .bind(&source_record_id)
        .bind(input.observed_at_unix_ms)
        .bind(&input.raw_source_sha256)
        .bind(&input.canonical_sha256)
        .bind(&prepared.canonical_payload_json)
        .bind(&prepared.exact_decimals_json)
        .bind(&input.observed_alter_id)
        .bind(input.status.as_str())
        .bind(&input.safe_rejection_code)
        .execute(&mut **transaction)
        .await?;
        Ok(ObservedTransactionResult {
            outcome: ObserveRecordOutcome::Inserted {
                observation_id: observation_id.clone(),
            },
            source_record_id,
        })
    }

    pub async fn begin_snapshot_window_attempt(
        &self,
        input: BeginSnapshotWindowAttemptInput,
    ) -> Result<BeginSnapshotWindowAttemptResult, MirrorError> {
        validate_nonempty(&input.batch_id, 128, "window_attempt_batch_id")?;
        validate_nonempty(&input.window_id, 128, "window_attempt_window_id")?;
        if input.started_at_unix_ms <= 0 {
            return Err(MirrorError::InvalidInput("window_attempt_started_at"));
        }
        let mut transaction = self.pool.begin().await?;
        acquire_mirror_write_lock(&mut transaction).await?;
        let batch_state = sqlx::query_scalar::<_, String>(
            "SELECT state FROM tally_observation_batches WHERE id = ?1",
        )
        .bind(&input.batch_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(MirrorError::NotFound)?;
        if batch_state != "staging" {
            return Err(MirrorError::BatchClosed);
        }
        sqlx::query(
            "UPDATE tally_snapshot_window_attempts \
             SET state = 'abandoned', completed_at_unix_ms = \
               CASE WHEN started_at_unix_ms > ?1 THEN started_at_unix_ms ELSE ?1 END, \
               terminal_safe_reason_code = CASE WHEN started_at_unix_ms > ?1 \
                 THEN 'local_clock_moved_backwards' ELSE NULL END \
             WHERE batch_id = ?2 AND window_id = ?3 AND state = 'open'",
        )
        .bind(input.started_at_unix_ms)
        .bind(&input.batch_id)
        .bind(&input.window_id)
        .execute(&mut *transaction)
        .await?;
        // Query cumulatively rather than relying on the row just changed. If the process loses
        // the begin acknowledgement before saving its new attempt ref, a later begin can still
        // recover rollback evidence from an earlier implicitly abandoned attempt.
        let prior_abandonment = sqlx::query(
            "SELECT completed_at_unix_ms FROM tally_snapshot_window_attempts \
             WHERE batch_id = ?1 AND window_id = ?2 AND state = 'abandoned' \
               AND terminal_safe_reason_code = 'local_clock_moved_backwards' \
             ORDER BY attempt_ordinal DESC LIMIT 1",
        )
        .bind(&input.batch_id)
        .bind(&input.window_id)
        .fetch_optional(&mut *transaction)
        .await?
        .map(|row| -> Result<_, MirrorError> {
            Ok(AbandonSnapshotWindowAttemptResult {
                completed_at_unix_ms: row.try_get("completed_at_unix_ms")?,
                local_clock_moved_backwards: true,
            })
        })
        .transpose()?;
        let next_ordinal = sqlx::query_scalar::<_, i64>(
            "SELECT COALESCE(MAX(attempt_ordinal), 0) + 1 \
             FROM tally_snapshot_window_attempts WHERE batch_id = ?1 AND window_id = ?2",
        )
        .bind(&input.batch_id)
        .bind(&input.window_id)
        .fetch_one(&mut *transaction)
        .await?;
        let attempt_ordinal = u32::try_from(next_ordinal)
            .map_err(|_| MirrorError::InvalidInput("window_attempt_ordinal"))?;
        let attempt_id = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO tally_snapshot_window_attempts(\
               id, batch_id, window_id, attempt_ordinal, state, started_at_unix_ms\
             ) VALUES (?1, ?2, ?3, ?4, 'open', ?5)",
        )
        .bind(&attempt_id)
        .bind(&input.batch_id)
        .bind(&input.window_id)
        .bind(next_ordinal)
        .bind(input.started_at_unix_ms)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(BeginSnapshotWindowAttemptResult {
            attempt: SnapshotWindowAttemptRef {
                attempt_id,
                batch_id: input.batch_id,
                window_id: input.window_id,
                attempt_ordinal,
            },
            prior_abandonment,
        })
    }

    pub async fn stage_snapshot_window_membership(
        &self,
        attempt: &SnapshotWindowAttemptRef,
        membership: SnapshotWindowMembershipInput,
    ) -> Result<StageSnapshotWindowMembershipsResult, MirrorError> {
        self.stage_snapshot_window_memberships(attempt, vec![membership])
            .await
    }

    pub async fn abandon_snapshot_window_attempt(
        &self,
        attempt: &SnapshotWindowAttemptRef,
        observed_completed_at_unix_ms: i64,
    ) -> Result<AbandonSnapshotWindowAttemptResult, MirrorError> {
        validate_snapshot_window_attempt_ref(attempt)?;
        if observed_completed_at_unix_ms <= 0 {
            return Err(MirrorError::InvalidInput("window_attempt_completed_at"));
        }
        let mut transaction = self.pool.begin().await?;
        acquire_mirror_write_lock(&mut transaction).await?;
        let stored = sqlx::query(
            "SELECT state, started_at_unix_ms, completed_at_unix_ms, \
               terminal_safe_reason_code \
             FROM tally_snapshot_window_attempts \
             WHERE id = ?1 AND batch_id = ?2 AND window_id = ?3 AND attempt_ordinal = ?4",
        )
        .bind(&attempt.attempt_id)
        .bind(&attempt.batch_id)
        .bind(&attempt.window_id)
        .bind(i64::from(attempt.attempt_ordinal))
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(MirrorError::NotFound)?;
        let state: String = stored.try_get("state")?;
        let started_at_unix_ms: i64 = stored.try_get("started_at_unix_ms")?;
        if state == "abandoned" {
            let completed_at_unix_ms = stored
                .try_get::<Option<i64>, _>("completed_at_unix_ms")?
                .ok_or(MirrorError::InvalidInput("window_attempt_completed_at"))?;
            let safe_reason_code =
                stored.try_get::<Option<String>, _>("terminal_safe_reason_code")?;
            transaction.commit().await?;
            return Ok(AbandonSnapshotWindowAttemptResult {
                completed_at_unix_ms,
                local_clock_moved_backwards: safe_reason_code.as_deref()
                    == Some("local_clock_moved_backwards"),
            });
        }
        if state != "open" {
            return Err(MirrorError::WindowAttemptClosed);
        }
        // A process can restart after the local wall clock has moved behind the timestamp that
        // was durably recorded when the attempt opened. Abandonment is cleanup, so waiting for
        // wall time to catch up would strand the run in `Staging`. Preserve the database's
        // monotonic timestamp invariant while reporting the rollback to the proof layer.
        let local_clock_moved_backwards = observed_completed_at_unix_ms < started_at_unix_ms;
        let completed_at_unix_ms = observed_completed_at_unix_ms.max(started_at_unix_ms);
        let updated = sqlx::query(
            "UPDATE tally_snapshot_window_attempts \
             SET state = 'abandoned', completed_at_unix_ms = ?1, \
               terminal_safe_reason_code = ?2 \
             WHERE id = ?3 AND batch_id = ?4 AND window_id = ?5 \
               AND attempt_ordinal = ?6 AND state = 'open'",
        )
        .bind(completed_at_unix_ms)
        .bind(local_clock_moved_backwards.then_some("local_clock_moved_backwards"))
        .bind(&attempt.attempt_id)
        .bind(&attempt.batch_id)
        .bind(&attempt.window_id)
        .bind(i64::from(attempt.attempt_ordinal))
        .execute(&mut *transaction)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(MirrorError::WindowAttemptClosed);
        }
        transaction.commit().await?;
        Ok(AbandonSnapshotWindowAttemptResult {
            completed_at_unix_ms,
            local_clock_moved_backwards,
        })
    }

    pub async fn abandon_open_snapshot_window_attempts_for_batch(
        &self,
        batch_id: &str,
        observed_completed_at_unix_ms: i64,
    ) -> Result<SnapshotWindowAttemptCleanupResult, MirrorError> {
        validate_nonempty(batch_id, 128, "window_attempt_batch_id")?;
        if observed_completed_at_unix_ms <= 0 {
            return Err(MirrorError::InvalidInput("window_attempt_completed_at"));
        }
        let mut transaction = self.pool.begin().await?;
        acquire_mirror_write_lock(&mut transaction).await?;
        let batch_exists = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_observation_batches WHERE id = ?1",
        )
        .bind(batch_id)
        .fetch_one(&mut *transaction)
        .await?;
        if batch_exists != 1 {
            return Err(MirrorError::NotFound);
        }
        sqlx::query(
            "UPDATE tally_snapshot_window_attempts SET state = 'abandoned', \
               completed_at_unix_ms = MAX(started_at_unix_ms, ?1), \
               terminal_safe_reason_code = CASE WHEN started_at_unix_ms > ?1 \
                 THEN 'local_clock_moved_backwards' ELSE NULL END \
             WHERE batch_id = ?2 AND state = 'open'",
        )
        .bind(observed_completed_at_unix_ms)
        .bind(batch_id)
        .execute(&mut *transaction)
        .await?;
        // Keep timeline and gap evidence independent. The floor covers every terminal attempt,
        // including a normal-clock orphan completed after an already-staged pending decision;
        // the rollback flag is the cumulative durable ANY of the reviewed terminal reason.
        let aggregate = sqlx::query(
            "SELECT MAX(completed_at_unix_ms) AS completed_at_floor, \
               COALESCE(MAX(CASE WHEN terminal_safe_reason_code = \
                 'local_clock_moved_backwards' THEN 1 ELSE 0 END), 0) AS clock_rollback \
             FROM tally_snapshot_window_attempts \
             WHERE batch_id = ?1 AND state IN ('abandoned', 'complete')",
        )
        .bind(batch_id)
        .fetch_one(&mut *transaction)
        .await?;
        let result = SnapshotWindowAttemptCleanupResult {
            completed_at_floor: aggregate.try_get("completed_at_floor")?,
            local_clock_moved_backwards: aggregate.try_get::<i64, _>("clock_rollback")? != 0,
        };
        transaction.commit().await?;
        Ok(result)
    }

    pub async fn stage_snapshot_window_memberships(
        &self,
        attempt: &SnapshotWindowAttemptRef,
        memberships: Vec<SnapshotWindowMembershipInput>,
    ) -> Result<StageSnapshotWindowMembershipsResult, MirrorError> {
        validate_snapshot_window_attempt_ref(attempt)?;
        if memberships.is_empty() || memberships.len() > MAX_WINDOW_STAGE_CHUNK {
            return Err(MirrorError::InvalidInput("window_membership_chunk_size"));
        }
        for membership in &memberships {
            validate_snapshot_window_membership_input(membership, &attempt.batch_id)?;
        }

        let mut transaction = self.pool.begin().await?;
        acquire_mirror_write_lock(&mut transaction).await?;
        ensure_open_snapshot_window_attempt(&mut transaction, attempt).await?;
        let mut result = StageSnapshotWindowMembershipsResult::default();
        for membership in memberships {
            let (
                record_key,
                canonical_sha256,
                canonical_payload_json,
                exact_decimals_json,
                provenance_state,
                source_record_id,
                observation_id,
                safe_reason_code,
            ) = match membership {
                SnapshotWindowMembershipInput::Observed {
                    record_key,
                    observation,
                } => {
                    let prepared = prepare_observed_record(&observation)?;
                    let observed = Self::observe_record_in_transaction(
                        &mut transaction,
                        &observation,
                        &prepared,
                    )
                    .await?;
                    let observation_id = match observed.outcome {
                        ObserveRecordOutcome::Inserted { observation_id } => {
                            result.inserted_observations += 1;
                            observation_id
                        }
                        ObserveRecordOutcome::AlreadyPresentIdentical { observation_id } => {
                            result.replayed_observations += 1;
                            observation_id
                        }
                    };
                    (
                        record_key,
                        observation
                            .canonical_sha256
                            .expect("validated accepted observation"),
                        prepared
                            .canonical_payload_json
                            .expect("validated accepted observation"),
                        prepared.exact_decimals_json,
                        "observed",
                        Some(observed.source_record_id),
                        Some(observation_id),
                        None,
                    )
                }
                SnapshotWindowMembershipInput::ProvenanceUnavailable {
                    record_key,
                    canonical_sha256,
                    canonical_payload,
                    exact_decimals,
                    safe_reason_code,
                } => {
                    result.provenance_unavailable_memberships += 1;
                    (
                        record_key,
                        canonical_sha256,
                        canonical_json(&canonical_payload)?,
                        validate_and_serialize_decimals(&exact_decimals)?,
                        "unavailable",
                        None,
                        None,
                        Some(safe_reason_code),
                    )
                }
            };

            let existing = sqlx::query(
                "SELECT canonical_sha256, canonical_payload_json, exact_decimals_json, \
                        provenance_state, source_record_id, observation_id, safe_reason_code \
                 FROM tally_snapshot_window_memberships \
                 WHERE batch_id = ?1 AND window_id = ?2 AND record_key = ?3",
            )
            .bind(&attempt.batch_id)
            .bind(&attempt.window_id)
            .bind(&record_key)
            .fetch_optional(&mut *transaction)
            .await?;
            if let Some(existing) = existing {
                let identical = existing.try_get::<String, _>("canonical_sha256")?
                    == canonical_sha256
                    && existing.try_get::<String, _>("canonical_payload_json")?
                        == canonical_payload_json
                    && existing.try_get::<String, _>("exact_decimals_json")? == exact_decimals_json
                    && existing.try_get::<String, _>("provenance_state")? == provenance_state
                    && existing.try_get::<Option<String>, _>("source_record_id")?
                        == source_record_id
                    && existing.try_get::<Option<String>, _>("observation_id")? == observation_id
                    && existing.try_get::<Option<String>, _>("safe_reason_code")?
                        == safe_reason_code;
                if !identical {
                    return Err(MirrorError::WindowMembershipConflict);
                }
                sqlx::query(
                    "UPDATE tally_snapshot_window_memberships SET last_seen_attempt_id = ?1 \
                     WHERE batch_id = ?2 AND window_id = ?3 AND record_key = ?4",
                )
                .bind(&attempt.attempt_id)
                .bind(&attempt.batch_id)
                .bind(&attempt.window_id)
                .bind(&record_key)
                .execute(&mut *transaction)
                .await?;
                result.replayed_memberships += 1;
            } else {
                sqlx::query(
                    "INSERT INTO tally_snapshot_window_memberships(\
                       batch_id, window_id, record_key, canonical_sha256, canonical_payload_json, \
                       exact_decimals_json, provenance_state, source_record_id, observation_id, \
                       safe_reason_code, first_seen_attempt_id, last_seen_attempt_id\
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11)",
                )
                .bind(&attempt.batch_id)
                .bind(&attempt.window_id)
                .bind(&record_key)
                .bind(&canonical_sha256)
                .bind(&canonical_payload_json)
                .bind(&exact_decimals_json)
                .bind(provenance_state)
                .bind(&source_record_id)
                .bind(&observation_id)
                .bind(&safe_reason_code)
                .bind(&attempt.attempt_id)
                .execute(&mut *transaction)
                .await?;
                result.inserted_memberships += 1;
            }
        }
        transaction.commit().await?;
        Ok(result)
    }

    pub async fn complete_snapshot_window_attempt(
        &self,
        attempt: &SnapshotWindowAttemptRef,
        observed_completed_at_unix_ms: i64,
        evidence: Value,
    ) -> Result<SnapshotWindowCompletionResult, MirrorError> {
        validate_snapshot_window_attempt_ref(attempt)?;
        if observed_completed_at_unix_ms <= 0 {
            return Err(MirrorError::InvalidInput("window_attempt_completed_at"));
        }
        let evidence_json = canonical_json(&evidence)?;
        if evidence_json.len() > MAX_WINDOW_EVIDENCE_JSON_BYTES {
            return Err(MirrorError::InvalidInput("window_attempt_evidence_size"));
        }
        let mut transaction = self.pool.begin().await?;
        acquire_mirror_write_lock(&mut transaction).await?;
        ensure_open_snapshot_window_attempt(&mut transaction, attempt).await?;
        let started_at_unix_ms = sqlx::query_scalar::<_, i64>(
            "SELECT started_at_unix_ms FROM tally_snapshot_window_attempts \
             WHERE id = ?1 AND batch_id = ?2 AND window_id = ?3 AND attempt_ordinal = ?4",
        )
        .bind(&attempt.attempt_id)
        .bind(&attempt.batch_id)
        .bind(&attempt.window_id)
        .bind(i64::from(attempt.attempt_ordinal))
        .fetch_one(&mut *transaction)
        .await?;
        let local_clock_moved_backwards = observed_completed_at_unix_ms < started_at_unix_ms;
        let completed_at_unix_ms = observed_completed_at_unix_ms.max(started_at_unix_ms);
        let disappeared = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_snapshot_window_memberships \
             WHERE batch_id = ?1 AND window_id = ?2 AND last_seen_attempt_id <> ?3",
        )
        .bind(&attempt.batch_id)
        .bind(&attempt.window_id)
        .bind(&attempt.attempt_id)
        .fetch_one(&mut *transaction)
        .await?;
        if disappeared != 0 {
            return Err(MirrorError::WindowMembershipDisappeared);
        }
        let (member_count, membership_sha256) = Self::snapshot_window_membership_digest(
            &mut transaction,
            &attempt.batch_id,
            &attempt.window_id,
            Some(&attempt.attempt_id),
            None,
        )
        .await?;
        let material = SnapshotWindowReceiptMaterial {
            schema: "bridge.tally.snapshot-window-receipt/1",
            attempt_id: &attempt.attempt_id,
            batch_id: &attempt.batch_id,
            window_id: &attempt.window_id,
            attempt_ordinal: attempt.attempt_ordinal,
            member_count,
            membership_sha256: &membership_sha256,
            evidence: &evidence,
            completed_at_unix_ms,
        };
        let receipt_sha256 = sha256_json(&material)?;
        let receipt = SnapshotWindowReceipt {
            schema: material.schema.to_string(),
            attempt_id: attempt.attempt_id.clone(),
            batch_id: attempt.batch_id.clone(),
            window_id: attempt.window_id.clone(),
            attempt_ordinal: attempt.attempt_ordinal,
            member_count,
            membership_sha256,
            evidence,
            completed_at_unix_ms,
            receipt_sha256: receipt_sha256.clone(),
        };
        let receipt_json = canonical_json(&serde_json::to_value(&receipt)?)?;
        sqlx::query(
            "UPDATE tally_snapshot_window_attempts \
             SET state = 'complete', completed_at_unix_ms = ?1, receipt_json = ?2, \
                 receipt_sha256 = ?3, terminal_safe_reason_code = ?4 \
             WHERE id = ?5 AND batch_id = ?6 AND window_id = ?7",
        )
        .bind(completed_at_unix_ms)
        .bind(receipt_json)
        .bind(receipt_sha256)
        .bind(local_clock_moved_backwards.then_some("local_clock_moved_backwards"))
        .bind(&attempt.attempt_id)
        .bind(&attempt.batch_id)
        .bind(&attempt.window_id)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(SnapshotWindowCompletionResult {
            receipt,
            local_clock_moved_backwards,
        })
    }

    pub async fn load_latest_completed_window_receipt(
        &self,
        batch_id: &str,
        window_id: &str,
    ) -> Result<Option<SnapshotWindowCompletionResult>, MirrorError> {
        validate_nonempty(batch_id, 128, "window_attempt_batch_id")?;
        validate_nonempty(window_id, 128, "window_attempt_window_id")?;
        let mut transaction = self.pool.begin().await?;
        let row = sqlx::query(
            "SELECT id, attempt_ordinal, receipt_json, receipt_sha256, terminal_safe_reason_code \
             FROM tally_snapshot_window_attempts \
             WHERE batch_id = ?1 AND window_id = ?2 AND state = 'complete' \
             ORDER BY attempt_ordinal DESC LIMIT 1",
        )
        .bind(batch_id)
        .bind(window_id)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some(row) = row else {
            transaction.commit().await?;
            return Ok(None);
        };
        let attempt_id: String = row.try_get("id")?;
        let attempt_ordinal = u32::try_from(row.try_get::<i64, _>("attempt_ordinal")?)
            .map_err(|_| MirrorError::VerificationInvariant)?;
        let receipt_json: String = row.try_get("receipt_json")?;
        let stored_receipt_sha256: String = row.try_get("receipt_sha256")?;
        let local_clock_moved_backwards = row
            .try_get::<Option<String>, _>("terminal_safe_reason_code")?
            .as_deref()
            == Some("local_clock_moved_backwards");
        if receipt_json.len() > MAX_WINDOW_EVIDENCE_JSON_BYTES + 4_096 {
            return Err(MirrorError::VerificationInvariant);
        }
        let receipt: SnapshotWindowReceipt = serde_json::from_str(&receipt_json)?;
        if receipt.schema != "bridge.tally.snapshot-window-receipt/1"
            || receipt.attempt_id != attempt_id
            || receipt.batch_id != batch_id
            || receipt.window_id != window_id
            || receipt.attempt_ordinal != attempt_ordinal
            || receipt.completed_at_unix_ms <= 0
            || receipt.membership_sha256.len() != 64
            || receipt.receipt_sha256.len() != 64
            || canonical_json(&receipt.evidence)?.len() > MAX_WINDOW_EVIDENCE_JSON_BYTES
        {
            return Err(MirrorError::VerificationInvariant);
        }
        validate_sha256(&receipt.membership_sha256)
            .map_err(|_| MirrorError::VerificationInvariant)?;
        validate_sha256(&receipt.receipt_sha256).map_err(|_| MirrorError::VerificationInvariant)?;
        let computed = sha256_json(&SnapshotWindowReceiptMaterial {
            schema: "bridge.tally.snapshot-window-receipt/1",
            attempt_id: &receipt.attempt_id,
            batch_id: &receipt.batch_id,
            window_id: &receipt.window_id,
            attempt_ordinal: receipt.attempt_ordinal,
            member_count: receipt.member_count,
            membership_sha256: &receipt.membership_sha256,
            evidence: &receipt.evidence,
            completed_at_unix_ms: receipt.completed_at_unix_ms,
        })?;
        if computed != receipt.receipt_sha256 || computed != stored_receipt_sha256 {
            return Err(MirrorError::VerificationInvariant);
        }
        let (member_count, membership_sha256) = Self::snapshot_window_membership_digest(
            &mut transaction,
            batch_id,
            window_id,
            None,
            Some(attempt_ordinal),
        )
        .await?;
        if member_count != receipt.member_count || membership_sha256 != receipt.membership_sha256 {
            return Err(MirrorError::VerificationInvariant);
        }
        transaction.commit().await?;
        Ok(Some(SnapshotWindowCompletionResult {
            receipt,
            local_clock_moved_backwards,
        }))
    }

    async fn snapshot_window_membership_digest(
        transaction: &mut Transaction<'_, Sqlite>,
        batch_id: &str,
        window_id: &str,
        last_seen_attempt_id: Option<&str>,
        maximum_first_seen_ordinal: Option<u32>,
    ) -> Result<(u32, String), MirrorError> {
        if last_seen_attempt_id.is_some() == maximum_first_seen_ordinal.is_some() {
            return Err(MirrorError::InvalidInput("window_membership_digest_scope"));
        }
        let mut digest = Sha256::new();
        digest.update(b"[");
        let mut first = true;
        let mut count = 0_u32;
        let mut after_record_key = String::new();
        loop {
            let rows = sqlx::query(
                "SELECT membership.record_key, membership.canonical_sha256, \
                        membership.provenance_state \
                 FROM tally_snapshot_window_memberships AS membership \
                 JOIN tally_snapshot_window_attempts AS first_attempt \
                   ON first_attempt.id = membership.first_seen_attempt_id \
                  AND first_attempt.batch_id = membership.batch_id \
                  AND first_attempt.window_id = membership.window_id \
                 WHERE membership.batch_id = ?1 AND membership.window_id = ?2 \
                   AND ((?3 IS NOT NULL AND membership.last_seen_attempt_id = ?3) OR \
                        (?4 IS NOT NULL AND first_attempt.attempt_ordinal <= ?4)) \
                   AND membership.record_key > ?5 \
                 ORDER BY membership.record_key LIMIT ?6",
            )
            .bind(batch_id)
            .bind(window_id)
            .bind(last_seen_attempt_id)
            .bind(maximum_first_seen_ordinal.map(i64::from))
            .bind(&after_record_key)
            .bind(WINDOW_MEMBERSHIP_DIGEST_PAGE_SIZE)
            .fetch_all(&mut **transaction)
            .await?;
            if rows.is_empty() {
                break;
            }
            for row in rows {
                let record_key: String = row.try_get("record_key")?;
                let canonical_sha256: String = row.try_get("canonical_sha256")?;
                let provenance_state: String = row.try_get("provenance_state")?;
                if !first {
                    digest.update(b",");
                }
                first = false;
                digest.update(serde_json::to_vec(&SnapshotWindowMembershipDigestEntry {
                    record_key: &record_key,
                    canonical_sha256: &canonical_sha256,
                    provenance_state: &provenance_state,
                })?);
                count = count
                    .checked_add(1)
                    .ok_or(MirrorError::InvalidInput("window_membership_count"))?;
                after_record_key = record_key;
            }
        }
        digest.update(b"]");
        Ok((count, hex_digest(digest.finalize())))
    }

    pub async fn load_completed_window_canonical_record_map(
        &self,
        attempt: &SnapshotWindowAttemptRef,
    ) -> Result<BTreeMap<String, String>, MirrorError> {
        validate_snapshot_window_attempt_ref(attempt)?;
        let complete = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_snapshot_window_attempts \
             WHERE id = ?1 AND batch_id = ?2 AND window_id = ?3 \
               AND attempt_ordinal = ?4 AND state = 'complete'",
        )
        .bind(&attempt.attempt_id)
        .bind(&attempt.batch_id)
        .bind(&attempt.window_id)
        .bind(i64::from(attempt.attempt_ordinal))
        .fetch_one(&self.pool)
        .await?;
        if complete != 1 {
            return Err(MirrorError::NotFound);
        }
        let rows = sqlx::query(
            "SELECT membership.record_key, membership.canonical_sha256 \
             FROM tally_snapshot_window_memberships AS membership \
             JOIN tally_snapshot_window_attempts AS first_attempt \
               ON first_attempt.id = membership.first_seen_attempt_id \
              AND first_attempt.batch_id = membership.batch_id \
              AND first_attempt.window_id = membership.window_id \
             WHERE membership.batch_id = ?1 AND membership.window_id = ?2 \
               AND first_attempt.attempt_ordinal <= ?3 ORDER BY membership.record_key",
        )
        .bind(&attempt.batch_id)
        .bind(&attempt.window_id)
        .bind(i64::from(attempt.attempt_ordinal))
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                let key: String = row.try_get("record_key")?;
                let canonical_sha256: String = row.try_get("canonical_sha256")?;
                validate_sha256(&canonical_sha256)
                    .map_err(|_| MirrorError::VerificationInvariant)?;
                Ok((key, canonical_sha256))
            })
            .collect()
    }

    pub async fn commit_batch(&self, input: CommitBatchInput) -> Result<CommitResult, MirrorError> {
        let mut input = input.into_parts();
        input.gap_codes.sort();
        input.gap_codes.dedup();
        input.warning_codes.sort();
        input.warning_codes.dedup();
        if input.gap_codes.len() > 32 || input.warning_codes.len() > 32 {
            return Err(MirrorError::InvalidInput("proof_code_count"));
        }
        if input.proof_contract_version == 0 || input.freshness_target_seconds <= 0 {
            return Err(MirrorError::InvalidInput("proof_or_freshness_version"));
        }
        validate_optional_sha256(input.snapshot_sha256.as_deref())?;
        validate_optional_sha256(input.record_counts_sha256.as_deref())?;
        if (input.proof_contract_version >= 3) != input.record_counts_sha256.is_some() {
            return Err(MirrorError::InvalidInput("proof_record_counts_digest"));
        }
        validate_optional_token(input.checkpoint_after.as_deref())?;
        for code in input.gap_codes.iter().chain(&input.warning_codes) {
            validate_safe_code(code)?;
        }

        let mut transaction = self.pool.begin().await?;
        // Acquire SQLite's write lock before reading the proof-chain head.
        sqlx::query("UPDATE tally_schema_migrations SET version = version WHERE version = 4")
            .execute(&mut *transaction)
            .await?;

        let batch = sqlx::query(
            "SELECT run_id, capability_snapshot_id, company_id, pack_id, started_at_unix_ms, state \
             FROM tally_observation_batches WHERE id = ?1",
        )
        .bind(&input.batch_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(MirrorError::NotFound)?;
        let state: String = batch.try_get("state")?;
        if state != "staging" {
            return Err(MirrorError::BatchClosed);
        }
        let started_at_unix_ms: i64 = batch.try_get("started_at_unix_ms")?;
        if input.completed_at_unix_ms < started_at_unix_ms {
            return Err(MirrorError::InvalidInput("batch_completed_at"));
        }

        let counts = sqlx::query(
            "SELECT \
               COALESCE(SUM(CASE WHEN validation_status = 'accepted' THEN 1 ELSE 0 END), 0) \
                 AS accepted_records, \
               COALESCE(SUM(CASE WHEN validation_status = 'rejected' THEN 1 ELSE 0 END), 0) \
                 AS rejected_records \
             FROM tally_record_observations WHERE batch_id = ?1",
        )
        .bind(&input.batch_id)
        .fetch_one(&mut *transaction)
        .await?;
        let accepted_records: i64 = counts.try_get("accepted_records")?;
        let rejected_records: i64 = counts.try_get("rejected_records")?;
        let provenance_unavailable_records = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_snapshot_window_memberships \
             WHERE batch_id = ?1 AND provenance_state = 'unavailable'",
        )
        .bind(&input.batch_id)
        .fetch_one(&mut *transaction)
        .await?;

        if input.verification == VerificationState::Verified
            && (input.outcome != RunOutcome::Completed
                || rejected_records != 0
                || !input.gap_codes.is_empty()
                || input.snapshot_sha256.is_none()
                || input.checkpoint_after.is_none())
        {
            return Err(MirrorError::VerificationInvariant);
        }
        if input.verification != VerificationState::Verified && input.checkpoint_after.is_some() {
            return Err(MirrorError::VerificationInvariant);
        }

        // Proof construction must consume cleanup evidence before this transaction. Refuse to
        // silently close an attempt here: doing so would let an immutable proof omit evidence
        // discovered only after its hash was staged.
        let open_attempt_count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_snapshot_window_attempts \
             WHERE batch_id = ?1 AND state = 'open'",
        )
        .bind(&input.batch_id)
        .fetch_one(&mut *transaction)
        .await?;
        if open_attempt_count != 0 {
            return Err(MirrorError::OpenWindowAttempts);
        }

        let run_id: String = batch.try_get("run_id")?;
        let capability_snapshot_id: String = batch.try_get("capability_snapshot_id")?;
        let company_id: String = batch.try_get("company_id")?;
        let pack_id: String = batch.try_get("pack_id")?;
        let current_checkpoint = sqlx::query_scalar::<_, String>(
            "SELECT checkpoint_token FROM tally_checkpoints WHERE company_id = ?1 AND pack_id = ?2",
        )
        .bind(&company_id)
        .bind(&pack_id)
        .fetch_optional(&mut *transaction)
        .await?;
        // Only a verified commit advances checkpoint authority and therefore needs a compare-and-
        // swap against the current head. Non-advancing proofs retain the checkpoint observed at
        // the start of their run, even if another run has advanced the live head meanwhile. This
        // lets a losing verified run close its staging batch with a truthful terminal proof.
        if input.verification == VerificationState::Verified
            && current_checkpoint != input.expected_checkpoint_before
        {
            return Err(MirrorError::ConcurrentCheckpoint);
        }
        let checkpoint_before = input.expected_checkpoint_before.clone();

        sqlx::query(
            "UPDATE tally_observation_batches SET state = ?1, completed_at_unix_ms = ?2, \
             snapshot_sha256 = ?3, accepted_records = ?4, rejected_records = ?5, \
             provenance_unavailable_records = ?6 WHERE id = ?7",
        )
        .bind(input.verification.batch_state())
        .bind(input.completed_at_unix_ms)
        .bind(&input.snapshot_sha256)
        .bind(accepted_records)
        .bind(rejected_records)
        .bind(provenance_unavailable_records)
        .bind(&input.batch_id)
        .execute(&mut *transaction)
        .await?;

        let previous_entry_sha256 = sqlx::query_scalar::<_, String>(
            "SELECT entry_sha256 FROM tally_proof_ledger ORDER BY sequence DESC LIMIT 1",
        )
        .fetch_optional(&mut *transaction)
        .await?;
        let proof_id = Uuid::new_v4().to_string();
        // Proof creation is a local persistence event and cannot truthfully precede the run
        // completion it seals, even when the wall clock moved backwards during the run.
        let created_at_unix_ms = Utc::now()
            .timestamp_millis()
            .max(input.completed_at_unix_ms);
        let hash_input = ProofHashInput {
            proof_contract_version: input.proof_contract_version,
            previous_entry_sha256: previous_entry_sha256.as_deref(),
            proof_id: &proof_id,
            run_id: &run_id,
            batch_id: &input.batch_id,
            capability_snapshot_id: &capability_snapshot_id,
            company_id: &company_id,
            pack_id: &pack_id,
            outcome: input.outcome,
            verification: input.verification,
            started_at_unix_ms,
            completed_at_unix_ms: input.completed_at_unix_ms,
            accepted_records,
            rejected_records,
            provenance_unavailable_records: (input.proof_contract_version >= 2)
                .then_some(provenance_unavailable_records),
            record_counts_sha256: input.record_counts_sha256.as_deref(),
            snapshot_sha256: input.snapshot_sha256.as_deref(),
            checkpoint_before: checkpoint_before.as_deref(),
            checkpoint_after: input.checkpoint_after.as_deref(),
            gap_codes: &input.gap_codes,
            warning_codes: &input.warning_codes,
            created_at_unix_ms,
        };
        let proof_sha256 = sha256_json(&hash_input)?;
        let gap_codes_json = serde_json::to_string(&input.gap_codes)?;
        let warning_codes_json = serde_json::to_string(&input.warning_codes)?;

        sqlx::query(
            "INSERT INTO tally_proof_ledger(\
               id, proof_contract_version, previous_entry_sha256, entry_sha256, run_id, batch_id, \
               capability_snapshot_id, company_id, pack_id, outcome, verification_state, \
               started_at_unix_ms, completed_at_unix_ms, accepted_records, rejected_records, \
               provenance_unavailable_records, record_counts_sha256, snapshot_sha256, checkpoint_before, \
               checkpoint_after, gap_codes_json, warning_codes_json, created_at_unix_ms\
             ) VALUES (\
               ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, \
               ?17, ?18, ?19, ?20, ?21, ?22, ?23\
             )",
        )
        .bind(&proof_id)
        .bind(i64::from(input.proof_contract_version))
        .bind(&previous_entry_sha256)
        .bind(&proof_sha256)
        .bind(&run_id)
        .bind(&input.batch_id)
        .bind(&capability_snapshot_id)
        .bind(&company_id)
        .bind(&pack_id)
        .bind(input.outcome.as_str())
        .bind(input.verification.as_str())
        .bind(started_at_unix_ms)
        .bind(input.completed_at_unix_ms)
        .bind(accepted_records)
        .bind(rejected_records)
        .bind(provenance_unavailable_records)
        .bind(&input.record_counts_sha256)
        .bind(&input.snapshot_sha256)
        .bind(&checkpoint_before)
        .bind(&input.checkpoint_after)
        .bind(gap_codes_json)
        .bind(warning_codes_json)
        .bind(created_at_unix_ms)
        .execute(&mut *transaction)
        .await?;

        let checkpoint_advanced = input.verification == VerificationState::Verified;
        if checkpoint_advanced {
            sqlx::query(
                "INSERT INTO tally_checkpoints(\
                   company_id, pack_id, checkpoint_token, run_id, proof_id, snapshot_sha256, \
                   verified_at_unix_ms, freshness_target_seconds, generation\
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1) \
                 ON CONFLICT(company_id, pack_id) DO UPDATE SET \
                   checkpoint_token = excluded.checkpoint_token, run_id = excluded.run_id, \
                   proof_id = excluded.proof_id, snapshot_sha256 = excluded.snapshot_sha256, \
                   verified_at_unix_ms = excluded.verified_at_unix_ms, \
                   freshness_target_seconds = excluded.freshness_target_seconds, \
                   generation = tally_checkpoints.generation + 1",
            )
            .bind(&company_id)
            .bind(&pack_id)
            .bind(
                input
                    .checkpoint_after
                    .as_deref()
                    .expect("verified checkpoint"),
            )
            .bind(&run_id)
            .bind(&proof_id)
            .bind(
                input
                    .snapshot_sha256
                    .as_deref()
                    .expect("verified snapshot hash"),
            )
            .bind(input.completed_at_unix_ms)
            .bind(input.freshness_target_seconds)
            .execute(&mut *transaction)
            .await?;
        }

        transaction.commit().await?;
        Ok(CommitResult {
            proof_id,
            proof_sha256,
            checkpoint_advanced,
            facts: CommitReceiptFacts {
                proof_contract_version: input.proof_contract_version,
                run_id,
                batch_id: input.batch_id,
                capability_snapshot_id,
                company_id,
                pack_id,
                outcome: input.outcome,
                verification: input.verification,
                started_at_unix_ms,
                completed_at_unix_ms: input.completed_at_unix_ms,
                accepted_records,
                rejected_records,
                provenance_unavailable_records,
                record_counts_sha256: input.record_counts_sha256,
                snapshot_sha256: input.snapshot_sha256,
                checkpoint_before,
                checkpoint_after: input.checkpoint_after,
                gap_codes: input.gap_codes,
                warning_codes: input.warning_codes,
            },
        })
    }

    pub async fn batch_observation_counts(
        &self,
        batch_id: &str,
        run_id: &str,
    ) -> Result<ObservationCounts, MirrorError> {
        validate_nonempty(batch_id, 128, "batch_id")?;
        validate_nonempty(run_id, 128, "run_id")?;
        let row = sqlx::query(
            "SELECT \
               COUNT(DISTINCT batch.id) AS batch_count, \
               COALESCE(SUM(CASE WHEN observation.validation_status = 'accepted' THEN 1 ELSE 0 END), 0) \
                 AS accepted_records, \
               COALESCE(SUM(CASE WHEN observation.validation_status = 'rejected' THEN 1 ELSE 0 END), 0) \
                 AS rejected_records \
             FROM tally_observation_batches AS batch \
             LEFT JOIN tally_record_observations AS observation ON observation.batch_id = batch.id \
             WHERE batch.id = ?1 AND batch.run_id = ?2",
        )
        .bind(batch_id)
        .bind(run_id)
        .fetch_one(&self.pool)
        .await?;
        if row.try_get::<i64, _>("batch_count")? != 1 {
            return Err(MirrorError::NotFound);
        }
        Ok(ObservationCounts {
            accepted_records: row.try_get("accepted_records")?,
            rejected_records: row.try_get("rejected_records")?,
            provenance_unavailable_records: sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM tally_snapshot_window_memberships \
                 WHERE batch_id = ?1 AND provenance_state = 'unavailable'",
            )
            .bind(batch_id)
            .fetch_one(&self.pool)
            .await?,
        })
    }

    pub async fn freshness(
        &self,
        company_id: &str,
        pack_id: &str,
        now_unix_ms: i64,
    ) -> Result<FreshnessStatus, MirrorError> {
        let checkpoint = sqlx::query(
            "SELECT checkpoint_token, proof_id, verified_at_unix_ms, freshness_target_seconds \
             FROM tally_checkpoints WHERE company_id = ?1 AND pack_id = ?2",
        )
        .bind(company_id)
        .bind(pack_id)
        .fetch_optional(&self.pool)
        .await?;

        let Some(checkpoint) = checkpoint else {
            return Ok(FreshnessStatus {
                state: FreshnessState::NeverVerified,
                verified_at_unix_ms: None,
                age_seconds: None,
                checkpoint_token: None,
                proof_id: None,
            });
        };
        let verified_at_unix_ms: i64 = checkpoint.try_get("verified_at_unix_ms")?;
        let target_seconds: i64 = checkpoint.try_get("freshness_target_seconds")?;
        let clock_moved_backwards = now_unix_ms < verified_at_unix_ms;
        let age_seconds = now_unix_ms.saturating_sub(verified_at_unix_ms).max(0) / 1_000;
        Ok(FreshnessStatus {
            state: if !clock_moved_backwards && age_seconds <= target_seconds {
                FreshnessState::Fresh
            } else {
                FreshnessState::Stale
            },
            verified_at_unix_ms: Some(verified_at_unix_ms),
            age_seconds: Some(age_seconds),
            checkpoint_token: Some(checkpoint.try_get("checkpoint_token")?),
            proof_id: Some(checkpoint.try_get("proof_id")?),
        })
    }

    pub async fn commit_receipt_for_batch(
        &self,
        batch_id: &str,
        run_id: &str,
    ) -> Result<CommitResult, MirrorError> {
        self.proof_receipt_for_batch(batch_id, run_id, true).await
    }

    pub(crate) async fn historical_commit_receipt_for_batch(
        &self,
        batch_id: &str,
        run_id: &str,
    ) -> Result<CommitResult, MirrorError> {
        self.proof_receipt_for_batch(batch_id, run_id, false).await
    }

    async fn proof_receipt_for_batch(
        &self,
        batch_id: &str,
        run_id: &str,
        require_current_checkpoint: bool,
    ) -> Result<CommitResult, MirrorError> {
        validate_nonempty(batch_id, 128, "batch_id")?;
        validate_nonempty(run_id, 128, "run_id")?;
        let row = sqlx::query(
            "SELECT sequence, id, proof_contract_version, previous_entry_sha256, entry_sha256, \
               run_id, batch_id, capability_snapshot_id, company_id, pack_id, outcome, \
               verification_state, started_at_unix_ms, completed_at_unix_ms, accepted_records, \
               rejected_records, provenance_unavailable_records, record_counts_sha256, snapshot_sha256, \
               checkpoint_before, checkpoint_after, gap_codes_json, warning_codes_json, \
               created_at_unix_ms \
             FROM tally_proof_ledger WHERE batch_id = ?1 AND run_id = ?2",
        )
        .bind(batch_id)
        .bind(run_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(MirrorError::NotFound)?;

        let sequence: i64 = row.try_get("sequence")?;
        let proof_id: String = row.try_get("id")?;
        let proof_contract_version: i64 = row.try_get("proof_contract_version")?;
        let proof_contract_version = u16::try_from(proof_contract_version)
            .map_err(|_| MirrorError::InvalidInput("proof_contract_version"))?;
        let previous_entry_sha256: Option<String> = row.try_get("previous_entry_sha256")?;
        let entry_sha256: String = row.try_get("entry_sha256")?;
        let stored_run_id: String = row.try_get("run_id")?;
        let stored_batch_id: String = row.try_get("batch_id")?;
        let capability_snapshot_id: String = row.try_get("capability_snapshot_id")?;
        let company_id: String = row.try_get("company_id")?;
        let pack_id: String = row.try_get("pack_id")?;
        let outcome_text: String = row.try_get("outcome")?;
        let verification_text: String = row.try_get("verification_state")?;
        let started_at_unix_ms: i64 = row.try_get("started_at_unix_ms")?;
        let completed_at_unix_ms: i64 = row.try_get("completed_at_unix_ms")?;
        let accepted_records: i64 = row.try_get("accepted_records")?;
        let rejected_records: i64 = row.try_get("rejected_records")?;
        let provenance_unavailable_records: i64 = row.try_get("provenance_unavailable_records")?;
        let record_counts_sha256: Option<String> = row.try_get("record_counts_sha256")?;
        let snapshot_sha256: Option<String> = row.try_get("snapshot_sha256")?;
        let checkpoint_before: Option<String> = row.try_get("checkpoint_before")?;
        let checkpoint_after: Option<String> = row.try_get("checkpoint_after")?;
        let gap_codes_json: String = row.try_get("gap_codes_json")?;
        let warning_codes_json: String = row.try_get("warning_codes_json")?;
        let created_at_unix_ms: i64 = row.try_get("created_at_unix_ms")?;
        let gap_codes: Vec<String> = serde_json::from_str(&gap_codes_json)?;
        let warning_codes: Vec<String> = serde_json::from_str(&warning_codes_json)?;
        if gap_codes.len() > 32 || warning_codes.len() > 32 {
            return Err(MirrorError::VerificationInvariant);
        }
        for code in gap_codes.iter().chain(&warning_codes) {
            validate_safe_code(code).map_err(|_| MirrorError::VerificationInvariant)?;
        }
        let outcome = parse_run_outcome(&outcome_text)?;
        let verification = parse_verification_state(&verification_text)?;
        if (proof_contract_version >= 3) != record_counts_sha256.is_some()
            || record_counts_sha256
                .as_deref()
                .is_some_and(|digest| validate_sha256(digest).is_err())
        {
            return Err(MirrorError::VerificationInvariant);
        }

        let expected_previous = sqlx::query_scalar::<_, String>(
            "SELECT entry_sha256 FROM tally_proof_ledger WHERE sequence < ?1 \
             ORDER BY sequence DESC LIMIT 1",
        )
        .bind(sequence)
        .fetch_optional(&self.pool)
        .await?;
        if expected_previous != previous_entry_sha256 {
            return Err(MirrorError::VerificationInvariant);
        }
        let expected_hash = sha256_json(&ProofHashInput {
            proof_contract_version,
            previous_entry_sha256: previous_entry_sha256.as_deref(),
            proof_id: &proof_id,
            run_id: &stored_run_id,
            batch_id: &stored_batch_id,
            capability_snapshot_id: &capability_snapshot_id,
            company_id: &company_id,
            pack_id: &pack_id,
            outcome,
            verification,
            started_at_unix_ms,
            completed_at_unix_ms,
            accepted_records,
            rejected_records,
            provenance_unavailable_records: (proof_contract_version >= 2)
                .then_some(provenance_unavailable_records),
            record_counts_sha256: record_counts_sha256.as_deref(),
            snapshot_sha256: snapshot_sha256.as_deref(),
            checkpoint_before: checkpoint_before.as_deref(),
            checkpoint_after: checkpoint_after.as_deref(),
            gap_codes: &gap_codes,
            warning_codes: &warning_codes,
            created_at_unix_ms,
        })?;
        if stored_run_id != run_id
            || stored_batch_id != batch_id
            || validate_sha256(&entry_sha256).is_err()
            || expected_hash != entry_sha256
        {
            return Err(MirrorError::VerificationInvariant);
        }

        let checkpoint_advanced = if verification == VerificationState::Verified {
            let checkpoint_after = checkpoint_after
                .as_deref()
                .ok_or(MirrorError::VerificationInvariant)?;
            if require_current_checkpoint {
                let matches = sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(*) FROM tally_checkpoints WHERE proof_id = ?1 AND run_id = ?2 \
                     AND checkpoint_token = ?3",
                )
                .bind(&proof_id)
                .bind(run_id)
                .bind(checkpoint_after)
                .fetch_one(&self.pool)
                .await?;
                if matches != 1 {
                    return Err(MirrorError::VerificationInvariant);
                }
            }
            true
        } else {
            if checkpoint_after.is_some() {
                return Err(MirrorError::VerificationInvariant);
            }
            false
        };
        Ok(CommitResult {
            proof_id,
            proof_sha256: entry_sha256,
            checkpoint_advanced,
            facts: CommitReceiptFacts {
                proof_contract_version,
                run_id: stored_run_id,
                batch_id: stored_batch_id,
                capability_snapshot_id,
                company_id,
                pack_id,
                outcome,
                verification,
                started_at_unix_ms,
                completed_at_unix_ms,
                accepted_records,
                rejected_records,
                provenance_unavailable_records,
                record_counts_sha256,
                snapshot_sha256,
                checkpoint_before,
                checkpoint_after,
                gap_codes,
                warning_codes,
            },
        })
    }

    /// Returns immutable, hash-addressed proof summaries newest first. The payload deliberately
    /// excludes source data and display names so operator diagnostics cannot become a data leak.
    /// The run ID remains available as a safe local support correlation token.
    pub async fn latest_proofs(
        &self,
        company_id: &str,
        limit: u32,
    ) -> Result<Vec<ProofSummary>, MirrorError> {
        if company_id.trim().is_empty() || limit == 0 || limit > 100 {
            return Err(MirrorError::InvalidInput("proof_query"));
        }
        let rows = sqlx::query(
            "SELECT run_id, batch_id FROM tally_proof_ledger \
             WHERE company_id = ?1 ORDER BY sequence DESC LIMIT ?2",
        )
        .bind(company_id)
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await?;

        let mut summaries = Vec::with_capacity(rows.len());
        for row in rows {
            let run_id: String = row.try_get("run_id")?;
            let batch_id: String = row.try_get("batch_id")?;
            let receipt = self
                .historical_commit_receipt_for_batch(&batch_id, &run_id)
                .await?;
            if receipt.facts.company_id != company_id {
                return Err(MirrorError::VerificationInvariant);
            }
            let facts = receipt.facts;
            summaries.push(ProofSummary {
                integrity_state: "entry_hash_valid",
                run_id: facts.run_id.clone(),
                selection_token: receipt.proof_id,
                proof_sha256: receipt.proof_sha256,
                pack_id: facts.pack_id,
                outcome: facts.outcome.as_str().to_string(),
                verification_state: facts.verification.as_str().to_string(),
                started_at_unix_ms: facts.started_at_unix_ms,
                completed_at_unix_ms: Some(facts.completed_at_unix_ms),
                accepted_records: facts.accepted_records,
                rejected_records: facts.rejected_records,
                provenance_unavailable_records: facts.provenance_unavailable_records,
                gap_codes: facts.gap_codes,
                warning_codes: facts.warning_codes,
            });
        }
        Ok(summaries)
    }

    /// Builds an allow-list-only support artifact after revalidating the local
    /// proof chain and restart-safe snapshot receipt. It deliberately omits
    /// names, source identities, internal IDs, endpoints, checkpoints, source
    /// records, amounts, payloads, and drill-down hashes.
    pub async fn redacted_proof_export(
        &self,
        company_id: &str,
        proof_id: &str,
        exported_at_unix_ms: i64,
    ) -> Result<RedactedProofExport, MirrorError> {
        validate_nonempty(company_id, 128, "company_id")?;
        validate_nonempty(proof_id, 128, "proof_id")?;
        let selected = sqlx::query(
            "SELECT sequence, run_id, batch_id FROM tally_proof_ledger \
             WHERE id = ?1 AND company_id = ?2",
        )
        .bind(proof_id)
        .bind(company_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(MirrorError::NotFound)?;
        let selected_sequence: i64 = selected.try_get("sequence")?;
        let run_id: String = selected.try_get("run_id")?;
        let batch_id: String = selected.try_get("batch_id")?;

        let mut last_sequence = 0_i64;
        while last_sequence < selected_sequence {
            let chain_rows = sqlx::query(
                "SELECT sequence, run_id, batch_id FROM tally_proof_ledger \
                 WHERE sequence > ?1 AND sequence <= ?2 ORDER BY sequence ASC LIMIT 250",
            )
            .bind(last_sequence)
            .bind(selected_sequence)
            .fetch_all(&self.pool)
            .await?;
            if chain_rows.is_empty() {
                return Err(MirrorError::VerificationInvariant);
            }
            for row in chain_rows {
                let sequence: i64 = row.try_get("sequence")?;
                let chain_run_id: String = row.try_get("run_id")?;
                let chain_batch_id: String = row.try_get("batch_id")?;
                self.historical_commit_receipt_for_batch(&chain_batch_id, &chain_run_id)
                    .await?;
                last_sequence = sequence;
            }
        }

        let receipt = self
            .historical_commit_receipt_for_batch(&batch_id, &run_id)
            .await?;
        if receipt.proof_id != proof_id || receipt.facts.company_id != company_id {
            return Err(MirrorError::VerificationInvariant);
        }
        let counts = self.batch_observation_counts(&batch_id, &run_id).await?;
        if counts.accepted_records != receipt.facts.accepted_records
            || counts.rejected_records != receipt.facts.rejected_records
            || counts.provenance_unavailable_records != receipt.facts.provenance_unavailable_records
        {
            return Err(MirrorError::VerificationInvariant);
        }

        let state = crate::sync::snapshot::SqliteSnapshotStateStore::new(self.pool.clone())
            .load_by_run_id(&run_id)
            .await
            .map_err(|_| MirrorError::VerificationInvariant)?
            .ok_or(MirrorError::VerificationInvariant)?;
        let stored_receipt = state
            .commit_receipt
            .as_ref()
            .ok_or(MirrorError::VerificationInvariant)?;
        let proof = state
            .proof
            .as_ref()
            .ok_or(MirrorError::VerificationInvariant)?;
        let plan = state
            .plan
            .as_ref()
            .ok_or(MirrorError::VerificationInvariant)?;
        let mut proof_gap_codes = proof
            .gaps
            .iter()
            .map(|gap| gap.safe_reason_code.clone())
            .collect::<Vec<_>>();
        proof_gap_codes.sort();
        proof_gap_codes.dedup();
        let mut receipt_gap_codes = receipt.facts.gap_codes.clone();
        receipt_gap_codes.sort();
        receipt_gap_codes.dedup();
        if state.batch_id.as_deref() != Some(batch_id.as_str())
            || stored_receipt.proof_id.as_deref() != Some(proof_id)
            || stored_receipt.proof_sha256.as_deref() != Some(receipt.proof_sha256.as_str())
            || stored_receipt.checkpoint_advanced != receipt.checkpoint_advanced
            || proof.run_id != run_id
            || proof.proof_contract_version != receipt.facts.proof_contract_version
            || proof.started_at_unix_ms != receipt.facts.started_at_unix_ms
            || proof.completed_at_unix_ms != Some(receipt.facts.completed_at_unix_ms)
            || plan.pack != proof.pack
            || plan.pack_schema_version != proof.pack_schema_version
            || proof.snapshot_sha256 != receipt.facts.snapshot_sha256
            || !proof_outcome_matches(proof.outcome, receipt.facts.outcome)
            || !proof_verification_matches(proof.verification, receipt.facts.verification)
            || proof_gap_codes != receipt_gap_codes
            || crate::sync::snapshot::pack_code(proof.pack) != receipt.facts.pack_id
        {
            return Err(MirrorError::VerificationInvariant);
        }

        let freshness = self
            .freshness(company_id, &receipt.facts.pack_id, exported_at_unix_ms)
            .await?;
        for code in &receipt.facts.gap_codes {
            validate_export_code(code)?;
        }
        for code in &receipt.facts.warning_codes {
            validate_export_warning_code(code)?;
        }
        let freshness_state = match freshness.state {
            FreshnessState::Fresh => "fresh",
            FreshnessState::Stale => "stale",
            FreshnessState::NeverVerified => "never_verified",
        };
        let payload = RedactedProofPayload {
            schema: "bridge.tally.redacted-proof-of-sync",
            schema_version: 1,
            exported_at_unix_ms,
            redaction_profile: "public_support_v1",
            subject: RedactedSubject {
                reference: "company-1",
                identity_disclosed: false,
            },
            proofs: vec![RedactedProofEntry {
                entry_index: 1,
                proof_contract_version: receipt.facts.proof_contract_version,
                pack_id: receipt.facts.pack_id.clone(),
                pack_schema_version: plan.pack_schema_version,
                outcome: receipt.facts.outcome.as_str().to_string(),
                verification_state: receipt.facts.verification.as_str().to_string(),
                started_at_unix_ms: receipt.facts.started_at_unix_ms,
                completed_at_unix_ms: receipt.facts.completed_at_unix_ms,
                counts: RedactedCounts {
                    provenance_backed_accepted_records: receipt.facts.accepted_records,
                    provenance_unavailable_records: receipt.facts.provenance_unavailable_records,
                    rejected_records: receipt.facts.rejected_records,
                },
                gaps: receipt.facts.gap_codes.clone(),
                warnings: receipt.facts.warning_codes.clone(),
                local_ledger: RedactedLedgerEvidence {
                    chain_validation: "valid_at_export",
                },
            }],
            current_status: RedactedCurrentStatus {
                freshness_state,
                verified_at_unix_ms: freshness.verified_at_unix_ms,
                checkpoint_present: freshness.checkpoint_token.is_some(),
            },
        };
        finish_redacted_export(payload)
    }

    /// Returns local-session aliases for bounded mismatch drill-down. Stored
    /// source-derived tokens never cross this API and are not included in the
    /// public support export.
    pub async fn local_reconciliation_mismatches(
        &self,
        company_id: &str,
        proof_id: &str,
        now_unix_ms: i64,
    ) -> Result<Vec<LocalReconciliationMismatch>, MirrorError> {
        self.redacted_proof_export(company_id, proof_id, now_unix_ms)
            .await?;
        let run_id = sqlx::query_scalar::<_, String>(
            "SELECT run_id FROM tally_proof_ledger WHERE id = ?1 AND company_id = ?2",
        )
        .bind(proof_id)
        .bind(company_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(MirrorError::NotFound)?;
        let state = crate::sync::snapshot::SqliteSnapshotStateStore::new(self.pool.clone())
            .load_by_run_id(&run_id)
            .await
            .map_err(|_| MirrorError::VerificationInvariant)?
            .ok_or(MirrorError::VerificationInvariant)?;
        let mut internal = state
            .windows
            .values()
            .filter_map(|window| window.evidence.as_ref())
            .flat_map(|evidence| evidence.mismatches.iter())
            .map(|mismatch| {
                (
                    mismatch.safe_reason_code.clone(),
                    mismatch.safe_record_ids.clone(),
                )
            })
            .collect::<Vec<_>>();
        internal.sort();
        internal.dedup();
        internal.truncate(32);
        let mut aliases = BTreeMap::<String, String>::new();
        let mut result = Vec::with_capacity(internal.len());
        for (reason_code, tokens) in internal {
            validate_safe_code(&reason_code)?;
            let mut record_aliases = Vec::new();
            for token in tokens.into_iter().take(20) {
                let next = aliases.len() + 1;
                let alias = aliases
                    .entry(token)
                    .or_insert_with(|| format!("local-record-{next:04}"))
                    .clone();
                record_aliases.push(alias);
            }
            record_aliases.sort();
            record_aliases.dedup();
            result.push(LocalReconciliationMismatch {
                reason_code,
                record_aliases,
            });
        }
        Ok(result)
    }
}

fn persisted_company_profile_from_row(
    row: SqliteRow,
) -> Result<PersistedCompanyProfile, sqlx::Error> {
    let canonical_endpoint: String = row.try_get("canonical_origin")?;
    let company_guid: String = row.try_get("company_guid")?;
    let company_number: String = row.try_get("company_number")?;
    let books_from_yyyymmdd: String = row.try_get("books_from_yyyymmdd")?;
    let name: String = row.try_get("display_name")?;
    Ok(PersistedCompanyProfile {
        name: name.clone(),
        guid: company_guid.clone(),
        company_number: company_number.clone(),
        books_from_yyyymmdd: books_from_yyyymmdd.clone(),
        guid_observed: true,
        mirror_company_id: row.try_get("id")?,
        correlation_key: company_profile_correlation_key(
            &canonical_endpoint,
            &company_guid,
            &company_number,
            &name,
            &books_from_yyyymmdd,
        ),
        identity_confidence: row.try_get("identity_confidence")?,
        canonical_endpoint,
        last_observed_at_unix_ms: row.try_get("last_observed_at_unix_ms")?,
    })
}

fn client_group_label_migration_profile_from_row(
    row: SqliteRow,
) -> Result<ClientGroupLabelMigrationProfile, sqlx::Error> {
    let canonical_endpoint: String = row.try_get("canonical_origin")?;
    let guid: String = row.try_get("company_guid")?;
    let company_number: Option<String> = row.try_get("company_number")?;
    let books_from_yyyymmdd: Option<String> = row.try_get("books_from_yyyymmdd")?;
    let name: String = row.try_get("display_name")?;
    let correlation_key = company_number
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .zip(
            books_from_yyyymmdd
                .as_deref()
                .filter(|value| !value.trim().is_empty()),
        )
        .map(|(company_number, books_from_yyyymmdd)| {
            company_profile_correlation_key(
                &canonical_endpoint,
                &guid,
                company_number,
                &name,
                books_from_yyyymmdd,
            )
        });
    Ok(ClientGroupLabelMigrationProfile {
        guid,
        correlation_key,
        identity_confidence: row.try_get("identity_confidence")?,
    })
}

/// Returns an opaque, stable join key for the same observed company at the same endpoint.
/// The key deliberately does not expose the locally persisted Tally GUID.
pub(crate) fn company_profile_correlation_key(
    canonical_origin: &str,
    company_guid: &str,
    company_number: &str,
    display_name: &str,
    books_from_yyyymmdd: &str,
) -> String {
    let mut digest = Sha256::new();
    digest.update(b"bridge.tally.company-profile-correlation/2\0");
    digest.update(canonical_origin.as_bytes());
    digest.update(b"\0");
    digest.update(company_guid.to_ascii_lowercase().as_bytes());
    digest.update(b"\0");
    digest.update(company_number.as_bytes());
    digest.update(b"\0");
    digest.update(display_name.as_bytes());
    digest.update(b"\0");
    digest.update(books_from_yyyymmdd.as_bytes());
    hex_digest(digest.finalize())
}

fn finish_redacted_export(
    payload: RedactedProofPayload,
) -> Result<RedactedProofExport, MirrorError> {
    let payload_bytes = serde_json::to_vec(&payload)?;
    let mut digest = Sha256::new();
    digest.update(b"bridge-tally-redacted-proof-v1\0");
    digest.update((payload_bytes.len() as u64).to_be_bytes());
    digest.update(&payload_bytes);
    let payload_sha256 = hex_digest(digest.finalize());
    let document = RedactedProofDocument {
        payload,
        integrity: RedactedIntegrity {
            canonicalization: "serde-struct-order-v1",
            hash_algorithm: "sha-256",
            domain: "bridge-tally-redacted-proof-v1",
            payload_sha256: payload_sha256.clone(),
            signature: None,
            integrity_claim: "checksum_only",
            authenticity_claim: "none",
        },
    };
    let json = serde_json::to_string_pretty(&document)?;
    if json.len() > 256 * 1024 {
        return Err(MirrorError::InvalidInput("proof_export_too_large"));
    }
    Ok(RedactedProofExport {
        json,
        payload_sha256,
    })
}

fn proof_outcome_matches(proof: bridge_tally_core::RunOutcome, receipt: RunOutcome) -> bool {
    matches!(
        (proof, receipt),
        (
            bridge_tally_core::RunOutcome::Completed,
            RunOutcome::Completed
        ) | (bridge_tally_core::RunOutcome::Failed, RunOutcome::Failed)
            | (
                bridge_tally_core::RunOutcome::Cancelled,
                RunOutcome::Cancelled
            )
            | (
                bridge_tally_core::RunOutcome::OutcomeUnknown,
                RunOutcome::OutcomeUnknown
            )
    )
}

fn proof_verification_matches(
    proof: bridge_tally_core::VerificationState,
    receipt: VerificationState,
) -> bool {
    matches!(
        (proof, receipt),
        (
            bridge_tally_core::VerificationState::Verified,
            VerificationState::Verified
        ) | (
            bridge_tally_core::VerificationState::Partial,
            VerificationState::Partial
        ) | (
            bridge_tally_core::VerificationState::Unverified,
            VerificationState::Unverified
        )
    )
}

fn validate_export_code(value: &str) -> Result<(), MirrorError> {
    let allowed = REVIEWED_TALLY_TERMINAL_CODES.contains(&value)
        || matches!(
            value,
            "accept_dedupe_count_mismatch"
                | "accounting_reconciliation_unavailable"
                | "capability_not_supported"
                | "capability_not_verified"
                | "capability_profile_changed"
                | "capability_profile_drift_check_unavailable"
                | "capability_profile_changed_during_run"
                | "company_identity_ambiguous"
                | "company_identity_display_scope_ambiguous"
                | "company_identity_not_found"
                | "complete_source_count_disagreement"
                | "duplicate_record_across_windows"
                | "duplicate_source_identity"
                | "education_report_family_unsupported"
                | "foreign_master_text_rendering_degraded"
                | "missing_snapshot_window"
                | "missing_source_identity"
                | "parse_accept_count_mismatch"
                | "record_evidence_mismatch"
                | "record_provenance_unavailable"
                | "reconciliation_mismatch"
                | "rejected_snapshot_records"
                | "report_tie_out_unavailable"
                | "report_tie_out_mismatch"
                | "report_tie_out_evidence_invalid"
                | "period_report_profile_unobserved"
                | "response_date_outside_window"
                | "response_pack_mismatch"
                | "response_parse_failed"
                | "response_validation_failed"
                | "run_cancelled"
                | "source_accepted_count_mismatch"
                | "source_changed_during_resume"
                | "source_changed_during_snapshot"
                | "source_count_evidence_invalid"
                | "source_count_scope_mismatch"
                | "source_count_unavailable"
                | "source_count_window_only"
                | "source_cut_consistency_unavailable"
                | "source_cut_atomicity_unavailable"
                | "source_changed_during_run"
                | "source_outcome_unknown"
                | "tally_protocol_failed"
                | "tally_unreachable"
                | "typed_pack_validation_failed"
                | "voucher_entry_applicability_unavailable"
                | "voucher_entry_polarity_unavailable"
                | "voucher_header_entry_total_unavailable"
                | "window_source_accepted_count_mismatch"
        );
    if allowed {
        Ok(())
    } else {
        Err(MirrorError::VerificationInvariant)
    }
}

fn validate_export_warning_code(value: &str) -> Result<(), MirrorError> {
    let warning = crate::warning_codes::WarningCode::parse(value)
        .ok_or(MirrorError::VerificationInvariant)?;
    match warning {
        crate::warning_codes::WarningCode::AdaptiveWindowSplit
        | crate::warning_codes::WarningCode::ForeignMasterTextRenderingDegraded
        | crate::warning_codes::WarningCode::NativeOutstandingsAsOfUnconfirmedWithoutEffectiveDateEvidence
        | crate::warning_codes::WarningCode::NativeOutstandingsAsOfUnconfirmedWithoutBillReferences => Ok(()),
    }
}

#[derive(Debug)]
struct IdentityRow {
    id: String,
    guid: Option<String>,
    remote_id: Option<String>,
    master_id: Option<String>,
    fallback_fingerprint: Option<String>,
}

async fn find_identity_matches(
    transaction: &mut Transaction<'_, Sqlite>,
    table: &'static str,
    owner_column: &'static str,
    owner_id: &str,
    object_type: Option<&str>,
    identity: &SourceIdentityInput,
) -> Result<Vec<IdentityRow>, MirrorError> {
    let rows = match (table, owner_column) {
        ("tally_companies", "endpoint_id") => {
            sqlx::query(
                "SELECT id, company_guid AS guid, remote_id, master_id, fallback_fingerprint \
             FROM tally_companies WHERE endpoint_id = ?1 AND (\
               (?2 IS NOT NULL AND company_guid = ?2 COLLATE NOCASE) OR \
               (?3 IS NOT NULL AND remote_id = ?3) OR \
               (?4 IS NOT NULL AND master_id = ?4) OR \
               (?5 IS NOT NULL AND fallback_fingerprint = ?5)\
             )",
            )
            .bind(owner_id)
            .bind(identity.guid.as_deref())
            .bind(identity.remote_id.as_deref())
            .bind(identity.master_id.as_deref())
            .bind(identity.fallback_fingerprint.as_deref())
            .fetch_all(&mut **transaction)
            .await?
        }
        ("tally_source_records", "company_id") => {
            sqlx::query(
                "SELECT id, source_guid AS guid, remote_id, master_id, fallback_fingerprint \
             FROM tally_source_records WHERE company_id = ?1 AND object_type = ?2 AND (\
               (?3 IS NOT NULL AND source_guid = ?3) OR \
               (?4 IS NOT NULL AND remote_id = ?4) OR \
               (?5 IS NOT NULL AND master_id = ?5) OR \
               (?6 IS NOT NULL AND fallback_fingerprint = ?6)\
             )",
            )
            .bind(owner_id)
            .bind(object_type.ok_or(MirrorError::InvalidInput("object_type"))?)
            .bind(identity.guid.as_deref())
            .bind(identity.remote_id.as_deref())
            .bind(identity.master_id.as_deref())
            .bind(identity.fallback_fingerprint.as_deref())
            .fetch_all(&mut **transaction)
            .await?
        }
        _ => return Err(MirrorError::InvalidInput("identity_query_scope")),
    };

    rows.into_iter()
        .map(|row| {
            Ok(IdentityRow {
                id: row.try_get("id")?,
                guid: row.try_get("guid")?,
                remote_id: row.try_get("remote_id")?,
                master_id: row.try_get("master_id")?,
                fallback_fingerprint: row.try_get("fallback_fingerprint")?,
            })
        })
        .collect()
}

fn unique_match(mut matches: Vec<IdentityRow>) -> Result<Option<IdentityRow>, MirrorError> {
    matches.sort_by(|left, right| left.id.cmp(&right.id));
    matches.dedup_by(|left, right| left.id == right.id);
    if matches.len() > 1 {
        return Err(MirrorError::IdentityCollision);
    }
    Ok(matches.pop())
}

fn ensure_no_silent_identity_change(
    existing: &IdentityRow,
    incoming: &SourceIdentityInput,
) -> Result<(), MirrorError> {
    match (existing.guid.as_deref(), incoming.guid.as_deref()) {
        (Some(left), Some(right)) if !left.eq_ignore_ascii_case(right) => {
            return Err(MirrorError::IdentityCollision);
        }
        (None, Some(_)) => return Err(MirrorError::IdentityUpgradeRequiresAudit),
        _ => {}
    }
    for (stored, observed) in [
        (&existing.remote_id, &incoming.remote_id),
        (&existing.master_id, &incoming.master_id),
        (
            &existing.fallback_fingerprint,
            &incoming.fallback_fingerprint,
        ),
    ] {
        match (stored.as_deref(), observed.as_deref()) {
            (Some(left), Some(right)) if left != right => {
                return Err(MirrorError::IdentityCollision);
            }
            (None, Some(_)) => return Err(MirrorError::IdentityUpgradeRequiresAudit),
            _ => {}
        }
    }
    Ok(())
}

fn identity_confidence(identity: &SourceIdentityInput) -> Confidence {
    identity.confidence.unwrap_or(
        if identity.fallback_fingerprint.is_some()
            && identity.guid.is_none()
            && identity.remote_id.is_none()
            && identity.master_id.is_none()
        {
            Confidence::Inferred
        } else {
            Confidence::Observed
        },
    )
}

/// Generic identity matching lacks the complete Company-collection tuple, so
/// it cannot create or promote an observed company pin.
fn generic_company_confidence(identity: &SourceIdentityInput) -> Confidence {
    match identity_confidence(identity) {
        Confidence::Observed => Confidence::Documented,
        confidence => confidence,
    }
}

fn license_tier_key(tier: bridge_tally_core::LicenseTier) -> &'static str {
    match tier {
        bridge_tally_core::LicenseTier::Silver => "silver",
        bridge_tally_core::LicenseTier::Gold => "gold",
    }
}

fn validate_capability_snapshot(input: &CapabilitySnapshotInput) -> Result<(), MirrorError> {
    validate_nonempty(&input.canonical_origin, 512, "canonical_origin")?;
    validate_nonempty(&input.product, 128, "product")?;
    if input.profile_version == 0 {
        return Err(MirrorError::InvalidInput("profile_version"));
    }
    validate_optional_text(input.release.as_deref(), 128, "release")?;
    validate_optional_text(input.mode.as_deref(), 64, "mode")?;
    for item in &input.items {
        validate_safe_code(&item.key)?;
        validate_optional_safe_code(item.safe_reason_code.as_deref())?;
    }
    Ok(())
}

fn reviewed_setup_payload_sha256(input: &ReviewedSetupInput) -> Result<String, MirrorError> {
    let mut items = input
        .capability
        .items
        .iter()
        .map(|item| {
            serde_json::json!({
                "kind": item.kind.as_str(),
                "key": item.key,
                "state": item.state.as_str(),
                "confidence": item.confidence.as_str(),
                "safe_reason_code": item.safe_reason_code,
            })
        })
        .collect::<Vec<_>>();
    items.sort_by(|left, right| {
        left["kind"]
            .as_str()
            .unwrap_or_default()
            .cmp(right["kind"].as_str().unwrap_or_default())
            .then_with(|| {
                left["key"]
                    .as_str()
                    .unwrap_or_default()
                    .cmp(right["key"].as_str().unwrap_or_default())
            })
    });
    let selected_read_scope = input.selected_read_scope.as_ref().map(|scope| {
        let mut observations = scope
            .observations
            .iter()
            .map(|observation| {
                serde_json::json!({
                    "capability_key": observation.capability_key,
                    "state": observation.state.as_str(),
                    "confidence": observation.confidence.as_str(),
                    "safe_reason_code": observation.safe_reason_code,
                    "result_bucket": observation.result_bucket,
                    "request_sha256": observation.request_sha256,
                    "decoded_response_sha256": observation.decoded_response_sha256,
                    "response_encoding": observation.response_encoding,
                    "company_context_verified": observation.company_context_verified,
                    "schema_verified": observation.schema_verified,
                    "record_count_verified": observation.record_count_verified,
                    "identity_evidence_state": observation.identity_evidence_state,
                    "date_window_verified": observation.date_window_verified,
                })
            })
            .collect::<Vec<_>>();
        observations.sort_by(|left, right| {
            left["capability_key"]
                .as_str()
                .unwrap_or_default()
                .cmp(right["capability_key"].as_str().unwrap_or_default())
        });
        serde_json::json!({
            "scope_commitment_sha256": scope.scope_commitment_sha256,
            "parent_review_sha256": scope.parent_review_sha256,
            "ledger_profile_id": scope.ledger_profile_id,
            "voucher_profile_id": scope.voucher_profile_id,
            "voucher_from_yyyymmdd": scope.voucher_from_yyyymmdd,
            "voucher_to_yyyymmdd": scope.voucher_to_yyyymmdd,
            "observed_at_unix_ms": scope.observed_at_unix_ms,
            "observations": observations,
        })
    });
    let mut payload = serde_json::json!({
        "schema": "bridge.tally.reviewed-setup-payload/2",
        "capability": {
            "canonical_origin": input.capability.canonical_origin,
            "observed_at_unix_ms": input.capability.observed_at_unix_ms,
            "profile_version": input.capability.profile_version,
            "product": input.capability.product,
            "release": input.capability.release,
            "mode": input.capability.mode,
            "mode_confidence": input.capability.mode_confidence.as_str(),
            "items": items,
        },
        "company": {
            "display_name": input.company_display_name,
            "guid": input.company_identity.guid,
            "company_number": input.company_number,
            "books_from_yyyymmdd": input.books_from_yyyymmdd,
            "remote_id": input.company_identity.remote_id,
            "master_id": input.company_identity.master_id,
            "fallback_fingerprint": input.company_identity.fallback_fingerprint,
            "confidence": input.company_identity.confidence.map(Confidence::as_str),
        },
        "selected_read_scope": selected_read_scope,
    });
    // Missing historical observations retain their original commitment bytes.
    if let Some(tier) = input.capability.license_tier {
        payload["capability"]["license_tier"] = serde_json::json!(license_tier_key(tier));
    }
    let canonical = canonical_json(&payload)?;
    Ok(hex_digest(Sha256::digest(canonical.as_bytes())))
}

fn validate_selected_read_scope(
    scope: Option<&SelectedReadScopeInput>,
    capability: &CapabilitySnapshotInput,
    company_display_name: &str,
    company_guid: Option<&str>,
    company_number: &str,
    books_from_yyyymmdd: &str,
) -> Result<(), MirrorError> {
    let selected_items = capability
        .items
        .iter()
        .filter(|item| {
            item.kind == CapabilityKind::Feature
                && matches!(
                    item.key.as_str(),
                    "selected_ledger_read" | "selected_voucher_window_read"
                )
        })
        .collect::<Vec<_>>();
    let broad_reads_unknown = capability.items.iter().all(|item| {
        item.kind != CapabilityKind::Feature
            || !matches!(item.key.as_str(), "ledger_read" | "voucher_read")
            || item.state == CapabilityState::Unknown
    });
    if !broad_reads_unknown {
        return Err(MirrorError::InvalidInput("broad_read_claim_not_allowed"));
    }
    let Some(scope) = scope else {
        if selected_items.iter().any(|item| {
            item.state == CapabilityState::Supported || item.confidence == Confidence::Observed
        }) {
            return Err(MirrorError::InvalidInput("selected_read_scope_missing"));
        }
        return Ok(());
    };
    if capability.profile_version < 3 || scope.observations.len() != 2 {
        return Err(MirrorError::InvalidInput("selected_read_scope_shape"));
    }
    validate_sha256(&scope.scope_commitment_sha256)?;
    validate_sha256(&scope.parent_review_sha256)?;
    if scope.ledger_profile_id != "bridge.tally.ledgers/1"
        || scope.voucher_profile_id != "bridge.tally.vouchers/3"
    {
        return Err(MirrorError::InvalidInput("selected_read_profile"));
    }
    let from = TallyDate::parse(scope.voucher_from_yyyymmdd.clone())
        .map_err(|_| MirrorError::InvalidInput("selected_read_date_range"))?;
    let to = TallyDate::parse(scope.voucher_to_yyyymmdd.clone())
        .map_err(|_| MirrorError::InvalidInput("selected_read_date_range"))?;
    if from.as_str() > to.as_str() || scope.observed_at_unix_ms != capability.observed_at_unix_ms {
        return Err(MirrorError::InvalidInput("selected_read_date_range"));
    }
    let mut observed_keys = std::collections::BTreeSet::new();
    for observation in &scope.observations {
        if !matches!(
            observation.capability_key.as_str(),
            "selected_ledger_read" | "selected_voucher_window_read"
        ) || !observed_keys.insert(observation.capability_key.as_str())
        {
            return Err(MirrorError::InvalidInput("selected_read_observation_key"));
        }
        validate_safe_code(&observation.safe_reason_code)?;
        validate_safe_code(&observation.result_bucket)?;
        validate_optional_sha256(observation.request_sha256.as_deref())?;
        validate_optional_sha256(observation.decoded_response_sha256.as_deref())?;
        if let Some(encoding) = observation.response_encoding.as_deref() {
            if !matches!(
                encoding,
                "utf8" | "utf8_bom" | "utf16le" | "utf16le_bom" | "utf16be_bom"
            ) {
                return Err(MirrorError::InvalidInput("selected_read_response_encoding"));
            }
        }
        let matching_item = selected_items
            .iter()
            .find(|item| item.key == observation.capability_key)
            .ok_or(MirrorError::InvalidInput("selected_read_feature_missing"))?;
        if matching_item.state != observation.state
            || matching_item.confidence != observation.confidence
            || matching_item.safe_reason_code.as_deref()
                != Some(observation.safe_reason_code.as_str())
        {
            return Err(MirrorError::InvalidInput("selected_read_feature_mismatch"));
        }
        match observation.state {
            CapabilityState::Supported
                if observation.confidence == Confidence::Observed
                    && observation.request_sha256.is_some()
                    && observation.decoded_response_sha256.is_some()
                    && observation.response_encoding.is_some()
                    && observation.company_context_verified
                    && observation.schema_verified
                    && observation.record_count_verified
                    && ((observation.result_bucket == "empty_observed"
                        && observation.identity_evidence_state == "not_applicable_empty")
                        || (observation.result_bucket == "non_empty_observed"
                            && observation.identity_evidence_state == "verified"))
                    && ((observation.capability_key == "selected_ledger_read"
                        && !observation.date_window_verified)
                        || (observation.capability_key == "selected_voucher_window_read"
                            && observation.date_window_verified)) => {}
            CapabilityState::Unknown
                if matches!(
                    observation.confidence,
                    Confidence::Observed | Confidence::Unknown
                ) && observation.request_sha256.is_none()
                    && observation.decoded_response_sha256.is_none()
                    && observation.response_encoding.is_none()
                    && !observation.company_context_verified
                    && !observation.schema_verified
                    && !observation.record_count_verified
                    && observation.identity_evidence_state == "unverified"
                    && !observation.date_window_verified
                    && ((observation.result_bucket == "rejected"
                        && observation.confidence == Confidence::Observed)
                        || (observation.result_bucket == "skipped"
                            && observation.confidence == Confidence::Unknown)) => {}
            _ => return Err(MirrorError::InvalidInput("selected_read_observation_shape")),
        }
    }
    if observed_keys.len() != 2 || selected_items.len() != 2 {
        return Err(MirrorError::InvalidInput(
            "selected_read_observation_cardinality",
        ));
    }
    if scope.observations[0].capability_key != "selected_ledger_read"
        || scope.observations[1].capability_key != "selected_voucher_window_read"
    {
        return Err(MirrorError::InvalidInput("selected_read_observation_order"));
    }
    let company_guid = company_guid.ok_or(MirrorError::InvalidInput(
        "selected_read_company_guid_missing",
    ))?;
    validate_company_guid(company_guid)?;
    validate_company_number(company_number)?;
    validate_books_from_yyyymmdd(books_from_yyyymmdd)?;
    if scope.company_number != company_number || scope.books_from_yyyymmdd != books_from_yyyymmdd {
        return Err(MirrorError::InvalidInput(
            "selected_read_company_identity_mismatch",
        ));
    }
    let material = SelectedReadScopeCommitmentMaterial {
        parent_review_commitment_sha256: scope.parent_review_sha256.clone(),
        canonical_origin: capability.canonical_origin.clone(),
        company_guid_ascii_casefolded: company_guid.to_ascii_lowercase(),
        company_name: company_display_name.to_string(),
        company_number: company_number.to_string(),
        books_from_yyyymmdd: books_from_yyyymmdd.to_string(),
        ledger_profile_id: scope.ledger_profile_id.clone(),
        voucher_profile_id: scope.voucher_profile_id.clone(),
        voucher_from_yyyymmdd: scope.voucher_from_yyyymmdd.clone(),
        voucher_to_yyyymmdd: scope.voucher_to_yyyymmdd.clone(),
        observed_at_unix_ms: scope.observed_at_unix_ms,
        observations: scope
            .observations
            .iter()
            .map(|observation| SelectedReadObservationCommitmentMaterial {
                capability_key: observation.capability_key.clone(),
                state: observation.state.as_str().to_string(),
                confidence: observation.confidence.as_str().to_string(),
                safe_reason_code: observation.safe_reason_code.clone(),
                result_bucket: observation.result_bucket.clone(),
                request_sha256: observation.request_sha256.clone(),
                decoded_response_sha256: observation.decoded_response_sha256.clone(),
                response_encoding: observation.response_encoding.clone(),
                company_context_verified: observation.company_context_verified,
                schema_verified: observation.schema_verified,
                record_count_verified: observation.record_count_verified,
                identity_evidence_state: observation.identity_evidence_state.clone(),
                date_window_verified: observation.date_window_verified,
            })
            .collect(),
    };
    if selected_read_scope_commitment_sha256(&material)? != scope.scope_commitment_sha256 {
        return Err(MirrorError::InvalidInput(
            "selected_read_commitment_mismatch",
        ));
    }
    Ok(())
}

fn validate_company_guid(value: &str) -> Result<(), MirrorError> {
    if value.is_empty()
        || value.len() > 256
        || value.trim() != value
        || !value.is_ascii()
        || value.bytes().any(|byte| byte.is_ascii_control())
    {
        return Err(MirrorError::InvalidInput("company_guid"));
    }
    Ok(())
}

fn validate_company_number(value: &str) -> Result<(), MirrorError> {
    if value.is_empty() || value.len() > 16 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(MirrorError::InvalidInput("company_number"));
    }
    Ok(())
}

fn validate_books_from_yyyymmdd(value: &str) -> Result<(), MirrorError> {
    TallyDate::parse(value)
        .map(|_| ())
        .map_err(|_| MirrorError::InvalidInput("books_from_yyyymmdd"))
}

fn validate_company_input(input: &CompanyInput) -> Result<(), MirrorError> {
    validate_nonempty(&input.endpoint_id, 128, "endpoint_id")?;
    validate_nonempty(&input.display_name, 512, "display_name")?;
    validate_identity(&input.identity)?;
    if let Some(guid) = input.identity.guid.as_deref() {
        validate_company_guid(guid)?;
    }
    Ok(())
}

fn validate_identity(identity: &SourceIdentityInput) -> Result<(), MirrorError> {
    for value in [
        identity.guid.as_deref(),
        identity.remote_id.as_deref(),
        identity.master_id.as_deref(),
        identity.fallback_fingerprint.as_deref(),
    ] {
        validate_optional_text(value, 256, "source_identity")?;
    }
    if identity.guid.is_none()
        && identity.remote_id.is_none()
        && identity.master_id.is_none()
        && identity.fallback_fingerprint.is_none()
    {
        return Err(MirrorError::InvalidInput("source_identity"));
    }
    Ok(())
}

fn validate_observation_shape(input: &ObservedRecordInput) -> Result<(), MirrorError> {
    match input.status {
        ObservationStatus::Accepted
            if input.canonical_sha256.is_some()
                && input.canonical_payload.is_some()
                && input.safe_rejection_code.is_none() =>
        {
            Ok(())
        }
        ObservationStatus::Rejected
            if input.canonical_payload.is_none() && input.safe_rejection_code.is_some() =>
        {
            Ok(())
        }
        _ => Err(MirrorError::InvalidInput("observation_status_shape")),
    }
}

async fn acquire_mirror_write_lock(
    transaction: &mut Transaction<'_, Sqlite>,
) -> Result<(), MirrorError> {
    sqlx::query("UPDATE tally_schema_migrations SET version = version WHERE version = 4")
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

async fn ensure_open_snapshot_window_attempt(
    transaction: &mut Transaction<'_, Sqlite>,
    attempt: &SnapshotWindowAttemptRef,
) -> Result<(), MirrorError> {
    let state = sqlx::query_scalar::<_, String>(
        "SELECT state FROM tally_snapshot_window_attempts \
         WHERE id = ?1 AND batch_id = ?2 AND window_id = ?3 AND attempt_ordinal = ?4",
    )
    .bind(&attempt.attempt_id)
    .bind(&attempt.batch_id)
    .bind(&attempt.window_id)
    .bind(i64::from(attempt.attempt_ordinal))
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(MirrorError::NotFound)?;
    if state != "open" {
        return Err(MirrorError::WindowAttemptClosed);
    }
    Ok(())
}

fn validate_snapshot_window_attempt_ref(
    attempt: &SnapshotWindowAttemptRef,
) -> Result<(), MirrorError> {
    validate_nonempty(&attempt.attempt_id, 128, "window_attempt_id")?;
    validate_nonempty(&attempt.batch_id, 128, "window_attempt_batch_id")?;
    validate_nonempty(&attempt.window_id, 128, "window_attempt_window_id")?;
    if attempt.attempt_ordinal == 0 {
        return Err(MirrorError::InvalidInput("window_attempt_ordinal"));
    }
    Ok(())
}

fn validate_snapshot_window_record_key(record_key: &str) -> Result<(&str, &str), MirrorError> {
    if record_key.len() > 385 {
        return Err(MirrorError::InvalidInput("window_membership_record_key"));
    }
    let mut parts = record_key.split('\0');
    let object_type = parts.next().unwrap_or_default();
    let source_id = parts.next().unwrap_or_default();
    if parts.next().is_some() {
        return Err(MirrorError::InvalidInput("window_membership_record_key"));
    }
    validate_safe_code(object_type)?;
    validate_nonempty(source_id, 256, "window_membership_source_id")?;
    Ok((object_type, source_id))
}

fn validate_snapshot_window_membership_input(
    input: &SnapshotWindowMembershipInput,
    batch_id: &str,
) -> Result<(), MirrorError> {
    match input {
        SnapshotWindowMembershipInput::Observed {
            record_key,
            observation,
        } => {
            let (object_type, source_id) = validate_snapshot_window_record_key(record_key)?;
            if observation.batch_id != batch_id
                || observation.object_type != object_type
                || observation.status != ObservationStatus::Accepted
                || ![
                    observation.identity.guid.as_deref(),
                    observation.identity.remote_id.as_deref(),
                    observation.identity.master_id.as_deref(),
                    observation.identity.fallback_fingerprint.as_deref(),
                ]
                .contains(&Some(source_id))
            {
                return Err(MirrorError::InvalidInput(
                    "window_membership_observation_owner_or_identity",
                ));
            }
            prepare_observed_record(observation)?;
        }
        SnapshotWindowMembershipInput::ProvenanceUnavailable {
            record_key,
            canonical_sha256,
            canonical_payload,
            exact_decimals,
            safe_reason_code,
        } => {
            validate_snapshot_window_record_key(record_key)?;
            validate_sha256(canonical_sha256)?;
            canonical_json(canonical_payload)?;
            validate_and_serialize_decimals(exact_decimals)?;
            validate_safe_code(safe_reason_code)?;
        }
    }
    Ok(())
}

fn prepare_observed_record(
    input: &ObservedRecordInput,
) -> Result<PreparedObservedRecord, MirrorError> {
    validate_safe_code(&input.object_type)?;
    validate_identity(&input.identity)?;
    validate_sha256(&input.raw_source_sha256)?;
    validate_optional_sha256(input.canonical_sha256.as_deref())?;
    validate_optional_text(input.display_name.as_deref(), 512, "display_name")?;
    validate_optional_token(input.observed_alter_id.as_deref())?;
    validate_optional_safe_code(input.safe_rejection_code.as_deref())?;
    validate_observation_shape(input)?;
    Ok(PreparedObservedRecord {
        canonical_payload_json: input
            .canonical_payload
            .as_ref()
            .map(canonical_json)
            .transpose()?,
        exact_decimals_json: validate_and_serialize_decimals(&input.exact_decimals)?,
    })
}

fn validate_and_serialize_decimals(
    decimals: &BTreeMap<String, String>,
) -> Result<String, MirrorError> {
    for (field, value) in decimals {
        validate_safe_code(field)?;
        ExactDecimal::parse(value.clone())
            .map_err(|_| MirrorError::InvalidInput("exact_decimal"))?;
    }
    Ok(serde_json::to_string(decimals)?)
}

fn canonical_json(value: &Value) -> Result<String, MirrorError> {
    fn canonicalize(value: &Value) -> Result<Value, MirrorError> {
        match value {
            Value::Object(map) => {
                let mut ordered = serde_json::Map::new();
                let mut entries = map.iter().collect::<Vec<_>>();
                entries.sort_by_key(|(key, _)| *key);
                for (key, value) in entries {
                    ordered.insert(key.clone(), canonicalize(value)?);
                }
                Ok(Value::Object(ordered))
            }
            Value::Array(values) => values
                .iter()
                .map(canonicalize)
                .collect::<Result<Vec<_>, _>>()
                .map(Value::Array),
            Value::Number(number) if number.is_f64() => {
                Err(MirrorError::InvalidInput("floating_point_payload_number"))
            }
            other => Ok(other.clone()),
        }
    }

    Ok(serde_json::to_string(&canonicalize(value)?)?)
}

fn validate_nonempty(value: &str, max: usize, field: &'static str) -> Result<(), MirrorError> {
    if value.trim().is_empty() || value.len() > max || value.chars().any(char::is_control) {
        return Err(MirrorError::InvalidInput(field));
    }
    Ok(())
}

fn validate_optional_text(
    value: Option<&str>,
    max: usize,
    field: &'static str,
) -> Result<(), MirrorError> {
    if let Some(value) = value {
        validate_nonempty(value, max, field)?;
    }
    Ok(())
}

fn validate_safe_code(value: &str) -> Result<(), MirrorError> {
    if value.is_empty()
        || value.len() > 128
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'_' | b'-' | b'.' | b':')
        })
    {
        return Err(MirrorError::InvalidInput("safe_code"));
    }
    Ok(())
}

fn validate_optional_safe_code(value: Option<&str>) -> Result<(), MirrorError> {
    if let Some(value) = value {
        validate_safe_code(value)?;
    }
    Ok(())
}

fn validate_sha256(value: &str) -> Result<(), MirrorError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(MirrorError::InvalidInput("sha256"));
    }
    Ok(())
}

fn validate_optional_sha256(value: Option<&str>) -> Result<(), MirrorError> {
    if let Some(value) = value {
        validate_sha256(value)?;
    }
    Ok(())
}

fn validate_optional_token(value: Option<&str>) -> Result<(), MirrorError> {
    if let Some(value) = value {
        if value.is_empty()
            || value.len() > 128
            || !value.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':')
            })
        {
            return Err(MirrorError::InvalidInput("checkpoint_or_alter_token"));
        }
    }
    Ok(())
}

fn validate_date_range(from: Option<&str>, to: Option<&str>) -> Result<(), MirrorError> {
    match (from, to) {
        (None, None) => Ok(()),
        (Some(from), Some(to)) if valid_yyyymmdd(from) && valid_yyyymmdd(to) && from <= to => {
            Ok(())
        }
        _ => Err(MirrorError::InvalidInput("date_range")),
    }
}

fn valid_yyyymmdd(value: &str) -> bool {
    value.len() == 8 && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn parse_run_outcome(value: &str) -> Result<RunOutcome, MirrorError> {
    match value {
        "completed" => Ok(RunOutcome::Completed),
        "failed" => Ok(RunOutcome::Failed),
        "cancelled" => Ok(RunOutcome::Cancelled),
        "outcome_unknown" => Ok(RunOutcome::OutcomeUnknown),
        _ => Err(MirrorError::VerificationInvariant),
    }
}

fn parse_verification_state(value: &str) -> Result<VerificationState, MirrorError> {
    match value {
        "verified" => Ok(VerificationState::Verified),
        "partial" => Ok(VerificationState::Partial),
        "unverified" => Ok(VerificationState::Unverified),
        _ => Err(MirrorError::VerificationInvariant),
    }
}

#[derive(Serialize)]
struct ProofHashInput<'a> {
    proof_contract_version: u16,
    previous_entry_sha256: Option<&'a str>,
    proof_id: &'a str,
    run_id: &'a str,
    batch_id: &'a str,
    capability_snapshot_id: &'a str,
    company_id: &'a str,
    pack_id: &'a str,
    outcome: RunOutcome,
    verification: VerificationState,
    started_at_unix_ms: i64,
    completed_at_unix_ms: i64,
    accepted_records: i64,
    rejected_records: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    provenance_unavailable_records: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    record_counts_sha256: Option<&'a str>,
    snapshot_sha256: Option<&'a str>,
    checkpoint_before: Option<&'a str>,
    checkpoint_after: Option<&'a str>,
    gap_codes: &'a [String],
    warning_codes: &'a [String],
    created_at_unix_ms: i64,
}

fn sha256_json(value: &impl Serialize) -> Result<String, MirrorError> {
    let bytes = serde_json::to_vec(value)?;
    let digest = Sha256::digest(bytes);
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[derive(Serialize)]
struct FixtureEnrollmentCommitment<'a> {
    schema: &'static str,
    review_commitment_sha256: &'a str,
    company_id: &'a str,
    canonical_origin: &'a str,
    company_guid_ascii_casefolded: &'a str,
    contract_version: u16,
    disposable_company_attested: bool,
    no_customer_data_attested: bool,
    backup_guidance_acknowledged: bool,
}

fn fixture_enrollment_payload_sha256(
    material: &FixtureEnrollmentCommitment<'_>,
) -> Result<String, MirrorError> {
    sha256_json(material)
}

#[derive(Serialize)]
struct CanaryReservationCommitment<'a> {
    schema: &'static str,
    enrollment_id: &'a str,
    enrollment_payload_sha256: &'a str,
    company_id: &'a str,
    canonical_origin: &'a str,
    company_guid_ascii_casefolded: &'a str,
    review_commitment_sha256: &'a str,
    contract_version: u16,
}

fn canary_reservation_payload_sha256(
    material: &CanaryReservationCommitment<'_>,
) -> Result<String, MirrorError> {
    sha256_json(material)
}

#[derive(Serialize)]
struct FixtureRevocationCommitment<'a> {
    schema: &'static str,
    enrollment_id: &'a str,
    enrollment_payload_sha256: &'a str,
    safe_reason_code: &'static str,
    revoked_at_unix_ms: i64,
}

fn fixture_revocation_payload_sha256(
    enrollment_id: &str,
    enrollment_payload_sha256: &str,
    safe_reason_code: &'static str,
    revoked_at_unix_ms: i64,
) -> Result<String, MirrorError> {
    sha256_json(&FixtureRevocationCommitment {
        schema: "bridge.tally.write-fixture-revocation/1",
        enrollment_id,
        enrollment_payload_sha256,
        safe_reason_code,
        revoked_at_unix_ms,
    })
}

async fn retire_precomposite_fixture_enrollments(
    transaction: &mut Transaction<'_, Sqlite>,
    revoked_at_unix_ms: i64,
) -> Result<(), MirrorError> {
    let rows = sqlx::query(
        "SELECT enrollment.id, enrollment.enrollment_payload_sha256 \
         FROM tally_write_fixture_enrollments AS enrollment \
         JOIN tally_companies AS company ON company.id = enrollment.company_id \
         LEFT JOIN tally_write_fixture_revocations AS revocation \
           ON revocation.enrollment_id = enrollment.id \
         WHERE company.identity_confidence = 'unknown' \
           AND revocation.enrollment_id IS NULL \
         ORDER BY enrollment.id",
    )
    .fetch_all(&mut **transaction)
    .await?;
    let enrollments = rows
        .into_iter()
        .map(|row| {
            Ok((
                row.try_get("id")?,
                row.try_get("enrollment_payload_sha256")?,
            ))
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()?;
    write_fixture_identity_reverification_revocations(transaction, enrollments, revoked_at_unix_ms)
        .await
}

async fn fixture_enrollments_for_v25_observed_identity_demotions(
    transaction: &mut Transaction<'_, Sqlite>,
) -> Result<Vec<(String, String)>, MirrorError> {
    let rows = sqlx::query(
        "SELECT enrollment.id, enrollment.enrollment_payload_sha256 \
         FROM tally_write_fixture_enrollments AS enrollment \
         JOIN tally_companies AS company ON company.id = enrollment.company_id \
         LEFT JOIN tally_write_fixture_revocations AS revocation \
           ON revocation.enrollment_id = enrollment.id \
         WHERE company.identity_confidence = 'observed' \
           AND ( \
             company.display_name IS NULL \
             OR length(trim(company.display_name, char(9) || char(10) || char(13) || ' ')) = 0 \
             OR company.company_guid IS NULL \
             OR length(trim(company.company_guid, char(9) || char(10) || char(13) || ' ')) = 0 \
             OR company.company_number IS NULL \
             OR length(trim(company.company_number, char(9) || char(10) || char(13) || ' ')) = 0 \
             OR company.books_from_yyyymmdd IS NULL \
             OR length(trim(company.books_from_yyyymmdd, char(9) || char(10) || char(13) || ' ')) = 0 \
           ) \
           AND revocation.enrollment_id IS NULL \
         ORDER BY enrollment.id",
    )
    .fetch_all(&mut **transaction)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok((
                row.try_get("id")?,
                row.try_get("enrollment_payload_sha256")?,
            ))
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()
        .map_err(MirrorError::from)
}

async fn write_fixture_identity_reverification_revocations(
    transaction: &mut Transaction<'_, Sqlite>,
    enrollments: Vec<(String, String)>,
    revoked_at_unix_ms: i64,
) -> Result<(), MirrorError> {
    let next_sequence = sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(MAX(event_sequence), 0) + 1 FROM tally_write_fixture_revocations",
    )
    .fetch_one(&mut **transaction)
    .await?;
    for (offset, (enrollment_id, enrollment_payload_sha256)) in enrollments.into_iter().enumerate()
    {
        let payload_sha256 = fixture_revocation_payload_sha256(
            &enrollment_id,
            &enrollment_payload_sha256,
            "company_identity_reverification_required",
            revoked_at_unix_ms,
        )?;
        sqlx::query(
            "INSERT INTO tally_write_fixture_revocations(\
               event_sequence, id, enrollment_id, revocation_payload_sha256, safe_reason_code, revoked_at_unix_ms\
             ) VALUES (?1, ?2, ?3, ?4, 'company_identity_reverification_required', ?5)",
        )
        .bind(next_sequence + i64::try_from(offset).expect("enumerated migration rows fit i64"))
        .bind(Uuid::new_v4().to_string())
        .bind(enrollment_id)
        .bind(payload_sha256)
        .bind(revoked_at_unix_ms)
        .execute(&mut **transaction)
        .await?;
    }
    Ok(())
}

async fn validate_legacy_v1_selected_read_scope_layouts(
    transaction: &mut Transaction<'_, Sqlite>,
) -> Result<(), MirrorError> {
    let scopes = sqlx::query(
        "SELECT scope.id, scope.scope_commitment_sha256, scope.parent_review_sha256, \
                endpoint.canonical_origin, company.company_guid, scope.ledger_profile_id, \
                scope.voucher_profile_id, scope.voucher_from_yyyymmdd, \
                scope.voucher_to_yyyymmdd, scope.observed_at_unix_ms \
         FROM tally_selected_read_scopes AS scope \
         JOIN tally_capability_snapshots AS snapshot ON snapshot.id = scope.capability_snapshot_id \
         JOIN tally_endpoints AS endpoint ON endpoint.id = snapshot.endpoint_id \
         JOIN tally_companies AS company ON company.id = scope.company_id \
         WHERE scope.scope_contract_version = 1 \
         ORDER BY scope.id",
    )
    .fetch_all(&mut **transaction)
    .await?;
    for scope in scopes {
        let scope_id: String = scope.try_get("id")?;
        let company_guid: Option<String> = scope.try_get("company_guid")?;
        let company_guid = company_guid.ok_or(MirrorError::InvalidInput(
            "selected_read_scope_v1_layout_unverified",
        ))?;
        let observations = sqlx::query(
            "SELECT capability_key, capability_state, confidence, safe_reason_code, \
                    result_bucket, request_sha256, decoded_response_sha256, response_encoding, \
                    company_context_verified, schema_verified, record_count_verified, \
                    identity_evidence_state, date_window_verified \
             FROM tally_selected_read_observations \
             WHERE scope_id = ?1 ORDER BY capability_key",
        )
        .bind(&scope_id)
        .fetch_all(&mut **transaction)
        .await?
        .into_iter()
        .map(|observation| {
            Ok(SelectedReadObservationCommitmentMaterial {
                capability_key: observation.try_get("capability_key")?,
                state: observation.try_get("capability_state")?,
                confidence: observation.try_get("confidence")?,
                safe_reason_code: observation.try_get("safe_reason_code")?,
                result_bucket: observation.try_get("result_bucket")?,
                request_sha256: observation.try_get("request_sha256")?,
                decoded_response_sha256: observation.try_get("decoded_response_sha256")?,
                response_encoding: observation.try_get("response_encoding")?,
                company_context_verified: observation
                    .try_get::<i64, _>("company_context_verified")?
                    != 0,
                schema_verified: observation.try_get::<i64, _>("schema_verified")? != 0,
                record_count_verified: observation.try_get::<i64, _>("record_count_verified")? != 0,
                identity_evidence_state: observation.try_get("identity_evidence_state")?,
                date_window_verified: observation.try_get::<i64, _>("date_window_verified")? != 0,
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()?;
        let material = SelectedReadScopeCommitmentMaterialV1 {
            parent_review_commitment_sha256: scope.try_get("parent_review_sha256")?,
            canonical_origin: scope.try_get("canonical_origin")?,
            company_guid_ascii_casefolded: company_guid.to_ascii_lowercase(),
            ledger_profile_id: scope.try_get("ledger_profile_id")?,
            voucher_profile_id: scope.try_get("voucher_profile_id")?,
            voucher_from_yyyymmdd: scope.try_get("voucher_from_yyyymmdd")?,
            voucher_to_yyyymmdd: scope.try_get("voucher_to_yyyymmdd")?,
            observed_at_unix_ms: scope.try_get("observed_at_unix_ms")?,
            observations,
        };
        let stored_commitment: String = scope.try_get("scope_commitment_sha256")?;
        if selected_read_scope_commitment_sha256_v1(&material)? != stored_commitment {
            return Err(MirrorError::InvalidInput(
                "selected_read_scope_v1_layout_unverified",
            ));
        }
    }
    Ok(())
}

fn hex_digest(bytes: impl AsRef<[u8]>) -> String {
    bytes
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
#[path = "tally_mirror_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "tally_capability_license_tests.rs"]
mod capability_license_tests;
