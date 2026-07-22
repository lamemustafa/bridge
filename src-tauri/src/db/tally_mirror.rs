use std::collections::BTreeMap;

use bridge_tally_core::{ExactDecimal, PackSchemaVersion, TallyDate};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{Row, Sqlite, SqlitePool, Transaction};
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
    pub ledger_profile_id: String,
    pub voucher_profile_id: String,
    pub voucher_from_yyyymmdd: String,
    pub voucher_to_yyyymmdd: String,
    pub observed_at_unix_ms: i64,
    pub observations: Vec<SelectedReadObservationCommitmentMaterial>,
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

pub(crate) fn selected_read_scope_commitment_sha256(
    material: &SelectedReadScopeCommitmentMaterial,
) -> Result<String, MirrorError> {
    sha256_json(&SelectedReadScopeCommitmentEnvelope {
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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PersistedCompanyProfile {
    pub name: String,
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
    pub verified_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteCanaryPreflightEvidenceRef {
    pub id: String,
    pub attempt_id: String,
    pub verified_at_unix_ms: i64,
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

    pub async fn persisted_company_profiles(
        &self,
    ) -> Result<PersistedCompanyProfilePage, MirrorError> {
        const LIMIT: u32 = 500;
        let mut transaction = self.pool.begin().await?;
        let total_profiles = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_companies AS c \
             WHERE c.identity_confidence = 'observed' \
               AND c.company_guid IS NOT NULL AND TRIM(c.company_guid) <> ''",
        )
        .fetch_one(&mut *transaction)
        .await?;
        let rows = sqlx::query(
            "SELECT c.id, c.display_name, c.company_guid, c.identity_confidence, \
             c.last_observed_at_unix_ms, e.canonical_origin \
             FROM tally_companies AS c \
             JOIN tally_endpoints AS e ON e.id = c.endpoint_id \
             WHERE c.identity_confidence = 'observed' \
               AND c.company_guid IS NOT NULL AND TRIM(c.company_guid) <> '' \
             ORDER BY c.last_observed_at_unix_ms DESC, c.id ASC LIMIT ?1",
        )
        .bind(i64::from(LIMIT))
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        let profiles = rows
            .into_iter()
            .map(|row| {
                let canonical_endpoint: String = row.try_get("canonical_origin")?;
                let company_guid: String = row.try_get("company_guid")?;
                Ok(PersistedCompanyProfile {
                    name: row.try_get("display_name")?,
                    guid_observed: true,
                    mirror_company_id: row.try_get("id")?,
                    correlation_key: company_profile_correlation_key(
                        &canonical_endpoint,
                        &company_guid,
                    ),
                    identity_confidence: row.try_get("identity_confidence")?,
                    canonical_endpoint,
                    last_observed_at_unix_ms: row.try_get("last_observed_at_unix_ms")?,
                })
            })
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
        if company_id.trim().is_empty() {
            return Err(MirrorError::InvalidInput("company_pin"));
        }
        let row = sqlx::query(
            "SELECT c.id AS company_id, c.endpoint_id, e.canonical_origin, c.display_name, \
             c.company_guid, c.identity_confidence FROM tally_companies c \
             JOIN tally_endpoints e ON e.id = c.endpoint_id WHERE c.id = ?1",
        )
        .bind(company_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(MirrorError::NotFound)?;
        let identity_confidence: String = row.try_get("identity_confidence")?;
        if identity_confidence != "observed" {
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
               contract_version, verified_at_unix_ms \
             ) VALUES (?1, ?2, ?3, ?4, 1, ?5) ON CONFLICT(attempt_id) DO NOTHING",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(&input.attempt_id)
        .bind(&input.readback_state_sha256)
        .bind(&input.identity_coverage_sha256)
        .bind(input.verified_at_unix_ms)
        .execute(&mut *transaction)
        .await?;
        let evidence = sqlx::query(
            "SELECT id, readback_state_sha256, identity_coverage_sha256, verified_at_unix_ms \
             FROM tally_write_canary_preflight_evidence WHERE attempt_id = ?1",
        )
        .bind(&input.attempt_id)
        .fetch_one(&mut *transaction)
        .await?;
        let existing_state_sha256: String = evidence.try_get("readback_state_sha256")?;
        let existing_coverage_sha256: String = evidence.try_get("identity_coverage_sha256")?;
        if existing_state_sha256 != input.readback_state_sha256
            || existing_coverage_sha256 != input.identity_coverage_sha256
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
        sqlx::raw_sql(MIRROR_MIGRATION_V2)
            .execute(&mut *transaction)
            .await?;
        sqlx::raw_sql(MIRROR_MIGRATION_V3)
            .execute(&mut *transaction)
            .await?;
        sqlx::raw_sql(MIRROR_MIGRATION_V4)
            .execute(&mut *transaction)
            .await?;
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
        if selected_read_evidence_installed == 0 {
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
        sqlx::query(
            "UPDATE tally_schema_migrations SET applied_at_unix_ms = ?1 \
             WHERE version IN (2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18) AND applied_at_unix_ms = 0",
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
        sqlx::query(
            "INSERT INTO tally_capability_snapshots(\
               id, endpoint_id, observed_at_unix_ms, profile_version, product, release, mode, \
               mode_confidence\
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )
        .bind(&snapshot_id)
        .bind(&endpoint_id)
        .bind(input.observed_at_unix_ms)
        .bind(i64::from(input.profile_version))
        .bind(input.product)
        .bind(input.release)
        .bind(input.mode)
        .bind(input.mode_confidence.as_str())
        .execute(&mut **transaction)
        .await?;

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
                let incoming_confidence = identity_confidence(&input.identity).as_str();
                sqlx::query(
                    "UPDATE tally_companies SET display_name = ?1, last_observed_at_unix_ms = ?2, \
                     identity_confidence = CASE \
                       WHEN ?3 = 'documented' THEN 'documented' \
                       WHEN ?3 = 'observed' AND identity_confidence IN ('inferred', 'unknown') \
                         THEN 'observed' \
                       WHEN ?3 = 'inferred' AND identity_confidence = 'unknown' THEN 'inferred' \
                       ELSE identity_confidence END WHERE id = ?4",
                )
                .bind(&input.display_name)
                .bind(input.observed_at_unix_ms)
                .bind(incoming_confidence)
                .bind(&existing.id)
                .execute(&mut **transaction)
                .await?;
                existing.id
            }
            None => {
                let id = Uuid::new_v4().to_string();
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
                .bind(identity_confidence(&input.identity).as_str())
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
        validate_selected_read_scope(
            input.selected_read_scope.as_ref(),
            &input.capability,
            &input.company_display_name,
            input.company_identity.guid.as_deref(),
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
        let company = Self::upsert_company_in_transaction(
            &mut transaction,
            CompanyInput {
                endpoint_id: snapshot.endpoint_id.clone(),
                display_name: input.company_display_name,
                identity: input.company_identity,
                observed_at_unix_ms,
            },
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
             ) VALUES (?1, ?2, ?3, 1, ?4, ?5, ?6, ?7, ?8, ?9, ?10, \
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
        for code in receipt
            .facts
            .gap_codes
            .iter()
            .chain(&receipt.facts.warning_codes)
        {
            validate_export_code(code)?;
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

/// Returns an opaque, stable join key for the same observed company at the same endpoint.
/// The key deliberately does not expose the locally persisted Tally GUID.
pub(crate) fn company_profile_correlation_key(
    canonical_origin: &str,
    company_guid: &str,
) -> String {
    let mut digest = Sha256::new();
    digest.update(b"bridge.tally.company-profile-correlation/1\0");
    digest.update(canonical_origin.as_bytes());
    digest.update(b"\0");
    digest.update(company_guid.to_ascii_lowercase().as_bytes());
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
                | "company_identity_not_found"
                | "complete_source_count_disagreement"
                | "duplicate_record_across_windows"
                | "duplicate_source_identity"
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
    let payload = serde_json::json!({
        "schema": "bridge.tally.reviewed-setup-payload/1",
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
            "remote_id": input.company_identity.remote_id,
            "master_id": input.company_identity.master_id,
            "fallback_fingerprint": input.company_identity.fallback_fingerprint,
            "confidence": input.company_identity.confidence.map(Confidence::as_str),
        },
        "selected_read_scope": selected_read_scope,
    });
    let canonical = canonical_json(&payload)?;
    Ok(hex_digest(Sha256::digest(canonical.as_bytes())))
}

fn validate_selected_read_scope(
    scope: Option<&SelectedReadScopeInput>,
    capability: &CapabilitySnapshotInput,
    company_display_name: &str,
    company_guid: Option<&str>,
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
                "utf8" | "utf8_bom" | "utf16le_bom" | "utf16be_bom"
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
    let material = SelectedReadScopeCommitmentMaterial {
        parent_review_commitment_sha256: scope.parent_review_sha256.clone(),
        canonical_origin: capability.canonical_origin.clone(),
        company_guid_ascii_casefolded: company_guid.to_ascii_lowercase(),
        company_name: company_display_name.to_string(),
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
    revoked_at_unix_ms: i64,
) -> Result<String, MirrorError> {
    sha256_json(&FixtureRevocationCommitment {
        schema: "bridge.tally.write-fixture-revocation/1",
        enrollment_id,
        enrollment_payload_sha256,
        safe_reason_code: "operator_revoked",
        revoked_at_unix_ms,
    })
}

fn hex_digest(bytes: impl AsRef<[u8]>) -> String {
    bytes
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use sqlx::sqlite::SqlitePoolOptions;

    use super::*;
    use crate::sync::reconciliation::{proof_record_counts_sha256, CommitBatchParts};

    const HASH_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const HASH_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    async fn repository() -> TallyMirrorRepository {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .after_connect(|connection, _| {
                Box::pin(async move {
                    sqlx::query("PRAGMA foreign_keys = ON")
                        .execute(connection)
                        .await?;
                    Ok(())
                })
            })
            .connect("sqlite::memory:")
            .await
            .expect("connect to in-memory SQLite");
        let repository = TallyMirrorRepository::new(pool);
        repository.migrate().await.expect("run mirror migration");
        repository
    }

    async fn repository_through_v9() -> TallyMirrorRepository {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .after_connect(|connection, _| {
                Box::pin(async move {
                    sqlx::query("PRAGMA foreign_keys = ON")
                        .execute(connection)
                        .await?;
                    Ok(())
                })
            })
            .connect("sqlite::memory:")
            .await
            .expect("connect to v9 in-memory SQLite");
        let mut transaction = pool.begin().await.expect("begin v9 migration");
        for migration in [
            MIRROR_MIGRATION_V2,
            MIRROR_MIGRATION_V3,
            MIRROR_MIGRATION_V4,
            MIRROR_MIGRATION_V5,
            MIRROR_MIGRATION_V6,
            MIRROR_MIGRATION_V7,
            MIRROR_MIGRATION_V8,
            MIRROR_MIGRATION_V9,
        ] {
            sqlx::raw_sql(migration)
                .execute(&mut *transaction)
                .await
                .expect("apply migration through v9");
        }
        transaction.commit().await.expect("commit v9 schema");
        TallyMirrorRepository::new(pool)
    }

    async fn repository_through_v13() -> TallyMirrorRepository {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .after_connect(|connection, _| {
                Box::pin(async move {
                    sqlx::query("PRAGMA foreign_keys = ON")
                        .execute(connection)
                        .await?;
                    Ok(())
                })
            })
            .connect("sqlite::memory:")
            .await
            .expect("connect to v13 in-memory SQLite");
        let mut transaction = pool.begin().await.expect("begin v13 migration");
        for migration in [
            MIRROR_MIGRATION_V2,
            MIRROR_MIGRATION_V3,
            MIRROR_MIGRATION_V4,
            MIRROR_MIGRATION_V5,
            MIRROR_MIGRATION_V6,
            MIRROR_MIGRATION_V7,
            MIRROR_MIGRATION_V8,
            MIRROR_MIGRATION_V9,
            MIRROR_MIGRATION_V10,
            MIRROR_MIGRATION_V11,
            MIRROR_MIGRATION_V12,
            MIRROR_MIGRATION_V13,
        ] {
            sqlx::raw_sql(migration)
                .execute(&mut *transaction)
                .await
                .expect("apply migration through v13");
        }
        transaction.commit().await.expect("commit v13 schema");
        TallyMirrorRepository::new(pool)
    }

    async fn seed_repository(
        repository: TallyMirrorRepository,
    ) -> (TallyMirrorRepository, CapabilitySnapshotRef, CompanyRef) {
        seed_repository_with_core_evidence(
            repository,
            CapabilityState::Supported,
            Confidence::Observed,
            None,
        )
        .await
    }

    async fn seed_repository_with_core_evidence(
        repository: TallyMirrorRepository,
        core_state: CapabilityState,
        core_confidence: Confidence,
        core_reason: Option<&str>,
    ) -> (TallyMirrorRepository, CapabilitySnapshotRef, CompanyRef) {
        let snapshot = repository
            .save_capability_snapshot(CapabilitySnapshotInput {
                canonical_origin: "http://127.0.0.1:9000".to_string(),
                observed_at_unix_ms: 1_000,
                profile_version: 1,
                product: "TallyPrime".to_string(),
                release: None,
                mode: Some("Education".to_string()),
                mode_confidence: Confidence::Observed,
                items: vec![
                    CapabilityItemInput {
                        kind: CapabilityKind::Transport,
                        key: "xml_http".to_string(),
                        state: CapabilityState::Supported,
                        confidence: Confidence::Observed,
                        safe_reason_code: None,
                    },
                    CapabilityItemInput {
                        kind: CapabilityKind::Pack,
                        key: "core_accounting".to_string(),
                        state: core_state,
                        confidence: core_confidence,
                        safe_reason_code: core_reason.map(str::to_string),
                    },
                ],
            })
            .await
            .expect("save capability snapshot");
        let company = repository
            .upsert_company(CompanyInput {
                endpoint_id: snapshot.endpoint_id.clone(),
                display_name: "Synthetic Bridge Test".to_string(),
                identity: SourceIdentityInput {
                    guid: Some("company-guid-1".to_string()),
                    confidence: Some(Confidence::Observed),
                    ..Default::default()
                },
                observed_at_unix_ms: 1_000,
            })
            .await
            .expect("save company");
        (repository, snapshot, company)
    }

    async fn seeded_repository() -> (TallyMirrorRepository, CapabilitySnapshotRef, CompanyRef) {
        seed_repository(repository().await).await
    }

    async fn seeded_repository_with_core_evidence(
        state: CapabilityState,
        confidence: Confidence,
        reason: Option<&str>,
    ) -> (TallyMirrorRepository, CapabilitySnapshotRef, CompanyRef) {
        seed_repository_with_core_evidence(repository().await, state, confidence, reason).await
    }

    fn reviewed_setup_input(review_commitment_sha256: &str) -> ReviewedSetupInput {
        ReviewedSetupInput {
            review_commitment_sha256: review_commitment_sha256.to_string(),
            capability: CapabilitySnapshotInput {
                canonical_origin: "http://127.0.0.1:9000".to_string(),
                observed_at_unix_ms: 2_000,
                profile_version: 2,
                product: "TallyPrime".to_string(),
                release: Some("synthetic".to_string()),
                mode: Some("Education".to_string()),
                mode_confidence: Confidence::Observed,
                items: vec![CapabilityItemInput {
                    kind: CapabilityKind::Feature,
                    key: "write".to_string(),
                    state: CapabilityState::Unknown,
                    confidence: Confidence::Unknown,
                    safe_reason_code: Some("write_probe_not_run".to_string()),
                }],
            },
            company_display_name: "Synthetic Reviewed Company".to_string(),
            company_identity: SourceIdentityInput {
                guid: Some("reviewed-company-guid".to_string()),
                confidence: Some(Confidence::Observed),
                ..Default::default()
            },
            selected_read_scope: None,
        }
    }

    #[tokio::test]
    async fn reviewed_setup_rolls_back_snapshot_items_and_company_on_identity_collision() {
        let (repository, snapshot, _) = seeded_repository().await;
        repository
            .upsert_company(CompanyInput {
                endpoint_id: snapshot.endpoint_id,
                display_name: "Synthetic Collision Peer".to_string(),
                identity: SourceIdentityInput {
                    remote_id: Some("remote-peer-2".to_string()),
                    confidence: Some(Confidence::Observed),
                    ..Default::default()
                },
                observed_at_unix_ms: 1_000,
            })
            .await
            .expect("seed second identity");

        let counts_before = setup_row_counts(&repository).await;
        let error = repository
            .save_reviewed_setup(ReviewedSetupInput {
                review_commitment_sha256: HASH_A.to_string(),
                capability: CapabilitySnapshotInput {
                    canonical_origin: "http://127.0.0.1:9000".to_string(),
                    observed_at_unix_ms: 2_000,
                    profile_version: 2,
                    product: "Unknown".to_string(),
                    release: None,
                    mode: None,
                    mode_confidence: Confidence::Unknown,
                    items: vec![CapabilityItemInput {
                        kind: CapabilityKind::Feature,
                        key: "write".to_string(),
                        state: CapabilityState::Unknown,
                        confidence: Confidence::Unknown,
                        safe_reason_code: Some("write_probe_not_run".to_string()),
                    }],
                },
                company_display_name: "Synthetic Ambiguous".to_string(),
                company_identity: SourceIdentityInput {
                    guid: Some("company-guid-1".to_string()),
                    remote_id: Some("remote-peer-2".to_string()),
                    confidence: Some(Confidence::Observed),
                    ..Default::default()
                },
                selected_read_scope: None,
            })
            .await
            .expect_err("cross-record identity match must roll back the setup save");
        assert!(matches!(error, MirrorError::IdentityCollision));
        assert_eq!(setup_row_counts(&repository).await, counts_before);
    }

    #[tokio::test]
    async fn reviewed_setup_replay_after_lost_acknowledgement_is_idempotent() {
        let repository = repository().await;
        let input = reviewed_setup_input(HASH_A);
        let first = repository
            .save_reviewed_setup(input.clone())
            .await
            .expect("commit reviewed setup before acknowledgement is lost");
        let counts_after_commit = setup_row_counts(&repository).await;

        let replay = repository
            .save_reviewed_setup(input)
            .await
            .expect("replay the exact reviewed setup");

        assert_eq!(replay.snapshot.id, first.snapshot.id);
        assert_eq!(replay.snapshot.endpoint_id, first.snapshot.endpoint_id);
        assert_eq!(replay.company.id, first.company.id);
        assert_eq!(setup_row_counts(&repository).await, counts_after_commit);
        let consumptions = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_reviewed_setup_consumptions \
             WHERE review_commitment_sha256 = ?1",
        )
        .bind(HASH_A)
        .fetch_one(&repository.pool)
        .await
        .expect("count durable review consumption");
        assert_eq!(consumptions, 1);
    }

    #[tokio::test]
    async fn reviewed_setup_commitment_cannot_be_reused_for_changed_payload() {
        let repository = repository().await;
        let input = reviewed_setup_input(HASH_A);
        repository
            .save_reviewed_setup(input.clone())
            .await
            .expect("commit first reviewed setup");
        let counts_after_commit = setup_row_counts(&repository).await;
        let mut changed = input;
        changed.company_display_name = "Synthetic Changed Company".to_string();

        assert!(matches!(
            repository.save_reviewed_setup(changed).await,
            Err(MirrorError::InvalidInput("review_commitment_reused"))
        ));
        assert_eq!(setup_row_counts(&repository).await, counts_after_commit);
    }

    #[tokio::test]
    async fn reviewed_setup_consumption_authority_is_immutable() {
        let repository = repository().await;
        repository
            .save_reviewed_setup(reviewed_setup_input(HASH_A))
            .await
            .expect("commit reviewed setup");

        assert!(sqlx::query(
            "UPDATE tally_reviewed_setup_consumptions SET consumed_at_unix_ms = 3 \
             WHERE review_commitment_sha256 = ?1",
        )
        .bind(HASH_A)
        .execute(&repository.pool)
        .await
        .is_err());
        assert!(sqlx::query(
            "DELETE FROM tally_reviewed_setup_consumptions WHERE review_commitment_sha256 = ?1",
        )
        .bind(HASH_A)
        .execute(&repository.pool)
        .await
        .is_err());
    }

    #[tokio::test]
    async fn write_fixture_enrollment_is_idempotent_revocable_and_identity_safe() {
        let repository = repository().await;
        let saved = repository
            .save_reviewed_setup(reviewed_setup_input(HASH_A))
            .await
            .expect("persist observed company pin before local fixture enrollment");
        let input = WriteFixtureEnrollmentInput {
            company_id: saved.company.id.clone(),
            review_commitment_sha256: HASH_B.to_string(),
            disposable_company_attested: true,
            no_customer_data_attested: true,
            backup_guidance_acknowledged: true,
            enrolled_at_unix_ms: 3_000,
        };

        let first = repository
            .enroll_write_fixture(input.clone())
            .await
            .expect("locally enroll synthetic fixture");
        let replay = repository
            .enroll_write_fixture(input.clone())
            .await
            .expect("exact fixture enrollment replay is idempotent");
        assert_eq!(replay, first);

        let active = repository
            .write_fixture_enrollment_status(&saved.company.id)
            .await
            .expect("read safe local fixture status");
        assert_eq!(active.fixture_state, "active");
        assert_eq!(active.candidate_gate, "enrolled");
        assert_eq!(active.write_capability, "unknown");
        let serialized = serde_json::to_string(&active).expect("serialize safe status");
        assert!(!serialized.contains("Synthetic Reviewed Company"));
        assert!(!serialized.contains("reviewed-company-guid"));

        let mut competing = input;
        competing.review_commitment_sha256 = HASH_A.to_string();
        competing.enrolled_at_unix_ms = 4_000;
        assert!(repository.enroll_write_fixture(competing).await.is_err());

        let revoked = repository
            .revoke_write_fixture_enrollment(&saved.company.id, 5_000)
            .await
            .expect("append local revocation");
        assert_eq!(revoked.fixture_state, "revoked");
        assert_eq!(revoked.candidate_gate, "not_enrolled");
        assert_eq!(revoked.revoked_at_unix_ms, Some(5_000));
        assert_eq!(
            repository
                .revoke_write_fixture_enrollment(&saved.company.id, 6_000)
                .await
                .expect("repeat revocation is local and idempotent"),
            revoked
        );

        let renewed = repository
            .enroll_write_fixture(WriteFixtureEnrollmentInput {
                company_id: saved.company.id.clone(),
                review_commitment_sha256: HASH_A.to_string(),
                disposable_company_attested: true,
                no_customer_data_attested: true,
                backup_guidance_acknowledged: true,
                // Deliberately older than the revoked enrollment: wall clocks can roll back.
                enrolled_at_unix_ms: 2_000,
            })
            .await
            .expect("a freshly reviewed fixture may enroll after revocation");
        assert_ne!(renewed.id, first.id);
        assert_eq!(
            repository
                .write_fixture_enrollment_status(&saved.company.id)
                .await
                .expect("active enrollment wins over historical timestamp ordering")
                .fixture_state,
            "active"
        );
        let final_revocation = repository
            .revoke_write_fixture_enrollment(&saved.company.id, 1_000)
            .await
            .expect("revoke the active enrollment despite clock rollback");
        assert_eq!(final_revocation.fixture_state, "revoked");
        assert_eq!(final_revocation.revoked_at_unix_ms, Some(1_000));
        let latest_revoked = repository
            .write_fixture_enrollment_status(&saved.company.id)
            .await
            .expect("latest revoked evidence uses revocation ordering");
        assert_eq!(latest_revoked.fixture_state, "revoked");
        assert_eq!(latest_revoked.revoked_at_unix_ms, Some(1_000));
        assert_eq!(
            repository
                .revoke_write_fixture_enrollment(&saved.company.id, 500)
                .await
                .expect("repeat revocation reports the latest committed evidence"),
            latest_revoked
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM tally_write_fixture_revocations")
                .fetch_one(&repository.pool)
                .await
                .expect("count immutable revocations"),
            2
        );
        assert!(
            sqlx::query("UPDATE tally_write_fixture_enrollments SET enrolled_at_unix_ms = 1")
                .execute(&repository.pool)
                .await
                .is_err()
        );
        assert!(sqlx::query("DELETE FROM tally_write_fixture_revocations")
            .execute(&repository.pool)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn write_canary_reservation_is_fixture_bound_single_use_and_revocable() {
        let repository = repository().await;
        let saved = repository
            .save_reviewed_setup(reviewed_setup_input(HASH_A))
            .await
            .expect("persist observed company before reserving a canary");
        repository
            .enroll_write_fixture(WriteFixtureEnrollmentInput {
                company_id: saved.company.id.clone(),
                review_commitment_sha256: HASH_B.to_string(),
                disposable_company_attested: true,
                no_customer_data_attested: true,
                backup_guidance_acknowledged: true,
                enrolled_at_unix_ms: 3_000,
            })
            .await
            .expect("enroll the disposable fixture");

        let reservation_input = WriteCanaryReservationInput {
            company_id: saved.company.id.clone(),
            review_commitment_sha256: HASH_B.to_string(),
            reserved_at_unix_ms: 4_000,
        };
        let first = repository
            .reserve_write_canary(reservation_input.clone())
            .await
            .expect("reserve the single canary slot");
        let replay = repository
            .reserve_write_canary(reservation_input)
            .await
            .expect("replay the exact reservation safely");
        assert_eq!(replay, first);

        assert!(matches!(
            repository
                .reserve_write_canary(WriteCanaryReservationInput {
                    company_id: saved.company.id.clone(),
                    review_commitment_sha256: HASH_A.to_string(),
                    reserved_at_unix_ms: 5_000,
                })
                .await,
            Err(MirrorError::InvalidInput("fixture_enrollment_not_active"))
        ));
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM tally_write_canary_reservations")
                .fetch_one(&repository.pool)
                .await
                .expect("count durable canary reservations"),
            1
        );

        repository
            .revoke_write_fixture_enrollment(&saved.company.id, 6_000)
            .await
            .expect("revoke the fixture before any dispatch");
        assert!(matches!(
            repository
                .reserve_write_canary(WriteCanaryReservationInput {
                    company_id: saved.company.id,
                    review_commitment_sha256: HASH_B.to_string(),
                    reserved_at_unix_ms: 7_000,
                })
                .await,
            Err(MirrorError::InvalidInput("fixture_enrollment_not_active"))
        ));
        assert!(
            sqlx::query("UPDATE tally_write_canary_reservations SET reserved_at_unix_ms = 1")
                .execute(&repository.pool)
                .await
                .is_err()
        );
        assert!(sqlx::query("DELETE FROM tally_write_canary_reservations")
            .execute(&repository.pool)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn write_canary_payload_binding_is_exact_immutable_and_fixture_bound() {
        let repository = repository().await;
        let saved = repository
            .save_reviewed_setup(reviewed_setup_input(HASH_A))
            .await
            .expect("persist observed company before binding a canary payload");
        repository
            .enroll_write_fixture(WriteFixtureEnrollmentInput {
                company_id: saved.company.id.clone(),
                review_commitment_sha256: HASH_B.to_string(),
                disposable_company_attested: true,
                no_customer_data_attested: true,
                backup_guidance_acknowledged: true,
                enrolled_at_unix_ms: 3_000,
            })
            .await
            .expect("enroll the disposable fixture");
        let reservation = repository
            .reserve_write_canary(WriteCanaryReservationInput {
                company_id: saved.company.id.clone(),
                review_commitment_sha256: HASH_B.to_string(),
                reserved_at_unix_ms: 4_000,
            })
            .await
            .expect("reserve the only canary slot");
        let reservation_payload_sha256 = sqlx::query_scalar::<_, String>(
            "SELECT reservation_payload_sha256 FROM tally_write_canary_reservations WHERE id = ?1",
        )
        .bind(&reservation.id)
        .fetch_one(&repository.pool)
        .await
        .expect("load immutable reservation payload commitment");
        let input = WriteCanaryPayloadBindingInput {
            company_id: saved.company.id.clone(),
            review_commitment_sha256: HASH_B.to_string(),
            reservation_id: reservation.id.clone(),
            reservation_payload_sha256: reservation_payload_sha256.clone(),
            wire_sha256: HASH_A.to_string(),
            intended_state_sha256: HASH_B.to_string(),
            identity_query_sha256: HASH_A.to_string(),
            bound_at_unix_ms: 5_000,
        };
        let mut mismatched_reservation = input.clone();
        mismatched_reservation.reservation_payload_sha256 = HASH_A.to_string();
        assert!(matches!(
            repository
                .bind_write_canary_payload(mismatched_reservation)
                .await,
            Err(MirrorError::InvalidInput("canary_reservation_not_active"))
        ));
        let first = repository
            .bind_write_canary_payload(input.clone())
            .await
            .expect("bind the exact canary commitments");
        assert_eq!(
            repository
                .bind_write_canary_payload(input.clone())
                .await
                .expect("replay the exact payload binding safely"),
            first
        );
        let active_binding = ActiveWriteCanaryPayloadBindingInput {
            company_id: input.company_id.clone(),
            review_commitment_sha256: input.review_commitment_sha256.clone(),
            reservation_id: input.reservation_id.clone(),
            reservation_payload_sha256: input.reservation_payload_sha256.clone(),
            wire_sha256: input.wire_sha256.clone(),
            intended_state_sha256: input.intended_state_sha256.clone(),
            identity_query_sha256: input.identity_query_sha256.clone(),
        };
        assert_eq!(
            repository
                .active_write_canary_payload_binding(active_binding.clone())
                .await
                .expect("load the exact active payload binding"),
            first
        );
        let mut mismatched_active_binding = active_binding.clone();
        mismatched_active_binding.wire_sha256 = HASH_B.to_string();
        assert!(matches!(
            repository
                .active_write_canary_payload_binding(mismatched_active_binding)
                .await,
            Err(MirrorError::InvalidInput(
                "canary_payload_binding_not_active"
            ))
        ));
        let preflight_input = BeginWriteCanaryPreflightInput {
            binding: active_binding.clone(),
            started_at_unix_ms: 5_500,
        };
        let preflight = repository
            .begin_write_canary_preflight(preflight_input.clone())
            .await
            .expect("claim the one sealed preflight attempt");
        assert_eq!(preflight.payload_binding_id, first.id);
        assert!(matches!(
            repository
                .begin_write_canary_preflight(preflight_input)
                .await,
            Err(MirrorError::InvalidInput(
                "canary_preflight_attempt_already_started"
            ))
        ));
        assert!(sqlx::query(
            "UPDATE tally_write_canary_preflight_attempts SET started_at_unix_ms = 1"
        )
        .execute(&repository.pool)
        .await
        .is_err());
        assert!(
            sqlx::query("DELETE FROM tally_write_canary_preflight_attempts")
                .execute(&repository.pool)
                .await
                .is_err()
        );
        let evidence_input = WriteCanaryPreflightEvidenceInput {
            attempt_id: preflight.id.clone(),
            readback_state_sha256: HASH_A.to_string(),
            identity_coverage_sha256: HASH_B.to_string(),
            verified_at_unix_ms: 5_750,
        };
        let mut early_evidence = evidence_input.clone();
        early_evidence.verified_at_unix_ms = 5_499;
        assert!(matches!(
            repository
                .record_write_canary_preflight_evidence(early_evidence)
                .await,
            Err(MirrorError::InvalidInput(
                "canary_preflight_evidence_before_attempt"
            ))
        ));
        let evidence = repository
            .record_write_canary_preflight_evidence(evidence_input.clone())
            .await
            .expect("persist digest-only sealed preflight evidence");
        assert_eq!(
            repository
                .record_write_canary_preflight_evidence(evidence_input.clone())
                .await
                .expect("replay the exact preflight evidence safely"),
            evidence
        );
        let mut changed_evidence = evidence_input.clone();
        changed_evidence.readback_state_sha256 = HASH_B.to_string();
        assert!(matches!(
            repository
                .record_write_canary_preflight_evidence(changed_evidence)
                .await,
            Err(MirrorError::InvalidInput(
                "canary_preflight_evidence_already_recorded"
            ))
        ));
        assert!(sqlx::query(
            "UPDATE tally_write_canary_preflight_evidence SET verified_at_unix_ms = 1"
        )
        .execute(&repository.pool)
        .await
        .is_err());
        assert!(
            sqlx::query("DELETE FROM tally_write_canary_preflight_evidence")
                .execute(&repository.pool)
                .await
                .is_err()
        );
        let mut changed = input;
        changed.identity_query_sha256 = HASH_B.to_string();
        assert!(matches!(
            repository.bind_write_canary_payload(changed).await,
            Err(MirrorError::InvalidInput("canary_payload_already_bound"))
        ));
        assert!(
            sqlx::query("UPDATE tally_write_canary_payload_bindings SET bound_at_unix_ms = 1")
                .execute(&repository.pool)
                .await
                .is_err()
        );
        assert!(
            sqlx::query("DELETE FROM tally_write_canary_payload_bindings")
                .execute(&repository.pool)
                .await
                .is_err()
        );

        repository
            .revoke_write_fixture_enrollment(&saved.company.id, 6_000)
            .await
            .expect("revoke fixture before a second binding");
        assert!(matches!(
            repository
                .active_write_canary_payload_binding(active_binding)
                .await,
            Err(MirrorError::InvalidInput(
                "canary_payload_binding_not_active"
            ))
        ));
        assert!(matches!(
            repository
                .begin_write_canary_preflight(BeginWriteCanaryPreflightInput {
                    binding: ActiveWriteCanaryPayloadBindingInput {
                        company_id: saved.company.id.clone(),
                        review_commitment_sha256: HASH_B.to_string(),
                        reservation_id: reservation.id.clone(),
                        reservation_payload_sha256: reservation_payload_sha256.clone(),
                        wire_sha256: HASH_A.to_string(),
                        intended_state_sha256: HASH_B.to_string(),
                        identity_query_sha256: HASH_A.to_string(),
                    },
                    started_at_unix_ms: 6_500,
                })
                .await,
            Err(MirrorError::InvalidInput(
                "canary_payload_binding_not_active"
            ))
        ));
        assert!(matches!(
            repository
                .record_write_canary_preflight_evidence(evidence_input)
                .await,
            Err(MirrorError::InvalidInput("canary_preflight_not_active"))
        ));
        assert!(matches!(
            repository
                .bind_write_canary_payload(WriteCanaryPayloadBindingInput {
                    company_id: saved.company.id,
                    review_commitment_sha256: HASH_B.to_string(),
                    reservation_id: reservation.id,
                    reservation_payload_sha256: HASH_A.to_string(),
                    wire_sha256: HASH_A.to_string(),
                    intended_state_sha256: HASH_B.to_string(),
                    identity_query_sha256: HASH_A.to_string(),
                    bound_at_unix_ms: 7_000,
                })
                .await,
            Err(MirrorError::InvalidInput("canary_reservation_not_active"))
        ));
    }

    #[tokio::test]
    async fn v13_fixture_revocations_upgrade_to_durable_sequence() {
        let repository = repository_through_v13().await;
        let saved = repository
            .save_reviewed_setup(reviewed_setup_input(HASH_A))
            .await
            .expect("seed observed company for legacy fixture evidence");
        sqlx::query(
            "INSERT INTO tally_write_fixture_enrollments(\
               id, company_id, review_commitment_sha256, enrollment_payload_sha256, \
               contract_version, disposable_company_attested, no_customer_data_attested, \
               backup_guidance_acknowledged, enrolled_at_unix_ms\
             ) VALUES ('legacy-enrollment', ?1, ?2, ?3, 1, 1, 1, 1, 3000)",
        )
        .bind(&saved.company.id)
        .bind(HASH_B)
        .bind(HASH_A)
        .execute(&repository.pool)
        .await
        .expect("seed legacy enrollment");
        sqlx::query(
            "INSERT INTO tally_write_fixture_revocations(\
               id, enrollment_id, revocation_payload_sha256, safe_reason_code, revoked_at_unix_ms\
             ) VALUES ('legacy-revocation', 'legacy-enrollment', ?1, 'operator_revoked', 4000)",
        )
        .bind(HASH_B)
        .execute(&repository.pool)
        .await
        .expect("seed legacy revocation");

        repository
            .migrate()
            .await
            .expect("upgrade fixture revocation evidence from v13 to v14");
        let status = repository
            .write_fixture_enrollment_status(&saved.company.id)
            .await
            .expect("read upgraded legacy fixture status");
        assert_eq!(status.fixture_state, "revoked");
        assert_eq!(status.revoked_at_unix_ms, Some(4_000));
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT event_sequence FROM tally_write_fixture_revocations WHERE id = 'legacy-revocation'",
            )
            .fetch_one(&repository.pool)
            .await
            .expect("read durable backfilled sequence"),
            1
        );
        assert!(sqlx::query(
            "UPDATE tally_write_fixture_revocations SET event_sequence = 2 WHERE id = 'legacy-revocation'",
        )
        .execute(&repository.pool)
        .await
        .is_err());
    }

    #[tokio::test]
    async fn already_sequenced_v13_fixture_revocations_upgrade_idempotently() {
        let repository = repository_through_v13().await;
        let saved = repository
            .save_reviewed_setup(reviewed_setup_input(HASH_A))
            .await
            .expect("seed observed company for already-sequenced v13 fixture evidence");
        // Emulate the pre-merge v13 schema that already carried this column.
        sqlx::query(
            "ALTER TABLE tally_write_fixture_revocations \
             ADD COLUMN event_sequence INTEGER NOT NULL DEFAULT 0",
        )
        .execute(&repository.pool)
        .await
        .expect("add pre-existing legacy event sequence");

        repository
            .migrate()
            .await
            .expect("upgrade already-sequenced v13 schema without duplicate column");
        repository
            .enroll_write_fixture(WriteFixtureEnrollmentInput {
                company_id: saved.company.id.clone(),
                review_commitment_sha256: HASH_B.to_string(),
                disposable_company_attested: true,
                no_customer_data_attested: true,
                backup_guidance_acknowledged: true,
                enrolled_at_unix_ms: 3_000,
            })
            .await
            .expect("enroll after already-sequenced upgrade");
        repository
            .revoke_write_fixture_enrollment(&saved.company.id, 4_000)
            .await
            .expect("revoke after already-sequenced upgrade");
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT event_sequence FROM tally_write_fixture_revocations LIMIT 1",
            )
            .fetch_one(&repository.pool)
            .await
            .expect("read durable sequence after alternate upgrade path"),
            1
        );
    }

    #[tokio::test]
    async fn reviewed_setup_atomically_persists_scoped_selected_read_evidence() {
        let repository = repository().await;
        let feature = |key: &str, state, confidence, reason: &str| CapabilityItemInput {
            kind: CapabilityKind::Feature,
            key: key.to_string(),
            state,
            confidence,
            safe_reason_code: Some(reason.to_string()),
        };
        let observation = |key: &str, date_window_verified| SelectedReadObservationInput {
            capability_key: key.to_string(),
            state: CapabilityState::Supported,
            confidence: Confidence::Observed,
            safe_reason_code: if key == "selected_ledger_read" {
                "selected_ledger_read_non_empty_observed".to_string()
            } else {
                "selected_voucher_window_non_empty_observed".to_string()
            },
            result_bucket: "non_empty_observed".to_string(),
            request_sha256: Some(HASH_A.to_string()),
            decoded_response_sha256: Some(HASH_B.to_string()),
            response_encoding: Some("utf8".to_string()),
            company_context_verified: true,
            schema_verified: true,
            record_count_verified: true,
            identity_evidence_state: "verified".to_string(),
            date_window_verified,
        };
        let observations = vec![
            observation("selected_ledger_read", false),
            observation("selected_voucher_window_read", true),
        ];
        let commitment_observations = observations
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
            .collect();
        let scope_commitment_sha256 =
            selected_read_scope_commitment_sha256(&SelectedReadScopeCommitmentMaterial {
                parent_review_commitment_sha256: HASH_B.to_string(),
                canonical_origin: "http://127.0.0.1:9000".to_string(),
                company_guid_ascii_casefolded: "qualified-company-guid".to_string(),
                company_name: "Synthetic Qualified Company".to_string(),
                ledger_profile_id: "bridge.tally.ledgers/1".to_string(),
                voucher_profile_id: "bridge.tally.vouchers/3".to_string(),
                voucher_from_yyyymmdd: "20260701".to_string(),
                voucher_to_yyyymmdd: "20260731".to_string(),
                observed_at_unix_ms: 2_000,
                observations: commitment_observations,
            })
            .expect("compute selected-read commitment");
        let saved = repository
            .save_reviewed_setup(ReviewedSetupInput {
                review_commitment_sha256: HASH_A.to_string(),
                capability: CapabilitySnapshotInput {
                    canonical_origin: "http://127.0.0.1:9000".to_string(),
                    observed_at_unix_ms: 2_000,
                    profile_version: 3,
                    product: "Unknown".to_string(),
                    release: None,
                    mode: None,
                    mode_confidence: Confidence::Unknown,
                    items: vec![
                        feature(
                            "ledger_read",
                            CapabilityState::Unknown,
                            Confidence::Unknown,
                            "selected_read_probe_not_run",
                        ),
                        feature(
                            "voucher_read",
                            CapabilityState::Unknown,
                            Confidence::Unknown,
                            "selected_read_probe_not_run",
                        ),
                        feature(
                            "selected_ledger_read",
                            CapabilityState::Supported,
                            Confidence::Observed,
                            "selected_ledger_read_non_empty_observed",
                        ),
                        feature(
                            "selected_voucher_window_read",
                            CapabilityState::Supported,
                            Confidence::Observed,
                            "selected_voucher_window_non_empty_observed",
                        ),
                    ],
                },
                company_display_name: "Synthetic Qualified Company".to_string(),
                company_identity: SourceIdentityInput {
                    guid: Some("qualified-company-guid".to_string()),
                    confidence: Some(Confidence::Observed),
                    ..Default::default()
                },
                selected_read_scope: Some(SelectedReadScopeInput {
                    scope_commitment_sha256,
                    parent_review_sha256: HASH_B.to_string(),
                    ledger_profile_id: "bridge.tally.ledgers/1".to_string(),
                    voucher_profile_id: "bridge.tally.vouchers/3".to_string(),
                    voucher_from_yyyymmdd: "20260701".to_string(),
                    voucher_to_yyyymmdd: "20260731".to_string(),
                    observed_at_unix_ms: 2_000,
                    observations,
                }),
            })
            .await
            .expect("atomically save selected-read evidence");

        let scope_count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_selected_read_scopes \
             WHERE capability_snapshot_id = ?1 AND company_id = ?2",
        )
        .bind(&saved.snapshot.id)
        .bind(&saved.company.id)
        .fetch_one(&repository.pool)
        .await
        .expect("count saved selected-read scope");
        let observation_count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_selected_read_observations \
             WHERE capability_snapshot_id = ?1",
        )
        .bind(&saved.snapshot.id)
        .fetch_one(&repository.pool)
        .await
        .expect("count saved selected-read observations");
        assert_eq!((scope_count, observation_count), (1, 2));
        assert!(sqlx::query(
            "UPDATE tally_selected_read_scopes SET completeness_state = 'not_claimed' WHERE capability_snapshot_id = ?1",
        )
        .bind(&saved.snapshot.id)
        .execute(&repository.pool)
        .await
        .is_err());
    }

    #[tokio::test]
    async fn selected_read_observation_cannot_cross_wire_scope_and_snapshot() {
        let repository = repository().await;
        sqlx::raw_sql(
            "INSERT INTO tally_endpoints VALUES ('ep', 'http://127.0.0.1:9000', 1, 2);\
             INSERT INTO tally_capability_snapshots VALUES ('snap-a', 'ep', 1, 3, 'Unknown', NULL, NULL, 'unknown');\
             INSERT INTO tally_capability_snapshots VALUES ('snap-b', 'ep', 2, 3, 'Unknown', NULL, NULL, 'unknown');\
             INSERT INTO tally_capability_items VALUES ('snap-b', 'feature', 'selected_ledger_read', 'unknown', 'unknown', 'qualification_prerequisite_failed');\
             INSERT INTO tally_companies VALUES ('company', 'ep', 'Synthetic', 'company-guid', NULL, NULL, NULL, 'observed', 1, 2);\
             INSERT INTO tally_selected_read_scopes VALUES (\
               'scope-a', 'snap-a', 'company', 1,\
               'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',\
               'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',\
               'bridge.tally.ledgers/1', 'bridge.tally.vouchers/3',\
               '20260701', '20260731', 2, 'not_claimed', 1, 0\
             );",
        )
        .execute(&repository.pool)
        .await
        .expect("seed two independent snapshot authorities");

        let error = sqlx::query(
            "INSERT INTO tally_selected_read_observations(\
               scope_id, capability_snapshot_id, capability_kind, capability_key,\
               capability_state, confidence, safe_reason_code, result_bucket,\
               request_sha256, decoded_response_sha256, response_encoding,\
               company_context_verified, schema_verified, record_count_verified,\
               identity_evidence_state, date_window_verified\
             ) VALUES (\
               'scope-a', 'snap-b', 'feature', 'selected_ledger_read',\
               'unknown', 'unknown', 'qualification_prerequisite_failed', 'skipped',\
               NULL, NULL, NULL, 0, 0, 0, 'unverified', 0\
             )",
        )
        .execute(&repository.pool)
        .await
        .expect_err("scope and observation snapshot must be the same authority");
        assert!(error
            .to_string()
            .to_ascii_lowercase()
            .contains("foreign key"));
    }

    async fn setup_row_counts(repository: &TallyMirrorRepository) -> (i64, i64, i64, i64) {
        let endpoints = sqlx::query_scalar("SELECT COUNT(*) FROM tally_endpoints")
            .fetch_one(&repository.pool)
            .await
            .unwrap();
        let snapshots = sqlx::query_scalar("SELECT COUNT(*) FROM tally_capability_snapshots")
            .fetch_one(&repository.pool)
            .await
            .unwrap();
        let items = sqlx::query_scalar("SELECT COUNT(*) FROM tally_capability_items")
            .fetch_one(&repository.pool)
            .await
            .unwrap();
        let companies = sqlx::query_scalar("SELECT COUNT(*) FROM tally_companies")
            .fetch_one(&repository.pool)
            .await
            .unwrap();
        (endpoints, snapshots, items, companies)
    }

    async fn begin_batch(
        repository: &TallyMirrorRepository,
        snapshot: &CapabilitySnapshotRef,
        company: &CompanyRef,
        run_id: &str,
    ) -> String {
        repository
            .begin_batch(BeginBatchInput {
                run_id: run_id.to_string(),
                capability_snapshot_id: snapshot.id.clone(),
                company_id: company.id.clone(),
                pack_id: "core_accounting".to_string(),
                pack_schema_major: 1,
                pack_schema_minor: 0,
                source_transport: "xml_http".to_string(),
                source_release: None,
                requested_from_yyyymmdd: Some("20260401".to_string()),
                requested_to_yyyymmdd: Some("20260401".to_string()),
                started_at_unix_ms: 2_000,
            })
            .await
            .expect("begin batch")
    }

    fn replayable_observation(batch_id: &str) -> ObservedRecordInput {
        ObservedRecordInput {
            batch_id: batch_id.to_string(),
            object_type: "ledger".to_string(),
            display_name: Some("Synthetic Replay Ledger".to_string()),
            identity: SourceIdentityInput {
                guid: Some("ledger-guid-replay".to_string()),
                confidence: Some(Confidence::Observed),
                ..Default::default()
            },
            observed_at_unix_ms: 2_100,
            raw_source_sha256: HASH_A.to_string(),
            canonical_sha256: Some(HASH_B.to_string()),
            canonical_payload: Some(
                json!({"amount": "1180.00", "name": "Synthetic Replay Ledger"}),
            ),
            exact_decimals: BTreeMap::from([(
                "opening_balance".to_string(),
                "1180.00".to_string(),
            )]),
            observed_alter_id: Some("42".to_string()),
            status: ObservationStatus::Accepted,
            safe_rejection_code: None,
        }
    }

    #[tokio::test]
    async fn identical_lost_ack_replay_returns_existing_observation_without_duplication() {
        let (repository, snapshot, company) = seeded_repository().await;
        let batch_id = begin_batch(&repository, &snapshot, &company, "replay-run").await;
        let input = replayable_observation(&batch_id);
        let inserted = repository
            .observe_record_idempotent(input.clone())
            .await
            .expect("insert first observation");
        let ObserveRecordOutcome::Inserted { observation_id } = inserted else {
            panic!("first observation must be inserted");
        };

        let mut replay = input.clone();
        replay.observed_at_unix_ms = 9_999;
        assert_eq!(
            repository
                .observe_record_idempotent(replay.clone())
                .await
                .expect("accept exact lost-ack replay"),
            ObserveRecordOutcome::AlreadyPresentIdentical {
                observation_id: observation_id.clone(),
            }
        );
        let observation_count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_record_observations WHERE batch_id = ?1",
        )
        .bind(&batch_id)
        .fetch_one(&repository.pool)
        .await
        .expect("count replay observations");
        assert_eq!(observation_count, 1);

        assert!(matches!(
            repository.observe_record(replay).await,
            Err(MirrorError::DuplicateObservation)
        ));
    }

    #[tokio::test]
    async fn changed_replay_is_a_conflict_and_mutates_neither_record_nor_observation() {
        let (repository, snapshot, company) = seeded_repository().await;
        let batch_id = begin_batch(&repository, &snapshot, &company, "conflict-run").await;
        let input = replayable_observation(&batch_id);
        repository
            .observe_record_idempotent(input.clone())
            .await
            .expect("insert first observation");

        let source_before = sqlx::query_as::<_, (Option<String>, String, Option<i64>)>(
            "SELECT display_name, last_seen_batch_id, tombstoned_at_unix_ms \
             FROM tally_source_records \
             WHERE company_id = ?1 AND object_type = 'ledger' AND source_guid = ?2",
        )
        .bind(&company.id)
        .bind(input.identity.guid.as_deref())
        .fetch_one(&repository.pool)
        .await
        .expect("read source record before conflict");
        let observation_before = sqlx::query_as::<
            _,
            (
                i64,
                String,
                Option<String>,
                Option<String>,
                String,
                Option<String>,
                String,
                Option<String>,
            ),
        >(
            "SELECT observed_at_unix_ms, raw_source_sha256, canonical_sha256, \
               canonical_payload_json, exact_decimals_json, observed_alter_id, \
               validation_status, safe_rejection_code \
             FROM tally_record_observations WHERE batch_id = ?1",
        )
        .bind(&batch_id)
        .fetch_one(&repository.pool)
        .await
        .expect("read observation before conflict");

        let mut changed = input;
        changed.display_name = Some("Changed Replay Name".to_string());
        changed.observed_at_unix_ms = 8_888;
        changed.raw_source_sha256 = HASH_B.to_string();
        changed.canonical_sha256 = Some(HASH_A.to_string());
        changed.canonical_payload = Some(json!({"amount": "999.00", "name": "Changed"}));
        changed.exact_decimals =
            BTreeMap::from([("opening_balance".to_string(), "999.00".to_string())]);
        assert!(matches!(
            repository.observe_record_idempotent(changed).await,
            Err(MirrorError::ObservationConflict)
        ));
        let mut legacy_changed = replayable_observation(&batch_id);
        legacy_changed.raw_source_sha256 = HASH_B.to_string();
        assert!(matches!(
            repository.observe_record(legacy_changed).await,
            Err(MirrorError::DuplicateObservation)
        ));

        let source_after = sqlx::query_as::<_, (Option<String>, String, Option<i64>)>(
            "SELECT display_name, last_seen_batch_id, tombstoned_at_unix_ms \
             FROM tally_source_records \
             WHERE company_id = ?1 AND object_type = 'ledger' AND source_guid = ?2",
        )
        .bind(&company.id)
        .bind("ledger-guid-replay")
        .fetch_one(&repository.pool)
        .await
        .expect("read source record after conflict");
        let observation_after = sqlx::query_as::<
            _,
            (
                i64,
                String,
                Option<String>,
                Option<String>,
                String,
                Option<String>,
                String,
                Option<String>,
            ),
        >(
            "SELECT observed_at_unix_ms, raw_source_sha256, canonical_sha256, \
               canonical_payload_json, exact_decimals_json, observed_alter_id, \
               validation_status, safe_rejection_code \
             FROM tally_record_observations WHERE batch_id = ?1",
        )
        .bind(&batch_id)
        .fetch_one(&repository.pool)
        .await
        .expect("read observation after conflict");
        assert_eq!(source_after, source_before);
        assert_eq!(observation_after, observation_before);
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM tally_record_observations WHERE batch_id = ?1",
            )
            .bind(&batch_id)
            .fetch_one(&repository.pool)
            .await
            .expect("count observations after conflict"),
            1
        );
    }

    fn unavailable_membership(
        record_key: &str,
        canonical_sha256: &str,
        name: &str,
    ) -> SnapshotWindowMembershipInput {
        SnapshotWindowMembershipInput::ProvenanceUnavailable {
            record_key: record_key.to_string(),
            canonical_sha256: canonical_sha256.to_string(),
            canonical_payload: json!({"name": name}),
            exact_decimals: BTreeMap::new(),
            safe_reason_code: "record_provenance_unavailable".to_string(),
        }
    }

    async fn begin_window_attempt(
        repository: &TallyMirrorRepository,
        batch_id: &str,
        started_at_unix_ms: i64,
    ) -> SnapshotWindowAttemptRef {
        repository
            .begin_snapshot_window_attempt(BeginSnapshotWindowAttemptInput {
                batch_id: batch_id.to_string(),
                window_id: "voucher:20260401:20260401".to_string(),
                started_at_unix_ms,
            })
            .await
            .expect("begin window attempt")
            .attempt
    }

    #[tokio::test]
    async fn normalized_window_staging_is_idempotent_and_loads_bounded_receipt_and_map() {
        let (repository, snapshot, company) = seeded_repository().await;
        let batch_id = begin_batch(&repository, &snapshot, &company, "window-stage-run").await;
        let attempt = begin_window_attempt(&repository, &batch_id, 3_000).await;
        let observed = SnapshotWindowMembershipInput::Observed {
            record_key: "ledger\0ledger-guid-replay".to_string(),
            observation: Box::new(replayable_observation(&batch_id)),
        };
        let unavailable = unavailable_membership(
            "voucher\0voucher-guid-unavailable",
            HASH_A,
            "Synthetic Unavailable Voucher",
        );
        let staged = repository
            .stage_snapshot_window_memberships(
                &attempt,
                vec![observed.clone(), unavailable.clone()],
            )
            .await
            .expect("atomically stage mixed provenance chunk");
        assert_eq!(
            staged,
            StageSnapshotWindowMembershipsResult {
                inserted_memberships: 2,
                inserted_observations: 1,
                provenance_unavailable_memberships: 1,
                ..Default::default()
            }
        );
        let replayed = repository
            .stage_snapshot_window_memberships(&attempt, vec![observed, unavailable])
            .await
            .expect("replay exact mixed provenance chunk");
        assert_eq!(replayed.replayed_memberships, 2);
        assert_eq!(replayed.replayed_observations, 1);
        assert_eq!(replayed.provenance_unavailable_memberships, 1);

        let receipt = repository
            .complete_snapshot_window_attempt(&attempt, 4_000, json!({"response_bytes": 321}))
            .await
            .expect("complete immutable attempt");
        assert_eq!(receipt.member_count, 2);
        assert_eq!(
            repository
                .load_latest_completed_window_receipt(&batch_id, &attempt.window_id)
                .await
                .expect("load latest receipt"),
            Some(receipt.clone())
        );
        let map = repository
            .load_completed_window_canonical_record_map(&attempt)
            .await
            .expect("load ordered canonical map");
        assert_eq!(
            map.keys().cloned().collect::<Vec<_>>(),
            vec![
                "ledger\0ledger-guid-replay".to_string(),
                "voucher\0voucher-guid-unavailable".to_string()
            ]
        );
        assert!(matches!(
            repository
                .stage_snapshot_window_membership(
                    &attempt,
                    unavailable_membership("voucher\0later", HASH_B, "Later")
                )
                .await,
            Err(MirrorError::WindowAttemptClosed)
        ));
        assert!(sqlx::query(
            "UPDATE tally_snapshot_window_attempts SET receipt_sha256 = ?1 WHERE id = ?2",
        )
        .bind(HASH_B)
        .bind(&attempt.attempt_id)
        .execute(&repository.pool)
        .await
        .is_err());
        assert!(sqlx::query(
            "UPDATE tally_snapshot_window_memberships SET canonical_sha256 = ?1 \
             WHERE batch_id = ?2 AND window_id = ?3 AND record_key = ?4",
        )
        .bind(HASH_B)
        .bind(&batch_id)
        .bind(&attempt.window_id)
        .bind("voucher\0voucher-guid-unavailable")
        .execute(&repository.pool)
        .await
        .is_err());
        assert!(sqlx::query(
            "DELETE FROM tally_snapshot_window_memberships \
             WHERE batch_id = ?1 AND window_id = ?2",
        )
        .bind(&batch_id)
        .bind(&attempt.window_id)
        .execute(&repository.pool)
        .await
        .is_err());
    }

    #[tokio::test]
    async fn membership_content_conflict_rolls_back_the_entire_chunk() {
        let (repository, snapshot, company) = seeded_repository().await;
        let batch_id = begin_batch(&repository, &snapshot, &company, "window-conflict-run").await;
        let first = begin_window_attempt(&repository, &batch_id, 3_000).await;
        repository
            .stage_snapshot_window_membership(
                &first,
                unavailable_membership("voucher\0stable", HASH_A, "Stable"),
            )
            .await
            .expect("seed immutable membership");
        let second = begin_window_attempt(&repository, &batch_id, 4_000).await;
        assert!(matches!(
            repository
                .stage_snapshot_window_memberships(
                    &second,
                    vec![
                        unavailable_membership("voucher\0new", HASH_B, "New"),
                        unavailable_membership("voucher\0stable", HASH_B, "Changed"),
                    ],
                )
                .await,
            Err(MirrorError::WindowMembershipConflict)
        ));
        let new_count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_snapshot_window_memberships WHERE record_key = ?1",
        )
        .bind("voucher\0new")
        .fetch_one(&repository.pool)
        .await
        .expect("count rolled-back addition");
        assert_eq!(new_count, 0);
        let last_seen = sqlx::query_scalar::<_, String>(
            "SELECT last_seen_attempt_id FROM tally_snapshot_window_memberships \
             WHERE batch_id = ?1 AND window_id = ?2 AND record_key = ?3",
        )
        .bind(&batch_id)
        .bind(&first.window_id)
        .bind("voucher\0stable")
        .fetch_one(&repository.pool)
        .await
        .expect("read unchanged last seen attempt");
        assert_eq!(last_seen, first.attempt_id);
    }

    #[tokio::test]
    async fn completion_detects_membership_from_abandoned_partial_attempt_and_allows_additions() {
        let (repository, snapshot, company) = seeded_repository().await;
        let batch_id = begin_batch(&repository, &snapshot, &company, "window-disappear-run").await;
        let crashed = begin_window_attempt(&repository, &batch_id, 3_000).await;
        let old = unavailable_membership("voucher\0old", HASH_A, "Old");
        repository
            .stage_snapshot_window_membership(&crashed, old.clone())
            .await
            .expect("stage before synthetic crash");
        let resumed = begin_window_attempt(&repository, &batch_id, 4_000).await;
        let addition = unavailable_membership("voucher\0addition", HASH_B, "Addition");
        repository
            .stage_snapshot_window_membership(&resumed, addition)
            .await
            .expect("stage addition on resumed attempt");
        assert!(matches!(
            repository
                .complete_snapshot_window_attempt(&resumed, 5_000, json!({}))
                .await,
            Err(MirrorError::WindowMembershipDisappeared)
        ));
        repository
            .stage_snapshot_window_membership(&resumed, old)
            .await
            .expect("re-observe pre-crash membership");
        let receipt = repository
            .complete_snapshot_window_attempt(&resumed, 5_001, json!({}))
            .await
            .expect("complete after exact full membership replay");
        assert_eq!(receipt.member_count, 2, "additions remain permitted");
        let crashed_state = sqlx::query_scalar::<_, String>(
            "SELECT state FROM tally_snapshot_window_attempts WHERE id = ?1",
        )
        .bind(crashed.attempt_id)
        .fetch_one(&repository.pool)
        .await
        .expect("read crashed attempt state");
        assert_eq!(crashed_state, "abandoned");
    }

    #[tokio::test]
    async fn chunk_limit_and_owner_binding_fail_closed_without_mutation() {
        let (repository, snapshot, company) = seeded_repository().await;
        let batch_id = begin_batch(&repository, &snapshot, &company, "window-bounds-run").await;
        let attempt = begin_window_attempt(&repository, &batch_id, 3_000).await;
        let oversized = (0..=MAX_WINDOW_STAGE_CHUNK)
            .map(|index| {
                unavailable_membership(&format!("voucher\0item-{index}"), HASH_A, "Synthetic")
            })
            .collect();
        assert!(matches!(
            repository
                .stage_snapshot_window_memberships(&attempt, oversized)
                .await,
            Err(MirrorError::InvalidInput("window_membership_chunk_size"))
        ));
        let forged = SnapshotWindowAttemptRef {
            window_id: "voucher:other".to_string(),
            ..attempt
        };
        assert!(matches!(
            repository
                .stage_snapshot_window_membership(
                    &forged,
                    unavailable_membership("voucher\0forged", HASH_A, "Forged")
                )
                .await,
            Err(MirrorError::NotFound)
        ));
        let membership_count =
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM tally_snapshot_window_memberships")
                .fetch_one(&repository.pool)
                .await
                .expect("count bounded memberships");
        assert_eq!(membership_count, 0);
    }

    #[tokio::test]
    async fn explicit_window_abandon_clamps_clock_rollback_and_is_terminal_immutable() {
        let (repository, snapshot, company) = seeded_repository().await;
        let batch_id = begin_batch(&repository, &snapshot, &company, "window-abandon-run").await;
        let attempt = begin_window_attempt(&repository, &batch_id, 3_000).await;
        repository
            .stage_snapshot_window_membership(
                &attempt,
                unavailable_membership("voucher\0partial", HASH_A, "Partial"),
            )
            .await
            .expect("stage partial membership");
        assert!(matches!(
            repository
                .abandon_snapshot_window_attempt(&attempt, 0)
                .await,
            Err(MirrorError::InvalidInput("window_attempt_completed_at"))
        ));
        let forged = SnapshotWindowAttemptRef {
            window_id: "voucher:foreign".to_string(),
            ..attempt.clone()
        };
        assert!(matches!(
            repository
                .abandon_snapshot_window_attempt(&forged, 4_000)
                .await,
            Err(MirrorError::NotFound)
        ));
        let abandonment = repository
            .abandon_snapshot_window_attempt(&attempt, 2_999)
            .await
            .expect("clock rollback is clamped for terminal cleanup");
        assert_eq!(
            abandonment,
            AbandonSnapshotWindowAttemptResult {
                completed_at_unix_ms: 3_000,
                local_clock_moved_backwards: true,
            }
        );
        let stored = sqlx::query_as::<
            _,
            (
                String,
                Option<i64>,
                Option<String>,
                Option<String>,
                Option<String>,
            ),
        >(
            "SELECT state, completed_at_unix_ms, receipt_json, receipt_sha256, \
               terminal_safe_reason_code \
             FROM tally_snapshot_window_attempts WHERE id = ?1",
        )
        .bind(&attempt.attempt_id)
        .fetch_one(&repository.pool)
        .await
        .expect("read abandoned attempt");
        assert_eq!(
            stored,
            (
                "abandoned".to_string(),
                Some(3_000),
                None,
                None,
                Some("local_clock_moved_backwards".to_string()),
            )
        );
        let replayed = repository
            .abandon_snapshot_window_attempt(&attempt, 4_001)
            .await
            .expect("lost acknowledgement replays persisted abandonment evidence");
        assert_eq!(replayed, abandonment);
        assert!(matches!(
            repository
                .complete_snapshot_window_attempt(&attempt, 4_001, json!({}))
                .await,
            Err(MirrorError::WindowAttemptClosed)
        ));
        assert!(sqlx::query(
            "UPDATE tally_snapshot_window_attempts SET completed_at_unix_ms = 5000 WHERE id = ?1",
        )
        .bind(&attempt.attempt_id)
        .execute(&repository.pool)
        .await
        .is_err());
    }

    #[tokio::test]
    async fn implicit_begin_abandonment_replays_cumulative_clock_evidence() {
        let (repository, snapshot, company) = seeded_repository().await;
        let batch_id =
            begin_batch(&repository, &snapshot, &company, "window-begin-clock-run").await;
        let first = repository
            .begin_snapshot_window_attempt(BeginSnapshotWindowAttemptInput {
                batch_id: batch_id.clone(),
                window_id: "voucher:clock-window".to_string(),
                started_at_unix_ms: 5_000,
            })
            .await
            .unwrap();
        assert!(first.prior_abandonment.is_none());
        let second = repository
            .begin_snapshot_window_attempt(BeginSnapshotWindowAttemptInput {
                batch_id: batch_id.clone(),
                window_id: "voucher:clock-window".to_string(),
                started_at_unix_ms: 4_000,
            })
            .await
            .unwrap();
        assert_eq!(
            second.prior_abandonment,
            Some(AbandonSnapshotWindowAttemptResult {
                completed_at_unix_ms: 5_000,
                local_clock_moved_backwards: true,
            })
        );
        let third = repository
            .begin_snapshot_window_attempt(BeginSnapshotWindowAttemptInput {
                batch_id,
                window_id: "voucher:clock-window".to_string(),
                started_at_unix_ms: 6_000,
            })
            .await
            .unwrap();
        assert_eq!(third.prior_abandonment, second.prior_abandonment);
        assert_eq!(third.attempt.attempt_ordinal, 3);
    }

    #[tokio::test]
    async fn window_completion_clamps_and_reloads_clock_rollback_evidence() {
        let (repository, snapshot, company) = seeded_repository().await;
        let batch_id = begin_batch(
            &repository,
            &snapshot,
            &company,
            "window-complete-clock-run",
        )
        .await;
        let attempt = begin_window_attempt(&repository, &batch_id, 5_000).await;
        repository
            .stage_snapshot_window_membership(
                &attempt,
                unavailable_membership("voucher\0clock", HASH_A, "Clock"),
            )
            .await
            .unwrap();
        let completion = repository
            .complete_snapshot_window_attempt(&attempt, 4_999, json!({}))
            .await
            .expect("completion clamps rollback instead of stranding the run");
        assert!(completion.local_clock_moved_backwards);
        assert_eq!(completion.completed_at_unix_ms, 5_000);
        assert_eq!(
            repository
                .load_latest_completed_window_receipt(&batch_id, &attempt.window_id)
                .await
                .unwrap(),
            Some(completion)
        );
    }

    #[tokio::test]
    async fn completed_window_attempt_cannot_be_abandoned() {
        let (repository, snapshot, company) = seeded_repository().await;
        let batch_id = begin_batch(&repository, &snapshot, &company, "window-complete-run").await;
        let attempt = begin_window_attempt(&repository, &batch_id, 3_000).await;
        repository
            .stage_snapshot_window_membership(
                &attempt,
                unavailable_membership("voucher\0complete", HASH_A, "Complete"),
            )
            .await
            .expect("stage complete membership");
        repository
            .complete_snapshot_window_attempt(&attempt, 4_000, json!({}))
            .await
            .expect("complete attempt");
        assert!(matches!(
            repository
                .abandon_snapshot_window_attempt(&attempt, 4_001)
                .await,
            Err(MirrorError::WindowAttemptClosed)
        ));
        let state = sqlx::query_scalar::<_, String>(
            "SELECT state FROM tally_snapshot_window_attempts WHERE id = ?1",
        )
        .bind(attempt.attempt_id)
        .fetch_one(&repository.pool)
        .await
        .expect("read complete state");
        assert_eq!(state, "complete");
    }

    #[tokio::test]
    async fn commit_rejects_open_attempt_until_proof_bound_cleanup_completes() {
        let (repository, snapshot, company) = seeded_repository().await;
        let run_id = "orphan-window-terminal-run";
        let batch_id = begin_batch(&repository, &snapshot, &company, run_id).await;
        let attempt = begin_window_attempt(&repository, &batch_id, 3_000).await;
        repository
            .stage_snapshot_window_membership(
                &attempt,
                unavailable_membership("voucher\0orphan", HASH_A, "Orphan"),
            )
            .await
            .expect("stage unavailable orphan membership");

        let input = CommitBatchInput::test_only(CommitBatchParts {
            batch_id: batch_id.clone(),
            proof_contract_version: 2,
            outcome: RunOutcome::Failed,
            verification: VerificationState::Unverified,
            completed_at_unix_ms: 4_000,
            record_counts_sha256: None,
            snapshot_sha256: None,
            expected_checkpoint_before: None,
            checkpoint_after: None,
            freshness_target_seconds: 60,
            gap_codes: vec!["record_provenance_unavailable".to_string()],
            warning_codes: Vec::new(),
        });
        assert!(matches!(
            repository.commit_batch(input.clone()).await,
            Err(MirrorError::OpenWindowAttempts)
        ));
        assert_eq!(
            sqlx::query_scalar::<_, String>(
                "SELECT state FROM tally_snapshot_window_attempts WHERE id = ?1",
            )
            .bind(&attempt.attempt_id)
            .fetch_one(&repository.pool)
            .await
            .unwrap(),
            "open"
        );
        repository
            .abandon_open_snapshot_window_attempts_for_batch(&batch_id, 4_000)
            .await
            .expect("proof preparation closes attempts before commit");
        let receipt = repository
            .commit_batch(input)
            .await
            .expect("commit succeeds only after explicit evidence-bearing cleanup");
        assert_eq!(receipt.facts.provenance_unavailable_records, 1);
        assert_eq!(
            sqlx::query_scalar::<_, String>(
                "SELECT state FROM tally_snapshot_window_attempts WHERE id = ?1",
            )
            .bind(&attempt.attempt_id)
            .fetch_one(&repository.pool)
            .await
            .unwrap(),
            "abandoned"
        );
        assert!(matches!(
            repository
                .stage_snapshot_window_membership(
                    &attempt,
                    unavailable_membership("voucher\0late", HASH_B, "Late"),
                )
                .await,
            Err(MirrorError::WindowAttemptClosed)
        ));
        let recovered = repository
            .historical_commit_receipt_for_batch(&batch_id, run_id)
            .await
            .expect("v2 unavailable count is ledger-bound");
        assert_eq!(recovered.facts.provenance_unavailable_records, 1);
    }

    #[tokio::test]
    async fn window_completion_clamps_pre_start_time_and_begin_handles_clock_rollback() {
        let (repository, snapshot, company) = seeded_repository().await;
        let batch_id = begin_batch(&repository, &snapshot, &company, "window-clock-run").await;
        let first = begin_window_attempt(&repository, &batch_id, 5_000).await;
        let completion = repository
            .complete_snapshot_window_attempt(&first, 4_999, json!({}))
            .await
            .expect("pre-start completion is clamped with durable rollback evidence");
        assert_eq!(completion.receipt.completed_at_unix_ms, 5_000);
        assert!(completion.local_clock_moved_backwards);
        let second = begin_window_attempt(&repository, &batch_id, 4_000).await;
        let first_terminal = sqlx::query_as::<_, (String, i64)>(
            "SELECT state, completed_at_unix_ms FROM tally_snapshot_window_attempts WHERE id = ?1",
        )
        .bind(&first.attempt_id)
        .fetch_one(&repository.pool)
        .await
        .expect("read clock-rollback abandonment");
        assert_eq!(first_terminal, ("complete".to_string(), 5_000));
        assert!(sqlx::query(
            "UPDATE tally_snapshot_window_attempts \
             SET state = 'abandoned', completed_at_unix_ms = 3999 WHERE id = ?1",
        )
        .bind(&second.attempt_id)
        .execute(&repository.pool)
        .await
        .is_err());
    }

    #[tokio::test]
    async fn window_membership_receipt_digest_pages_without_changing_commitment() {
        let (repository, snapshot, company) = seeded_repository().await;
        let batch_id = begin_batch(&repository, &snapshot, &company, "window-paged-digest").await;
        let attempt = begin_window_attempt(&repository, &batch_id, 3_000).await;
        let memberships = (0..513)
            .map(|index| {
                unavailable_membership(
                    &format!("voucher\0paged-{index:04}"),
                    HASH_A,
                    &format!("Paged {index:04}"),
                )
            })
            .collect::<Vec<_>>();
        for chunk in memberships.chunks(MAX_WINDOW_STAGE_CHUNK) {
            repository
                .stage_snapshot_window_memberships(&attempt, chunk.to_vec())
                .await
                .expect("stage bounded digest page fixture");
        }
        let expected_values = (0..513)
            .map(|index| {
                (
                    format!("voucher\0paged-{index:04}"),
                    HASH_A.to_string(),
                    "unavailable".to_string(),
                )
            })
            .collect::<Vec<_>>();
        let expected_entries = expected_values
            .iter()
            .map(|(record_key, canonical_sha256, provenance_state)| {
                SnapshotWindowMembershipDigestEntry {
                    record_key,
                    canonical_sha256,
                    provenance_state,
                }
            })
            .collect::<Vec<_>>();
        let expected_sha256 = sha256_json(&expected_entries).expect("hash legacy vector form");
        let receipt = repository
            .complete_snapshot_window_attempt(&attempt, 4_000, json!({}))
            .await
            .expect("complete paged digest attempt");
        assert_eq!(receipt.member_count, 513);
        assert_eq!(receipt.membership_sha256, expected_sha256);
        assert_eq!(
            repository
                .load_latest_completed_window_receipt(&batch_id, &attempt.window_id)
                .await
                .expect("revalidate paged receipt"),
            Some(receipt)
        );
    }

    #[tokio::test]
    async fn earlier_v9_schema_upgrades_additively_and_preserves_v1_proof_hashes() {
        let repository = repository_through_v9().await;
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM pragma_table_info('tally_proof_ledger') \
                 WHERE name = 'provenance_unavailable_records'",
            )
            .fetch_one(&repository.pool)
            .await
            .unwrap(),
            0
        );
        let (repository, snapshot, company) = seed_repository(repository).await;
        let run_id = "legacy-v1-proof-run";
        let batch_id = begin_batch(&repository, &snapshot, &company, run_id).await;
        let proof_id = Uuid::new_v4().to_string();
        let completed_at_unix_ms = 3_000;
        let created_at_unix_ms = 3_001;
        let empty_codes = Vec::<String>::new();
        let proof_sha256 = sha256_json(&ProofHashInput {
            proof_contract_version: 1,
            previous_entry_sha256: None,
            proof_id: &proof_id,
            run_id,
            batch_id: &batch_id,
            capability_snapshot_id: &snapshot.id,
            company_id: &company.id,
            pack_id: "core_accounting",
            outcome: RunOutcome::Failed,
            verification: VerificationState::Unverified,
            started_at_unix_ms: 2_000,
            completed_at_unix_ms,
            accepted_records: 0,
            rejected_records: 0,
            provenance_unavailable_records: None,
            record_counts_sha256: None,
            snapshot_sha256: None,
            checkpoint_before: None,
            checkpoint_after: None,
            gap_codes: &empty_codes,
            warning_codes: &empty_codes,
            created_at_unix_ms,
        })
        .unwrap();
        sqlx::query(
            "UPDATE tally_observation_batches SET state = 'failed', \
             completed_at_unix_ms = ?1 WHERE id = ?2",
        )
        .bind(completed_at_unix_ms)
        .bind(&batch_id)
        .execute(&repository.pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO tally_proof_ledger(\
               id, proof_contract_version, previous_entry_sha256, entry_sha256, run_id, batch_id, \
               capability_snapshot_id, company_id, pack_id, outcome, verification_state, \
               started_at_unix_ms, completed_at_unix_ms, accepted_records, rejected_records, \
               snapshot_sha256, checkpoint_before, checkpoint_after, gap_codes_json, \
               warning_codes_json, created_at_unix_ms\
             ) VALUES (?1, 1, NULL, ?2, ?3, ?4, ?5, ?6, 'core_accounting', 'failed', \
               'unverified', 2000, ?7, 0, 0, NULL, NULL, NULL, '[]', '[]', ?8)",
        )
        .bind(&proof_id)
        .bind(&proof_sha256)
        .bind(run_id)
        .bind(&batch_id)
        .bind(&snapshot.id)
        .bind(&company.id)
        .bind(completed_at_unix_ms)
        .bind(created_at_unix_ms)
        .execute(&repository.pool)
        .await
        .unwrap();

        repository.migrate().await.expect("upgrade v9 through v12");
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM tally_schema_migrations WHERE version = 10",
            )
            .fetch_one(&repository.pool)
            .await
            .unwrap(),
            1
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM tally_schema_migrations WHERE version = 11",
            )
            .fetch_one(&repository.pool)
            .await
            .unwrap(),
            1
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM tally_schema_migrations WHERE version = 12",
            )
            .fetch_one(&repository.pool)
            .await
            .unwrap(),
            1
        );
        let receipt = repository
            .historical_commit_receipt_for_batch(&batch_id, run_id)
            .await
            .expect("v1 receipt remains hash-valid after v10");
        assert_eq!(receipt.proof_sha256, proof_sha256);
        assert_eq!(receipt.facts.proof_contract_version, 1);
        assert_eq!(receipt.facts.provenance_unavailable_records, 0);
        assert_eq!(receipt.facts.record_counts_sha256, None);
    }

    #[tokio::test]
    async fn migration_is_versioned_and_idempotent() {
        let repository = repository().await;
        repository.migrate().await.expect("reapply migration");
        let table_count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name IN (\
             'tally_capability_snapshots', 'tally_companies', 'tally_observation_batches', \
             'tally_record_observations', 'tally_proof_ledger', 'tally_checkpoints')",
        )
        .fetch_one(&repository.pool)
        .await
        .expect("count mirror tables");
        let migration_count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_schema_migrations \
             WHERE version IN (2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18)",
        )
        .fetch_one(&repository.pool)
        .await
        .expect("count migration marker");
        assert_eq!(table_count, 6);
        assert_eq!(migration_count, 17);
        let snapshot_state_table = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM sqlite_master \
             WHERE type = 'table' AND name = 'tally_snapshot_run_states'",
        )
        .fetch_one(&repository.pool)
        .await
        .expect("count snapshot state table");
        assert_eq!(snapshot_state_table, 1);
        let incremental_table_count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name IN (\
             'tally_incremental_capability_observations', \
             'tally_incremental_establishment_receipts', \
             'tally_incremental_checkpoint_heads')",
        )
        .fetch_one(&repository.pool)
        .await
        .expect("count incremental foundation tables");
        let incremental_trigger_count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'trigger' AND name LIKE \
             'tally_incremental_%'",
        )
        .fetch_one(&repository.pool)
        .await
        .expect("count incremental immutability triggers");
        assert_eq!(incremental_table_count, 3);
        assert_eq!(incremental_trigger_count, 9);
        let selected_read_tables = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name IN (\
             'tally_selected_read_scopes', 'tally_selected_read_observations')",
        )
        .fetch_one(&repository.pool)
        .await
        .expect("count selected-read evidence tables");
        assert_eq!(selected_read_tables, 2);
        let reviewed_setup_consumption_tables = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' \
             AND name = 'tally_reviewed_setup_consumptions'",
        )
        .fetch_one(&repository.pool)
        .await
        .expect("count reviewed-setup consumption table");
        assert_eq!(reviewed_setup_consumption_tables, 1);
        let write_fixture_tables = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name IN (\
             'tally_write_fixture_enrollments', 'tally_write_fixture_revocations', \
             'tally_write_canary_reservations', 'tally_write_canary_payload_bindings')",
        )
        .fetch_one(&repository.pool)
        .await
        .expect("count durable write fixture tables");
        assert_eq!(write_fixture_tables, 4);
        let normalized_staging_tables = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name IN (\
             'tally_snapshot_window_attempts', 'tally_snapshot_window_memberships')",
        )
        .fetch_one(&repository.pool)
        .await
        .expect("count normalized staging tables");
        assert_eq!(normalized_staging_tables, 2);
        let guid_index_sql = sqlx::query_scalar::<_, String>(
            "SELECT sql FROM sqlite_master WHERE type = 'index' AND name = 'uq_tally_companies_guid'",
        )
        .fetch_one(&repository.pool)
        .await
        .expect("read company GUID index");
        assert!(guid_index_sql.contains("company_guid COLLATE NOCASE"));
        let applied_at = sqlx::query_scalar::<_, i64>(
            "SELECT MIN(applied_at_unix_ms) FROM tally_schema_migrations \
             WHERE version IN (2, 3, 4, 5, 6, 7, 8, 9)",
        )
        .fetch_one(&repository.pool)
        .await
        .expect("read migration timestamp");
        assert!(applied_at > 0, "migration marker must contain real time");
    }

    #[tokio::test]
    async fn v3_receipt_binds_canonical_record_counts_and_rejects_pre_start_completion() {
        let (repository, snapshot, company) = seeded_repository().await;
        let batch_id = begin_batch(&repository, &snapshot, &company, "v3-count-binding").await;
        let record_counts = BTreeMap::from([
            ("group.accepted_unique".to_string(), 3),
            ("locally_staged.accepted".to_string(), 3),
            ("locally_staged.rejected".to_string(), 0),
        ]);
        let record_counts_sha256 = proof_record_counts_sha256(&record_counts);
        let receipt = repository
            .commit_batch(CommitBatchInput::test_only(CommitBatchParts {
                batch_id: batch_id.clone(),
                proof_contract_version: 3,
                outcome: RunOutcome::Failed,
                verification: VerificationState::Unverified,
                completed_at_unix_ms: 2_500,
                record_counts_sha256: Some(record_counts_sha256.clone()),
                snapshot_sha256: None,
                expected_checkpoint_before: None,
                checkpoint_after: None,
                freshness_target_seconds: 60,
                gap_codes: vec!["source_outcome_unknown".to_string()],
                warning_codes: Vec::new(),
            }))
            .await
            .expect("commit v3 count-bound proof");
        assert_eq!(
            receipt.facts.record_counts_sha256.as_deref(),
            Some(record_counts_sha256.as_str())
        );
        let recovered = repository
            .historical_commit_receipt_for_batch(&batch_id, "v3-count-binding")
            .await
            .expect("revalidate count-bound historical receipt");
        assert_eq!(recovered, receipt);

        let clock_batch = begin_batch(&repository, &snapshot, &company, "clock-regression").await;
        let error = repository
            .commit_batch(CommitBatchInput::test_only(CommitBatchParts {
                batch_id: clock_batch.clone(),
                proof_contract_version: 3,
                outcome: RunOutcome::Failed,
                verification: VerificationState::Unverified,
                completed_at_unix_ms: 1_999,
                record_counts_sha256: Some(proof_record_counts_sha256(&BTreeMap::new())),
                snapshot_sha256: None,
                expected_checkpoint_before: None,
                checkpoint_after: None,
                freshness_target_seconds: 60,
                gap_codes: vec!["source_outcome_unknown".to_string()],
                warning_codes: Vec::new(),
            }))
            .await
            .expect_err("proof completion before batch start must fail closed");
        assert!(matches!(
            error,
            MirrorError::InvalidInput("batch_completed_at")
        ));
        assert_eq!(
            sqlx::query_scalar::<_, String>(
                "SELECT state FROM tally_observation_batches WHERE id = ?1",
            )
            .bind(clock_batch)
            .fetch_one(&repository.pool)
            .await
            .unwrap(),
            "staging"
        );
    }

    #[tokio::test]
    async fn v7_fails_closed_on_legacy_casefold_company_guid_collision() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect to legacy in-memory SQLite");
        let mut transaction = pool.begin().await.expect("begin legacy migration");
        for migration in [
            MIRROR_MIGRATION_V2,
            MIRROR_MIGRATION_V3,
            MIRROR_MIGRATION_V4,
            MIRROR_MIGRATION_V5,
            MIRROR_MIGRATION_V6,
        ] {
            sqlx::raw_sql(migration)
                .execute(&mut *transaction)
                .await
                .expect("install legacy migration");
        }
        sqlx::query(
            "INSERT INTO tally_endpoints(id, canonical_origin, created_at_unix_ms, last_observed_at_unix_ms) \
             VALUES ('endpoint-1', 'http://127.0.0.1:9000', 1, 1)",
        )
        .execute(&mut *transaction)
        .await
        .expect("seed endpoint");
        for (id, guid) in [("company-a", "CASE-GUID"), ("company-b", "case-guid")] {
            sqlx::query(
                "INSERT INTO tally_companies(\
                   id, endpoint_id, display_name, company_guid, identity_confidence, \
                   first_observed_at_unix_ms, last_observed_at_unix_ms\
                 ) VALUES (?1, 'endpoint-1', ?1, ?2, 'observed', 1, 1)",
            )
            .bind(id)
            .bind(guid)
            .execute(&mut *transaction)
            .await
            .expect("seed case-variant company");
        }
        transaction.commit().await.expect("commit legacy database");

        let repository = TallyMirrorRepository::new(pool);
        assert!(matches!(
            repository.migrate().await,
            Err(MirrorError::InvalidInput("company_guid_casefold_collision"))
        ));
        let v7_markers = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_schema_migrations WHERE version = 7",
        )
        .fetch_one(&repository.pool)
        .await
        .expect("inspect v7 marker");
        assert_eq!(v7_markers, 0);
        let index_sql = sqlx::query_scalar::<_, String>(
            "SELECT sql FROM sqlite_master WHERE type = 'index' AND name = 'uq_tally_companies_guid'",
        )
        .fetch_one(&repository.pool)
        .await
        .expect("inspect rolled-back index");
        assert!(!index_sql.contains("COLLATE NOCASE"));
    }

    #[tokio::test]
    async fn commit_pending_restart_accepts_only_exact_sealed_core_evidence() {
        let (ordinary, ordinary_snapshot, ordinary_company) = seeded_repository().await;
        assert!(ordinary
            .capability_snapshot_matches_plan(
                &ordinary_snapshot.id,
                &ordinary_company.id,
                1,
                "TallyPrime",
                None,
                Some("Education"),
            )
            .await
            .expect("ordinary observed-support contract remains available to other flows"));

        let (repository, snapshot, company) = seeded_repository_with_core_evidence(
            CapabilityState::Unknown,
            Confidence::Observed,
            Some("sealed_profile_executed"),
        )
        .await;
        assert!(repository
            .core_snapshot_resume_evidence_matches_plan(
                &snapshot.id,
                &company.id,
                1,
                "TallyPrime",
                None,
                Some("Education"),
            )
            .await
            .expect("CommitPending restart accepts exact sealed Core evidence"));

        for (state, confidence, reason) in [
            (
                CapabilityState::Supported,
                Confidence::Observed,
                "sealed_profile_executed",
            ),
            (
                CapabilityState::Unknown,
                Confidence::Inferred,
                "sealed_profile_executed",
            ),
            (
                CapabilityState::Unknown,
                Confidence::Observed,
                "some_other_observation",
            ),
        ] {
            let (altered, altered_snapshot, altered_company) =
                seeded_repository_with_core_evidence(state, confidence, Some(reason)).await;
            assert!(
                !altered
                    .core_snapshot_resume_evidence_matches_plan(
                        &altered_snapshot.id,
                        &altered_company.id,
                        1,
                        "TallyPrime",
                        None,
                        Some("Education"),
                    )
                    .await
                    .expect("reject altered persisted Core evidence"),
                "resume must reject state={state:?}, confidence={confidence:?}, reason={reason}"
            );
        }

        assert!(!repository
            .core_snapshot_resume_evidence_matches_plan(
                &snapshot.id,
                &company.id,
                1,
                "TallyPrime",
                Some("different-release"),
                Some("Education"),
            )
            .await
            .expect("reject changed capability profile"));
    }

    #[tokio::test]
    async fn batch_identity_remains_composite_across_capability_packs() {
        let (repository, snapshot, company) = seeded_repository().await;
        let core = begin_batch(&repository, &snapshot, &company, "multi-pack-run").await;
        let tax = repository
            .begin_batch(BeginBatchInput {
                run_id: "multi-pack-run".to_string(),
                capability_snapshot_id: snapshot.id,
                company_id: company.id,
                pack_id: "india_tax".to_string(),
                pack_schema_major: 1,
                pack_schema_minor: 0,
                source_transport: "xml_http".to_string(),
                source_release: None,
                requested_from_yyyymmdd: Some("20260401".to_string()),
                requested_to_yyyymmdd: Some("20260401".to_string()),
                started_at_unix_ms: 2_000,
            })
            .await
            .expect("same run may carry a distinct pack batch");
        assert_ne!(core, tax);
    }

    #[tokio::test]
    async fn raw_multi_statement_migration_rolls_back_ddl_on_failure() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect to in-memory SQLite");
        let mut transaction = pool.begin().await.expect("begin migration transaction");
        let result = sqlx::raw_sql(
            "CREATE TABLE rollback_probe(id INTEGER PRIMARY KEY); \
             INSERT INTO rollback_probe(id) VALUES (1); \
             INSERT INTO table_that_does_not_exist(id) VALUES (1);",
        )
        .execute(&mut *transaction)
        .await;
        assert!(result.is_err(), "synthetic migration must fail");
        transaction
            .rollback()
            .await
            .expect("rollback failed migration");

        let table_count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'rollback_probe'",
        )
        .fetch_one(&pool)
        .await
        .expect("inspect schema after rollback");
        assert_eq!(table_count, 0, "DDL must not survive migration rollback");
    }

    #[tokio::test]
    async fn recovery_migration_fails_closed_on_duplicate_legacy_run_ids() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect to legacy in-memory SQLite");
        let mut transaction = pool.begin().await.expect("begin legacy migration");
        sqlx::raw_sql(MIRROR_MIGRATION_V2)
            .execute(&mut *transaction)
            .await
            .expect("install v2");
        sqlx::raw_sql(MIRROR_MIGRATION_V3)
            .execute(&mut *transaction)
            .await
            .expect("install v3");
        sqlx::raw_sql(MIRROR_MIGRATION_V4)
            .execute(&mut *transaction)
            .await
            .expect("install v4");
        transaction.commit().await.expect("commit legacy schema");
        for resume_key in ["legacy:a", "legacy:b"] {
            sqlx::query(
                "INSERT INTO tally_snapshot_run_states(\
                   resume_key, run_id, generation, state_sha256, state_json, updated_at_unix_ms\
                 ) VALUES (?1, 'duplicated-run', 1, ?2, '{}', 1)",
            )
            .bind(resume_key)
            .bind(HASH_A)
            .execute(&pool)
            .await
            .expect("seed ambiguous legacy recovery row");
        }
        let repository = TallyMirrorRepository::new(pool);
        assert!(matches!(
            repository.migrate().await,
            Err(MirrorError::InvalidInput("snapshot_state_duplicate_run_id"))
        ));
    }

    #[tokio::test]
    async fn stable_company_identity_survives_rename() {
        let (repository, snapshot, original) = seeded_repository().await;
        let renamed = repository
            .upsert_company(CompanyInput {
                endpoint_id: snapshot.endpoint_id,
                display_name: "Synthetic Bridge Test Renamed".to_string(),
                identity: SourceIdentityInput {
                    guid: Some("company-guid-1".to_string()),
                    confidence: Some(Confidence::Observed),
                    ..Default::default()
                },
                observed_at_unix_ms: 2_000,
            })
            .await
            .expect("rename company by stable identity");
        assert_eq!(original.id, renamed.id);
        assert_eq!(renamed.display_name, "Synthetic Bridge Test Renamed");
    }

    #[tokio::test]
    async fn company_guid_casing_resolves_to_one_stable_pin() {
        let (repository, snapshot, _) = seeded_repository().await;
        let uppercase = repository
            .upsert_company(CompanyInput {
                endpoint_id: snapshot.endpoint_id.clone(),
                display_name: "Synthetic Case Company".to_string(),
                identity: SourceIdentityInput {
                    guid: Some("CASE-GUID-2".to_string()),
                    confidence: Some(Confidence::Observed),
                    ..Default::default()
                },
                observed_at_unix_ms: 2_000,
            })
            .await
            .expect("save uppercase GUID");
        let lowercase = repository
            .upsert_company(CompanyInput {
                endpoint_id: snapshot.endpoint_id.clone(),
                display_name: "Synthetic Case Company Renamed".to_string(),
                identity: SourceIdentityInput {
                    guid: Some("case-guid-2".to_string()),
                    confidence: Some(Confidence::Observed),
                    ..Default::default()
                },
                observed_at_unix_ms: 3_000,
            })
            .await
            .expect("save lowercase GUID");

        assert_eq!(uppercase.id, lowercase.id);
        let count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_companies \
             WHERE endpoint_id = ?1 AND company_guid = ?2 COLLATE NOCASE",
        )
        .bind(&snapshot.endpoint_id)
        .bind("case-guid-2")
        .fetch_one(&repository.pool)
        .await
        .expect("count case-folded company pins");
        assert_eq!(count, 1);
    }

    #[test]
    fn company_profile_correlation_is_casefolded_scoped_and_opaque() {
        let first =
            company_profile_correlation_key("http://127.0.0.1:9000", "SENSITIVE-COMPANY-GUID");
        let same =
            company_profile_correlation_key("http://127.0.0.1:9000", "sensitive-company-guid");
        let other_endpoint =
            company_profile_correlation_key("http://127.0.0.1:9001", "sensitive-company-guid");
        let other_company =
            company_profile_correlation_key("http://127.0.0.1:9000", "other-company-guid");

        assert_eq!(first, same);
        assert_ne!(first, other_endpoint);
        assert_ne!(first, other_company);
        assert_eq!(first.len(), 64);
        assert!(!first.contains("sensitive"));
    }

    #[tokio::test]
    async fn persisted_profiles_only_return_observed_stable_company_pins() {
        let (repository, snapshot, observed) = seeded_repository().await;
        repository
            .upsert_company(CompanyInput {
                endpoint_id: snapshot.endpoint_id,
                display_name: "Synthetic Inferred Company".to_string(),
                identity: SourceIdentityInput {
                    fallback_fingerprint: Some("inferred-company-fingerprint".to_string()),
                    confidence: Some(Confidence::Inferred),
                    ..Default::default()
                },
                observed_at_unix_ms: 2_000,
            })
            .await
            .expect("save inferred company");

        let page = repository
            .persisted_company_profiles()
            .await
            .expect("load persisted profiles");
        let profiles = page.profiles;
        assert_eq!(page.total_profiles, 1);
        assert!(!page.truncated);
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].mirror_company_id, observed.id);
        assert!(profiles[0].guid_observed);
        assert_eq!(
            profiles[0].correlation_key,
            company_profile_correlation_key("http://127.0.0.1:9000", "company-guid-1")
        );
        assert_eq!(profiles[0].identity_confidence, "observed");
        assert_eq!(profiles[0].canonical_endpoint, "http://127.0.0.1:9000");
    }

    #[tokio::test]
    async fn mirror_explorer_is_paged_and_omits_source_content() {
        let (repository, snapshot, company) = seeded_repository().await;
        let batch_id = begin_batch(&repository, &snapshot, &company, "explorer-run").await;
        for (guid, name, hash) in [
            ("ledger-guid-sensitive", "Private Sales Ledger", HASH_A),
            ("voucher-guid-sensitive", "Private Receipt Voucher", HASH_B),
        ] {
            repository
                .observe_record(ObservedRecordInput {
                    batch_id: batch_id.clone(),
                    object_type: if guid.starts_with("ledger") {
                        "ledger".to_string()
                    } else {
                        "voucher".to_string()
                    },
                    display_name: Some(name.to_string()),
                    identity: SourceIdentityInput {
                        guid: Some(guid.to_string()),
                        confidence: Some(Confidence::Observed),
                        ..Default::default()
                    },
                    observed_at_unix_ms: 2_100,
                    raw_source_sha256: hash.to_string(),
                    canonical_sha256: Some(hash.to_string()),
                    canonical_payload: Some(json!({"private_amount": "1180.00"})),
                    exact_decimals: BTreeMap::from([(
                        "private_amount".to_string(),
                        "1180.00".to_string(),
                    )]),
                    observed_alter_id: None,
                    status: ObservationStatus::Accepted,
                    safe_rejection_code: None,
                })
                .await
                .expect("store explorer record");
        }

        let first = repository
            .mirror_explorer_page(&company.id, "core_accounting", 0, 1)
            .await
            .expect("load first explorer page");
        let second = repository
            .mirror_explorer_page(&company.id, "core_accounting", 1, 1)
            .await
            .expect("load second explorer page");
        assert_eq!(first.total_records, 2);
        assert_eq!(first.records.len(), 1);
        assert_eq!(first.records[0].local_alias, "local-record-1");
        assert_eq!(second.records[0].local_alias, "local-record-2");

        let serialized = serde_json::to_string(&(first, second)).expect("serialize explorer pages");
        for sensitive in [
            "Private Sales Ledger",
            "Private Receipt Voucher",
            "ledger-guid-sensitive",
            "voucher-guid-sensitive",
            "1180.00",
            HASH_A,
            HASH_B,
        ] {
            assert!(!serialized.contains(sensitive));
        }
    }

    #[tokio::test]
    async fn observed_probe_upgrades_company_pin_confidence() {
        let (repository, snapshot, company) = seeded_repository().await;
        sqlx::query("UPDATE tally_companies SET identity_confidence = 'inferred' WHERE id = ?1")
            .bind(&company.id)
            .execute(&repository.pool)
            .await
            .expect("weaken synthetic confidence");
        assert!(matches!(
            repository.snapshot_source_pin(&company.id).await,
            Err(MirrorError::InvalidInput("company_identity_not_observed"))
        ));

        repository
            .upsert_company(CompanyInput {
                endpoint_id: snapshot.endpoint_id,
                display_name: company.display_name,
                identity: SourceIdentityInput {
                    guid: Some("company-guid-1".to_string()),
                    confidence: Some(Confidence::Observed),
                    ..Default::default()
                },
                observed_at_unix_ms: 2_000,
            })
            .await
            .expect("upgrade pin with direct observation");
        let pin = repository
            .snapshot_source_pin(&company.id)
            .await
            .expect("observed pin is snapshot eligible");
        assert_eq!(pin.company_guid, "company-guid-1");
    }

    #[tokio::test]
    async fn fallback_identity_is_not_silently_upgraded() {
        let repository = repository().await;
        let snapshot = repository
            .save_capability_snapshot(CapabilitySnapshotInput {
                canonical_origin: "http://127.0.0.1:9000".to_string(),
                observed_at_unix_ms: 1,
                profile_version: 1,
                product: "TallyPrime".to_string(),
                release: None,
                mode: None,
                mode_confidence: Confidence::Unknown,
                items: vec![],
            })
            .await
            .expect("save snapshot");
        repository
            .upsert_company(CompanyInput {
                endpoint_id: snapshot.endpoint_id.clone(),
                display_name: "Synthetic".to_string(),
                identity: SourceIdentityInput {
                    fallback_fingerprint: Some("fallback-1".to_string()),
                    ..Default::default()
                },
                observed_at_unix_ms: 1,
            })
            .await
            .expect("save fallback identity");
        let error = repository
            .upsert_company(CompanyInput {
                endpoint_id: snapshot.endpoint_id,
                display_name: "Synthetic".to_string(),
                identity: SourceIdentityInput {
                    guid: Some("new-guid".to_string()),
                    fallback_fingerprint: Some("fallback-1".to_string()),
                    confidence: Some(Confidence::Observed),
                    ..Default::default()
                },
                observed_at_unix_ms: 2,
            })
            .await
            .expect_err("identity upgrade must require an audit event");
        assert!(matches!(error, MirrorError::IdentityUpgradeRequiresAudit));
    }

    #[tokio::test]
    async fn verified_commit_atomically_advances_checkpoint_and_proof_is_immutable() {
        let (repository, snapshot, company) = seeded_repository().await;
        let batch_id = begin_batch(&repository, &snapshot, &company, "run-1").await;
        let resumed_batch_id = begin_batch(&repository, &snapshot, &company, "run-1").await;
        assert_eq!(resumed_batch_id, batch_id);
        repository
            .observe_record(ObservedRecordInput {
                batch_id: batch_id.clone(),
                object_type: "ledger".to_string(),
                display_name: Some("Synthetic Sales".to_string()),
                identity: SourceIdentityInput {
                    guid: Some("ledger-guid-1".to_string()),
                    confidence: Some(Confidence::Observed),
                    ..Default::default()
                },
                observed_at_unix_ms: 2_100,
                raw_source_sha256: HASH_A.to_string(),
                canonical_sha256: Some(HASH_B.to_string()),
                canonical_payload: Some(json!({"amount": "1180.00", "name": "Synthetic Sales"})),
                exact_decimals: BTreeMap::from([(
                    "opening_balance".to_string(),
                    "1180.00".to_string(),
                )]),
                observed_alter_id: Some("42".to_string()),
                status: ObservationStatus::Accepted,
                safe_rejection_code: None,
            })
            .await
            .expect("store observed record");

        let commit = repository
            .commit_batch(CommitBatchInput::test_only(CommitBatchParts {
                batch_id: batch_id.clone(),
                proof_contract_version: 1,
                outcome: RunOutcome::Completed,
                verification: VerificationState::Verified,
                completed_at_unix_ms: 3_000,
                record_counts_sha256: None,
                snapshot_sha256: Some(HASH_B.to_string()),
                expected_checkpoint_before: None,
                checkpoint_after: Some("alter_id:42".to_string()),
                freshness_target_seconds: 60,
                gap_codes: vec![],
                warning_codes: vec!["report_tie_out_unavailable".to_string()],
            }))
            .await
            .expect("commit verified batch");
        assert!(commit.checkpoint_advanced);
        assert_eq!(commit.proof_sha256.len(), 64);
        let recovered = repository
            .commit_receipt_for_batch(&batch_id, "run-1")
            .await
            .expect("recover exact proof receipt");
        assert_eq!(recovered.proof_id, commit.proof_id);
        assert_eq!(recovered.proof_sha256, commit.proof_sha256);
        assert!(recovered.checkpoint_advanced);

        let fresh = repository
            .freshness(&company.id, "core_accounting", 30_000)
            .await
            .expect("read freshness");
        assert_eq!(fresh.state, FreshnessState::Fresh);
        assert_eq!(fresh.checkpoint_token.as_deref(), Some("alter_id:42"));
        let clock_skew = repository
            .freshness(&company.id, "core_accounting", 2_999)
            .await
            .expect("clock skew is fail-closed");
        assert_eq!(clock_skew.state, FreshnessState::Stale);
        assert_eq!(clock_skew.age_seconds, Some(0));

        let proofs = repository
            .latest_proofs(&company.id, 10)
            .await
            .expect("read immutable proof summary");
        assert_eq!(proofs.len(), 1);
        assert_eq!(proofs[0].selection_token, commit.proof_id);
        assert_eq!(proofs[0].proof_sha256, commit.proof_sha256);
        assert_eq!(proofs[0].verification_state, "verified");
        assert_eq!(proofs[0].accepted_records, 1);
        assert_eq!(
            proofs[0].warning_codes,
            vec!["report_tie_out_unavailable".to_string()]
        );

        let next_batch_id = begin_batch(&repository, &snapshot, &company, "run-2").await;
        repository
            .commit_batch(CommitBatchInput::test_only(CommitBatchParts {
                batch_id: next_batch_id,
                proof_contract_version: 1,
                outcome: RunOutcome::Completed,
                verification: VerificationState::Verified,
                completed_at_unix_ms: 4_000,
                record_counts_sha256: None,
                snapshot_sha256: Some(HASH_A.to_string()),
                expected_checkpoint_before: Some("alter_id:42".to_string()),
                checkpoint_after: Some("alter_id:43".to_string()),
                freshness_target_seconds: 60,
                gap_codes: vec![],
                warning_codes: vec![],
            }))
            .await
            .expect("advance generic checkpoint with a later verified proof");
        assert!(matches!(
            repository
                .commit_receipt_for_batch(&batch_id, "run-1")
                .await,
            Err(MirrorError::VerificationInvariant)
        ));
        let historical = repository
            .historical_commit_receipt_for_batch(&batch_id, "run-1")
            .await
            .expect("immutable historical proof remains authentic after a later checkpoint");
        assert_eq!(historical.proof_id, commit.proof_id);
        assert_eq!(historical.proof_sha256, commit.proof_sha256);

        let mutation = sqlx::query("UPDATE tally_proof_ledger SET entry_sha256 = ?1 WHERE id = ?2")
            .bind(HASH_A)
            .bind(&commit.proof_id)
            .execute(&repository.pool)
            .await;
        assert!(mutation.is_err(), "proof ledger must be append-only");
    }

    #[tokio::test]
    async fn partial_batch_cannot_advance_checkpoint_and_float_payloads_are_rejected() {
        let (repository, snapshot, company) = seeded_repository().await;
        let batch_id = begin_batch(&repository, &snapshot, &company, "run-2").await;
        let float_error = repository
            .observe_record(ObservedRecordInput {
                batch_id: batch_id.clone(),
                object_type: "ledger".to_string(),
                display_name: None,
                identity: SourceIdentityInput {
                    guid: Some("ledger-guid-2".to_string()),
                    ..Default::default()
                },
                observed_at_unix_ms: 2_100,
                raw_source_sha256: HASH_A.to_string(),
                canonical_sha256: Some(HASH_B.to_string()),
                canonical_payload: Some(json!({"amount": 12.5})),
                exact_decimals: BTreeMap::new(),
                observed_alter_id: None,
                status: ObservationStatus::Accepted,
                safe_rejection_code: None,
            })
            .await
            .expect_err("floating accounting representation must not enter the mirror");
        assert!(matches!(
            float_error,
            MirrorError::InvalidInput("floating_point_payload_number")
        ));

        repository
            .observe_record(ObservedRecordInput {
                batch_id: batch_id.clone(),
                object_type: "ledger".to_string(),
                display_name: None,
                identity: SourceIdentityInput {
                    guid: Some("ledger-guid-2".to_string()),
                    ..Default::default()
                },
                observed_at_unix_ms: 2_100,
                raw_source_sha256: HASH_A.to_string(),
                canonical_sha256: None,
                canonical_payload: None,
                exact_decimals: BTreeMap::new(),
                observed_alter_id: None,
                status: ObservationStatus::Rejected,
                safe_rejection_code: Some("invalid_exact_decimal".to_string()),
            })
            .await
            .expect("store safe rejection evidence");

        repository
            .commit_batch(CommitBatchInput::test_only(CommitBatchParts {
                batch_id,
                proof_contract_version: 1,
                outcome: RunOutcome::Completed,
                verification: VerificationState::Partial,
                completed_at_unix_ms: 3_000,
                record_counts_sha256: None,
                snapshot_sha256: Some(HASH_B.to_string()),
                expected_checkpoint_before: None,
                checkpoint_after: None,
                freshness_target_seconds: 60,
                gap_codes: vec!["rejected_records_present".to_string()],
                warning_codes: vec![],
            }))
            .await
            .expect("commit partial proof without checkpoint");
        let freshness = repository
            .freshness(&company.id, "core_accounting", 4_000)
            .await
            .expect("read freshness");
        assert_eq!(freshness.state, FreshnessState::NeverVerified);
    }

    #[tokio::test]
    async fn verified_checkpoint_commit_is_compare_and_swap_protected() {
        let (repository, snapshot, company) = seeded_repository().await;
        let first_batch = begin_batch(&repository, &snapshot, &company, "cas-run-1").await;
        let second_batch = begin_batch(&repository, &snapshot, &company, "cas-run-2").await;
        repository
            .commit_batch(CommitBatchInput::test_only(CommitBatchParts {
                batch_id: first_batch,
                proof_contract_version: 1,
                outcome: RunOutcome::Completed,
                verification: VerificationState::Verified,
                completed_at_unix_ms: 3_000,
                record_counts_sha256: None,
                snapshot_sha256: Some(HASH_A.to_string()),
                expected_checkpoint_before: None,
                checkpoint_after: Some("full:first".to_string()),
                freshness_target_seconds: 60,
                gap_codes: vec![],
                warning_codes: vec![],
            }))
            .await
            .expect("first verified run advances an empty checkpoint");

        let conflict = repository
            .commit_batch(CommitBatchInput::test_only(CommitBatchParts {
                batch_id: second_batch,
                proof_contract_version: 1,
                outcome: RunOutcome::Completed,
                verification: VerificationState::Verified,
                completed_at_unix_ms: 3_001,
                record_counts_sha256: None,
                snapshot_sha256: Some(HASH_B.to_string()),
                expected_checkpoint_before: None,
                checkpoint_after: Some("full:second".to_string()),
                freshness_target_seconds: 60,
                gap_codes: vec![],
                warning_codes: vec![],
            }))
            .await
            .expect_err("stale checkpoint authority must fail closed");
        assert!(matches!(conflict, MirrorError::ConcurrentCheckpoint));
        let freshness = repository
            .freshness(&company.id, "core_accounting", 3_001)
            .await
            .expect("read winning checkpoint");
        assert_eq!(freshness.checkpoint_token.as_deref(), Some("full:first"));
    }

    #[test]
    fn public_support_export_is_minimized_uncorrelatable_and_checksum_stable() {
        let build = || RedactedProofPayload {
            schema: "bridge.tally.redacted-proof-of-sync",
            schema_version: 1,
            exported_at_unix_ms: 123,
            redaction_profile: "public_support_v1",
            subject: RedactedSubject {
                reference: "company-1",
                identity_disclosed: false,
            },
            proofs: vec![RedactedProofEntry {
                entry_index: 1,
                proof_contract_version: 1,
                pack_id: "core_accounting".to_string(),
                pack_schema_version: PackSchemaVersion { major: 2, minor: 0 },
                outcome: "completed".to_string(),
                verification_state: "partial".to_string(),
                started_at_unix_ms: 100,
                completed_at_unix_ms: 120,
                counts: RedactedCounts {
                    provenance_backed_accepted_records: 2,
                    provenance_unavailable_records: 0,
                    rejected_records: 0,
                },
                gaps: vec!["report_tie_out_unavailable".to_string()],
                warnings: Vec::new(),
                local_ledger: RedactedLedgerEvidence {
                    chain_validation: "valid_at_export",
                },
            }],
            current_status: RedactedCurrentStatus {
                freshness_state: "never_verified",
                verified_at_unix_ms: None,
                checkpoint_present: false,
            },
        };
        let first = finish_redacted_export(build()).expect("build public support artifact");
        let second = finish_redacted_export(build()).expect("repeat deterministic artifact");
        assert_eq!(first.payload_sha256, second.payload_sha256);
        assert_eq!(first.json, second.json);
        for forbidden in [
            "Synthetic Company",
            "company-guid",
            "run-id",
            "batch-id",
            "proof-id",
            "checkpoint-token",
            "1180.00",
            "snapshot_commitment_sha256",
            "entry_sha256",
            "rid:",
        ] {
            assert!(!first.json.contains(forbidden), "leaked {forbidden}");
        }
        assert!(first
            .json
            .contains("\"integrity_claim\": \"checksum_only\""));
        assert!(first.json.contains("\"authenticity_claim\": \"none\""));
        assert!(validate_export_code("report_tie_out_unavailable").is_ok());
        assert!(validate_export_code("period_report_profile_unobserved").is_ok());
        assert!(validate_export_code("future_unreviewed_code").is_err());
    }

    #[test]
    fn redacted_proof_export_accepts_every_reviewed_precise_tally_terminal_code() {
        for code in REVIEWED_TALLY_TERMINAL_CODES {
            validate_export_code(code).expect("reviewed terminal code must be exportable");
        }
        assert!(REVIEWED_TALLY_TERMINAL_CODES.contains(&"window_membership_replay_conflict"));
        assert!(REVIEWED_TALLY_TERMINAL_CODES.contains(&"adaptive_window_limit_reached"));
        assert!(REVIEWED_TALLY_TERMINAL_CODES.contains(&"minimum_window_response_too_large"));
        let payload = RedactedProofPayload {
            schema: "bridge.tally.redacted-proof-of-sync",
            schema_version: 1,
            exported_at_unix_ms: 1,
            redaction_profile: "public_support_v1",
            subject: RedactedSubject {
                reference: "selected_company",
                identity_disclosed: false,
            },
            proofs: vec![RedactedProofEntry {
                entry_index: 0,
                proof_contract_version: 1,
                pack_id: "core_accounting".to_string(),
                pack_schema_version: PackSchemaVersion { major: 1, minor: 0 },
                outcome: "failed".to_string(),
                verification_state: "unverified".to_string(),
                started_at_unix_ms: 1,
                completed_at_unix_ms: 2,
                counts: RedactedCounts {
                    provenance_backed_accepted_records: 0,
                    provenance_unavailable_records: 0,
                    rejected_records: 0,
                },
                gaps: REVIEWED_TALLY_TERMINAL_CODES
                    .iter()
                    .map(|code| (*code).to_string())
                    .collect(),
                warnings: Vec::new(),
                local_ledger: RedactedLedgerEvidence {
                    chain_validation: "valid_at_export",
                },
            }],
            current_status: RedactedCurrentStatus {
                freshness_state: "never_verified",
                verified_at_unix_ms: None,
                checkpoint_present: false,
            },
        };
        let export = finish_redacted_export(payload).expect("export every reviewed terminal code");
        for code in REVIEWED_TALLY_TERMINAL_CODES {
            assert!(export.json.contains(code), "export omitted {code}");
        }
    }
}
