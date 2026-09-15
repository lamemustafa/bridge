use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::future::Future;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use bridge_tally_core::report_tie_out::{
    assess_core_period_report, scoped_mismatch_record_alias, TieOutState,
};
use bridge_tally_core::{
    CanonicalText, CapabilityPackId, CapabilityProfile, CapabilityState, CompanyRef,
    EvidenceConfidence, Freshness, PackBatch, PackSchemaVersion, ProofManifest, ReadResponseScope,
    ReadWindow, RequestContext, TallyConnector, TallyError, TransportId,
};
use chrono::{Duration as ChronoDuration, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{Row, SqlitePool};

use crate::db::tally_mirror::{
    AbandonSnapshotWindowAttemptResult, BeginBatchInput, BeginSnapshotWindowAttemptInput,
    CommitReceiptFacts, CommitResult, FreshnessState, MirrorError, ObservationCounts,
    SnapshotWindowAttemptRef, SnapshotWindowMembershipInput, SnapshotWindowReceipt,
    TallyMirrorRepository,
};
use crate::sync::reconciliation::{
    build_reconciliation, build_terminal_proof, canonicalize_window, proof_record_counts_sha256,
    CanonicalWindowContext, CommitBatchInput, CommitBatchParts, ComparisonScope, EndProfileCheck,
    ExternalReferenceCatalog, ReconciliationDecision, ReconciliationError, ReconciliationInput,
    ReconciliationMismatch, ReportTieOutEvidence, SourceStabilityCheck, TerminalKind,
    WindowEvidence,
};
use crate::tally::core_snapshot_start_authorized;
use crate::warning_codes::WarningCode;

const SNAPSHOT_STATE_VERSION: u16 = 5;
const LEGACY_SNAPSHOT_STATE_VERSION_V3: u16 = 3;
const LEGACY_SNAPSHOT_STATE_VERSION_V4: u16 = 4;
const MAX_DURABLE_STATE_BYTES: usize = 16 * 1024 * 1024;
const MAX_SNAPSHOT_PLAN_BYTES: usize = 4 * 1024 * 1024;
const MAX_SNAPSHOT_WINDOWS: usize = 1024;
const MAX_STAGED_KEYS_PER_WINDOW: usize = 1_000_000;
const MAX_WINDOW_STAGE_CHUNK: usize = 256;
// Reconciliation currently materializes one bounded canonical identity/hash map. Record keys are
// validated at 385 bytes and hashes at 64 bytes before staging, so this also places a conservative
// deterministic ceiling on transient reconciliation bytes without trusting allocator behavior.
const MAX_RECONCILIATION_RECORDS: u64 = 100_000;
const RECONCILIATION_RECORD_BUDGET_CODE: &str = "reconciliation_record_budget_exceeded";
const WORKER_LEASE_TTL_MS: i64 = 5 * 60 * 1000;
#[cfg(not(test))]
const WORKER_LEASE_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
#[cfg(test)]
const WORKER_LEASE_HEARTBEAT_INTERVAL: Duration = Duration::from_millis(25);
const CANCELLATION_POLL_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct PlannedWindow {
    pub id: String,
    pub range: ReadWindow,
    pub query_profile: CanonicalText,
    pub filters_sha256: CanonicalText,
}

impl PlannedWindow {
    pub fn deterministic(pack: CapabilityPackId, range: ReadWindow) -> Self {
        let mut digest = Sha256::new();
        digest.update(b"bridge-tally-planned-window-v1\0");
        digest.update(pack_code(pack).as_bytes());
        digest.update(b"\0");
        digest.update(range.from_yyyymmdd.as_bytes());
        digest.update(b"\0");
        digest.update(range.to_yyyymmdd.as_bytes());
        let hash = hex_digest(digest.finalize());
        Self {
            id: format!("window:{}", &hash[..24]),
            range,
            query_profile: CanonicalText::parse(match pack {
                CapabilityPackId::CoreAccounting => "core_accounting_v3".to_string(),
                _ => format!("{}_v1", pack_code(pack)),
            })
            .expect("built-in query profile is canonical"),
            filters_sha256: CanonicalText::parse(sha256_bytes(
                format!("bridge-default-filter-v1:{}", pack_code(pack)).as_bytes(),
            ))
            .expect("SHA-256 is canonical text"),
        }
    }

    fn adaptive_child(parent: &Self, range: ReadWindow) -> Self {
        let mut digest = Sha256::new();
        digest.update(b"bridge-tally-adaptive-window-child-v1\0");
        digest.update(parent.id.as_bytes());
        digest.update(b"\0");
        digest.update(parent.query_profile.as_str().as_bytes());
        digest.update(b"\0");
        digest.update(parent.filters_sha256.as_str().as_bytes());
        digest.update(b"\0");
        digest.update(range.from_yyyymmdd.as_bytes());
        digest.update(b"\0");
        digest.update(range.to_yyyymmdd.as_bytes());
        let hash = hex_digest(digest.finalize());
        Self {
            id: format!("window:{}", &hash[..24]),
            range,
            query_profile: parent.query_profile.clone(),
            filters_sha256: parent.filters_sha256.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SplitTrigger {
    VoucherResponseSizeLimit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SplitAlgorithm {
    CalendarMidpointV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct AdaptiveWindowPolicy {
    pub policy_version: u16,
    pub split_trigger: SplitTrigger,
    pub split_algorithm: SplitAlgorithm,
    pub minimum_days: u16,
    pub maximum_leaf_windows: u16,
}

impl AdaptiveWindowPolicy {
    pub fn bounded_default() -> Self {
        Self {
            policy_version: 1,
            split_trigger: SplitTrigger::VoucherResponseSizeLimit,
            split_algorithm: SplitAlgorithm::CalendarMidpointV1,
            minimum_days: 1,
            maximum_leaf_windows: MAX_SNAPSHOT_WINDOWS as u16,
        }
    }

    fn validate(&self) -> Result<(), SnapshotError> {
        if self.policy_version != 1
            || self.split_trigger != SplitTrigger::VoucherResponseSizeLimit
            || self.split_algorithm != SplitAlgorithm::CalendarMidpointV1
            || self.minimum_days != 1
            || self.maximum_leaf_windows == 0
            || usize::from(self.maximum_leaf_windows) > MAX_SNAPSHOT_WINDOWS
        {
            return Err(SnapshotError::InvalidPlan("adaptive_window_policy"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct SnapshotPlan {
    pub resume_key: String,
    pub run_id: String,
    pub capability_snapshot_id: String,
    pub mirror_company_id: String,
    pub company: CompanyRef,
    pub pack: CapabilityPackId,
    pub pack_schema_version: PackSchemaVersion,
    #[serde(default)]
    pub capability_profile_version: u16,
    #[serde(default)]
    pub capability_profile_sha256: String,
    #[serde(default)]
    pub source_product: String,
    pub source_transport: String,
    pub source_release: Option<String>,
    #[serde(default)]
    pub source_mode: Option<String>,
    pub external_references: ExternalReferenceCatalog,
    pub windows: Vec<PlannedWindow>,
    #[serde(default)]
    pub adaptive_window_policy: Option<AdaptiveWindowPolicy>,
    #[serde(default)]
    pub capability_canary_window: Option<PlannedWindow>,
    pub started_at_unix_ms: i64,
    pub freshness_target_seconds: i64,
}

impl SnapshotPlan {
    pub fn fingerprint(&self) -> Result<String, SnapshotError> {
        #[derive(Serialize)]
        struct PlanFingerprint<'a> {
            capability_snapshot_id: &'a str,
            mirror_company_id: &'a str,
            company: &'a CompanyRef,
            pack: CapabilityPackId,
            pack_schema_version: PackSchemaVersion,
            capability_profile_version: u16,
            capability_profile_sha256: &'a str,
            source_product: &'a str,
            source_transport: &'a str,
            source_release: &'a Option<String>,
            source_mode: &'a Option<String>,
            external_references: &'a ExternalReferenceCatalog,
            windows: &'a [PlannedWindow],
            adaptive_window_policy: &'a Option<AdaptiveWindowPolicy>,
            capability_canary_window: &'a Option<PlannedWindow>,
            started_at_unix_ms: i64,
            freshness_target_seconds: i64,
        }
        if self.adaptive_window_policy.is_none() && self.capability_canary_window.is_none() {
            return self.legacy_fingerprint_v3();
        }
        sha256_json(&PlanFingerprint {
            capability_snapshot_id: &self.capability_snapshot_id,
            mirror_company_id: &self.mirror_company_id,
            company: &self.company,
            pack: self.pack,
            pack_schema_version: self.pack_schema_version,
            capability_profile_version: self.capability_profile_version,
            capability_profile_sha256: &self.capability_profile_sha256,
            source_product: &self.source_product,
            source_transport: &self.source_transport,
            source_release: &self.source_release,
            source_mode: &self.source_mode,
            external_references: &self.external_references,
            windows: &self.windows,
            adaptive_window_policy: &self.adaptive_window_policy,
            capability_canary_window: &self.capability_canary_window,
            started_at_unix_ms: self.started_at_unix_ms,
            freshness_target_seconds: self.freshness_target_seconds,
        })
    }

    fn legacy_fingerprint_v3(&self) -> Result<String, SnapshotError> {
        #[derive(Serialize)]
        struct LegacyPlanFingerprint<'a> {
            capability_snapshot_id: &'a str,
            mirror_company_id: &'a str,
            company: &'a CompanyRef,
            pack: CapabilityPackId,
            pack_schema_version: PackSchemaVersion,
            capability_profile_version: u16,
            capability_profile_sha256: &'a str,
            source_product: &'a str,
            source_transport: &'a str,
            source_release: &'a Option<String>,
            source_mode: &'a Option<String>,
            external_references: &'a ExternalReferenceCatalog,
            windows: &'a [PlannedWindow],
            started_at_unix_ms: i64,
            freshness_target_seconds: i64,
        }
        sha256_json(&LegacyPlanFingerprint {
            capability_snapshot_id: &self.capability_snapshot_id,
            mirror_company_id: &self.mirror_company_id,
            company: &self.company,
            pack: self.pack,
            pack_schema_version: self.pack_schema_version,
            capability_profile_version: self.capability_profile_version,
            capability_profile_sha256: &self.capability_profile_sha256,
            source_product: &self.source_product,
            source_transport: &self.source_transport,
            source_release: &self.source_release,
            source_mode: &self.source_mode,
            external_references: &self.external_references,
            windows: &self.windows,
            started_at_unix_ms: self.started_at_unix_ms,
            freshness_target_seconds: self.freshness_target_seconds,
        })
    }

    fn validate(&self) -> Result<(), SnapshotError> {
        let policy = self
            .adaptive_window_policy
            .as_ref()
            .ok_or(SnapshotError::InvalidPlan("adaptive_window_policy"))?;
        policy.validate()?;
        let canary = self
            .capability_canary_window
            .as_ref()
            .ok_or(SnapshotError::InvalidPlan("capability_canary_window"))?;
        if self.resume_key.is_empty()
            || self.run_id.is_empty()
            || self.capability_snapshot_id.is_empty()
            || self.mirror_company_id.is_empty()
            || self.windows.is_empty()
            || self.windows.len() > MAX_SNAPSHOT_WINDOWS
            || self.capability_profile_version == 0
            || !is_lower_sha256(&self.capability_profile_sha256)
            || self.source_product.is_empty()
            || self.source_product.len() > 128
            || self.source_product.chars().any(char::is_control)
            || self.source_transport.is_empty()
            || self.source_transport.len() > 64
            || self.source_transport.chars().any(char::is_control)
            || self.source_release.as_ref().is_some_and(|value| {
                value.is_empty() || value.len() > 128 || value.chars().any(char::is_control)
            })
            || self.source_mode.as_ref().is_some_and(|value| {
                value.is_empty() || value.len() > 64 || value.chars().any(char::is_control)
            })
            || self.freshness_target_seconds <= 0
        {
            return Err(SnapshotError::InvalidPlan("run_metadata"));
        }
        let mut ids = BTreeSet::new();
        let mut ranges = BTreeSet::new();
        for window in &self.windows {
            if window.id.is_empty()
                || !valid_yyyymmdd(&window.range.from_yyyymmdd)
                || !valid_yyyymmdd(&window.range.to_yyyymmdd)
                || window.range.from_yyyymmdd > window.range.to_yyyymmdd
                || !is_lower_sha256(window.filters_sha256.as_str())
                || !ids.insert(window.id.clone())
                || !ranges.insert((
                    window.range.from_yyyymmdd.clone(),
                    window.range.to_yyyymmdd.clone(),
                ))
            {
                return Err(SnapshotError::InvalidPlan("windows"));
            }
        }
        let mut sorted_ranges = ranges.into_iter().collect::<Vec<_>>();
        sorted_ranges.sort();
        for pair in sorted_ranges.windows(2) {
            if pair[1].0 <= pair[0].1 {
                return Err(SnapshotError::InvalidPlan("overlapping_windows"));
            }
        }
        let canary_from = parse_yyyymmdd(&canary.range.from_yyyymmdd)
            .ok_or(SnapshotError::InvalidPlan("capability_canary_window"))?;
        let canary_to = parse_yyyymmdd(&canary.range.to_yyyymmdd)
            .ok_or(SnapshotError::InvalidPlan("capability_canary_window"))?;
        if canary_from != canary_to
            || *canary
                != PlannedWindow::deterministic(
                    self.pack,
                    ReadWindow {
                        from_yyyymmdd: canary.range.from_yyyymmdd.clone(),
                        to_yyyymmdd: canary.range.to_yyyymmdd.clone(),
                    },
                )
            || !self.windows.iter().any(|root| {
                root.query_profile == canary.query_profile
                    && root.filters_sha256 == canary.filters_sha256
                    && root.range.from_yyyymmdd <= canary.range.from_yyyymmdd
                    && root.range.to_yyyymmdd >= canary.range.to_yyyymmdd
            })
        {
            return Err(SnapshotError::InvalidPlan("capability_canary_window"));
        }
        if serde_json::to_vec(self)
            .map_err(|_| SnapshotError::Serialization)?
            .len()
            > MAX_SNAPSHOT_PLAN_BYTES
        {
            return Err(SnapshotError::InvalidPlan("plan_size"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotPhase {
    Prepare,
    CapabilityCheck,
    CompanyIdentityCheck,
    PlanWindows,
    Extract,
    Normalize,
    Validate,
    Stage,
    Reconcile,
    CommitPending,
    EmitProof,
    Completed,
    Partial,
    Failed,
    Cancelled,
}

impl SnapshotPhase {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Partial | Self::Failed | Self::Cancelled
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowPhase {
    Pending,
    Extracting,
    Normalizing,
    Validating,
    Staging,
    Complete,
    Split,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct WindowSplit {
    pub policy_version: u16,
    pub trigger: SplitTrigger,
    pub algorithm: SplitAlgorithm,
    pub left_window_id: String,
    pub right_window_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct WindowStageAttempt {
    pub attempt_id: String,
    pub batch_id: String,
    pub window_id: String,
    pub attempt_ordinal: u32,
    /// Safe warnings observed while reading this particular attempt. They become
    /// run-level warnings only after the attempt has completed immutably.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub warning_codes: BTreeSet<WarningCode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct WindowStageReceipt {
    pub attempt: WindowStageAttempt,
    pub member_count: u32,
    pub membership_sha256: String,
    pub receipt_sha256: String,
}

impl From<&SnapshotWindowAttemptRef> for WindowStageAttempt {
    fn from(value: &SnapshotWindowAttemptRef) -> Self {
        Self {
            attempt_id: value.attempt_id.clone(),
            batch_id: value.batch_id.clone(),
            window_id: value.window_id.clone(),
            attempt_ordinal: value.attempt_ordinal,
            warning_codes: BTreeSet::new(),
        }
    }
}

impl WindowStageAttempt {
    fn repository_ref(&self) -> SnapshotWindowAttemptRef {
        SnapshotWindowAttemptRef {
            attempt_id: self.attempt_id.clone(),
            batch_id: self.batch_id.clone(),
            window_id: self.window_id.clone(),
            attempt_ordinal: self.attempt_ordinal,
        }
    }
}

impl From<&SnapshotWindowReceipt> for WindowStageReceipt {
    fn from(value: &SnapshotWindowReceipt) -> Self {
        Self {
            attempt: WindowStageAttempt {
                attempt_id: value.attempt_id.clone(),
                batch_id: value.batch_id.clone(),
                window_id: value.window_id.clone(),
                attempt_ordinal: value.attempt_ordinal,
                warning_codes: BTreeSet::new(),
            },
            member_count: value.member_count,
            membership_sha256: value.membership_sha256.clone(),
            receipt_sha256: value.receipt_sha256.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct WindowProgress {
    pub planned: PlannedWindow,
    pub phase: WindowPhase,
    #[serde(default)]
    pub parent_window_id: Option<String>,
    #[serde(default)]
    pub split: Option<WindowSplit>,
    /// Legacy v4 identity set. New v5 states keep this empty and use the
    /// normalized encrypted membership table instead.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub staged_record_keys: BTreeSet<String>,
    #[serde(default)]
    pub stage_attempt: Option<WindowStageAttempt>,
    #[serde(default)]
    pub stage_receipt: Option<WindowStageReceipt>,
    pub evidence: Option<WindowEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct PhaseProgress {
    pub phase: SnapshotPhase,
    pub active_window_id: Option<String>,
    pub completed_windows: u32,
    pub total_windows: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum PendingDecisionKind {
    Reconciled,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
struct PendingCommit {
    kind: PendingDecisionKind,
    completed_at_unix_ms: i64,
    safe_reason_code: Option<String>,
    intended_checkpoint: Option<String>,
    #[serde(default)]
    expected_receipt_facts_sha256: Option<String>,
    /// Compact, hash-bound authority for exact local recovery. Canonical membership stays in
    /// normalized SQLite and is deliberately not embedded here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reconciled_proof: Option<Box<ProofManifest>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct StoredCommitReceipt {
    pub proof_id: Option<String>,
    pub proof_sha256: Option<String>,
    pub checkpoint_advanced: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DurableSnapshotState {
    pub state_version: u16,
    #[serde(default)]
    pub generation: u64,
    #[serde(skip)]
    pub row_integrity_bound: bool,
    pub resume_key: String,
    pub run_id: String,
    pub plan_sha256: String,
    /// Existing v2 states without this field remain inspectable, but cannot be reconstructed and
    /// resumed after restart without inventing missing immutable-plan authority.
    #[serde(default)]
    pub plan: Option<SnapshotPlan>,
    pub batch_id: Option<String>,
    pub checkpoint_before: Option<String>,
    pub freshness_before: Freshness,
    pub progress: PhaseProgress,
    pub windows: BTreeMap<String, WindowProgress>,
    pub gap_codes: BTreeSet<String>,
    pub warning_codes: BTreeSet<WarningCode>,
    #[serde(default)]
    pub end_profile_check: EndProfileCheck,
    #[serde(default)]
    pub source_stability_check: SourceStabilityCheck,
    pending_commit: Option<PendingCommit>,
    pub proof: Option<ProofManifest>,
    pub commit_receipt: Option<StoredCommitReceipt>,
}

impl DurableSnapshotState {
    pub fn new(plan: &SnapshotPlan, freshness_before: Freshness) -> Result<Self, SnapshotError> {
        plan.validate()?;
        let windows = plan
            .windows
            .iter()
            .cloned()
            .map(|planned| {
                (
                    planned.id.clone(),
                    WindowProgress {
                        planned,
                        phase: WindowPhase::Pending,
                        parent_window_id: None,
                        split: None,
                        staged_record_keys: BTreeSet::new(),
                        stage_attempt: None,
                        stage_receipt: None,
                        evidence: None,
                    },
                )
            })
            .collect();
        Ok(Self {
            state_version: SNAPSHOT_STATE_VERSION,
            generation: 0,
            row_integrity_bound: false,
            resume_key: plan.resume_key.clone(),
            run_id: plan.run_id.clone(),
            plan_sha256: plan.fingerprint()?,
            plan: Some(plan.clone()),
            batch_id: None,
            checkpoint_before: None,
            freshness_before,
            progress: PhaseProgress {
                phase: SnapshotPhase::Prepare,
                active_window_id: None,
                completed_windows: 0,
                total_windows: plan.windows.len() as u32,
            },
            windows,
            gap_codes: BTreeSet::new(),
            warning_codes: BTreeSet::new(),
            end_profile_check: EndProfileCheck::Unavailable,
            source_stability_check: SourceStabilityCheck::Unavailable,
            pending_commit: None,
            proof: None,
            commit_receipt: None,
        })
    }

    pub fn assert_resumable_with(&self, plan: &SnapshotPlan) -> Result<(), SnapshotError> {
        if self.state_version != SNAPSHOT_STATE_VERSION {
            return Err(SnapshotError::ResumePlanUnavailable);
        }
        plan.validate()?;
        if self.resume_key != plan.resume_key
            || self.run_id != plan.run_id
            || self.plan_sha256 != plan.fingerprint()?
        {
            return Err(SnapshotError::ResumePlanMismatch);
        }
        Ok(())
    }

    pub fn recoverable_plan(&self) -> Result<SnapshotPlan, SnapshotError> {
        if self.state_version != SNAPSHOT_STATE_VERSION
            || !self.row_integrity_bound
            || self.generation == 0
        {
            return Err(SnapshotError::ResumePlanUnavailable);
        }
        let plan = self
            .plan
            .clone()
            .ok_or(SnapshotError::ResumePlanUnavailable)?;
        self.assert_resumable_with(&plan)?;
        Ok(plan)
    }

    fn validate_invariants(&self) -> Result<(), SnapshotError> {
        if !matches!(
            self.state_version,
            LEGACY_SNAPSHOT_STATE_VERSION_V3
                | LEGACY_SNAPSHOT_STATE_VERSION_V4
                | SNAPSHOT_STATE_VERSION
        ) || self.resume_key.is_empty()
            || self.run_id.is_empty()
            || !is_lower_sha256(&self.plan_sha256)
            || self.windows.is_empty()
            || self.windows.len() > MAX_SNAPSHOT_WINDOWS
            || self.progress.total_windows as usize != self.executable_leaves().len()
            || self.progress.completed_windows as usize
                != self
                    .windows
                    .values()
                    .filter(|window| window.phase == WindowPhase::Complete)
                    .count()
            || self.windows.values().any(|window| {
                window.staged_record_keys.len() > MAX_STAGED_KEYS_PER_WINDOW
                    || (self.state_version == SNAPSHOT_STATE_VERSION
                        && !window.staged_record_keys.is_empty())
            })
        {
            return Err(SnapshotError::CorruptState);
        }
        if let Some(active) = &self.progress.active_window_id {
            if self
                .windows
                .get(active)
                .is_none_or(|window| window.phase == WindowPhase::Split)
                || self.progress.phase.is_terminal()
            {
                return Err(SnapshotError::CorruptState);
            }
        }
        if let Some(plan) = &self.plan {
            if self.state_version == SNAPSHOT_STATE_VERSION {
                self.assert_resumable_with(plan)?;
                validate_window_graph(self, plan)?;
                for window in self.windows.values() {
                    let stage_shape_valid = match window.phase {
                        WindowPhase::Complete => {
                            window.stage_attempt.is_none()
                                && window.stage_receipt.is_some()
                                && window.evidence.is_some()
                        }
                        WindowPhase::Staging => {
                            window.stage_attempt.is_some() && window.stage_receipt.is_none()
                        }
                        WindowPhase::Split => {
                            window.stage_attempt.is_none()
                                && window.stage_receipt.is_none()
                                && window.evidence.is_none()
                        }
                        _ => window.stage_attempt.is_none() && window.stage_receipt.is_none(),
                    };
                    if !stage_shape_valid
                        || window.stage_attempt.as_ref().is_some_and(|attempt| {
                            attempt.window_id != window.planned.id
                                || attempt.batch_id != self.batch_id.clone().unwrap_or_default()
                                || attempt.attempt_id.is_empty()
                                || attempt.attempt_ordinal == 0
                        })
                        || window.stage_receipt.as_ref().is_some_and(|receipt| {
                            receipt.attempt.window_id != window.planned.id
                                || receipt.attempt.batch_id
                                    != self.batch_id.clone().unwrap_or_default()
                                || receipt.member_count as u64
                                    != window
                                        .evidence
                                        .as_ref()
                                        .map_or(0, |evidence| evidence.deduped_count)
                                || window.evidence.as_ref().is_none_or(|evidence| {
                                    evidence.record_set_sha256.as_deref()
                                        != Some(receipt.membership_sha256.as_str())
                                })
                                || !is_lower_sha256(&receipt.membership_sha256)
                                || !is_lower_sha256(&receipt.receipt_sha256)
                        })
                    {
                        return Err(SnapshotError::CorruptState);
                    }
                }
            } else if self.state_version == LEGACY_SNAPSHOT_STATE_VERSION_V4 {
                if self.plan_sha256 != plan.fingerprint()? {
                    return Err(SnapshotError::CorruptState);
                }
                validate_window_graph(self, plan)?;
            } else if self.plan_sha256 != plan.legacy_fingerprint_v3()?
                || plan.windows.len() != self.windows.len()
                || plan.windows.iter().any(|planned| {
                    self.windows.get(&planned.id).is_none_or(|window| {
                        window.planned != *planned
                            || window.parent_window_id.is_some()
                            || window.split.is_some()
                    })
                })
            {
                return Err(SnapshotError::CorruptState);
            }
        }
        let terminal = self.progress.phase.is_terminal();
        let terminal_evidence_valid = if terminal {
            self.proof.is_some() && self.commit_receipt.is_some()
        } else {
            self.proof.is_none() && self.commit_receipt.is_none()
        };
        if !terminal_evidence_valid
            || (self.progress.phase == SnapshotPhase::CommitPending)
                != self.pending_commit.is_some()
        {
            return Err(SnapshotError::CorruptState);
        }
        if self.pending_commit.as_ref().is_some_and(|pending| {
            pending
                .expected_receipt_facts_sha256
                .as_deref()
                .is_none_or(|hash| !is_lower_sha256(hash))
        }) {
            return Err(SnapshotError::CorruptState);
        }
        if let Some(receipt) = &self.commit_receipt {
            if receipt.proof_id.as_ref().is_none_or(String::is_empty)
                || receipt
                    .proof_sha256
                    .as_deref()
                    .is_none_or(|hash| !is_lower_sha256(hash))
            {
                return Err(SnapshotError::CorruptState);
            }
        }
        if let Some(proof) = &self.proof {
            if proof.run_id != self.run_id {
                return Err(SnapshotError::CorruptState);
            }
            if let Some(plan) = &self.plan {
                if proof.source_identity != plan.company.identity
                    || proof.pack != plan.pack
                    || proof.pack_schema_version != plan.pack_schema_version
                {
                    return Err(SnapshotError::CorruptState);
                }
            }
        }
        Ok(())
    }

    fn set_phase(&mut self, phase: SnapshotPhase, active_window_id: Option<String>) {
        self.progress.phase = phase;
        self.progress.active_window_id = active_window_id;
        self.progress.completed_windows = self
            .windows
            .values()
            .filter(|window| window.phase == WindowPhase::Complete)
            .count() as u32;
        self.progress.total_windows = self.executable_leaves().len() as u32;
    }

    fn executable_leaves(&self) -> Vec<PlannedWindow> {
        let mut leaves = self
            .windows
            .values()
            .filter(|window| window.phase != WindowPhase::Split)
            .map(|window| window.planned.clone())
            .collect::<Vec<_>>();
        leaves.sort_by(|left, right| {
            left.range
                .from_yyyymmdd
                .cmp(&right.range.from_yyyymmdd)
                .then_with(|| left.range.to_yyyymmdd.cmp(&right.range.to_yyyymmdd))
                .then_with(|| left.id.cmp(&right.id))
        });
        leaves
    }
}

enum SplitLeafResult {
    Created,
    MinimumReached,
    LeafLimitReached,
}

fn split_leaf(
    state: &mut DurableSnapshotState,
    plan: &SnapshotPlan,
    window_id: &str,
) -> Result<SplitLeafResult, SnapshotError> {
    let policy = plan
        .adaptive_window_policy
        .as_ref()
        .ok_or(SnapshotError::StateInvariant("adaptive_window_policy"))?;
    let parent = state
        .windows
        .get(window_id)
        .ok_or(SnapshotError::StateInvariant("window"))?;
    if parent.phase != WindowPhase::Extracting
        || parent.split.is_some()
        || !parent.staged_record_keys.is_empty()
        || parent.stage_attempt.is_some()
        || parent.stage_receipt.is_some()
        || parent.evidence.is_some()
    {
        return Err(SnapshotError::StateInvariant("split_window_transition"));
    }
    let Some((left_range, right_range)) = midpoint_split(&parent.planned.range) else {
        return Ok(SplitLeafResult::MinimumReached);
    };
    if state
        .windows
        .len()
        .checked_add(2)
        .is_none_or(|nodes| nodes > MAX_SNAPSHOT_WINDOWS)
        || state.executable_leaves().len().saturating_add(1)
            > usize::from(policy.maximum_leaf_windows)
    {
        return Ok(SplitLeafResult::LeafLimitReached);
    }
    let parent_planned = parent.planned.clone();
    let left = PlannedWindow::adaptive_child(&parent_planned, left_range);
    let right = PlannedWindow::adaptive_child(&parent_planned, right_range);
    if left.id == right.id
        || state.windows.contains_key(&left.id)
        || state.windows.contains_key(&right.id)
    {
        return Err(SnapshotError::StateInvariant("adaptive_window_identity"));
    }
    let split = WindowSplit {
        policy_version: policy.policy_version,
        trigger: policy.split_trigger,
        algorithm: policy.split_algorithm,
        left_window_id: left.id.clone(),
        right_window_id: right.id.clone(),
    };
    let progress = state
        .windows
        .get_mut(window_id)
        .ok_or(SnapshotError::StateInvariant("window"))?;
    progress.phase = WindowPhase::Split;
    progress.split = Some(split);
    for child in [left, right] {
        state.windows.insert(
            child.id.clone(),
            WindowProgress {
                planned: child,
                phase: WindowPhase::Pending,
                parent_window_id: Some(window_id.to_string()),
                split: None,
                staged_record_keys: BTreeSet::new(),
                stage_attempt: None,
                stage_receipt: None,
                evidence: None,
            },
        );
    }
    state.warning_codes.insert(WarningCode::AdaptiveWindowSplit);
    state.set_phase(SnapshotPhase::PlanWindows, None);
    Ok(SplitLeafResult::Created)
}

fn validate_window_graph(
    state: &DurableSnapshotState,
    plan: &SnapshotPlan,
) -> Result<(), SnapshotError> {
    let policy = plan
        .adaptive_window_policy
        .as_ref()
        .ok_or(SnapshotError::CorruptState)?;
    let root_ids = plan
        .windows
        .iter()
        .map(|window| window.id.clone())
        .collect::<BTreeSet<_>>();
    if root_ids.len() != plan.windows.len()
        || plan.windows.iter().any(|planned| {
            state.windows.get(&planned.id).is_none_or(|window| {
                window.planned != *planned || window.parent_window_id.is_some()
            })
        })
    {
        return Err(SnapshotError::CorruptState);
    }

    let mut visited = BTreeSet::new();
    let mut stack = root_ids.iter().cloned().collect::<Vec<_>>();
    while let Some(window_id) = stack.pop() {
        if !visited.insert(window_id.clone()) {
            return Err(SnapshotError::CorruptState);
        }
        let window = state
            .windows
            .get(&window_id)
            .ok_or(SnapshotError::CorruptState)?;
        match (&window.phase, &window.split) {
            (WindowPhase::Split, Some(split)) => {
                if split.policy_version != policy.policy_version
                    || split.trigger != policy.split_trigger
                    || split.algorithm != policy.split_algorithm
                    || !window.staged_record_keys.is_empty()
                    || window.stage_attempt.is_some()
                    || window.stage_receipt.is_some()
                    || window.evidence.is_some()
                {
                    return Err(SnapshotError::CorruptState);
                }
                let (left_range, right_range) =
                    midpoint_split(&window.planned.range).ok_or(SnapshotError::CorruptState)?;
                let expected_left = PlannedWindow::adaptive_child(&window.planned, left_range);
                let expected_right = PlannedWindow::adaptive_child(&window.planned, right_range);
                for (child_id, expected) in [
                    (&split.left_window_id, expected_left),
                    (&split.right_window_id, expected_right),
                ] {
                    let child = state
                        .windows
                        .get(child_id)
                        .ok_or(SnapshotError::CorruptState)?;
                    if child.parent_window_id.as_deref() != Some(window_id.as_str())
                        || child.planned != expected
                    {
                        return Err(SnapshotError::CorruptState);
                    }
                    stack.push(child_id.clone());
                }
            }
            (WindowPhase::Split, None) | (_, Some(_)) => {
                return Err(SnapshotError::CorruptState);
            }
            (_, None) => {}
        }
    }
    if visited.len() != state.windows.len() {
        return Err(SnapshotError::CorruptState);
    }
    let leaves = state.executable_leaves();
    if leaves.is_empty() || leaves.len() > usize::from(policy.maximum_leaf_windows) {
        return Err(SnapshotError::CorruptState);
    }
    for pair in leaves.windows(2) {
        if pair[1].range.from_yyyymmdd <= pair[0].range.to_yyyymmdd {
            return Err(SnapshotError::CorruptState);
        }
    }
    Ok(())
}

fn midpoint_split(range: &ReadWindow) -> Option<(ReadWindow, ReadWindow)> {
    let from = parse_yyyymmdd(&range.from_yyyymmdd)?;
    let to = parse_yyyymmdd(&range.to_yyyymmdd)?;
    if from >= to {
        return None;
    }
    let midpoint = from + ChronoDuration::days((to - from).num_days() / 2);
    let right_from = midpoint + ChronoDuration::days(1);
    Some((
        ReadWindow {
            from_yyyymmdd: from.format("%Y%m%d").to_string(),
            to_yyyymmdd: midpoint.format("%Y%m%d").to_string(),
        },
        ReadWindow {
            from_yyyymmdd: right_from.format("%Y%m%d").to_string(),
            to_yyyymmdd: to.format("%Y%m%d").to_string(),
        },
    ))
}

fn parse_yyyymmdd(value: &str) -> Option<NaiveDate> {
    (value.len() == 8 && value.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| NaiveDate::parse_from_str(value, "%Y%m%d").ok())
        .flatten()
}

#[derive(Debug, thiserror::Error)]
pub enum SnapshotError {
    #[error("invalid snapshot plan ({0})")]
    InvalidPlan(&'static str),
    #[error("the durable run belongs to a different immutable plan")]
    ResumePlanMismatch,
    #[error("the durable run predates restart-safe plan persistence")]
    ResumePlanUnavailable,
    #[error("durable snapshot state is corrupt")]
    CorruptState,
    #[error("durable snapshot state migration is not installed")]
    StateMigrationMissing,
    #[error("another worker owns the durable snapshot lease")]
    LeaseUnavailable,
    #[error("durable snapshot process lock failed")]
    LeaseIo(#[source] std::io::Error),
    #[error("the durable snapshot generation changed concurrently")]
    StateConflict,
    #[error("snapshot state operation failed")]
    StateStore(#[source] sqlx::Error),
    #[error("mirror operation failed")]
    Mirror(#[from] MirrorError),
    #[error("snapshot reconciliation failed")]
    Reconciliation(#[from] ReconciliationError),
    #[error("snapshot checkpoint changed concurrently")]
    ConcurrentCheckpoint,
    #[error("snapshot state invariant failed ({0})")]
    StateInvariant(&'static str),
    #[error("canonical state serialization failed")]
    Serialization,
}

#[async_trait]
pub trait SnapshotStateStore: Send + Sync {
    async fn load(&self, resume_key: &str) -> Result<Option<DurableSnapshotState>, SnapshotError>;
    async fn save(&self, state: &mut DurableSnapshotState) -> Result<(), SnapshotError>;
    async fn heartbeat(&self, state: &DurableSnapshotState) -> Result<(), SnapshotError>;
}

#[derive(Clone)]
pub struct SqliteSnapshotStateStore {
    pool: SqlitePool,
    lease_owner: Option<String>,
    process_leases: Arc<Mutex<BTreeMap<String, File>>>,
}

impl SqliteSnapshotStateStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self {
            pool,
            lease_owner: None,
            process_leases: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    pub fn for_worker(pool: SqlitePool, lease_owner: String) -> Result<Self, SnapshotError> {
        if lease_owner.is_empty()
            || lease_owner.len() > 128
            || lease_owner.chars().any(char::is_control)
        {
            return Err(SnapshotError::InvalidPlan("lease_owner"));
        }
        Ok(Self {
            pool,
            lease_owner: Some(lease_owner),
            process_leases: Arc::new(Mutex::new(BTreeMap::new())),
        })
    }

    async fn process_lease_path(&self, resume_key: &str) -> Result<Option<PathBuf>, SnapshotError> {
        let database_path = sqlx::query_scalar::<_, String>(
            "SELECT file FROM pragma_database_list WHERE name = 'main'",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(SnapshotError::StateStore)?;
        if database_path.is_empty() {
            // SQLite in-memory databases have no cross-process identity. Their test-only lease
            // behavior therefore retains the bounded UTC fallback below.
            return Ok(None);
        }
        let database_path = PathBuf::from(database_path);
        let file_name = database_path.file_name().ok_or_else(|| {
            SnapshotError::LeaseIo(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "snapshot database path has no file name",
            ))
        })?;
        let mut lock_name = file_name.to_os_string();
        lock_name.push(format!(
            ".snapshot-{}.lock",
            sha256_bytes(resume_key.as_bytes())
        ));
        Ok(Some(database_path.with_file_name(lock_name)))
    }

    async fn acquire_process_lease(&self, resume_key: &str) -> Result<bool, SnapshotError> {
        let Some(lock_path) = self.process_lease_path(resume_key).await? else {
            return Ok(false);
        };
        let mut leases = self
            .process_leases
            .lock()
            .map_err(|_| SnapshotError::LeaseUnavailable)?;
        if leases.contains_key(resume_key) {
            return Ok(true);
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)
            .map_err(SnapshotError::LeaseIo)?;
        match file.try_lock() {
            Ok(()) => {
                leases.insert(resume_key.to_string(), file);
                Ok(true)
            }
            Err(error) => {
                let error: std::io::Error = error.into();
                if error.kind() == std::io::ErrorKind::WouldBlock {
                    Err(SnapshotError::LeaseUnavailable)
                } else {
                    Err(SnapshotError::LeaseIo(error))
                }
            }
        }
    }

    fn holds_process_lease(&self, resume_key: &str) -> Result<bool, SnapshotError> {
        self.process_leases
            .lock()
            .map(|leases| leases.contains_key(resume_key))
            .map_err(|_| SnapshotError::LeaseUnavailable)
    }

    async fn required_process_lease(&self, resume_key: &str) -> Result<bool, SnapshotError> {
        let file_backed = self.process_lease_path(resume_key).await?.is_some();
        let held = self.holds_process_lease(resume_key)?;
        if file_backed && !held {
            return Err(SnapshotError::LeaseUnavailable);
        }
        Ok(held)
    }

    fn drop_process_lease(&self, resume_key: &str) -> Result<(), SnapshotError> {
        self.process_leases
            .lock()
            .map_err(|_| SnapshotError::LeaseUnavailable)?
            .remove(resume_key);
        Ok(())
    }

    pub async fn migrate(&self) -> Result<(), SnapshotError> {
        let installed = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_schema_migrations WHERE version IN (4, 5, 9, 10, 12)",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(SnapshotError::StateStore)?;
        let table_exists = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM sqlite_master \
             WHERE type = 'table' AND name = 'tally_snapshot_run_states'",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(SnapshotError::StateStore)?;
        let recovery_columns = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM pragma_table_info('tally_snapshot_run_states') \
             WHERE name IN ('row_sha256', 'lease_owner', 'lease_expires_at_unix_ms')",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(SnapshotError::StateStore)?;
        let unique_run_index = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' \
             AND name = 'idx_tally_snapshot_run_states_unique_run'",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(SnapshotError::StateStore)?;
        let composite_batch_identity = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM pragma_index_list('tally_observation_batches') AS list \
             WHERE list.[unique] = 1 \
               AND (SELECT COUNT(*) FROM pragma_index_info(list.name)) = 2 \
               AND EXISTS (SELECT 1 FROM pragma_index_info(list.name) \
                 WHERE seqno = 0 AND name = 'run_id') \
               AND EXISTS (SELECT 1 FROM pragma_index_info(list.name) \
                 WHERE seqno = 1 AND name = 'pack_id')",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(SnapshotError::StateStore)?;
        let recovery_triggers = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'trigger' AND name IN (\
             'trg_tally_snapshot_run_identity_immutable', \
             'trg_tally_snapshot_terminal_immutable', \
             'trg_tally_snapshot_state_no_delete')",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(SnapshotError::StateStore)?;
        let window_staging_tables = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name IN (\
             'tally_snapshot_window_attempts', 'tally_snapshot_window_memberships')",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(SnapshotError::StateStore)?;
        let window_staging_triggers = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'trigger' AND name IN (\
             'trg_tally_snapshot_window_attempt_terminal_immutable', \
             'trg_tally_snapshot_window_attempt_identity_immutable', \
             'trg_tally_snapshot_window_attempt_no_delete', \
             'trg_tally_snapshot_window_membership_insert_open_attempt', \
             'trg_tally_snapshot_window_membership_content_immutable', \
             'trg_tally_snapshot_window_membership_last_seen_advance', \
             'trg_tally_snapshot_window_membership_no_delete', \
             'trg_tally_snapshot_window_attempt_terminal_reason_insert', \
             'trg_tally_snapshot_window_attempt_terminal_reason_shape')",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(SnapshotError::StateStore)?;
        let terminal_evidence_column = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM pragma_table_info('tally_snapshot_window_attempts') \
             WHERE name = 'terminal_safe_reason_code'",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(SnapshotError::StateStore)?;
        let json_available = sqlx::query_scalar::<_, i64>("SELECT json_valid('{}')")
            .fetch_one(&self.pool)
            .await
            .map_err(SnapshotError::StateStore)?;
        if installed != 5
            || table_exists != 1
            || recovery_columns != 3
            || unique_run_index != 1
            || composite_batch_identity != 1
            || recovery_triggers != 3
            || window_staging_tables != 2
            || window_staging_triggers != 9
            || terminal_evidence_column != 1
            || json_available != 1
        {
            return Err(SnapshotError::StateMigrationMissing);
        }
        Ok(())
    }

    pub async fn claim(&self, resume_key: &str) -> Result<bool, SnapshotError> {
        let owner = self
            .lease_owner
            .as_deref()
            .ok_or(SnapshotError::LeaseUnavailable)?;
        let now = Utc::now().timestamp_millis();
        let process_locked = self.acquire_process_lease(resume_key).await?;
        let result = if process_locked {
            // The kernel releases this lock when a worker crashes. Once acquired, an old UTC
            // expiry cannot strand a restart after a wall-clock rollback, and a live owner
            // cannot be stolen regardless of timestamp movement.
            sqlx::query(
                "UPDATE tally_snapshot_run_states SET lease_owner = ?1, \
                   lease_expires_at_unix_ms = ?2 WHERE resume_key = ?3",
            )
            .bind(owner)
            .bind(now.saturating_add(WORKER_LEASE_TTL_MS))
            .bind(resume_key)
            .execute(&self.pool)
            .await
        } else {
            sqlx::query(
                "UPDATE tally_snapshot_run_states SET lease_owner = ?1, \
                   lease_expires_at_unix_ms = ?2 \
                 WHERE resume_key = ?3 AND (lease_owner IS NULL OR lease_owner = ?1 OR \
                   lease_expires_at_unix_ms <= ?4)",
            )
            .bind(owner)
            .bind(now.saturating_add(WORKER_LEASE_TTL_MS))
            .bind(resume_key)
            .bind(now)
            .execute(&self.pool)
            .await
        }
        .map_err(|error| {
            if process_locked {
                let _ = self.drop_process_lease(resume_key);
            }
            SnapshotError::StateStore(error)
        })?;
        if result.rows_affected() == 1 {
            return Ok(true);
        }
        let exists = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_snapshot_run_states WHERE resume_key = ?1",
        )
        .bind(resume_key)
        .fetch_one(&self.pool)
        .await
        .map_err(SnapshotError::StateStore)?;
        if exists == 0 {
            Ok(false)
        } else {
            if process_locked {
                self.drop_process_lease(resume_key)?;
            }
            Err(SnapshotError::LeaseUnavailable)
        }
    }

    pub async fn release(&self, resume_key: &str) -> Result<(), SnapshotError> {
        let owner = self
            .lease_owner
            .as_deref()
            .ok_or(SnapshotError::LeaseUnavailable)?;
        sqlx::query(
            "UPDATE tally_snapshot_run_states SET lease_owner = NULL, \
               lease_expires_at_unix_ms = NULL \
             WHERE resume_key = ?1 AND lease_owner = ?2",
        )
        .bind(resume_key)
        .bind(owner)
        .execute(&self.pool)
        .await
        .map_err(SnapshotError::StateStore)?;
        self.drop_process_lease(resume_key)?;
        Ok(())
    }

    pub async fn load_by_run_id(
        &self,
        run_id: &str,
    ) -> Result<Option<DurableSnapshotState>, SnapshotError> {
        if run_id.is_empty() || run_id.len() > 256 || run_id.chars().any(char::is_control) {
            return Err(SnapshotError::InvalidPlan("run_id"));
        }
        let rows = sqlx::query(
            "SELECT resume_key, run_id, generation, state_json, state_sha256, row_sha256 \
             FROM tally_snapshot_run_states \
             WHERE run_id = ?1 ORDER BY updated_at_unix_ms DESC LIMIT 2",
        )
        .bind(run_id)
        .fetch_all(&self.pool)
        .await
        .map_err(SnapshotError::StateStore)?;
        match rows.len() {
            0 => Ok(None),
            1 => decode_state_row(&rows[0]).map(Some),
            _ => Err(SnapshotError::CorruptState),
        }
    }

    pub async fn load_recent(
        &self,
        limit: u32,
    ) -> Result<Vec<DurableSnapshotState>, SnapshotError> {
        if !(1..=100).contains(&limit) {
            return Err(SnapshotError::InvalidPlan("recent_limit"));
        }
        let rows = sqlx::query(
            "SELECT resume_key, run_id, generation, state_json, state_sha256, row_sha256 \
             FROM tally_snapshot_run_states \
             ORDER BY updated_at_unix_ms DESC LIMIT ?1",
        )
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .map_err(SnapshotError::StateStore)?;
        rows.iter().map(decode_state_row).collect()
    }
}

#[async_trait]
impl SnapshotStateStore for SqliteSnapshotStateStore {
    async fn load(&self, resume_key: &str) -> Result<Option<DurableSnapshotState>, SnapshotError> {
        let row = sqlx::query(
            "SELECT resume_key, run_id, generation, state_json, state_sha256, row_sha256 \
             FROM tally_snapshot_run_states WHERE resume_key = ?1",
        )
        .bind(resume_key)
        .fetch_optional(&self.pool)
        .await
        .map_err(SnapshotError::StateStore)?;
        let Some(row) = row else {
            return Ok(None);
        };
        decode_state_row(&row).map(Some)
    }

    async fn save(&self, state: &mut DurableSnapshotState) -> Result<(), SnapshotError> {
        let owner = self
            .lease_owner
            .as_deref()
            .ok_or(SnapshotError::LeaseUnavailable)?;
        state.validate_invariants()?;
        // File-backed stores must prove kernel-level ownership even for the first insert. This
        // keeps direct callers from bypassing claim() and manufacturing a live-looking DB lease.
        let process_locked = self.required_process_lease(&state.resume_key).await?;
        let expected_generation = state.generation;
        let next_generation = expected_generation
            .checked_add(1)
            .ok_or(SnapshotError::StateConflict)?;
        state.generation = next_generation;
        let state_json = match serde_json::to_string(state) {
            Ok(json) if json.len() <= MAX_DURABLE_STATE_BYTES => json,
            Ok(_) => {
                state.generation = expected_generation;
                return Err(SnapshotError::StateInvariant("state_size"));
            }
            Err(_) => {
                state.generation = expected_generation;
                return Err(SnapshotError::Serialization);
            }
        };
        let state_sha256 = sha256_bytes(state_json.as_bytes());
        let row_sha256 = snapshot_state_row_sha256(
            &state.resume_key,
            &state.run_id,
            next_generation,
            &state_sha256,
        );
        let now = Utc::now().timestamp_millis();
        let terminal = state.progress.phase.is_terminal();
        let result = if expected_generation == 0 {
            sqlx::query(
                "INSERT OR IGNORE INTO tally_snapshot_run_states(\
                   resume_key, run_id, generation, state_sha256, state_json, row_sha256, \
                   lease_owner, lease_expires_at_unix_ms, updated_at_unix_ms\
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )
            .bind(&state.resume_key)
            .bind(&state.run_id)
            .bind(i64::try_from(next_generation).map_err(|_| SnapshotError::StateConflict)?)
            .bind(&state_sha256)
            .bind(&state_json)
            .bind(&row_sha256)
            .bind((!terminal).then_some(owner))
            .bind((!terminal).then_some(now.saturating_add(WORKER_LEASE_TTL_MS)))
            .bind(now)
            .execute(&self.pool)
            .await
        } else if process_locked {
            sqlx::query(
                "UPDATE tally_snapshot_run_states SET generation = ?1, state_sha256 = ?2, \
                   state_json = ?3, row_sha256 = ?4, lease_owner = ?5, \
                   lease_expires_at_unix_ms = ?6, updated_at_unix_ms = ?7 \
                 WHERE resume_key = ?8 AND run_id = ?9 AND generation = ?10 \
                   AND lease_owner = ?11",
            )
            .bind(i64::try_from(next_generation).map_err(|_| SnapshotError::StateConflict)?)
            .bind(&state_sha256)
            .bind(&state_json)
            .bind(&row_sha256)
            .bind((!terminal).then_some(owner))
            .bind((!terminal).then_some(now.saturating_add(WORKER_LEASE_TTL_MS)))
            .bind(now)
            .bind(&state.resume_key)
            .bind(&state.run_id)
            .bind(i64::try_from(expected_generation).map_err(|_| SnapshotError::StateConflict)?)
            .bind(owner)
            .execute(&self.pool)
            .await
        } else {
            sqlx::query(
                "UPDATE tally_snapshot_run_states SET generation = ?1, state_sha256 = ?2, \
                   state_json = ?3, row_sha256 = ?4, lease_owner = ?5, \
                   lease_expires_at_unix_ms = ?6, updated_at_unix_ms = ?7 \
                 WHERE resume_key = ?8 AND run_id = ?9 AND generation = ?10 \
                   AND lease_owner = ?11 AND lease_expires_at_unix_ms > ?12",
            )
            .bind(i64::try_from(next_generation).map_err(|_| SnapshotError::StateConflict)?)
            .bind(&state_sha256)
            .bind(&state_json)
            .bind(&row_sha256)
            .bind((!terminal).then_some(owner))
            .bind((!terminal).then_some(now.saturating_add(WORKER_LEASE_TTL_MS)))
            .bind(now)
            .bind(&state.resume_key)
            .bind(&state.run_id)
            .bind(i64::try_from(expected_generation).map_err(|_| SnapshotError::StateConflict)?)
            .bind(owner)
            .bind(now)
            .execute(&self.pool)
            .await
        };
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                state.generation = expected_generation;
                return Err(SnapshotError::StateStore(error));
            }
        };
        if result.rows_affected() != 1 {
            state.generation = expected_generation;
            return Err(SnapshotError::StateConflict);
        }
        state.row_integrity_bound = true;
        if terminal && process_locked {
            // Terminal state clears the DB owner in the same write; release the matching kernel
            // lock promptly rather than relying on the coordinator task to drop its store.
            self.drop_process_lease(&state.resume_key)?;
        }
        Ok(())
    }

    async fn heartbeat(&self, state: &DurableSnapshotState) -> Result<(), SnapshotError> {
        let owner = self
            .lease_owner
            .as_deref()
            .ok_or(SnapshotError::LeaseUnavailable)?;
        if state.progress.phase.is_terminal() || state.generation == 0 {
            return Err(SnapshotError::StateInvariant("heartbeat_state"));
        }
        let now = Utc::now().timestamp_millis();
        let process_locked = self.required_process_lease(&state.resume_key).await?;
        let mut query = sqlx::query(
            "UPDATE tally_snapshot_run_states SET lease_expires_at_unix_ms = ?1 \
             WHERE resume_key = ?2 AND run_id = ?3 AND generation = ?4 \
               AND lease_owner = ?5 AND (?6 OR lease_expires_at_unix_ms > ?7)",
        )
        .bind(now.saturating_add(WORKER_LEASE_TTL_MS))
        .bind(&state.resume_key)
        .bind(&state.run_id)
        .bind(i64::try_from(state.generation).map_err(|_| SnapshotError::StateConflict)?)
        .bind(owner)
        .bind(process_locked);
        query = query.bind(now);
        let result = query
            .execute(&self.pool)
            .await
            .map_err(SnapshotError::StateStore)?;
        if result.rows_affected() != 1 {
            return Err(SnapshotError::LeaseUnavailable);
        }
        Ok(())
    }
}

fn decode_state_row(row: &sqlx::sqlite::SqliteRow) -> Result<DurableSnapshotState, SnapshotError> {
    let row_resume_key: String = row
        .try_get("resume_key")
        .map_err(SnapshotError::StateStore)?;
    let row_run_id: String = row.try_get("run_id").map_err(SnapshotError::StateStore)?;
    let row_generation: i64 = row
        .try_get("generation")
        .map_err(SnapshotError::StateStore)?;
    let row_generation = u64::try_from(row_generation).map_err(|_| SnapshotError::CorruptState)?;
    let state_json: String = row
        .try_get("state_json")
        .map_err(SnapshotError::StateStore)?;
    if state_json.len() > MAX_DURABLE_STATE_BYTES {
        return Err(SnapshotError::CorruptState);
    }
    let state_sha256: String = row
        .try_get("state_sha256")
        .map_err(SnapshotError::StateStore)?;
    if sha256_bytes(state_json.as_bytes()) != state_sha256 {
        return Err(SnapshotError::CorruptState);
    }
    let row_sha256: Option<String> = row
        .try_get("row_sha256")
        .map_err(SnapshotError::StateStore)?;
    let mut state: DurableSnapshotState =
        serde_json::from_str(&state_json).map_err(|_| SnapshotError::CorruptState)?;
    if state.generation == 0 && row_sha256.is_none() {
        // Legacy v4 state: readable for evidence, deliberately not row-bound or restart-resumable.
        state.generation = row_generation;
    }
    if state.resume_key != row_resume_key
        || state.run_id != row_run_id
        || state.generation != row_generation
    {
        return Err(SnapshotError::CorruptState);
    }
    if let Some(row_sha256) = row_sha256 {
        if !is_lower_sha256(&row_sha256)
            || row_sha256
                != snapshot_state_row_sha256(
                    &row_resume_key,
                    &row_run_id,
                    row_generation,
                    &state_sha256,
                )
        {
            return Err(SnapshotError::CorruptState);
        }
        state.row_integrity_bound = true;
    }
    state.validate_invariants()?;
    Ok(state)
}

fn snapshot_state_row_sha256(
    resume_key: &str,
    run_id: &str,
    generation: u64,
    state_sha256: &str,
) -> String {
    let mut digest = Sha256::new();
    digest.update(b"bridge-tally-snapshot-state-row-v1\0");
    for value in [
        resume_key.as_bytes(),
        run_id.as_bytes(),
        state_sha256.as_bytes(),
    ] {
        digest.update((value.len() as u64).to_be_bytes());
        digest.update(value);
    }
    digest.update(generation.to_be_bytes());
    hex_digest(digest.finalize())
}

pub trait CancellationSignal: Send + Sync {
    fn is_cancelled(&self) -> bool;
}

#[derive(Debug, Default)]
pub struct AtomicCancellation {
    cancelled: AtomicBool,
}

impl AtomicCancellation {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

impl CancellationSignal for AtomicCancellation {
    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

#[derive(Debug, Clone)]
pub struct SnapshotRunResult {
    pub state: DurableSnapshotState,
    pub proof: ProofManifest,
    pub receipt: StoredCommitReceipt,
}

/// What a phase of the snapshot run decided: carry on with the durable state, or
/// stop with a terminal result.
/// `large_enum_variant` fires here and its remedy would be a regression.
/// `Continue` is large because it carries the durable state itself, which `run`
/// already moved by value before these phases existed; boxing it would add an
/// allocation on every phase to shrink an enum that is never stored, only
/// returned and immediately destructured. The rare variant is boxed instead,
/// which is the half that actually costs nothing.
#[allow(clippy::large_enum_variant)]
enum PhaseOutcome {
    Continue(DurableSnapshotState),
    /// Boxed so the common path stops paying for the rare one: `Continue` is
    /// moved on every phase, `Finished` happens at most once per run and only
    /// alongside I/O that dwarfs an allocation.
    Finished(Box<SnapshotRunResult>),
}

pub struct FullSnapshotEngine<'a, S, C> {
    mirror: &'a TallyMirrorRepository,
    state_store: &'a S,
    connector: &'a C,
}

enum ConnectorAwait<T> {
    Completed(Result<T, TallyError>),
    Cancelled,
}

struct CleanupDecisionEvidence {
    gap_changed: bool,
    completed_at_floor: Option<i64>,
}

impl<'a, S, C> FullSnapshotEngine<'a, S, C>
where
    S: SnapshotStateStore,
    C: TallyConnector,
{
    fn record_attempt_abandonment(
        state: &mut DurableSnapshotState,
        result: AbandonSnapshotWindowAttemptResult,
    ) {
        Self::record_local_clock_rollback(state, result.local_clock_moved_backwards);
    }

    fn record_local_clock_rollback(state: &mut DurableSnapshotState, moved_backwards: bool) {
        if moved_backwards {
            state
                .gap_codes
                .insert("local_clock_moved_backwards".to_string());
        }
    }

    fn clamp_run_completion(
        state: &mut DurableSnapshotState,
        started_at_unix_ms: i64,
        observed_completed_at_unix_ms: i64,
    ) -> i64 {
        Self::record_local_clock_rollback(
            state,
            observed_completed_at_unix_ms < started_at_unix_ms,
        );
        observed_completed_at_unix_ms.max(started_at_unix_ms)
    }

    async fn close_open_attempts_before_decision(
        &self,
        state: &mut DurableSnapshotState,
    ) -> Result<CleanupDecisionEvidence, SnapshotError> {
        let already_recorded = state.gap_codes.contains("local_clock_moved_backwards");
        let batch_id = state
            .batch_id
            .clone()
            .ok_or(SnapshotError::StateInvariant("batch_id"))?;
        let cleanup = self
            .mirror
            .abandon_open_snapshot_window_attempts_for_batch(
                &batch_id,
                Utc::now().timestamp_millis(),
            )
            .await?;
        Self::record_local_clock_rollback(state, cleanup.local_clock_moved_backwards);
        Ok(CleanupDecisionEvidence {
            gap_changed: !already_recorded
                && state.gap_codes.contains("local_clock_moved_backwards"),
            completed_at_floor: cleanup.completed_at_floor,
        })
    }

    pub fn new(mirror: &'a TallyMirrorRepository, state_store: &'a S, connector: &'a C) -> Self {
        Self {
            mirror,
            state_store,
            connector,
        }
    }

    async fn await_connector<T, F>(
        &self,
        state: &DurableSnapshotState,
        cancellation: &dyn CancellationSignal,
        future: F,
    ) -> Result<ConnectorAwait<T>, SnapshotError>
    where
        F: Future<Output = Result<T, TallyError>>,
    {
        self.state_store.heartbeat(state).await?;
        if cancellation.is_cancelled() {
            return Ok(ConnectorAwait::Cancelled);
        }
        tokio::pin!(future);
        let mut heartbeat = tokio::time::interval_at(
            tokio::time::Instant::now() + WORKER_LEASE_HEARTBEAT_INTERVAL,
            WORKER_LEASE_HEARTBEAT_INTERVAL,
        );
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut cancellation_poll = tokio::time::interval(CANCELLATION_POLL_INTERVAL);
        cancellation_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        cancellation_poll.tick().await;
        loop {
            tokio::select! {
                result = &mut future => {
                    self.state_store.heartbeat(state).await?;
                    return Ok(if cancellation.is_cancelled()
                        || matches!(result, Err(TallyError::Cancelled))
                    {
                        ConnectorAwait::Cancelled
                    } else {
                        ConnectorAwait::Completed(result)
                    });
                },
                _ = heartbeat.tick() => self.state_store.heartbeat(state).await?,
                _ = cancellation_poll.tick() => {
                    if cancellation.is_cancelled() {
                        return Ok(ConnectorAwait::Cancelled);
                    }
                }
            }
        }
    }

    async fn recover_window_attempts(
        &self,
        state: &mut DurableSnapshotState,
    ) -> Result<(), SnapshotError> {
        let staging_window_ids = state
            .windows
            .iter()
            .filter(|(_, progress)| progress.phase == WindowPhase::Staging)
            .map(|(window_id, _)| window_id.clone())
            .collect::<Vec<_>>();
        for window_id in staging_window_ids {
            let (planned, durable_attempt) = state
                .windows
                .get(&window_id)
                .and_then(|progress| {
                    progress
                        .stage_attempt
                        .clone()
                        .map(|attempt| (progress.planned.clone(), attempt))
                })
                .ok_or(SnapshotError::CorruptState)?;
            let repository_attempt = durable_attempt.repository_ref();
            let latest = self
                .mirror
                .load_latest_completed_window_receipt(
                    &repository_attempt.batch_id,
                    &repository_attempt.window_id,
                )
                .await?;
            if let Some(completion) = latest.filter(|completion| {
                completion.receipt.attempt_id == repository_attempt.attempt_id
                    && completion.receipt.attempt_ordinal == repository_attempt.attempt_ordinal
            }) {
                let attempt_warning_codes = durable_attempt.warning_codes.clone();
                Self::record_local_clock_rollback(state, completion.local_clock_moved_backwards);
                let receipt = completion.receipt;
                let mut evidence = receipt_window_evidence(&planned, &receipt)?;
                evidence.record_set_sha256 = Some(receipt.membership_sha256.clone());
                if evidence.record_provenance_scope == ComparisonScope::Unavailable {
                    state
                        .gap_codes
                        .insert("record_provenance_unavailable".to_string());
                }
                let progress = state
                    .windows
                    .get_mut(&window_id)
                    .ok_or(SnapshotError::CorruptState)?;
                progress.stage_attempt = None;
                progress.stage_receipt = Some(WindowStageReceipt::from(&receipt));
                progress.evidence = Some(evidence);
                progress.phase = WindowPhase::Complete;
                state.warning_codes.extend(attempt_warning_codes);
            } else {
                match self
                    .mirror
                    .abandon_snapshot_window_attempt(
                        &repository_attempt,
                        Utc::now().timestamp_millis(),
                    )
                    .await
                {
                    Ok(result) => Self::record_attempt_abandonment(state, result),
                    Err(MirrorError::NotFound | MirrorError::WindowAttemptClosed) => {}
                    Err(error) => return Err(error.into()),
                }
                let progress = state
                    .windows
                    .get_mut(&window_id)
                    .ok_or(SnapshotError::CorruptState)?;
                progress.stage_attempt = None;
                progress.stage_receipt = None;
                progress.evidence = None;
                progress.phase = WindowPhase::Pending;
            }
            state.set_phase(SnapshotPhase::PlanWindows, Some(window_id));
            self.state_store.save(state).await?;
        }
        Ok(())
    }

    async fn hydrate_completed_window_records(
        &self,
        state: &mut DurableSnapshotState,
    ) -> Result<(), SnapshotError> {
        if reconciliation_record_budget_exceeded(state)? {
            // Every caller must terminalize this condition. Keep a defensive invariant here so a
            // future call site cannot accidentally restore the unbounded allocation path.
            return Err(SnapshotError::StateInvariant(
                "reconciliation_record_budget",
            ));
        }
        let mut local_clock_moved_backwards = false;
        for progress in state
            .windows
            .values_mut()
            .filter(|progress| progress.phase == WindowPhase::Complete)
        {
            let durable_receipt = progress
                .stage_receipt
                .as_ref()
                .ok_or(SnapshotError::CorruptState)?;
            let stored_completion = self
                .mirror
                .load_latest_completed_window_receipt(
                    &durable_receipt.attempt.batch_id,
                    &durable_receipt.attempt.window_id,
                )
                .await?
                .ok_or(SnapshotError::CorruptState)?;
            local_clock_moved_backwards |= stored_completion.local_clock_moved_backwards;
            let stored_receipt = stored_completion.receipt;
            if stored_receipt.attempt_id != durable_receipt.attempt.attempt_id
                || stored_receipt.attempt_ordinal != durable_receipt.attempt.attempt_ordinal
                || stored_receipt.member_count != durable_receipt.member_count
                || stored_receipt.membership_sha256 != durable_receipt.membership_sha256
                || stored_receipt.receipt_sha256 != durable_receipt.receipt_sha256
            {
                return Err(SnapshotError::CorruptState);
            }
            let mut stored_evidence = receipt_window_evidence(&progress.planned, &stored_receipt)?;
            stored_evidence.record_set_sha256 = Some(stored_receipt.membership_sha256.clone());
            let evidence = progress
                .evidence
                .as_mut()
                .ok_or(SnapshotError::CorruptState)?;
            let mut durable_evidence = evidence.clone();
            durable_evidence.canonical_records.clear();
            if durable_evidence != stored_evidence {
                return Err(SnapshotError::CorruptState);
            }
            let records = self
                .mirror
                .load_completed_window_canonical_record_map(
                    &durable_receipt.attempt.repository_ref(),
                )
                .await?;
            if u32::try_from(records.len()).ok() != Some(durable_receipt.member_count) {
                return Err(SnapshotError::CorruptState);
            }
            if evidence.record_set_sha256.as_deref()
                != Some(durable_receipt.membership_sha256.as_str())
            {
                return Err(SnapshotError::CorruptState);
            }
            evidence.canonical_records = records;
        }
        Self::record_local_clock_rollback(state, local_clock_moved_backwards);
        Ok(())
    }

    async fn abandon_open_window_attempts(
        &self,
        state: &mut DurableSnapshotState,
    ) -> Result<(), SnapshotError> {
        let attempts = state
            .windows
            .values()
            .filter_map(|progress| progress.stage_attempt.clone())
            .collect::<Vec<_>>();
        for attempt in attempts {
            match self
                .mirror
                .abandon_snapshot_window_attempt(
                    &attempt.repository_ref(),
                    Utc::now().timestamp_millis(),
                )
                .await
            {
                Ok(result) => Self::record_attempt_abandonment(state, result),
                Err(
                    MirrorError::NotFound
                    | MirrorError::WindowAttemptClosed
                    | MirrorError::BatchClosed,
                ) => {}
                Err(error) => return Err(error.into()),
            }
            let progress = state
                .windows
                .get_mut(&attempt.window_id)
                .ok_or(SnapshotError::CorruptState)?;
            progress.stage_attempt = None;
            progress.stage_receipt = None;
            progress.evidence = None;
            progress.phase = WindowPhase::Pending;
        }
        Ok(())
    }

    pub async fn run(
        &self,
        plan: &SnapshotPlan,
        cancellation: &dyn CancellationSignal,
    ) -> Result<SnapshotRunResult, SnapshotError> {
        plan.validate()?;
        let freshness = self
            .mirror
            .freshness(
                &plan.mirror_company_id,
                pack_code(plan.pack),
                Utc::now().timestamp_millis(),
            )
            .await?;
        let freshness_before = core_freshness(freshness.state);
        let mut state = match self.state_store.load(&plan.resume_key).await? {
            Some(state) => {
                state.assert_resumable_with(plan)?;
                state
            }
            None => {
                let mut state = DurableSnapshotState::new(plan, freshness_before)?;
                state.checkpoint_before = freshness.checkpoint_token;
                self.state_store.save(&mut state).await?;
                state
            }
        };

        if state.progress.phase.is_terminal() {
            return completed_result(state);
        }
        if state.progress.phase == SnapshotPhase::CommitPending {
            return self.resume_pending_commit(plan, state).await;
        }

        if state.batch_id.is_none() {
            let requested_from = plan
                .windows
                .iter()
                .map(|window| window.range.from_yyyymmdd.as_str())
                .min()
                .map(str::to_string);
            let requested_to = plan
                .windows
                .iter()
                .map(|window| window.range.to_yyyymmdd.as_str())
                .max()
                .map(str::to_string);
            state.batch_id = Some(
                self.mirror
                    .begin_batch(BeginBatchInput {
                        run_id: plan.run_id.clone(),
                        capability_snapshot_id: plan.capability_snapshot_id.clone(),
                        company_id: plan.mirror_company_id.clone(),
                        pack_id: pack_code(plan.pack).to_string(),
                        pack_schema_major: plan.pack_schema_version.major,
                        pack_schema_minor: plan.pack_schema_version.minor,
                        source_transport: plan.source_transport.clone(),
                        source_release: plan.source_release.clone(),
                        requested_from_yyyymmdd: requested_from,
                        requested_to_yyyymmdd: requested_to,
                        started_at_unix_ms: plan.started_at_unix_ms,
                    })
                    .await?,
            );
            self.state_store.save(&mut state).await?;
        }

        self.recover_window_attempts(&mut state).await?;

        if reconciliation_record_budget_exceeded(&state)? {
            return self
                .finish_terminal(
                    plan,
                    state,
                    TerminalKind::Failed,
                    RECONCILIATION_RECORD_BUDGET_CODE,
                )
                .await;
        }

        if cancellation.is_cancelled() {
            return self
                .finish_terminal(plan, state, TerminalKind::Cancelled, "run_cancelled")
                .await;
        }

        let state = match self.check_capability(plan, state, cancellation).await? {
            PhaseOutcome::Finished(result) => return Ok(*result),
            PhaseOutcome::Continue(state) => state,
        };
        let state = match self
            .check_company_identity(plan, state, cancellation)
            .await?
        {
            PhaseOutcome::Finished(result) => return Ok(*result),
            PhaseOutcome::Continue(state) => state,
        };

        let mut state = match self
            .plan_and_execute_windows(plan, state, cancellation)
            .await?
        {
            PhaseOutcome::Finished(result) => return Ok(*result),
            PhaseOutcome::Continue(state) => state,
        };

        if cancellation.is_cancelled() {
            return self
                .finish_terminal(plan, state, TerminalKind::Cancelled, "run_cancelled")
                .await;
        }
        state.source_stability_check = SourceStabilityCheck::Passed;
        let stability_windows = state.executable_leaves();
        for planned in &stability_windows {
            let context = RequestContext {
                run_id: plan.run_id.clone(),
                company: plan.company.clone(),
                pack: plan.pack,
                schema_version: plan.pack_schema_version,
                window: planned.range.clone(),
                query_profile: planned.query_profile.clone(),
                filters_sha256: planned.filters_sha256.clone(),
            };
            let reread_result = match self
                .await_connector(
                    &state,
                    cancellation,
                    self.connector.read_pack_window(&context),
                )
                .await?
            {
                ConnectorAwait::Completed(result) => result,
                ConnectorAwait::Cancelled => {
                    return self
                        .finish_terminal(plan, state, TerminalKind::Cancelled, "run_cancelled")
                        .await;
                }
            };
            let reread = match reread_result {
                Ok(window) => canonicalize_window(
                    &CanonicalWindowContext {
                        requested_pack: plan.pack,
                        schema_version: plan.pack_schema_version,
                        source_identity: &plan.company.identity,
                        query_profile: &planned.query_profile,
                        filters_sha256: &planned.filters_sha256,
                        external_references: &plan.external_references,
                        window_id: &planned.id,
                        requested_window: &planned.range,
                    },
                    &window,
                )
                .ok(),
                Err(_) => None,
            };
            let initial = state
                .windows
                .get(&planned.id)
                .and_then(|window| window.evidence.as_ref());
            match (initial, reread) {
                (Some(initial), Some(reread))
                    if same_source_semantics(initial, &reread.evidence) => {}
                (Some(_), Some(_)) => {
                    state.source_stability_check = SourceStabilityCheck::Mismatch;
                    break;
                }
                _ => {
                    state.source_stability_check = SourceStabilityCheck::Unavailable;
                    break;
                }
            }
        }
        if cancellation.is_cancelled() {
            return self
                .finish_terminal(plan, state, TerminalKind::Cancelled, "run_cancelled")
                .await;
        }
        self.state_store.save(&mut state).await?;

        let end_probe = match self
            .await_connector(&state, cancellation, self.connector.probe_fresh())
            .await?
        {
            ConnectorAwait::Completed(result) => result,
            ConnectorAwait::Cancelled => {
                return self
                    .finish_terminal(plan, state, TerminalKind::Cancelled, "run_cancelled")
                    .await;
            }
        };
        state.end_profile_check = match end_probe {
            Ok(probe)
                if probe.reachable
                    && probe.profile.packs.get(&plan.pack).is_some_and(|evidence| {
                        snapshot_pack_start_authorized(plan.pack, evidence)
                    })
                    && probe
                        .profile
                        .transports
                        .get(&TransportId::XmlHttp)
                        .is_some_and(|evidence| {
                            evidence.state == CapabilityState::Supported
                                && evidence.confidence == EvidenceConfidence::Observed
                        })
                    && capability_profile_sha256(&probe.profile)?
                        == plan.capability_profile_sha256 =>
            {
                EndProfileCheck::Passed
            }
            Ok(_) => EndProfileCheck::Mismatch,
            Err(_) => EndProfileCheck::Unavailable,
        };
        self.state_store.save(&mut state).await?;

        if cancellation.is_cancelled() {
            return self
                .finish_terminal(plan, state, TerminalKind::Cancelled, "run_cancelled")
                .await;
        }
        state.set_phase(SnapshotPhase::Reconcile, None);
        self.state_store.save(&mut state).await?;
        if cancellation.is_cancelled() {
            return self
                .finish_terminal(plan, state, TerminalKind::Cancelled, "run_cancelled")
                .await;
        }
        self.state_store.heartbeat(&state).await?;
        let cleanup = self.close_open_attempts_before_decision(&mut state).await?;
        self.hydrate_completed_window_records(&mut state).await?;
        let observed_completed_at_unix_ms = Utc::now()
            .timestamp_millis()
            .max(cleanup.completed_at_floor.unwrap_or(i64::MIN));
        let completed_at_unix_ms = Self::clamp_run_completion(
            &mut state,
            plan.started_at_unix_ms,
            observed_completed_at_unix_ms,
        );
        let decision = reconciliation_decision(plan, &mut state, completed_at_unix_ms)?;
        if cancellation.is_cancelled() {
            return self
                .finish_terminal(plan, state, TerminalKind::Cancelled, "run_cancelled")
                .await;
        }
        self.state_store.heartbeat(&state).await?;
        self.commit_decision(plan, state, decision, PendingDecisionKind::Reconciled, None)
            .await
    }

    /// One step of `run`'s phase machine. `Continue` hands the durable state to
    /// the next phase; `Finished` is a terminal outcome `run` returns as-is.
    ///
    /// The outcome is a type rather than a convention because `finish_terminal`
    /// consumes the state: a phase that ends the run cannot also give it back,
    /// so the two cases genuinely differ in what they own.
    async fn check_capability(
        &self,
        plan: &SnapshotPlan,
        mut state: DurableSnapshotState,
        cancellation: &dyn CancellationSignal,
    ) -> Result<PhaseOutcome, SnapshotError> {
        state.set_phase(SnapshotPhase::CapabilityCheck, None);
        self.state_store.save(&mut state).await?;
        let probe = match self
            .await_connector(&state, cancellation, self.connector.probe())
            .await?
        {
            ConnectorAwait::Completed(result) => result,
            ConnectorAwait::Cancelled => {
                return Ok(PhaseOutcome::Finished(Box::new(
                    self.finish_terminal(plan, state, TerminalKind::Cancelled, "run_cancelled")
                        .await?,
                )));
            }
        };
        match probe {
            Ok(probe) => {
                let pack_supported = probe.reachable
                    && probe.profile.packs.get(&plan.pack).is_some_and(|evidence| {
                        snapshot_pack_start_authorized(plan.pack, evidence)
                    })
                    && probe
                        .profile
                        .transports
                        .get(&TransportId::XmlHttp)
                        .is_some_and(|evidence| {
                            evidence.state == CapabilityState::Supported
                                && evidence.confidence == EvidenceConfidence::Observed
                        });
                if !pack_supported {
                    return Ok(PhaseOutcome::Finished(Box::new(
                        self.finish_terminal(
                            plan,
                            state,
                            TerminalKind::Failed,
                            "capability_not_verified",
                        )
                        .await?,
                    )));
                }
                if probe.profile.profile_version != plan.capability_profile_version
                    || capability_profile_sha256(&probe.profile)? != plan.capability_profile_sha256
                    || probe.profile.product != plan.source_product
                    || probe.profile.release != plan.source_release
                    || probe.profile.mode != plan.source_mode
                {
                    return Ok(PhaseOutcome::Finished(Box::new(
                        self.finish_terminal(
                            plan,
                            state,
                            TerminalKind::Failed,
                            "capability_profile_changed",
                        )
                        .await?,
                    )));
                }
            }
            Err(error) => {
                let code = tally_error_code(&error);
                return Ok(PhaseOutcome::Finished(Box::new(
                    self.finish_terminal(plan, state, terminal_kind(&error), code)
                        .await?,
                )));
            }
        }
        Ok(PhaseOutcome::Continue(state))
    }

    /// The identity half of the same gate: a run may only proceed against the
    /// company it was planned for.
    async fn check_company_identity(
        &self,
        plan: &SnapshotPlan,
        mut state: DurableSnapshotState,
        cancellation: &dyn CancellationSignal,
    ) -> Result<PhaseOutcome, SnapshotError> {
        state.set_phase(SnapshotPhase::CompanyIdentityCheck, None);
        self.state_store.save(&mut state).await?;
        let companies = match self
            .await_connector(&state, cancellation, self.connector.discover_companies())
            .await?
        {
            ConnectorAwait::Completed(result) => result,
            ConnectorAwait::Cancelled => {
                return Ok(PhaseOutcome::Finished(Box::new(
                    self.finish_terminal(plan, state, TerminalKind::Cancelled, "run_cancelled")
                        .await?,
                )));
            }
        };
        match companies {
            Ok(companies) => {
                // Setup rejects ambiguous case-insensitive GUID matches. Preserve that invariant
                // at execution time: after setup, Tally may load another company that resolves to
                // the same canonical identity, and selecting either by display name would no
                // longer establish which company supplied the proof-bound records.
                let matching_identities = companies
                    .iter()
                    .filter(|company| company.identity == plan.company.identity)
                    .take(2)
                    .count();
                if matching_identities == 1 {
                    // Exactly one live company remains bound to the reviewed identity.
                } else {
                    let reason = if matching_identities == 0 {
                        "company_identity_not_found"
                    } else {
                        "company_identity_ambiguous"
                    };
                    return Ok(PhaseOutcome::Finished(Box::new(
                        self.finish_terminal(plan, state, TerminalKind::Failed, reason)
                            .await?,
                    )));
                }
            }
            Err(error) => {
                let code = tally_error_code(&error);
                return Ok(PhaseOutcome::Finished(Box::new(
                    self.finish_terminal(plan, state, terminal_kind(&error), code)
                        .await?,
                )));
            }
        }

        Ok(PhaseOutcome::Continue(state))
    }

    /// The window phase: plan the executable leaves, then read, canonicalize and
    /// commit each one until none is left.
    ///
    /// This is the largest phase by a wide margin and the only one that loops.
    /// It is lifted whole rather than split further because its steps share a
    /// dozen locals per iteration; cutting between them would mean threading
    /// those through signatures, which trades one long function for several
    /// coupled ones.
    async fn plan_and_execute_windows(
        &self,
        plan: &SnapshotPlan,
        mut state: DurableSnapshotState,
        cancellation: &dyn CancellationSignal,
    ) -> Result<PhaseOutcome, SnapshotError> {
        state.set_phase(SnapshotPhase::PlanWindows, None);
        self.state_store.save(&mut state).await?;
        let mut attempted_leaf_ids = BTreeSet::new();
        while let Some(planned_owned) = state.executable_leaves().into_iter().find(|planned| {
            !attempted_leaf_ids.contains(&planned.id)
                && !state.windows.get(&planned.id).is_some_and(|window| {
                    window.phase == WindowPhase::Complete
                        && (plan.pack != CapabilityPackId::CoreAccounting
                            || window
                                .evidence
                                .as_ref()
                                .and_then(|evidence| evidence.report_tie_out.as_ref())
                                .is_some())
                })
        }) {
            attempted_leaf_ids.insert(planned_owned.id.clone());
            let planned = &planned_owned;
            let completed_with_required_evidence =
                state.windows.get(&planned.id).is_some_and(|window| {
                    window.phase == WindowPhase::Complete
                        && (plan.pack != CapabilityPackId::CoreAccounting
                            || window
                                .evidence
                                .as_ref()
                                .and_then(|evidence| evidence.report_tie_out.as_ref())
                                .is_some())
                });
            if completed_with_required_evidence {
                continue;
            }
            if let Some(progress) = state.windows.get_mut(&planned.id) {
                if progress.phase == WindowPhase::Complete {
                    if plan.pack != CapabilityPackId::CoreAccounting
                        || progress
                            .evidence
                            .as_ref()
                            .and_then(|evidence| evidence.report_tie_out.as_ref())
                            .is_some()
                    {
                        return Err(SnapshotError::StateInvariant(
                            "completed_window_report_retry",
                        ));
                    }
                    // Reopen the evidence-gathering path. Normalized mirror
                    // membership remains immutable and the next completed
                    // attempt must observe every prior identity again.
                    progress.phase = WindowPhase::Pending;
                    progress.stage_receipt = None;
                    progress.evidence = None;
                    state.set_phase(SnapshotPhase::PlanWindows, Some(planned.id.clone()));
                    self.state_store.save(&mut state).await?;
                }
            }
            if cancellation.is_cancelled() {
                return Ok(PhaseOutcome::Finished(Box::new(
                    self.finish_terminal(plan, state, TerminalKind::Cancelled, "run_cancelled")
                        .await?,
                )));
            }

            set_window_phase(
                &mut state,
                planned,
                WindowPhase::Extracting,
                SnapshotPhase::Extract,
            )?;
            self.state_store.save(&mut state).await?;
            let context = RequestContext {
                run_id: plan.run_id.clone(),
                company: plan.company.clone(),
                pack: plan.pack,
                schema_version: plan.pack_schema_version,
                window: planned.range.clone(),
                query_profile: planned.query_profile.clone(),
                filters_sha256: planned.filters_sha256.clone(),
            };
            let source_result = match self
                .await_connector(
                    &state,
                    cancellation,
                    self.connector.read_pack_window(&context),
                )
                .await?
            {
                ConnectorAwait::Completed(result) => result,
                ConnectorAwait::Cancelled => {
                    return Ok(PhaseOutcome::Finished(Box::new(
                        self.finish_terminal(plan, state, TerminalKind::Cancelled, "run_cancelled")
                            .await?,
                    )));
                }
            };
            let source_window = match source_result {
                Ok(source_window) => source_window,
                Err(TallyError::ReadResponseTooLarge {
                    scope: ReadResponseScope::VoucherWindow,
                }) => match split_leaf(&mut state, plan, &planned.id)? {
                    SplitLeafResult::Created => {
                        // The exact child graph is generation-CAS persisted
                        // before any child request may be dispatched.
                        self.state_store.save(&mut state).await?;
                        if cancellation.is_cancelled() {
                            return Ok(PhaseOutcome::Finished(Box::new(
                                self.finish_terminal(
                                    plan,
                                    state,
                                    TerminalKind::Cancelled,
                                    "run_cancelled",
                                )
                                .await?,
                            )));
                        }
                        continue;
                    }
                    SplitLeafResult::MinimumReached => {
                        return Ok(PhaseOutcome::Finished(Box::new(
                            self.finish_terminal(
                                plan,
                                state,
                                TerminalKind::Failed,
                                "minimum_window_response_too_large",
                            )
                            .await?,
                        )));
                    }
                    SplitLeafResult::LeafLimitReached => {
                        return Ok(PhaseOutcome::Finished(Box::new(
                            self.finish_terminal(
                                plan,
                                state,
                                TerminalKind::Failed,
                                "adaptive_window_limit_reached",
                            )
                            .await?,
                        )));
                    }
                },
                Err(error) => {
                    let code = tally_error_code(&error);
                    return Ok(PhaseOutcome::Finished(Box::new(
                        self.finish_terminal(plan, state, terminal_kind(&error), code)
                            .await?,
                    )));
                }
            };

            set_window_phase(
                &mut state,
                planned,
                WindowPhase::Normalizing,
                SnapshotPhase::Normalize,
            )?;
            self.state_store.save(&mut state).await?;
            let mut canonical = match canonicalize_window(
                &CanonicalWindowContext {
                    requested_pack: plan.pack,
                    schema_version: plan.pack_schema_version,
                    source_identity: &plan.company.identity,
                    query_profile: &planned.query_profile,
                    filters_sha256: &planned.filters_sha256,
                    external_references: &plan.external_references,
                    window_id: &planned.id,
                    requested_window: &planned.range,
                },
                &source_window,
            ) {
                Ok(canonical) => canonical,
                Err(error) => {
                    let code = match error {
                        ReconciliationError::PackMismatch => "response_pack_mismatch",
                        ReconciliationError::Serialization => "response_parse_failed",
                        ReconciliationError::InvalidTypedPack => "typed_pack_validation_failed",
                        ReconciliationError::InvalidSourceCountEvidence => {
                            "source_count_evidence_invalid"
                        }
                        ReconciliationError::SourceCountScopeMismatch => {
                            "source_count_scope_mismatch"
                        }
                        ReconciliationError::RecordEvidenceMismatch => "record_evidence_mismatch",
                        ReconciliationError::RecordProvenanceUnavailable => {
                            "record_provenance_unavailable"
                        }
                        ReconciliationError::InvalidInput(_) => "response_validation_failed",
                    };
                    return Ok(PhaseOutcome::Finished(Box::new(
                        self.finish_terminal(plan, state, TerminalKind::Failed, code)
                            .await?,
                    )));
                }
            };
            let mut attempt_warning_codes = BTreeSet::new();
            if let PackBatch::CoreAccounting(core) = &source_window.batch {
                if core.has_foreign_master_text_diagnostics() {
                    attempt_warning_codes.insert(WarningCode::ForeignMasterTextRenderingDegraded);
                }
                let report_result = match self
                    .await_connector(
                        &state,
                        cancellation,
                        self.connector.read_core_period_balance_report(&context),
                    )
                    .await?
                {
                    ConnectorAwait::Completed(result) => result,
                    ConnectorAwait::Cancelled => {
                        return Ok(PhaseOutcome::Finished(Box::new(
                            self.finish_terminal(
                                plan,
                                state,
                                TerminalKind::Cancelled,
                                "run_cancelled",
                            )
                            .await?,
                        )));
                    }
                };
                match report_result {
                    Ok(report) => {
                        state.gap_codes.remove("report_tie_out_unavailable");
                        state.gap_codes.remove("report_tie_out_evidence_invalid");
                        let report_sha256 = sha256_json(&report)?;
                        match assess_core_period_report(
                            core,
                            &plan.company.identity,
                            &planned.range,
                            &report,
                        ) {
                            Ok(assessment) => {
                                canonical.evidence.report_tie_out_scope =
                                    if assessment.state == TieOutState::Passed {
                                        crate::sync::reconciliation::ComparisonScope::Window
                                    } else {
                                        crate::sync::reconciliation::ComparisonScope::Unavailable
                                    };
                                canonical.evidence.report_tie_out = Some(ReportTieOutEvidence {
                                    source_identity: plan.company.identity.clone(),
                                    pack: plan.pack,
                                    pack_schema_version: plan.pack_schema_version,
                                    query_profile: planned.query_profile.clone(),
                                    filters_sha256: planned.filters_sha256.clone(),
                                    from_yyyymmdd: planned.range.from_yyyymmdd.clone(),
                                    to_yyyymmdd: planned.range.to_yyyymmdd.clone(),
                                    report_sha256,
                                    state: assessment.state,
                                    compared_ledger_count: assessment.compared_ledger_count,
                                    source_reported_count: report.source_reported_count,
                                    core_ledger_count: core.ledgers.len() as u64,
                                });
                                match assessment.state {
                                    TieOutState::Passed => {}
                                    TieOutState::Unavailable => {
                                        state
                                            .gap_codes
                                            .insert("period_report_profile_unobserved".to_string());
                                    }
                                    TieOutState::Mismatch => {
                                        let mut source_ids = assessment
                                            .mismatched_ledger_source_ids
                                            .iter()
                                            .map(|source_id| {
                                                scoped_mismatch_record_alias(
                                                    &plan.company.identity.observed_fingerprint,
                                                    &plan.run_id,
                                                    &planned.id,
                                                    source_id,
                                                )
                                            })
                                            .collect::<Vec<_>>();
                                        source_ids.sort();
                                        source_ids.dedup();
                                        source_ids.truncate(20);
                                        for code in assessment.safe_reason_codes {
                                            canonical.evidence.mismatches.push(
                                                ReconciliationMismatch {
                                                    safe_reason_code: code.to_string(),
                                                    safe_record_ids: source_ids.clone(),
                                                },
                                            );
                                        }
                                    }
                                }
                            }
                            Err(_) => {
                                state
                                    .gap_codes
                                    .insert("report_tie_out_evidence_invalid".to_string());
                            }
                        }
                    }
                    Err(_) => {
                        // Leave evidence absent so a resumed run retries this
                        // corroborating read before commit. The durable gap
                        // keeps a one-shot failure truthful if the run proceeds.
                        state
                            .gap_codes
                            .insert("report_tie_out_unavailable".to_string());
                    }
                }
            }
            set_window_phase(
                &mut state,
                planned,
                WindowPhase::Validating,
                SnapshotPhase::Validate,
            )?;
            self.state_store.save(&mut state).await?;
            let batch_id = state
                .batch_id
                .clone()
                .ok_or(SnapshotError::StateInvariant("batch_id"))?;
            let begin = self
                .mirror
                .begin_snapshot_window_attempt(BeginSnapshotWindowAttemptInput {
                    batch_id: batch_id.clone(),
                    window_id: planned.id.clone(),
                    started_at_unix_ms: Utc::now().timestamp_millis(),
                })
                .await?;
            if let Some(abandonment) = begin.prior_abandonment {
                Self::record_attempt_abandonment(&mut state, abandonment);
            }
            let attempt = begin.attempt;
            set_window_phase(
                &mut state,
                planned,
                WindowPhase::Staging,
                SnapshotPhase::Stage,
            )?;
            state
                .windows
                .get_mut(&planned.id)
                .ok_or(SnapshotError::StateInvariant("window"))?
                .stage_attempt = Some(WindowStageAttempt {
                warning_codes: attempt_warning_codes,
                ..WindowStageAttempt::from(&attempt)
            });
            self.state_store.save(&mut state).await?;

            let observed_at_unix_ms = Utc::now().timestamp_millis();
            let mut memberships = Vec::with_capacity(MAX_WINDOW_STAGE_CHUNK);
            for observation in canonical.observations {
                let record_key = format!("{}\0{}", observation.object_type, observation.source_id);
                let membership = match observation.mirror_input(&batch_id, observed_at_unix_ms) {
                    Ok(input) => SnapshotWindowMembershipInput::Observed {
                        record_key,
                        observation: Box::new(input),
                    },
                    Err(ReconciliationError::RecordProvenanceUnavailable) => {
                        // Preserve canonical truth without inventing raw provenance.
                        state
                            .gap_codes
                            .insert("record_provenance_unavailable".to_string());
                        SnapshotWindowMembershipInput::ProvenanceUnavailable {
                            record_key,
                            canonical_sha256: observation.canonical_sha256,
                            canonical_payload: observation.canonical_payload,
                            exact_decimals: observation.exact_decimals,
                            safe_reason_code: "record_provenance_unavailable".to_string(),
                        }
                    }
                    Err(error) => return Err(error.into()),
                };
                memberships.push(membership);
                if memberships.len() == MAX_WINDOW_STAGE_CHUNK {
                    if cancellation.is_cancelled() {
                        return Ok(PhaseOutcome::Finished(Box::new(
                            self.finish_terminal(
                                plan,
                                state,
                                TerminalKind::Cancelled,
                                "run_cancelled",
                            )
                            .await?,
                        )));
                    }
                    self.state_store.heartbeat(&state).await?;
                    let chunk = std::mem::replace(
                        &mut memberships,
                        Vec::with_capacity(MAX_WINDOW_STAGE_CHUNK),
                    );
                    match self
                        .mirror
                        .stage_snapshot_window_memberships(&attempt, chunk)
                        .await
                    {
                        Ok(_) => {}
                        Err(
                            MirrorError::ObservationConflict
                            | MirrorError::WindowMembershipConflict,
                        ) => {
                            return Ok(PhaseOutcome::Finished(Box::new(
                                self.finish_terminal(
                                    plan,
                                    state,
                                    TerminalKind::Failed,
                                    "window_membership_replay_conflict",
                                )
                                .await?,
                            )));
                        }
                        Err(error) => return Err(error.into()),
                    }
                }
            }
            if !memberships.is_empty() {
                if cancellation.is_cancelled() {
                    return Ok(PhaseOutcome::Finished(Box::new(
                        self.finish_terminal(plan, state, TerminalKind::Cancelled, "run_cancelled")
                            .await?,
                    )));
                }
                self.state_store.heartbeat(&state).await?;
                match self
                    .mirror
                    .stage_snapshot_window_memberships(&attempt, memberships)
                    .await
                {
                    Ok(_) => {}
                    Err(
                        MirrorError::ObservationConflict | MirrorError::WindowMembershipConflict,
                    ) => {
                        return Ok(PhaseOutcome::Finished(Box::new(
                            self.finish_terminal(
                                plan,
                                state,
                                TerminalKind::Failed,
                                "window_membership_replay_conflict",
                            )
                            .await?,
                        )));
                    }
                    Err(error) => return Err(error.into()),
                }
            }
            if cancellation.is_cancelled() {
                return Ok(PhaseOutcome::Finished(Box::new(
                    self.finish_terminal(plan, state, TerminalKind::Cancelled, "run_cancelled")
                        .await?,
                )));
            }
            let completion = match self
                .mirror
                .complete_snapshot_window_attempt(
                    &attempt,
                    Utc::now().timestamp_millis(),
                    serde_json::to_value(&canonical.evidence)
                        .map_err(|_| SnapshotError::Serialization)?,
                )
                .await
            {
                Ok(completion) => completion,
                Err(MirrorError::WindowMembershipDisappeared) => {
                    state
                        .gap_codes
                        .insert("source_changed_during_resume".to_string());
                    return Ok(PhaseOutcome::Finished(Box::new(
                        self.finish_terminal(
                            plan,
                            state,
                            TerminalKind::Failed,
                            "source_changed_during_resume",
                        )
                        .await?,
                    )));
                }
                Err(error) => return Err(error.into()),
            };
            Self::record_local_clock_rollback(&mut state, completion.local_clock_moved_backwards);
            let receipt = completion.receipt;
            canonical.evidence.record_set_sha256 = Some(receipt.membership_sha256.clone());
            let progress = state
                .windows
                .get_mut(&planned.id)
                .ok_or(SnapshotError::StateInvariant("window"))?;
            let attempt_warning_codes = progress
                .stage_attempt
                .take()
                .filter(|staged| {
                    staged.attempt_id == attempt.attempt_id
                        && staged.attempt_ordinal == attempt.attempt_ordinal
                })
                .ok_or(SnapshotError::StateInvariant("window_attempt"))?
                .warning_codes;
            progress.stage_receipt = Some(WindowStageReceipt::from(&receipt));
            progress.evidence = Some(canonical.evidence);
            progress.phase = WindowPhase::Complete;
            state.warning_codes.extend(attempt_warning_codes);
            state.set_phase(SnapshotPhase::Stage, Some(planned.id.clone()));
            self.state_store.save(&mut state).await?;
            if reconciliation_record_budget_exceeded(&state)? {
                return Ok(PhaseOutcome::Finished(Box::new(
                    self.finish_terminal(
                        plan,
                        state,
                        TerminalKind::Failed,
                        RECONCILIATION_RECORD_BUDGET_CODE,
                    )
                    .await?,
                )));
            }
        }

        Ok(PhaseOutcome::Continue(state))
    }

    async fn finish_terminal(
        &self,
        plan: &SnapshotPlan,
        state: DurableSnapshotState,
        kind: TerminalKind,
        safe_reason_code: &str,
    ) -> Result<SnapshotRunResult, SnapshotError> {
        let (state, decision, pending_kind) = self
            .prepare_terminal_decision(plan, state, kind, safe_reason_code)
            .await?;
        self.commit_decision(
            plan,
            state,
            decision,
            pending_kind,
            Some(safe_reason_code.to_string()),
        )
        .await
    }

    async fn prepare_terminal_decision(
        &self,
        plan: &SnapshotPlan,
        mut state: DurableSnapshotState,
        kind: TerminalKind,
        safe_reason_code: &str,
    ) -> Result<
        (
            DurableSnapshotState,
            ReconciliationDecision,
            PendingDecisionKind,
        ),
        SnapshotError,
    > {
        let cleanup = self.close_open_attempts_before_decision(&mut state).await?;
        self.abandon_open_window_attempts(&mut state).await?;
        state.gap_codes.insert(safe_reason_code.to_string());
        let batch_id = state
            .batch_id
            .clone()
            .ok_or(SnapshotError::StateInvariant("batch_id"))?;
        let observed_completed_at_unix_ms = Utc::now()
            .timestamp_millis()
            .max(cleanup.completed_at_floor.unwrap_or(i64::MIN));
        let completed_at = Self::clamp_run_completion(
            &mut state,
            plan.started_at_unix_ms,
            observed_completed_at_unix_ms,
        );
        let record_counts = terminal_record_counts(
            self.mirror
                .batch_observation_counts(&batch_id, &plan.run_id)
                .await?,
        )?;
        let decision = build_terminal_proof(
            batch_id,
            plan.run_id.clone(),
            plan.company.identity.clone(),
            plan.pack,
            plan.pack_schema_version,
            plan.started_at_unix_ms,
            completed_at,
            state.freshness_before,
            plan.freshness_target_seconds,
            kind,
            safe_reason_code.to_string(),
            state.gap_codes.clone(),
            state.warning_codes.clone(),
            record_counts,
        );
        let pending_kind = match kind {
            TerminalKind::Failed => PendingDecisionKind::Failed,
            TerminalKind::Cancelled => PendingDecisionKind::Cancelled,
        };
        Ok((state, decision, pending_kind))
    }

    async fn commit_decision(
        &self,
        plan: &SnapshotPlan,
        state: DurableSnapshotState,
        decision: ReconciliationDecision,
        kind: PendingDecisionKind,
        safe_reason_code: Option<String>,
    ) -> Result<SnapshotRunResult, SnapshotError> {
        let (state, decision) = self
            .stage_commit_decision(plan, state, decision, kind, safe_reason_code)
            .await?;
        match self
            .mirror
            .commit_batch(decision.mirror_commit.clone())
            .await
        {
            Ok(receipt) => {
                verify_commit_receipt(&state, &decision.proof, &receipt)?;
                self.finish_committed_state(state, decision.proof, receipt)
                    .await
            }
            Err(MirrorError::BatchClosed) => {
                self.resolve_closed_batch(plan, state, decision.proof).await
            }
            Err(MirrorError::ConcurrentCheckpoint) => {
                self.finish_checkpoint_conflict(plan, state).await
            }
            Err(error) => Err(error.into()),
        }
    }

    async fn stage_commit_decision(
        &self,
        plan: &SnapshotPlan,
        mut state: DurableSnapshotState,
        mut decision: ReconciliationDecision,
        kind: PendingDecisionKind,
        safe_reason_code: Option<String>,
    ) -> Result<(DurableSnapshotState, ReconciliationDecision), SnapshotError> {
        decision
            .mirror_commit
            .bind_expected_checkpoint(state.checkpoint_before.clone());
        let commit = decision.mirror_commit.parts();
        let batch_id = state
            .batch_id
            .as_deref()
            .ok_or(SnapshotError::StateInvariant("batch_id"))?;
        let counts = self
            .mirror
            .batch_observation_counts(batch_id, &plan.run_id)
            .await?;
        let expected_facts = CommitReceiptFacts {
            proof_contract_version: commit.proof_contract_version,
            run_id: plan.run_id.clone(),
            batch_id: batch_id.to_string(),
            capability_snapshot_id: plan.capability_snapshot_id.clone(),
            company_id: plan.mirror_company_id.clone(),
            pack_id: pack_code(plan.pack).to_string(),
            outcome: commit.outcome,
            verification: commit.verification,
            started_at_unix_ms: plan.started_at_unix_ms,
            completed_at_unix_ms: commit.completed_at_unix_ms,
            accepted_records: counts.accepted_records,
            rejected_records: counts.rejected_records,
            provenance_unavailable_records: counts.provenance_unavailable_records,
            record_counts_sha256: commit.record_counts_sha256.clone(),
            snapshot_sha256: commit.snapshot_sha256.clone(),
            checkpoint_before: state.checkpoint_before.clone(),
            checkpoint_after: commit.checkpoint_after.clone(),
            gap_codes: commit.gap_codes.clone(),
            warning_codes: commit.warning_codes.clone(),
        };
        let expected_receipt_facts_sha256 = sha256_json(&expected_facts)?;
        if state
            .pending_commit
            .as_ref()
            .and_then(|pending| pending.expected_receipt_facts_sha256.as_deref())
            .is_some_and(|stored| stored != expected_receipt_facts_sha256)
        {
            return Err(SnapshotError::StateInvariant("pending_commit_changed"));
        }
        state.gap_codes = commit.gap_codes.iter().cloned().collect();
        state.warning_codes = commit
            .warning_codes
            .iter()
            .map(|code| WarningCode::parse(code))
            .collect::<Option<BTreeSet<_>>>()
            .ok_or(SnapshotError::StateInvariant("warning_code"))?;
        state.set_phase(SnapshotPhase::CommitPending, None);
        state.pending_commit = Some(PendingCommit {
            kind,
            completed_at_unix_ms: commit.completed_at_unix_ms,
            safe_reason_code,
            intended_checkpoint: commit.checkpoint_after.clone(),
            expected_receipt_facts_sha256: Some(expected_receipt_facts_sha256),
            reconciled_proof: (kind == PendingDecisionKind::Reconciled)
                .then(|| Box::new(decision.proof.clone())),
        });
        self.state_store.save(&mut state).await?;
        Ok((state, decision))
    }

    async fn finish_checkpoint_conflict(
        &self,
        plan: &SnapshotPlan,
        mut state: DurableSnapshotState,
    ) -> Result<SnapshotRunResult, SnapshotError> {
        // The reconciled decision stored in CommitPending is no longer authoritative once its
        // checkpoint CAS loses. Replace it with a non-advancing terminal decision so the durable
        // state and staging batch cannot continue advertising a resumable run.
        state.pending_commit = None;
        let (state, decision, pending_kind) = self
            .prepare_terminal_decision(
                plan,
                state,
                TerminalKind::Failed,
                "snapshot_checkpoint_changed",
            )
            .await?;
        let (state, decision) = self
            .stage_commit_decision(
                plan,
                state,
                decision,
                pending_kind,
                Some("snapshot_checkpoint_changed".to_string()),
            )
            .await?;
        match self
            .mirror
            .commit_batch(decision.mirror_commit.clone())
            .await
        {
            Ok(receipt) => {
                verify_commit_receipt(&state, &decision.proof, &receipt)?;
                self.finish_committed_state(state, decision.proof, receipt)
                    .await
            }
            Err(MirrorError::BatchClosed) => {
                self.resolve_closed_batch(plan, state, decision.proof).await
            }
            // A non-advancing terminal proof never participates in checkpoint CAS. Reaching this
            // branch would mean the repository contract regressed.
            Err(MirrorError::ConcurrentCheckpoint) => Err(SnapshotError::StateInvariant(
                "terminal_checkpoint_conflict",
            )),
            Err(error) => Err(error.into()),
        }
    }

    async fn resume_pending_commit(
        &self,
        plan: &SnapshotPlan,
        mut state: DurableSnapshotState,
    ) -> Result<SnapshotRunResult, SnapshotError> {
        let pending = state
            .pending_commit
            .clone()
            .ok_or(SnapshotError::StateInvariant("pending_commit"))?;
        let batch_id = state
            .batch_id
            .clone()
            .ok_or(SnapshotError::StateInvariant("batch_id"))?;
        let committed_receipt = match self
            .mirror
            .historical_commit_receipt_for_batch(&batch_id, &plan.run_id)
            .await
        {
            Ok(receipt) => Some(receipt),
            Err(MirrorError::NotFound) => None,
            Err(error) => return Err(error.into()),
        };
        let budget_exceeded = pending.kind == PendingDecisionKind::Reconciled
            && reconciliation_record_budget_exceeded(&state)?;
        if let Some(receipt) = committed_receipt {
            let decision =
                if let Some(decision) = persisted_reconciled_decision(plan, &state, &pending)? {
                    decision
                } else if budget_exceeded {
                    return Err(SnapshotError::StateInvariant(
                        "legacy_committed_reconciliation_record_budget",
                    ));
                } else {
                    match pending.kind {
                        PendingDecisionKind::Reconciled => {
                            self.hydrate_completed_window_records(&mut state).await?;
                            reconciliation_decision(plan, &mut state, pending.completed_at_unix_ms)?
                        }
                        PendingDecisionKind::Failed | PendingDecisionKind::Cancelled => {
                            let record_counts = terminal_record_counts(
                                self.mirror
                                    .batch_observation_counts(&batch_id, &plan.run_id)
                                    .await?,
                            )?;
                            build_terminal_proof(
                                batch_id.clone(),
                                plan.run_id.clone(),
                                plan.company.identity.clone(),
                                plan.pack,
                                plan.pack_schema_version,
                                plan.started_at_unix_ms,
                                pending.completed_at_unix_ms,
                                state.freshness_before,
                                plan.freshness_target_seconds,
                                if pending.kind == PendingDecisionKind::Cancelled {
                                    TerminalKind::Cancelled
                                } else {
                                    TerminalKind::Failed
                                },
                                pending
                                    .safe_reason_code
                                    .clone()
                                    .ok_or(SnapshotError::StateInvariant("terminal_reason"))?,
                                state.gap_codes.clone(),
                                state.warning_codes.clone(),
                                record_counts,
                            )
                        }
                    }
                };
            verify_commit_receipt(&state, &decision.proof, &receipt)?;
            return self
                .finish_committed_state(state, decision.proof, receipt)
                .await;
        }

        let cleanup = self.close_open_attempts_before_decision(&mut state).await?;
        let rebuilt_completed_at_unix_ms = pending
            .completed_at_unix_ms
            .max(plan.started_at_unix_ms)
            .max(cleanup.completed_at_floor.unwrap_or(i64::MIN));
        let cleanup_changed_decision =
            cleanup.gap_changed || rebuilt_completed_at_unix_ms != pending.completed_at_unix_ms;
        let persisted_decision = if cleanup_changed_decision {
            None
        } else {
            persisted_reconciled_decision(plan, &state, &pending)?
        };
        let decision = if let Some(decision) = persisted_decision {
            Some(decision)
        } else if budget_exceeded {
            None
        } else {
            Some(match pending.kind {
                PendingDecisionKind::Reconciled => {
                    self.hydrate_completed_window_records(&mut state).await?;
                    reconciliation_decision(plan, &mut state, rebuilt_completed_at_unix_ms)?
                }
                PendingDecisionKind::Failed | PendingDecisionKind::Cancelled => {
                    let batch_id = state
                        .batch_id
                        .clone()
                        .ok_or(SnapshotError::StateInvariant("batch_id"))?;
                    let record_counts = terminal_record_counts(
                        self.mirror
                            .batch_observation_counts(&batch_id, &plan.run_id)
                            .await?,
                    )?;
                    build_terminal_proof(
                        batch_id,
                        plan.run_id.clone(),
                        plan.company.identity.clone(),
                        plan.pack,
                        plan.pack_schema_version,
                        plan.started_at_unix_ms,
                        rebuilt_completed_at_unix_ms,
                        state.freshness_before,
                        plan.freshness_target_seconds,
                        if pending.kind == PendingDecisionKind::Cancelled {
                            TerminalKind::Cancelled
                        } else {
                            TerminalKind::Failed
                        },
                        pending
                            .safe_reason_code
                            .clone()
                            .ok_or(SnapshotError::StateInvariant("terminal_reason"))?,
                        state.gap_codes.clone(),
                        state.warning_codes.clone(),
                        record_counts,
                    )
                }
            })
        };

        if cleanup_changed_decision {
            // The staged hash/proof predates newly recovered cleanup evidence. No immutable
            // receipt exists, so discard only the uncommitted decision and stage its rebuilt form.
            state.pending_commit = None;
        }
        if budget_exceeded {
            // No immutable receipt exists, so replacing the uncommitted advancing decision cannot
            // overwrite authority. Current rows carry the compact proof above; legacy rows do not
            // need it to fail closed before hydration.
            state.pending_commit = None;
            return self
                .finish_terminal(
                    plan,
                    state,
                    TerminalKind::Failed,
                    RECONCILIATION_RECORD_BUDGET_CODE,
                )
                .await;
        }
        let decision = decision.ok_or(SnapshotError::CorruptState)?;
        let freshness = self
            .mirror
            .freshness(
                &plan.mirror_company_id,
                pack_code(plan.pack),
                Utc::now().timestamp_millis(),
            )
            .await?;
        if pending.intended_checkpoint.is_some()
            && freshness.checkpoint_token != state.checkpoint_before
        {
            return self.finish_checkpoint_conflict(plan, state).await;
        }
        self.commit_decision(
            plan,
            state,
            decision,
            pending.kind,
            pending.safe_reason_code,
        )
        .await
    }

    /// `CommitPending` recovery is intentionally local-only: source data and capability state no
    /// longer influence a decision that was already staged and hash-bound. The exact immutable
    /// proof-ledger receipt is required before terminalizing a previously committed batch.
    async fn resolve_closed_batch(
        &self,
        plan: &SnapshotPlan,
        state: DurableSnapshotState,
        proof: ProofManifest,
    ) -> Result<SnapshotRunResult, SnapshotError> {
        state
            .pending_commit
            .as_ref()
            .ok_or(SnapshotError::StateInvariant("pending_commit"))?;
        let batch_id = state
            .batch_id
            .as_deref()
            .ok_or(SnapshotError::StateInvariant("batch_id"))?;
        let receipt = self
            .mirror
            .historical_commit_receipt_for_batch(batch_id, &plan.run_id)
            .await?;
        verify_commit_receipt(&state, &proof, &receipt)?;
        self.finish_committed_state(state, proof, receipt).await
    }

    async fn finish_committed_state(
        &self,
        mut state: DurableSnapshotState,
        proof: ProofManifest,
        receipt: CommitResult,
    ) -> Result<SnapshotRunResult, SnapshotError> {
        if receipt.proof_id.is_empty() || !is_lower_sha256(&receipt.proof_sha256) {
            return Err(SnapshotError::StateInvariant("commit_receipt"));
        }
        state.set_phase(SnapshotPhase::EmitProof, None);
        state.proof = Some(proof.clone());
        let receipt = StoredCommitReceipt {
            proof_id: Some(receipt.proof_id),
            proof_sha256: Some(receipt.proof_sha256),
            checkpoint_advanced: receipt.checkpoint_advanced,
        };
        state.commit_receipt = Some(receipt.clone());
        state.pending_commit = None;
        state.set_phase(
            match (proof.outcome, proof.verification) {
                (_, bridge_tally_core::VerificationState::Verified) => SnapshotPhase::Completed,
                (
                    bridge_tally_core::RunOutcome::Completed,
                    bridge_tally_core::VerificationState::Partial,
                ) => SnapshotPhase::Partial,
                (bridge_tally_core::RunOutcome::Cancelled, _) => SnapshotPhase::Cancelled,
                _ => SnapshotPhase::Failed,
            },
            None,
        );
        self.state_store.save(&mut state).await?;
        Ok(SnapshotRunResult {
            state,
            proof,
            receipt,
        })
    }
}

fn same_source_semantics(initial: &WindowEvidence, reread: &WindowEvidence) -> bool {
    initial.window_id == reread.window_id
        && initial.from_yyyymmdd == reread.from_yyyymmdd
        && initial.to_yyyymmdd == reread.to_yyyymmdd
        && initial.canonical_sha256 == reread.canonical_sha256
        && initial.query_profile == reread.query_profile
        && initial.filters_sha256 == reread.filters_sha256
        && initial.record_provenance_scope == reread.record_provenance_scope
        && initial.source_count_scope == reread.source_count_scope
        && initial.source_count == reread.source_count
        && initial.parsed_count == reread.parsed_count
        && initial.accepted_count == reread.accepted_count
        && initial.deduped_count == reread.deduped_count
        && initial.rejected_count == reread.rejected_count
        && initial.duplicate_identity_count == reread.duplicate_identity_count
        && initial.missing_identity_count == reread.missing_identity_count
        && initial.out_of_range_count == reread.out_of_range_count
        && initial.record_counts == reread.record_counts
        && initial.accepted_record_counts == reread.accepted_record_counts
        && initial.object_counts == reread.object_counts
        && initial.accounting_scope == reread.accounting_scope
        && initial.accounting_gap_codes == reread.accounting_gap_codes
}

fn receipt_window_evidence(
    planned: &PlannedWindow,
    receipt: &SnapshotWindowReceipt,
) -> Result<WindowEvidence, SnapshotError> {
    let evidence: WindowEvidence = serde_json::from_value(receipt.evidence.clone())
        .map_err(|_| SnapshotError::CorruptState)?;
    if evidence.window_id != planned.id
        || evidence.from_yyyymmdd != planned.range.from_yyyymmdd
        || evidence.to_yyyymmdd != planned.range.to_yyyymmdd
        || evidence.query_profile != planned.query_profile.as_str()
        || evidence.filters_sha256 != planned.filters_sha256.as_str()
        || evidence.deduped_count != u64::from(receipt.member_count)
        || !is_lower_sha256(&evidence.canonical_sha256)
        || evidence.record_set_sha256.is_some()
        || !evidence.canonical_records.is_empty()
    {
        return Err(SnapshotError::CorruptState);
    }
    Ok(evidence)
}

fn persisted_reconciled_decision(
    plan: &SnapshotPlan,
    state: &DurableSnapshotState,
    pending: &PendingCommit,
) -> Result<Option<ReconciliationDecision>, SnapshotError> {
    if pending.kind != PendingDecisionKind::Reconciled {
        if pending.reconciled_proof.is_some() {
            return Err(SnapshotError::CorruptState);
        }
        return Ok(None);
    }
    let Some(proof) = pending.reconciled_proof.as_deref().cloned() else {
        // Legacy v5 pending rows can be recomputed within the hydration bound. They cannot be
        // treated as exact proof authority above it.
        return Ok(None);
    };
    let completed_at = proof
        .completed_at_unix_ms
        .ok_or(SnapshotError::CorruptState)?;
    let verification = match proof.verification {
        bridge_tally_core::VerificationState::Verified => {
            crate::db::tally_mirror::VerificationState::Verified
        }
        bridge_tally_core::VerificationState::Partial => {
            crate::db::tally_mirror::VerificationState::Partial
        }
        bridge_tally_core::VerificationState::Unverified => {
            return Err(SnapshotError::CorruptState);
        }
    };
    let mut proof_gap_codes = proof
        .gaps
        .iter()
        .map(|gap| gap.safe_reason_code.clone())
        .collect::<Vec<_>>();
    proof_gap_codes.sort();
    proof_gap_codes.dedup();
    let expected_checkpoint = match verification {
        crate::db::tally_mirror::VerificationState::Verified => Some(format!(
            "full:{}",
            proof
                .snapshot_sha256
                .as_deref()
                .ok_or(SnapshotError::CorruptState)?
        )),
        crate::db::tally_mirror::VerificationState::Partial => None,
        crate::db::tally_mirror::VerificationState::Unverified => unreachable!(),
    };
    if proof.run_id != plan.run_id
        || proof.source_identity != plan.company.identity
        || proof.pack != plan.pack
        || proof.pack_schema_version != plan.pack_schema_version
        || proof.outcome != bridge_tally_core::RunOutcome::Completed
        || proof.started_at_unix_ms != plan.started_at_unix_ms
        || completed_at != pending.completed_at_unix_ms
        || pending.safe_reason_code.is_some()
        || pending.intended_checkpoint != expected_checkpoint
        || proof_gap_codes != state.gap_codes.iter().cloned().collect::<Vec<_>>()
        || proof
            .gaps
            .iter()
            .any(|gap| gap.pack != plan.pack || gap.safe_reason_code != gap.field_or_invariant)
    {
        return Err(SnapshotError::CorruptState);
    }
    let batch_id = state
        .batch_id
        .clone()
        .ok_or(SnapshotError::StateInvariant("batch_id"))?;
    let mirror_commit = CommitBatchInput::reconciled(CommitBatchParts {
        batch_id,
        proof_contract_version: proof.proof_contract_version,
        outcome: crate::db::tally_mirror::RunOutcome::Completed,
        verification,
        completed_at_unix_ms: completed_at,
        record_counts_sha256: Some(proof_record_counts_sha256(&proof.record_counts)),
        snapshot_sha256: proof.snapshot_sha256.clone(),
        expected_checkpoint_before: state.checkpoint_before.clone(),
        checkpoint_after: pending.intended_checkpoint.clone(),
        freshness_target_seconds: plan.freshness_target_seconds,
        gap_codes: proof_gap_codes,
        warning_codes: state
            .warning_codes
            .iter()
            .map(|warning| warning.as_str().to_string())
            .collect(),
    });
    Ok(Some(ReconciliationDecision {
        proof,
        mirror_commit,
        safe_mismatches: Vec::new(),
    }))
}

fn reconciliation_record_budget_exceeded(
    state: &DurableSnapshotState,
) -> Result<bool, SnapshotError> {
    aggregate_record_budget_exceeded(
        state
            .windows
            .values()
            .filter(|progress| progress.phase == WindowPhase::Complete)
            .map(|progress| {
                progress
                    .stage_receipt
                    .as_ref()
                    .map(|receipt| receipt.member_count)
                    .ok_or(SnapshotError::CorruptState)
            }),
    )
}

fn aggregate_record_budget_exceeded(
    counts: impl IntoIterator<Item = Result<u32, SnapshotError>>,
) -> Result<bool, SnapshotError> {
    let mut total = 0_u64;
    for count in counts {
        total = total
            .checked_add(u64::from(count?))
            .ok_or(SnapshotError::CorruptState)?;
        if total > MAX_RECONCILIATION_RECORDS {
            return Ok(true);
        }
    }
    Ok(false)
}

fn reconciliation_decision(
    plan: &SnapshotPlan,
    state: &mut DurableSnapshotState,
    completed_at_unix_ms: i64,
) -> Result<ReconciliationDecision, SnapshotError> {
    let completed_windows = state
        .windows
        .iter_mut()
        .filter_map(|(id, progress)| {
            progress.evidence.as_mut().map(|stored| {
                // Move the hydrated map into reconciliation instead of cloning the largest
                // structure. Normalized SQLite membership remains the restart authority.
                let canonical_records = std::mem::take(&mut stored.canonical_records);
                let mut evidence = stored.clone();
                evidence.canonical_records = canonical_records;
                (id.clone(), evidence)
            })
        })
        .collect();
    Ok(build_reconciliation(ReconciliationInput {
        batch_id: state
            .batch_id
            .clone()
            .ok_or(SnapshotError::StateInvariant("batch_id"))?,
        run_id: plan.run_id.clone(),
        source_identity: plan.company.identity.clone(),
        pack: plan.pack,
        pack_schema_version: plan.pack_schema_version,
        started_at_unix_ms: plan.started_at_unix_ms,
        completed_at_unix_ms,
        freshness_before: state.freshness_before,
        freshness_target_seconds: plan.freshness_target_seconds,
        planned_window_ids: state
            .executable_leaves()
            .into_iter()
            .map(|window| window.id)
            .collect(),
        completed_windows,
        end_profile_check: state.end_profile_check,
        source_stability_check: state.source_stability_check,
        explicit_gap_codes: state.gap_codes.clone(),
        warning_codes: state.warning_codes.clone(),
    })?)
}

fn completed_result(state: DurableSnapshotState) -> Result<SnapshotRunResult, SnapshotError> {
    let proof = state
        .proof
        .clone()
        .ok_or(SnapshotError::StateInvariant("proof"))?;
    let receipt = state
        .commit_receipt
        .clone()
        .ok_or(SnapshotError::StateInvariant("commit_receipt"))?;
    Ok(SnapshotRunResult {
        state,
        proof,
        receipt,
    })
}

fn set_window_phase(
    state: &mut DurableSnapshotState,
    planned: &PlannedWindow,
    window_phase: WindowPhase,
    snapshot_phase: SnapshotPhase,
) -> Result<(), SnapshotError> {
    let progress = state
        .windows
        .get_mut(&planned.id)
        .ok_or(SnapshotError::StateInvariant("window"))?;
    if matches!(progress.phase, WindowPhase::Complete | WindowPhase::Split) {
        return Err(SnapshotError::StateInvariant("completed_window_transition"));
    }
    progress.phase = window_phase;
    state.set_phase(snapshot_phase, Some(planned.id.clone()));
    Ok(())
}

fn snapshot_pack_start_authorized(
    pack: CapabilityPackId,
    evidence: &bridge_tally_core::CapabilityEvidence,
) -> bool {
    match pack {
        CapabilityPackId::CoreAccounting => core_snapshot_start_authorized(evidence),
        _ => {
            evidence.state == CapabilityState::Supported
                && evidence.confidence == EvidenceConfidence::Observed
        }
    }
}

fn terminal_kind(error: &TallyError) -> TerminalKind {
    if matches!(error, TallyError::Cancelled) {
        TerminalKind::Cancelled
    } else {
        TerminalKind::Failed
    }
}

fn tally_error_code(error: &TallyError) -> &'static str {
    match error {
        TallyError::Unreachable => "tally_unreachable",
        TallyError::Protocol { code } => match code.as_str() {
            "application_response_rejected" => "application_response_rejected",
            "canary_cache_unavailable" => "canary_cache_unavailable",
            "capability_cache_unavailable" => "capability_cache_unavailable",
            "capability_probe_required" => "capability_probe_required",
            "company_export_invalid" => "company_export_invalid",
            "company_identity_ambiguous" => "company_identity_ambiguous",
            "company_identity_display_scope_ambiguous" => {
                "company_identity_display_scope_ambiguous"
            }
            "company_identity_not_found" => "company_identity_not_found",
            "group_export_invalid" => "group_export_invalid",
            "http_status_failure" => "http_status_failure",
            "ledger_export_invalid" => "ledger_export_invalid",
            "period_report_invalid" => "period_report_invalid",
            "response_content_encoding_unsupported" => "response_content_encoding_unsupported",
            "response_encoding_invalid" => "response_encoding_invalid",
            "response_read_failed" => "response_read_failed",
            "response_size_limit_exceeded" => "response_size_limit_exceeded",
            "response_truncated" => "response_truncated",
            "unclassified_tally_error" => "unclassified_tally_error",
            "voucher_export_invalid" => "voucher_export_invalid",
            "voucher_type_export_invalid" => "voucher_type_export_invalid",
            _ => "tally_protocol_failed",
        },
        TallyError::InvalidData { code } => match code.as_str() {
            "company_identity_mismatch" => "company_identity_mismatch",
            "connector_context_invalid" => "connector_context_invalid",
            "endpoint_invalid" => "endpoint_invalid",
            "period_report_identity_missing" => "period_report_identity_missing",
            "period_report_scope_mismatch" => "period_report_scope_mismatch",
            "request_size_limit_exceeded" => "request_size_limit_exceeded",
            _ => "response_parse_failed",
        },
        TallyError::Unsupported { code } => match code.as_str() {
            "endpoint_circuit_open" => "endpoint_circuit_open",
            "endpoint_queue_deadline_exceeded" => "endpoint_queue_deadline_exceeded",
            "fresh_capability_probe_not_supported" => "fresh_capability_probe_not_supported",
            "http_client_initialization_failed" => "http_client_initialization_failed",
            "query_profile_not_supported" => "query_profile_not_supported",
            "runtime_capacity_reached" => "runtime_capacity_reached",
            "transport_policy_invalid" => "transport_policy_invalid",
            _ => "capability_not_supported",
        },
        TallyError::ReadResponseTooLarge { .. } => "voucher_response_size_limit_exceeded",
        TallyError::Cancelled => "run_cancelled",
        TallyError::OutcomeUnknown => "source_outcome_unknown",
    }
}

fn terminal_record_counts(
    counts: ObservationCounts,
) -> Result<BTreeMap<String, u64>, SnapshotError> {
    Ok(BTreeMap::from([
        (
            "locally_staged.accepted".to_string(),
            u64::try_from(counts.accepted_records)
                .map_err(|_| SnapshotError::StateInvariant("observation_counts"))?,
        ),
        (
            "locally_staged.rejected".to_string(),
            u64::try_from(counts.rejected_records)
                .map_err(|_| SnapshotError::StateInvariant("observation_counts"))?,
        ),
        (
            "locally_staged.provenance_unavailable".to_string(),
            u64::try_from(counts.provenance_unavailable_records)
                .map_err(|_| SnapshotError::StateInvariant("observation_counts"))?,
        ),
    ]))
}

fn core_freshness(state: FreshnessState) -> Freshness {
    match state {
        FreshnessState::Fresh => Freshness::Fresh,
        FreshnessState::Stale => Freshness::Stale,
        FreshnessState::NeverVerified => Freshness::NeverVerified,
    }
}

pub fn pack_code(pack: CapabilityPackId) -> &'static str {
    match pack {
        CapabilityPackId::CoreAccounting => "core_accounting",
        CapabilityPackId::IndiaTax => "india_tax",
        CapabilityPackId::BillsAndPayments => "bills_and_payments",
        CapabilityPackId::Inventory => "inventory",
    }
}

fn valid_yyyymmdd(value: &str) -> bool {
    parse_yyyymmdd(value).is_some()
}

fn is_lower_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn verify_commit_receipt(
    state: &DurableSnapshotState,
    proof: &ProofManifest,
    receipt: &CommitResult,
) -> Result<(), SnapshotError> {
    let pending = state
        .pending_commit
        .as_ref()
        .ok_or(SnapshotError::StateInvariant("pending_commit"))?;
    let expected = pending
        .expected_receipt_facts_sha256
        .as_deref()
        .ok_or(SnapshotError::StateInvariant("commit_receipt"))?;
    let record_counts_sha256 = proof_record_counts_sha256(&proof.record_counts);
    if receipt.checkpoint_advanced != pending.intended_checkpoint.is_some()
        || sha256_json(&receipt.facts)? != expected
        || (receipt.facts.proof_contract_version >= 3
            && receipt.facts.record_counts_sha256.as_deref() != Some(record_counts_sha256.as_str()))
    {
        return Err(SnapshotError::StateInvariant("commit_receipt"));
    }
    Ok(())
}

fn sha256_json(value: &impl Serialize) -> Result<String, SnapshotError> {
    let bytes = serde_json::to_vec(value).map_err(|_| SnapshotError::Serialization)?;
    Ok(sha256_bytes(&bytes))
}

pub fn capability_profile_sha256(profile: &CapabilityProfile) -> Result<String, SnapshotError> {
    sha256_json(profile)
}

fn sha256_bytes(bytes: &[u8]) -> String {
    hex_digest(Sha256::digest(bytes))
}

fn hex_digest(digest: impl AsRef<[u8]>) -> String {
    digest
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
#[path = "snapshot_tests.rs"]
pub(crate) mod tests;
