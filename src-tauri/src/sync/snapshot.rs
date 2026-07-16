use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
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
    BeginBatchInput, BeginSnapshotWindowAttemptInput, CommitReceiptFacts, CommitResult,
    FreshnessState, MirrorError, ObservationCounts, SnapshotWindowAttemptRef,
    SnapshotWindowMembershipInput, SnapshotWindowReceipt, TallyMirrorRepository,
};
use crate::sync::reconciliation::{
    build_reconciliation, build_terminal_proof, canonicalize_window, proof_record_counts_sha256,
    CanonicalWindowContext, ComparisonScope, EndProfileCheck, ExternalReferenceCatalog,
    ReconciliationDecision, ReconciliationError, ReconciliationInput, ReconciliationMismatch,
    ReportTieOutEvidence, SourceStabilityCheck, TerminalKind, WindowEvidence,
};

const SNAPSHOT_STATE_VERSION: u16 = 5;
const LEGACY_SNAPSHOT_STATE_VERSION_V3: u16 = 3;
const LEGACY_SNAPSHOT_STATE_VERSION_V4: u16 = 4;
const MAX_DURABLE_STATE_BYTES: usize = 16 * 1024 * 1024;
const MAX_SNAPSHOT_PLAN_BYTES: usize = 4 * 1024 * 1024;
const MAX_SNAPSHOT_WINDOWS: usize = 1024;
const MAX_STAGED_KEYS_PER_WINDOW: usize = 1_000_000;
const MAX_WINDOW_STAGE_CHUNK: usize = 256;
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
                CapabilityPackId::CoreAccounting => "core_accounting_v2".to_string(),
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
    pub warning_codes: BTreeSet<String>,
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
    state
        .warning_codes
        .insert("adaptive_window_split".to_string());
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
}

impl SqliteSnapshotStateStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self {
            pool,
            lease_owner: None,
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
        })
    }

    pub async fn migrate(&self) -> Result<(), SnapshotError> {
        let installed = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_schema_migrations WHERE version IN (4, 5, 9, 10)",
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
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'trigger' AND name LIKE \
             'trg_tally_snapshot_window_%'",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(SnapshotError::StateStore)?;
        let json_available = sqlx::query_scalar::<_, i64>("SELECT json_valid('{}')")
            .fetch_one(&self.pool)
            .await
            .map_err(SnapshotError::StateStore)?;
        if installed != 4
            || table_exists != 1
            || recovery_columns != 3
            || unique_run_index != 1
            || composite_batch_identity != 1
            || recovery_triggers != 3
            || window_staging_tables != 2
            || window_staging_triggers != 7
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
        let result = sqlx::query(
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
        .map_err(SnapshotError::StateStore)?;
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
        let result = sqlx::query(
            "UPDATE tally_snapshot_run_states SET lease_expires_at_unix_ms = ?1 \
             WHERE resume_key = ?2 AND run_id = ?3 AND generation = ?4 \
               AND lease_owner = ?5 AND lease_expires_at_unix_ms > ?6",
        )
        .bind(now.saturating_add(WORKER_LEASE_TTL_MS))
        .bind(&state.resume_key)
        .bind(&state.run_id)
        .bind(i64::try_from(state.generation).map_err(|_| SnapshotError::StateConflict)?)
        .bind(owner)
        .bind(now)
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

pub struct FullSnapshotEngine<'a, S, C> {
    mirror: &'a TallyMirrorRepository,
    state_store: &'a S,
    connector: &'a C,
}

enum ConnectorAwait<T> {
    Completed(Result<T, TallyError>),
    Cancelled,
}

impl<'a, S, C> FullSnapshotEngine<'a, S, C>
where
    S: SnapshotStateStore,
    C: TallyConnector,
{
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
            if let Some(receipt) = latest.filter(|receipt| {
                receipt.attempt_id == repository_attempt.attempt_id
                    && receipt.attempt_ordinal == repository_attempt.attempt_ordinal
            }) {
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
            } else {
                match self
                    .mirror
                    .abandon_snapshot_window_attempt(
                        &repository_attempt,
                        Utc::now().timestamp_millis(),
                    )
                    .await
                {
                    Ok(()) | Err(MirrorError::NotFound | MirrorError::WindowAttemptClosed) => {}
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
        for progress in state
            .windows
            .values_mut()
            .filter(|progress| progress.phase == WindowPhase::Complete)
        {
            let durable_receipt = progress
                .stage_receipt
                .as_ref()
                .ok_or(SnapshotError::CorruptState)?;
            let stored_receipt = self
                .mirror
                .load_latest_completed_window_receipt(
                    &durable_receipt.attempt.batch_id,
                    &durable_receipt.attempt.window_id,
                )
                .await?
                .ok_or(SnapshotError::CorruptState)?;
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
                Ok(())
                | Err(
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

        if cancellation.is_cancelled() {
            return self
                .finish_terminal(plan, state, TerminalKind::Cancelled, "run_cancelled")
                .await;
        }

        state.set_phase(SnapshotPhase::CapabilityCheck, None);
        self.state_store.save(&mut state).await?;
        let probe = match self
            .await_connector(&state, cancellation, self.connector.probe())
            .await?
        {
            ConnectorAwait::Completed(result) => result,
            ConnectorAwait::Cancelled => {
                return self
                    .finish_terminal(plan, state, TerminalKind::Cancelled, "run_cancelled")
                    .await;
            }
        };
        match probe {
            Ok(probe) => {
                let pack_supported = probe.reachable
                    && probe.profile.packs.get(&plan.pack).is_some_and(|evidence| {
                        evidence.state == CapabilityState::Supported
                            && evidence.confidence == EvidenceConfidence::Observed
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
                    return self
                        .finish_terminal(
                            plan,
                            state,
                            TerminalKind::Failed,
                            "capability_not_verified",
                        )
                        .await;
                }
                if probe.profile.profile_version != plan.capability_profile_version
                    || capability_profile_sha256(&probe.profile)? != plan.capability_profile_sha256
                    || probe.profile.product != plan.source_product
                    || probe.profile.release != plan.source_release
                    || probe.profile.mode != plan.source_mode
                {
                    return self
                        .finish_terminal(
                            plan,
                            state,
                            TerminalKind::Failed,
                            "capability_profile_changed",
                        )
                        .await;
                }
            }
            Err(error) => {
                let code = tally_error_code(&error);
                return self
                    .finish_terminal(plan, state, terminal_kind(&error), code)
                    .await;
            }
        }

        state.set_phase(SnapshotPhase::CompanyIdentityCheck, None);
        self.state_store.save(&mut state).await?;
        let companies = match self
            .await_connector(&state, cancellation, self.connector.discover_companies())
            .await?
        {
            ConnectorAwait::Completed(result) => result,
            ConnectorAwait::Cancelled => {
                return self
                    .finish_terminal(plan, state, TerminalKind::Cancelled, "run_cancelled")
                    .await;
            }
        };
        match companies {
            Ok(companies)
                if companies
                    .iter()
                    .any(|company| company.identity == plan.company.identity) => {}
            Ok(_) => {
                return self
                    .finish_terminal(
                        plan,
                        state,
                        TerminalKind::Failed,
                        "company_identity_not_found",
                    )
                    .await;
            }
            Err(error) => {
                let code = tally_error_code(&error);
                return self
                    .finish_terminal(plan, state, terminal_kind(&error), code)
                    .await;
            }
        }

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
                return self
                    .finish_terminal(plan, state, TerminalKind::Cancelled, "run_cancelled")
                    .await;
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
                    return self
                        .finish_terminal(plan, state, TerminalKind::Cancelled, "run_cancelled")
                        .await;
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
                            return self
                                .finish_terminal(
                                    plan,
                                    state,
                                    TerminalKind::Cancelled,
                                    "run_cancelled",
                                )
                                .await;
                        }
                        continue;
                    }
                    SplitLeafResult::MinimumReached => {
                        return self
                            .finish_terminal(
                                plan,
                                state,
                                TerminalKind::Failed,
                                "minimum_window_response_too_large",
                            )
                            .await;
                    }
                    SplitLeafResult::LeafLimitReached => {
                        return self
                            .finish_terminal(
                                plan,
                                state,
                                TerminalKind::Failed,
                                "adaptive_window_limit_reached",
                            )
                            .await;
                    }
                },
                Err(error) => {
                    let code = tally_error_code(&error);
                    return self
                        .finish_terminal(plan, state, terminal_kind(&error), code)
                        .await;
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
                    return self
                        .finish_terminal(plan, state, TerminalKind::Failed, code)
                        .await;
                }
            };
            if let PackBatch::CoreAccounting(core) = &source_window.batch {
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
                        return self
                            .finish_terminal(plan, state, TerminalKind::Cancelled, "run_cancelled")
                            .await;
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
            let attempt = self
                .mirror
                .begin_snapshot_window_attempt(BeginSnapshotWindowAttemptInput {
                    batch_id: batch_id.clone(),
                    window_id: planned.id.clone(),
                    started_at_unix_ms: Utc::now().timestamp_millis(),
                })
                .await?;
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
                .stage_attempt = Some(WindowStageAttempt::from(&attempt));
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
                        return self
                            .finish_terminal(plan, state, TerminalKind::Cancelled, "run_cancelled")
                            .await;
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
                            return self
                                .finish_terminal(
                                    plan,
                                    state,
                                    TerminalKind::Failed,
                                    "window_membership_replay_conflict",
                                )
                                .await;
                        }
                        Err(error) => return Err(error.into()),
                    }
                }
            }
            if !memberships.is_empty() {
                if cancellation.is_cancelled() {
                    return self
                        .finish_terminal(plan, state, TerminalKind::Cancelled, "run_cancelled")
                        .await;
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
                        return self
                            .finish_terminal(
                                plan,
                                state,
                                TerminalKind::Failed,
                                "window_membership_replay_conflict",
                            )
                            .await;
                    }
                    Err(error) => return Err(error.into()),
                }
            }
            if cancellation.is_cancelled() {
                return self
                    .finish_terminal(plan, state, TerminalKind::Cancelled, "run_cancelled")
                    .await;
            }
            let receipt = match self
                .mirror
                .complete_snapshot_window_attempt(
                    &attempt,
                    Utc::now().timestamp_millis(),
                    serde_json::to_value(&canonical.evidence)
                        .map_err(|_| SnapshotError::Serialization)?,
                )
                .await
            {
                Ok(receipt) => receipt,
                Err(MirrorError::WindowMembershipDisappeared) => {
                    state
                        .gap_codes
                        .insert("source_changed_during_resume".to_string());
                    return self
                        .finish_terminal(
                            plan,
                            state,
                            TerminalKind::Failed,
                            "source_changed_during_resume",
                        )
                        .await;
                }
                Err(error) => return Err(error.into()),
            };
            canonical.evidence.record_set_sha256 = Some(receipt.membership_sha256.clone());
            let progress = state
                .windows
                .get_mut(&planned.id)
                .ok_or(SnapshotError::StateInvariant("window"))?;
            progress.stage_attempt = None;
            progress.stage_receipt = Some(WindowStageReceipt::from(&receipt));
            progress.evidence = Some(canonical.evidence);
            progress.phase = WindowPhase::Complete;
            state.set_phase(SnapshotPhase::Stage, Some(planned.id.clone()));
            self.state_store.save(&mut state).await?;
        }

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
                        evidence.state == CapabilityState::Supported
                            && evidence.confidence == EvidenceConfidence::Observed
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
        self.hydrate_completed_window_records(&mut state).await?;
        let decision = reconciliation_decision(plan, &state, Utc::now().timestamp_millis())?;
        if cancellation.is_cancelled() {
            return self
                .finish_terminal(plan, state, TerminalKind::Cancelled, "run_cancelled")
                .await;
        }
        self.state_store.heartbeat(&state).await?;
        self.commit_decision(plan, state, decision, PendingDecisionKind::Reconciled, None)
            .await
    }

    async fn finish_terminal(
        &self,
        plan: &SnapshotPlan,
        mut state: DurableSnapshotState,
        kind: TerminalKind,
        safe_reason_code: &str,
    ) -> Result<SnapshotRunResult, SnapshotError> {
        self.abandon_open_window_attempts(&mut state).await?;
        state.gap_codes.insert(safe_reason_code.to_string());
        let batch_id = state
            .batch_id
            .clone()
            .ok_or(SnapshotError::StateInvariant("batch_id"))?;
        let completed_at = Utc::now().timestamp_millis();
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
        self.commit_decision(
            plan,
            state,
            decision,
            pending_kind,
            Some(safe_reason_code.to_string()),
        )
        .await
    }

    async fn commit_decision(
        &self,
        plan: &SnapshotPlan,
        mut state: DurableSnapshotState,
        mut decision: ReconciliationDecision,
        kind: PendingDecisionKind,
        safe_reason_code: Option<String>,
    ) -> Result<SnapshotRunResult, SnapshotError> {
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
        state.warning_codes = commit.warning_codes.iter().cloned().collect();
        state.set_phase(SnapshotPhase::CommitPending, None);
        state.pending_commit = Some(PendingCommit {
            kind,
            completed_at_unix_ms: commit.completed_at_unix_ms,
            safe_reason_code,
            intended_checkpoint: commit.checkpoint_after.clone(),
            expected_receipt_facts_sha256: Some(expected_receipt_facts_sha256),
        });
        self.state_store.save(&mut state).await?;
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
            Err(MirrorError::ConcurrentCheckpoint) => Err(SnapshotError::ConcurrentCheckpoint),
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
        let decision = match pending.kind {
            PendingDecisionKind::Reconciled => {
                self.hydrate_completed_window_records(&mut state).await?;
                reconciliation_decision(plan, &state, pending.completed_at_unix_ms)?
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
        };

        let batch_id = state
            .batch_id
            .as_deref()
            .ok_or(SnapshotError::StateInvariant("batch_id"))?;
        match self
            .mirror
            .historical_commit_receipt_for_batch(batch_id, &plan.run_id)
            .await
        {
            Ok(receipt) => {
                verify_commit_receipt(&state, &decision.proof, &receipt)?;
                return self
                    .finish_committed_state(state, decision.proof, receipt)
                    .await;
            }
            Err(MirrorError::NotFound) => {}
            Err(error) => return Err(error.into()),
        }
        let freshness = self
            .mirror
            .freshness(
                &plan.mirror_company_id,
                pack_code(plan.pack),
                Utc::now().timestamp_millis(),
            )
            .await?;
        if freshness.checkpoint_token != state.checkpoint_before {
            return Err(SnapshotError::ConcurrentCheckpoint);
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

fn reconciliation_decision(
    plan: &SnapshotPlan,
    state: &DurableSnapshotState,
    completed_at_unix_ms: i64,
) -> Result<ReconciliationDecision, SnapshotError> {
    let completed_windows = state
        .windows
        .iter()
        .filter_map(|(id, progress)| {
            progress
                .evidence
                .clone()
                .map(|evidence| (id.clone(), evidence))
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
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use bridge_tally_core::{
        CanonicalPackWindow, CapabilityEvidence, CapabilityProfile, CoreAccountingBatch,
        EvidenceConfidence, GroupRecord, ObservedSourceIdentities, PackBatch, ProbeResult,
        RawSourceSha256, SourceIdentity, SourceIdentityKind, SourceRecordEvidence, SourceRecordId,
        TransportId,
    };
    use bridge_tally_protocol::{
        BRIDGE_GROUP_EXPORT_SCHEMA, BRIDGE_LEDGER_EXPORT_SCHEMA, BRIDGE_VOUCHER_TYPE_EXPORT_SCHEMA,
    };
    use bridge_tally_transport::{TransportPolicy, XML_REQUEST_MAX_BYTES};
    use sqlx::sqlite::SqlitePoolOptions;
    use tally_protocol_simulator::{Fixture, ScenarioPlan, SequenceSimulator};

    use crate::db::tally_mirror::{
        CapabilityItemInput, CapabilityKind, CapabilitySnapshotInput, CompanyInput, Confidence,
        RunOutcome, SourceIdentityInput, VerificationState,
    };
    use crate::sync::reconciliation::{CommitBatchInput, CommitBatchParts};
    use crate::tally::{RuntimeTallyConnector, TallyConfig, TallyRuntime};

    use super::*;

    fn fake_profile() -> CapabilityProfile {
        CapabilityProfile {
            profile_version: 1,
            product: "TallyPrime".to_string(),
            release: None,
            mode: Some("Education".to_string()),
            transports: BTreeMap::from([(
                TransportId::XmlHttp,
                CapabilityEvidence {
                    state: CapabilityState::Supported,
                    confidence: EvidenceConfidence::Observed,
                    safe_reason_code: None,
                },
            )]),
            features: BTreeMap::new(),
            packs: BTreeMap::from([(
                CapabilityPackId::CoreAccounting,
                CapabilityEvidence {
                    state: CapabilityState::Supported,
                    confidence: EvidenceConfidence::Observed,
                    safe_reason_code: None,
                },
            )]),
        }
    }

    struct FakeConnector {
        batch: Mutex<VecDeque<Result<CanonicalPackWindow, TallyError>>>,
        company: CompanyRef,
        requests: Mutex<Vec<ReadWindow>>,
    }

    struct RuntimeCancelledStabilityConnector {
        inner: FakeConnector,
        request_count: Mutex<usize>,
    }

    struct RuntimeCancelledReportConnector {
        inner: FakeConnector,
    }

    struct RuntimeCancelledEndProbeConnector {
        inner: FakeConnector,
    }

    struct HeartbeatCountingStore {
        inner: SqliteSnapshotStateStore,
        heartbeats: Mutex<usize>,
    }

    struct RuntimeReadOnlyConnector {
        inner: RuntimeTallyConnector,
        company: CompanyRef,
    }

    struct FailAfterFirstSplitStore {
        inner: SqliteSnapshotStateStore,
        failed: AtomicBool,
    }

    struct FailAfterFirstCommitPendingStore {
        inner: SqliteSnapshotStateStore,
        failed: AtomicBool,
    }

    struct FailBeforeFirstCompletedWindowSaveStore {
        inner: SqliteSnapshotStateStore,
        failed: AtomicBool,
    }

    struct FailBeforeSecondStagingHeartbeatStore {
        inner: SqliteSnapshotStateStore,
        staging_heartbeats: Mutex<usize>,
    }

    struct SaveMetricsStore {
        inner: SqliteSnapshotStateStore,
        saves: Mutex<usize>,
        max_state_json_bytes: Mutex<usize>,
    }

    struct ReportConnector {
        inner: FakeConnector,
    }

    #[async_trait]
    impl SnapshotStateStore for HeartbeatCountingStore {
        async fn load(
            &self,
            resume_key: &str,
        ) -> Result<Option<DurableSnapshotState>, SnapshotError> {
            self.inner.load(resume_key).await
        }

        async fn save(&self, state: &mut DurableSnapshotState) -> Result<(), SnapshotError> {
            self.inner.save(state).await
        }

        async fn heartbeat(&self, state: &DurableSnapshotState) -> Result<(), SnapshotError> {
            *self.heartbeats.lock().unwrap() += 1;
            self.inner.heartbeat(state).await
        }
    }

    #[async_trait]
    impl SnapshotStateStore for FailAfterFirstSplitStore {
        async fn load(
            &self,
            resume_key: &str,
        ) -> Result<Option<DurableSnapshotState>, SnapshotError> {
            self.inner.load(resume_key).await
        }

        async fn save(&self, state: &mut DurableSnapshotState) -> Result<(), SnapshotError> {
            self.inner.save(state).await?;
            if state
                .windows
                .values()
                .any(|window| window.phase == WindowPhase::Split)
                && !self.failed.swap(true, Ordering::AcqRel)
            {
                return Err(SnapshotError::StateInvariant(
                    "injected_crash_after_split_commit",
                ));
            }
            Ok(())
        }

        async fn heartbeat(&self, state: &DurableSnapshotState) -> Result<(), SnapshotError> {
            self.inner.heartbeat(state).await
        }
    }

    #[async_trait]
    impl SnapshotStateStore for FailAfterFirstCommitPendingStore {
        async fn load(
            &self,
            resume_key: &str,
        ) -> Result<Option<DurableSnapshotState>, SnapshotError> {
            self.inner.load(resume_key).await
        }

        async fn save(&self, state: &mut DurableSnapshotState) -> Result<(), SnapshotError> {
            self.inner.save(state).await?;
            if state.progress.phase == SnapshotPhase::CommitPending
                && !self.failed.swap(true, Ordering::AcqRel)
            {
                return Err(SnapshotError::StateInvariant(
                    "injected_crash_after_commit_pending",
                ));
            }
            Ok(())
        }

        async fn heartbeat(&self, state: &DurableSnapshotState) -> Result<(), SnapshotError> {
            self.inner.heartbeat(state).await
        }
    }

    #[async_trait]
    impl SnapshotStateStore for FailBeforeFirstCompletedWindowSaveStore {
        async fn load(
            &self,
            resume_key: &str,
        ) -> Result<Option<DurableSnapshotState>, SnapshotError> {
            self.inner.load(resume_key).await
        }

        async fn save(&self, state: &mut DurableSnapshotState) -> Result<(), SnapshotError> {
            if state.windows.values().any(|window| {
                window.phase == WindowPhase::Complete && window.stage_receipt.is_some()
            }) && !self.failed.swap(true, Ordering::AcqRel)
            {
                return Err(SnapshotError::StateInvariant(
                    "injected_crash_after_window_attempt_completion",
                ));
            }
            self.inner.save(state).await
        }

        async fn heartbeat(&self, state: &DurableSnapshotState) -> Result<(), SnapshotError> {
            self.inner.heartbeat(state).await
        }
    }

    #[async_trait]
    impl SnapshotStateStore for FailBeforeSecondStagingHeartbeatStore {
        async fn load(
            &self,
            resume_key: &str,
        ) -> Result<Option<DurableSnapshotState>, SnapshotError> {
            self.inner.load(resume_key).await
        }

        async fn save(&self, state: &mut DurableSnapshotState) -> Result<(), SnapshotError> {
            self.inner.save(state).await
        }

        async fn heartbeat(&self, state: &DurableSnapshotState) -> Result<(), SnapshotError> {
            if state
                .windows
                .values()
                .any(|window| window.phase == WindowPhase::Staging)
            {
                let mut heartbeats = self.staging_heartbeats.lock().unwrap();
                *heartbeats += 1;
                if *heartbeats == 2 {
                    return Err(SnapshotError::StateInvariant(
                        "injected_crash_after_partial_window_staging",
                    ));
                }
            }
            self.inner.heartbeat(state).await
        }
    }

    #[async_trait]
    impl SnapshotStateStore for SaveMetricsStore {
        async fn load(
            &self,
            resume_key: &str,
        ) -> Result<Option<DurableSnapshotState>, SnapshotError> {
            self.inner.load(resume_key).await
        }

        async fn save(&self, state: &mut DurableSnapshotState) -> Result<(), SnapshotError> {
            let state_json = serde_json::to_vec(state).map_err(|_| SnapshotError::Serialization)?;
            {
                *self.saves.lock().unwrap() += 1;
                let mut max_bytes = self.max_state_json_bytes.lock().unwrap();
                *max_bytes = (*max_bytes).max(state_json.len());
            }
            self.inner.save(state).await
        }

        async fn heartbeat(&self, state: &DurableSnapshotState) -> Result<(), SnapshotError> {
            self.inner.heartbeat(state).await
        }
    }

    #[async_trait]
    impl TallyConnector for FakeConnector {
        async fn probe(&self) -> Result<ProbeResult, TallyError> {
            Ok(ProbeResult {
                reachable: true,
                profile: fake_profile(),
            })
        }

        async fn probe_fresh(&self) -> Result<ProbeResult, TallyError> {
            Ok(ProbeResult {
                reachable: true,
                profile: fake_profile(),
            })
        }

        async fn discover_companies(&self) -> Result<Vec<CompanyRef>, TallyError> {
            Ok(vec![self.company.clone()])
        }

        async fn read_pack_window(
            &self,
            context: &RequestContext,
        ) -> Result<CanonicalPackWindow, TallyError> {
            self.requests.lock().unwrap().push(context.window.clone());
            self.batch.lock().unwrap().pop_front().unwrap_or_else(|| {
                Ok(CanonicalPackWindow::without_source_count_evidence(
                    PackBatch::CoreAccounting(CoreAccountingBatch::default()),
                ))
            })
        }
    }

    #[async_trait]
    impl TallyConnector for ReportConnector {
        async fn probe(&self) -> Result<ProbeResult, TallyError> {
            self.inner.probe().await
        }

        async fn probe_fresh(&self) -> Result<ProbeResult, TallyError> {
            self.inner.probe_fresh().await
        }

        async fn discover_companies(&self) -> Result<Vec<CompanyRef>, TallyError> {
            self.inner.discover_companies().await
        }

        async fn read_pack_window(
            &self,
            context: &RequestContext,
        ) -> Result<CanonicalPackWindow, TallyError> {
            self.inner.read_pack_window(context).await
        }

        async fn read_core_period_balance_report(
            &self,
            context: &RequestContext,
        ) -> Result<bridge_tally_core::report_tie_out::LedgerPeriodBalanceReport, TallyError>
        {
            Ok(
                bridge_tally_core::report_tie_out::LedgerPeriodBalanceReport {
                    source_identity: context.company.identity.clone(),
                    window: context.window.clone(),
                    ordinary_books_scope_observed: true,
                    source_reported_count: 0,
                    balances: Vec::new(),
                },
            )
        }
    }

    fn core_groups(count: usize) -> CanonicalPackWindow {
        CanonicalPackWindow::without_source_count_evidence(PackBatch::CoreAccounting(
            CoreAccountingBatch {
                groups: (0..count)
                    .map(|index| GroupRecord {
                        source_id: format!("group-{index:06}"),
                        name: format!("Synthetic Group {index:06}"),
                        parent_source_id: None,
                    })
                    .collect(),
                ..CoreAccountingBatch::default()
            },
        ))
    }

    fn core_groups_with_provenance(count: usize) -> CanonicalPackWindow {
        let mut window = core_groups(count);
        window.record_evidence = Some(
            (0..count)
                .map(|index| {
                    let source_id = SourceRecordId::parse(format!("group-{index:06}"))
                        .expect("synthetic source id");
                    SourceRecordEvidence {
                        object_type: CanonicalText::parse("group").expect("object type"),
                        source_id: source_id.clone(),
                        identity_kind: SourceIdentityKind::Guid,
                        observed_identities: ObservedSourceIdentities {
                            guid: Some(source_id.clone()),
                            ..ObservedSourceIdentities::default()
                        },
                        raw_source_sha256: RawSourceSha256::parse(sha256_bytes(
                            source_id.as_str().as_bytes(),
                        ))
                        .expect("synthetic raw hash"),
                        alter_id: None,
                    }
                })
                .collect(),
        );
        window
    }

    #[async_trait]
    impl TallyConnector for RuntimeCancelledStabilityConnector {
        async fn probe(&self) -> Result<ProbeResult, TallyError> {
            self.inner.probe().await
        }

        async fn probe_fresh(&self) -> Result<ProbeResult, TallyError> {
            self.inner.probe_fresh().await
        }

        async fn discover_companies(&self) -> Result<Vec<CompanyRef>, TallyError> {
            self.inner.discover_companies().await
        }

        async fn read_pack_window(
            &self,
            context: &RequestContext,
        ) -> Result<CanonicalPackWindow, TallyError> {
            let response = self.inner.read_pack_window(context).await;
            let mut request_count = self.request_count.lock().unwrap();
            *request_count += 1;
            if *request_count == 2 {
                return Err(TallyError::Cancelled);
            }
            response
        }
    }

    #[async_trait]
    impl TallyConnector for RuntimeCancelledReportConnector {
        async fn probe(&self) -> Result<ProbeResult, TallyError> {
            self.inner.probe().await
        }

        async fn probe_fresh(&self) -> Result<ProbeResult, TallyError> {
            self.inner.probe_fresh().await
        }

        async fn discover_companies(&self) -> Result<Vec<CompanyRef>, TallyError> {
            self.inner.discover_companies().await
        }

        async fn read_pack_window(
            &self,
            context: &RequestContext,
        ) -> Result<CanonicalPackWindow, TallyError> {
            self.inner.read_pack_window(context).await
        }

        async fn read_core_period_balance_report(
            &self,
            _context: &RequestContext,
        ) -> Result<bridge_tally_core::report_tie_out::LedgerPeriodBalanceReport, TallyError>
        {
            Err(TallyError::Cancelled)
        }
    }

    #[async_trait]
    impl TallyConnector for RuntimeCancelledEndProbeConnector {
        async fn probe(&self) -> Result<ProbeResult, TallyError> {
            self.inner.probe().await
        }

        async fn probe_fresh(&self) -> Result<ProbeResult, TallyError> {
            Err(TallyError::Cancelled)
        }

        async fn discover_companies(&self) -> Result<Vec<CompanyRef>, TallyError> {
            self.inner.discover_companies().await
        }

        async fn read_pack_window(
            &self,
            context: &RequestContext,
        ) -> Result<CanonicalPackWindow, TallyError> {
            self.inner.read_pack_window(context).await
        }
    }

    #[async_trait]
    impl TallyConnector for RuntimeReadOnlyConnector {
        async fn probe(&self) -> Result<ProbeResult, TallyError> {
            Ok(ProbeResult {
                reachable: true,
                profile: fake_profile(),
            })
        }

        async fn probe_fresh(&self) -> Result<ProbeResult, TallyError> {
            self.probe().await
        }

        async fn discover_companies(&self) -> Result<Vec<CompanyRef>, TallyError> {
            Ok(vec![self.company.clone()])
        }

        async fn read_pack_window(
            &self,
            context: &RequestContext,
        ) -> Result<CanonicalPackWindow, TallyError> {
            self.inner.read_pack_window(context).await
        }
    }

    async fn setup() -> (
        SqlitePool,
        TallyMirrorRepository,
        SqliteSnapshotStateStore,
        SnapshotPlan,
    ) {
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
            .unwrap();
        let mirror = TallyMirrorRepository::new(pool.clone());
        mirror.migrate().await.unwrap();
        let store =
            SqliteSnapshotStateStore::for_worker(pool.clone(), "synthetic-test-worker".to_string())
                .unwrap();
        store.migrate().await.unwrap();
        let capability = mirror
            .save_capability_snapshot(CapabilitySnapshotInput {
                canonical_origin: "http://127.0.0.1:9000".to_string(),
                observed_at_unix_ms: 1_000,
                profile_version: 1,
                product: "TallyPrime".to_string(),
                release: None,
                mode: Some("Education".to_string()),
                mode_confidence: Confidence::Observed,
                items: vec![CapabilityItemInput {
                    kind: CapabilityKind::Pack,
                    key: "core_accounting".to_string(),
                    state: crate::db::tally_mirror::CapabilityState::Supported,
                    confidence: Confidence::Observed,
                    safe_reason_code: None,
                }],
            })
            .await
            .unwrap();
        let company_identity = SourceIdentity {
            bridge_source_lineage: "lineage".to_string(),
            company_guid: "company-guid".to_string(),
            observed_fingerprint: "fingerprint".to_string(),
        };
        let company = mirror
            .upsert_company(CompanyInput {
                endpoint_id: capability.endpoint_id.clone(),
                display_name: "Synthetic Company".to_string(),
                identity: SourceIdentityInput {
                    guid: Some(company_identity.company_guid.clone()),
                    ..SourceIdentityInput::default()
                },
                observed_at_unix_ms: 1_000,
            })
            .await
            .unwrap();
        let range = ReadWindow {
            from_yyyymmdd: "20260701".to_string(),
            to_yyyymmdd: "20260731".to_string(),
        };
        let root_window = PlannedWindow::deterministic(CapabilityPackId::CoreAccounting, range);
        let capability_canary_window = PlannedWindow::deterministic(
            CapabilityPackId::CoreAccounting,
            ReadWindow {
                from_yyyymmdd: "20260701".to_string(),
                to_yyyymmdd: "20260701".to_string(),
            },
        );
        let plan = SnapshotPlan {
            resume_key: "resume-1".to_string(),
            run_id: "run-1".to_string(),
            capability_snapshot_id: capability.id,
            mirror_company_id: company.id,
            company: CompanyRef {
                identity: company_identity,
                display_name: "Synthetic Company".to_string(),
            },
            pack: CapabilityPackId::CoreAccounting,
            pack_schema_version: PackSchemaVersion { major: 1, minor: 0 },
            capability_profile_version: 1,
            capability_profile_sha256: capability_profile_sha256(&fake_profile()).unwrap(),
            source_product: "TallyPrime".to_string(),
            source_transport: "xml_http".to_string(),
            source_release: None,
            source_mode: Some("Education".to_string()),
            external_references: ExternalReferenceCatalog::Unavailable,
            windows: vec![root_window],
            adaptive_window_policy: Some(AdaptiveWindowPolicy::bounded_default()),
            capability_canary_window: Some(capability_canary_window),
            started_at_unix_ms: 2_000,
            freshness_target_seconds: 300,
        };
        (pool, mirror, store, plan)
    }

    #[test]
    fn adaptive_midpoint_split_is_calendar_exact_and_deterministic() {
        let leap = ReadWindow {
            from_yyyymmdd: "20240228".to_string(),
            to_yyyymmdd: "20240302".to_string(),
        };
        let (left, right) = midpoint_split(&leap).expect("split leap window");
        assert_eq!(
            (left.from_yyyymmdd.as_str(), left.to_yyyymmdd.as_str()),
            ("20240228", "20240229")
        );
        assert_eq!(
            (right.from_yyyymmdd.as_str(), right.to_yyyymmdd.as_str()),
            ("20240301", "20240302")
        );

        let parent = PlannedWindow::deterministic(CapabilityPackId::CoreAccounting, leap);
        assert_eq!(
            PlannedWindow::adaptive_child(&parent, left.clone()),
            PlannedWindow::adaptive_child(&parent, left)
        );
        assert!(midpoint_split(&ReadWindow {
            from_yyyymmdd: "20240229".to_string(),
            to_yyyymmdd: "20240229".to_string(),
        })
        .is_none());
    }

    #[tokio::test]
    async fn adaptive_graph_tampering_fails_closed() {
        let (_, _, _, plan) = setup().await;
        let mut state = DurableSnapshotState::new(&plan, Freshness::NeverVerified).unwrap();
        let root_id = plan.windows[0].id.clone();
        state.windows.get_mut(&root_id).unwrap().phase = WindowPhase::Extracting;
        assert!(matches!(
            split_leaf(&mut state, &plan, &root_id).unwrap(),
            SplitLeafResult::Created
        ));
        state.validate_invariants().expect("valid split graph");

        let split = state.windows[&root_id].split.clone().unwrap();
        let mut missing_child = state.clone();
        missing_child.windows.remove(&split.left_window_id);
        assert!(matches!(
            missing_child.validate_invariants(),
            Err(SnapshotError::CorruptState)
        ));

        let mut drifted_child = state;
        drifted_child
            .windows
            .get_mut(&split.right_window_id)
            .unwrap()
            .planned
            .range
            .from_yyyymmdd = "20260716".to_string();
        assert!(matches!(
            drifted_child.validate_invariants(),
            Err(SnapshotError::CorruptState)
        ));
    }

    #[tokio::test]
    async fn oversized_voucher_window_persists_deterministic_children_before_dispatch() {
        let (_, mirror, store, plan) = setup().await;
        let original_plan_sha256 = plan.fingerprint().unwrap();
        let root = plan.windows[0].clone();
        let connector = FakeConnector {
            batch: Mutex::new(VecDeque::from([Err(TallyError::ReadResponseTooLarge {
                scope: ReadResponseScope::VoucherWindow,
            })])),
            company: plan.company.clone(),
            requests: Mutex::new(Vec::new()),
        };
        let result = FullSnapshotEngine::new(&mirror, &store, &connector)
            .run(&plan, &AtomicCancellation::default())
            .await
            .expect("adaptively split oversized window");

        assert_eq!(plan.fingerprint().unwrap(), original_plan_sha256);
        let root_progress = result.state.windows.get(&root.id).expect("root progress");
        assert_eq!(root_progress.phase, WindowPhase::Split);
        let leaves = result.state.executable_leaves();
        assert_eq!(leaves.len(), 2);
        assert_eq!(leaves[0].range.from_yyyymmdd, "20260701");
        assert_eq!(leaves[0].range.to_yyyymmdd, "20260716");
        assert_eq!(leaves[1].range.from_yyyymmdd, "20260717");
        assert_eq!(leaves[1].range.to_yyyymmdd, "20260731");
        assert_eq!(result.state.progress.total_windows, 2);
        assert!(result.state.warning_codes.contains("adaptive_window_split"));
        let requests = connector.requests.lock().unwrap();
        assert_eq!(
            requests
                .iter()
                .filter(|range| **range == root.range)
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn simulator_transport_limit_splits_only_the_voucher_window_before_child_dispatch() {
        const TEST_RESPONSE_LIMIT: usize = 4 * 1024;
        let (_, mirror, store, mut plan) = setup().await;
        plan.resume_key = "resume-transport-adaptive-split".to_string();
        plan.run_id = "run-transport-adaptive-split".to_string();
        plan.pack_schema_version = bridge_tally_core::CORE_ACCOUNTING_SCHEMA_VERSION;
        let company_guid = &plan.company.identity.company_guid;
        let master_xml = |schema: &str, object_type: &str| {
            format!(
                r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><COMPANYCONTEXT SCHEMA="{schema}" OBJECTTYPE="{object_type}" NAME="Synthetic Company" GUID="{company_guid}" RECORDCOUNT="0"/></BODY></ENVELOPE>"#
            )
        };
        let simulator = SequenceSimulator::spawn(vec![
            ScenarioPlan::new(Fixture::SyntheticXml(master_xml(
                BRIDGE_GROUP_EXPORT_SCHEMA,
                "GROUP",
            ))),
            ScenarioPlan::new(Fixture::SyntheticXml(master_xml(
                BRIDGE_LEDGER_EXPORT_SCHEMA,
                "LEDGER",
            ))),
            ScenarioPlan::new(Fixture::SyntheticXml(master_xml(
                BRIDGE_VOUCHER_TYPE_EXPORT_SCHEMA,
                "VOUCHERTYPE",
            ))),
            ScenarioPlan::new(Fixture::Oversized {
                minimum_bytes: TEST_RESPONSE_LIMIT + 1,
            }),
        ])
        .unwrap();
        let config = TallyConfig {
            host: "127.0.0.1".to_string(),
            port: simulator.address().port(),
        };
        let runtime = TallyRuntime::with_transport_policy(TransportPolicy {
            request_timeout: std::time::Duration::from_secs(5),
            status_response_max_bytes: TEST_RESPONSE_LIMIT,
            xml_request_max_bytes: XML_REQUEST_MAX_BYTES,
            xml_response_max_bytes: TEST_RESPONSE_LIMIT,
        });
        let root = plan.windows[0].clone();
        let canary_context = RequestContext {
            run_id: plan.run_id.clone(),
            company: plan.company.clone(),
            pack: plan.pack,
            schema_version: plan.pack_schema_version,
            window: root.range.clone(),
            query_profile: root.query_profile.clone(),
            filters_sha256: root.filters_sha256.clone(),
        };
        let connector = RuntimeReadOnlyConnector {
            inner: RuntimeTallyConnector::new(
                runtime,
                config,
                plan.company.clone(),
                canary_context,
            )
            .unwrap(),
            company: plan.company.clone(),
        };
        let crash_store = FailAfterFirstSplitStore {
            inner: store.clone(),
            failed: AtomicBool::new(false),
        };

        let error = FullSnapshotEngine::new(&mirror, &crash_store, &connector)
            .run(&plan, &AtomicCancellation::default())
            .await
            .expect_err("stop after the split graph is durably persisted");
        assert!(matches!(
            error,
            SnapshotError::StateInvariant("injected_crash_after_split_commit")
        ));
        let persisted = store.load(&plan.resume_key).await.unwrap().unwrap();
        assert_eq!(persisted.windows.len(), 3);
        assert!(persisted.windows[&root.id].split.is_some());
        assert_eq!(
            persisted
                .windows
                .values()
                .filter(|window| window.parent_window_id.as_deref() == Some(root.id.as_str()))
                .count(),
            2
        );
        let requests = simulator.finish().unwrap();
        assert_eq!(requests.len(), 4);
        assert!(requests.iter().all(|request| request.request_processed));
    }

    #[tokio::test]
    async fn adaptive_windowing_recursively_splits_in_deterministic_date_order() {
        let (_, mirror, store, plan) = setup().await;
        let connector = FakeConnector {
            batch: Mutex::new(VecDeque::from([
                Err(TallyError::ReadResponseTooLarge {
                    scope: ReadResponseScope::VoucherWindow,
                }),
                Err(TallyError::ReadResponseTooLarge {
                    scope: ReadResponseScope::VoucherWindow,
                }),
            ])),
            company: plan.company.clone(),
            requests: Mutex::new(Vec::new()),
        };
        let result = FullSnapshotEngine::new(&mirror, &store, &connector)
            .run(&plan, &AtomicCancellation::default())
            .await
            .expect("recursively split oversized left child");
        let ranges = result
            .state
            .executable_leaves()
            .into_iter()
            .map(|window| (window.range.from_yyyymmdd, window.range.to_yyyymmdd))
            .collect::<Vec<_>>();
        assert_eq!(
            ranges,
            vec![
                ("20260701".to_string(), "20260708".to_string()),
                ("20260709".to_string(), "20260716".to_string()),
                ("20260717".to_string(), "20260731".to_string()),
            ]
        );
        assert_eq!(result.state.progress.total_windows, 3);
    }

    #[tokio::test]
    async fn adaptive_leaf_limit_fails_closed_before_mutating_the_graph() {
        let (_, mirror, store, mut plan) = setup().await;
        plan.resume_key = "resume-adaptive-limit".to_string();
        plan.run_id = "run-adaptive-limit".to_string();
        plan.adaptive_window_policy
            .as_mut()
            .unwrap()
            .maximum_leaf_windows = 1;
        let root_id = plan.windows[0].id.clone();
        let connector = FakeConnector {
            batch: Mutex::new(VecDeque::from([Err(TallyError::ReadResponseTooLarge {
                scope: ReadResponseScope::VoucherWindow,
            })])),
            company: plan.company.clone(),
            requests: Mutex::new(Vec::new()),
        };
        let result = FullSnapshotEngine::new(&mirror, &store, &connector)
            .run(&plan, &AtomicCancellation::default())
            .await
            .expect("leaf limit is a terminal truthful result");
        assert_eq!(result.state.windows.len(), 1);
        assert!(result.state.windows[&root_id].split.is_none());
        assert!(result
            .state
            .gap_codes
            .contains("adaptive_window_limit_reached"));
        assert!(result
            .proof
            .gaps
            .iter()
            .any(|gap| gap.safe_reason_code == "adaptive_window_limit_reached"));
    }

    #[tokio::test]
    async fn adaptive_total_node_ceiling_is_checked_before_graph_mutation() {
        let (_, _, _, mut plan) = setup().await;
        let broad_range = ReadWindow {
            from_yyyymmdd: "00010101".to_string(),
            to_yyyymmdd: "99991231".to_string(),
        };
        plan.windows = vec![PlannedWindow::deterministic(
            CapabilityPackId::CoreAccounting,
            broad_range,
        )];
        plan.capability_canary_window = Some(PlannedWindow::deterministic(
            CapabilityPackId::CoreAccounting,
            ReadWindow {
                from_yyyymmdd: "00010101".to_string(),
                to_yyyymmdd: "00010101".to_string(),
            },
        ));
        let mut state = DurableSnapshotState::new(&plan, Freshness::NeverVerified).unwrap();
        while state.windows.len() < MAX_SNAPSHOT_WINDOWS - 1 {
            let candidate = state
                .executable_leaves()
                .into_iter()
                .max_by_key(|window| {
                    let from = parse_yyyymmdd(&window.range.from_yyyymmdd).unwrap();
                    let to = parse_yyyymmdd(&window.range.to_yyyymmdd).unwrap();
                    (to - from).num_days()
                })
                .unwrap();
            state.windows.get_mut(&candidate.id).unwrap().phase = WindowPhase::Extracting;
            assert!(matches!(
                split_leaf(&mut state, &plan, &candidate.id).unwrap(),
                SplitLeafResult::Created
            ));
        }
        assert_eq!(state.windows.len(), 1_023);
        state
            .validate_invariants()
            .expect("near-limit graph is valid");
        let candidate = state
            .executable_leaves()
            .into_iter()
            .max_by_key(|window| {
                let from = parse_yyyymmdd(&window.range.from_yyyymmdd).unwrap();
                let to = parse_yyyymmdd(&window.range.to_yyyymmdd).unwrap();
                (to - from).num_days()
            })
            .unwrap();
        state.windows.get_mut(&candidate.id).unwrap().phase = WindowPhase::Extracting;
        assert!(matches!(
            split_leaf(&mut state, &plan, &candidate.id).unwrap(),
            SplitLeafResult::LeafLimitReached
        ));
        assert_eq!(state.windows.len(), 1_023);
    }

    #[tokio::test]
    async fn one_day_overflow_fails_once_and_preserves_previous_checkpoint() {
        let (_, mirror, store, mut plan) = setup().await;
        let one_day = ReadWindow {
            from_yyyymmdd: "20260701".to_string(),
            to_yyyymmdd: "20260701".to_string(),
        };
        plan.resume_key = "resume-one-day-overflow".to_string();
        plan.run_id = "run-one-day-overflow".to_string();
        plan.windows = vec![PlannedWindow::deterministic(
            CapabilityPackId::CoreAccounting,
            one_day.clone(),
        )];
        plan.capability_canary_window = Some(PlannedWindow::deterministic(
            CapabilityPackId::CoreAccounting,
            one_day,
        ));
        let previous = seed_verified_checkpoint(&mirror, &plan).await;
        let freshness_before = mirror
            .freshness(
                &plan.mirror_company_id,
                pack_code(plan.pack),
                Utc::now().timestamp_millis(),
            )
            .await
            .unwrap();
        let mut state =
            DurableSnapshotState::new(&plan, core_freshness(freshness_before.state)).unwrap();
        state.checkpoint_before = freshness_before.checkpoint_token.clone();
        state.gap_codes.insert("earlier_safe_gap".to_string());
        state
            .warning_codes
            .insert("earlier_safe_warning".to_string());
        store.save(&mut state).await.unwrap();
        let connector = FakeConnector {
            batch: Mutex::new(VecDeque::from([Err(TallyError::ReadResponseTooLarge {
                scope: ReadResponseScope::VoucherWindow,
            })])),
            company: plan.company.clone(),
            requests: Mutex::new(Vec::new()),
        };
        let result = FullSnapshotEngine::new(&mirror, &store, &connector)
            .run(&plan, &AtomicCancellation::default())
            .await
            .expect("one-day overflow is a truthful terminal result");

        assert_eq!(result.state.progress.phase, SnapshotPhase::Failed);
        assert!(!result.receipt.checkpoint_advanced);
        assert!(result
            .state
            .gap_codes
            .contains("minimum_window_response_too_large"));
        let status =
            crate::sync::coordinator::status_from_state_for_test(result.state.clone(), false);
        assert_eq!(status.phase, SnapshotPhase::Failed);
        assert_eq!((status.completed_windows, status.total_windows), (0, 1));
        assert!(status
            .gap_codes
            .contains(&"minimum_window_response_too_large".to_string()));
        assert!(result
            .proof
            .gaps
            .iter()
            .any(|gap| gap.safe_reason_code == "minimum_window_response_too_large"));
        assert!(result
            .proof
            .gaps
            .iter()
            .any(|gap| gap.safe_reason_code == "earlier_safe_gap"));
        let ledger_receipt = mirror
            .historical_commit_receipt_for_batch(
                result.state.batch_id.as_deref().unwrap(),
                &plan.run_id,
            )
            .await
            .unwrap();
        assert_eq!(
            ledger_receipt.facts.gap_codes,
            vec!["earlier_safe_gap", "minimum_window_response_too_large"]
        );
        assert_eq!(
            ledger_receipt.facts.warning_codes,
            vec!["earlier_safe_warning"]
        );
        assert_eq!(connector.requests.lock().unwrap().len(), 1);
        let freshness_after = mirror
            .freshness(
                &plan.mirror_company_id,
                pack_code(plan.pack),
                Utc::now().timestamp_millis(),
            )
            .await
            .unwrap();
        assert_eq!(freshness_after.checkpoint_token, Some(previous));
        assert_eq!(freshness_after.proof_id, freshness_before.proof_id);
        assert_eq!(
            freshness_after.verified_at_unix_ms,
            freshness_before.verified_at_unix_ms
        );
    }

    #[tokio::test]
    async fn terminal_gap_and_warning_sets_survive_commit_pending_crash_resume() {
        let (_, mirror, store, mut plan) = setup().await;
        let one_day = ReadWindow {
            from_yyyymmdd: "20260701".to_string(),
            to_yyyymmdd: "20260701".to_string(),
        };
        plan.resume_key = "resume-terminal-commit-pending".to_string();
        plan.run_id = "run-terminal-commit-pending".to_string();
        plan.windows = vec![PlannedWindow::deterministic(
            CapabilityPackId::CoreAccounting,
            one_day.clone(),
        )];
        plan.capability_canary_window = Some(PlannedWindow::deterministic(
            CapabilityPackId::CoreAccounting,
            one_day,
        ));
        let mut state = DurableSnapshotState::new(&plan, Freshness::NeverVerified).unwrap();
        state.gap_codes.insert("earlier_safe_gap".to_string());
        state
            .warning_codes
            .insert("earlier_safe_warning".to_string());
        store.save(&mut state).await.unwrap();

        let connector = FakeConnector {
            batch: Mutex::new(VecDeque::from([Err(TallyError::ReadResponseTooLarge {
                scope: ReadResponseScope::VoucherWindow,
            })])),
            company: plan.company.clone(),
            requests: Mutex::new(Vec::new()),
        };
        let crash_store = FailAfterFirstCommitPendingStore {
            inner: store.clone(),
            failed: AtomicBool::new(false),
        };
        let error = FullSnapshotEngine::new(&mirror, &crash_store, &connector)
            .run(&plan, &AtomicCancellation::default())
            .await
            .expect_err("inject crash after durable CommitPending save");
        assert!(matches!(
            error,
            SnapshotError::StateInvariant("injected_crash_after_commit_pending")
        ));
        let pending = store.load(&plan.resume_key).await.unwrap().unwrap();
        assert_eq!(pending.progress.phase, SnapshotPhase::CommitPending);
        assert!(pending.gap_codes.contains("earlier_safe_gap"));
        assert!(pending
            .gap_codes
            .contains("minimum_window_response_too_large"));
        assert!(pending.warning_codes.contains("earlier_safe_warning"));

        let resumed = FullSnapshotEngine::new(&mirror, &store, &connector)
            .run(&plan, &AtomicCancellation::default())
            .await
            .expect("resume reconstructs and commits the identical terminal decision");
        assert_eq!(resumed.state.progress.phase, SnapshotPhase::Failed);
        assert_eq!(connector.requests.lock().unwrap().len(), 1);
        assert_eq!(
            resumed
                .proof
                .gaps
                .iter()
                .map(|gap| gap.safe_reason_code.as_str())
                .collect::<Vec<_>>(),
            vec!["earlier_safe_gap", "minimum_window_response_too_large"]
        );
        let receipt = mirror
            .historical_commit_receipt_for_batch(
                resumed.state.batch_id.as_deref().unwrap(),
                &plan.run_id,
            )
            .await
            .unwrap();
        assert_eq!(
            receipt.facts.gap_codes,
            vec!["earlier_safe_gap", "minimum_window_response_too_large"]
        );
        assert_eq!(receipt.facts.warning_codes, vec!["earlier_safe_warning"]);
    }

    #[tokio::test]
    async fn resume_after_split_commit_never_refetches_the_parent() {
        let (_, mirror, store, plan) = setup().await;
        let root = plan.windows[0].clone();
        let connector = FakeConnector {
            batch: Mutex::new(VecDeque::from([Err(TallyError::ReadResponseTooLarge {
                scope: ReadResponseScope::VoucherWindow,
            })])),
            company: plan.company.clone(),
            requests: Mutex::new(Vec::new()),
        };
        let crash_store = FailAfterFirstSplitStore {
            inner: store.clone(),
            failed: AtomicBool::new(false),
        };
        let error = FullSnapshotEngine::new(&mirror, &crash_store, &connector)
            .run(&plan, &AtomicCancellation::default())
            .await
            .expect_err("inject crash after durable split commit");
        assert!(matches!(
            error,
            SnapshotError::StateInvariant("injected_crash_after_split_commit")
        ));
        let persisted = store
            .load(&plan.resume_key)
            .await
            .unwrap()
            .expect("load split graph after crash");
        assert_eq!(
            persisted.windows.get(&root.id).unwrap().phase,
            WindowPhase::Split
        );

        let resumed = FullSnapshotEngine::new(&mirror, &store, &connector)
            .run(&plan, &AtomicCancellation::default())
            .await
            .expect("resume persisted children");
        assert!(resumed.state.progress.phase.is_terminal());
        assert_eq!(
            connector
                .requests
                .lock()
                .unwrap()
                .iter()
                .filter(|range| *range == &root.range)
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn report_mismatch_aliases_never_persist_raw_source_ids() {
        let (_, _, _, plan) = setup().await;
        let window = &plan.windows[0];
        let raw = "00000000-0000-4000-8000-000000000777";
        let alias = scoped_mismatch_record_alias(
            &plan.company.identity.observed_fingerprint,
            &plan.run_id,
            &window.id,
            raw,
        );
        assert!(alias.starts_with("rid:"));
        assert_eq!(alias.len(), 68);
        assert!(!alias.contains(raw));
        assert_eq!(
            alias,
            scoped_mismatch_record_alias(
                &plan.company.identity.observed_fingerprint,
                &plan.run_id,
                &window.id,
                raw,
            )
        );

        let mut another_run = plan.clone();
        another_run.run_id = "run-2".to_string();
        assert_ne!(
            alias,
            scoped_mismatch_record_alias(
                &another_run.company.identity.observed_fingerprint,
                &another_run.run_id,
                &window.id,
                raw,
            )
        );

        let durable = serde_json::to_string(&ReconciliationMismatch {
            safe_reason_code: "period_report_movement_mismatch".to_string(),
            safe_record_ids: vec![alias],
        })
        .unwrap();
        assert!(!durable.contains(raw));
    }

    async fn seed_verified_checkpoint(
        mirror: &TallyMirrorRepository,
        plan: &SnapshotPlan,
    ) -> String {
        let batch_id = mirror
            .begin_batch(BeginBatchInput {
                run_id: "seed-run".to_string(),
                capability_snapshot_id: plan.capability_snapshot_id.clone(),
                company_id: plan.mirror_company_id.clone(),
                pack_id: pack_code(plan.pack).to_string(),
                pack_schema_major: plan.pack_schema_version.major,
                pack_schema_minor: plan.pack_schema_version.minor,
                source_transport: plan.source_transport.clone(),
                source_release: None,
                requested_from_yyyymmdd: Some("20260601".to_string()),
                requested_to_yyyymmdd: Some("20260630".to_string()),
                started_at_unix_ms: 1_000,
            })
            .await
            .unwrap();
        let token = "full:seed".to_string();
        mirror
            .commit_batch(CommitBatchInput::test_only(CommitBatchParts {
                batch_id,
                proof_contract_version: 1,
                outcome: RunOutcome::Completed,
                verification: VerificationState::Verified,
                completed_at_unix_ms: 1_500,
                record_counts_sha256: None,
                snapshot_sha256: Some("a".repeat(64)),
                expected_checkpoint_before: None,
                checkpoint_after: Some(token.clone()),
                freshness_target_seconds: 300,
                gap_codes: Vec::new(),
                warning_codes: Vec::new(),
            }))
            .await
            .unwrap();
        token
    }

    #[tokio::test]
    async fn closed_commit_pending_recovers_historical_receipt_after_checkpoint_advances() {
        let (_, mirror, store, plan) = setup().await;
        let first_batch = mirror
            .begin_batch(BeginBatchInput {
                run_id: plan.run_id.clone(),
                capability_snapshot_id: plan.capability_snapshot_id.clone(),
                company_id: plan.mirror_company_id.clone(),
                pack_id: pack_code(plan.pack).to_string(),
                pack_schema_major: plan.pack_schema_version.major,
                pack_schema_minor: plan.pack_schema_version.minor,
                source_transport: plan.source_transport.clone(),
                source_release: None,
                requested_from_yyyymmdd: Some("20260701".to_string()),
                requested_to_yyyymmdd: Some("20260731".to_string()),
                started_at_unix_ms: plan.started_at_unix_ms,
            })
            .await
            .unwrap();
        let record_counts = BTreeMap::new();
        let first_checkpoint = "full:first-historical".to_string();
        let first_receipt = mirror
            .commit_batch(CommitBatchInput::test_only(CommitBatchParts {
                batch_id: first_batch.clone(),
                proof_contract_version: 3,
                outcome: RunOutcome::Completed,
                verification: VerificationState::Verified,
                completed_at_unix_ms: 3_000,
                record_counts_sha256: Some(proof_record_counts_sha256(&record_counts)),
                snapshot_sha256: Some("a".repeat(64)),
                expected_checkpoint_before: None,
                checkpoint_after: Some(first_checkpoint.clone()),
                freshness_target_seconds: 300,
                gap_codes: Vec::new(),
                warning_codes: Vec::new(),
            }))
            .await
            .unwrap();
        let proof = ProofManifest {
            proof_contract_version: 3,
            run_id: plan.run_id.clone(),
            source_identity: plan.company.identity.clone(),
            pack: plan.pack,
            pack_schema_version: plan.pack_schema_version,
            outcome: bridge_tally_core::RunOutcome::Completed,
            verification: bridge_tally_core::VerificationState::Verified,
            freshness: Freshness::Fresh,
            started_at_unix_ms: plan.started_at_unix_ms,
            completed_at_unix_ms: Some(3_000),
            record_counts,
            snapshot_sha256: Some("a".repeat(64)),
            gaps: Vec::new(),
        };
        let mut pending_state = DurableSnapshotState::new(&plan, Freshness::NeverVerified).unwrap();
        pending_state.batch_id = Some(first_batch.clone());
        pending_state.set_phase(SnapshotPhase::CommitPending, None);
        pending_state.pending_commit = Some(PendingCommit {
            kind: PendingDecisionKind::Reconciled,
            completed_at_unix_ms: 3_000,
            safe_reason_code: None,
            intended_checkpoint: Some(first_checkpoint.clone()),
            expected_receipt_facts_sha256: Some(sha256_json(&first_receipt.facts).unwrap()),
        });
        store.save(&mut pending_state).await.unwrap();

        let next_batch = mirror
            .begin_batch(BeginBatchInput {
                run_id: "later-run".to_string(),
                capability_snapshot_id: plan.capability_snapshot_id.clone(),
                company_id: plan.mirror_company_id.clone(),
                pack_id: pack_code(plan.pack).to_string(),
                pack_schema_major: plan.pack_schema_version.major,
                pack_schema_minor: plan.pack_schema_version.minor,
                source_transport: plan.source_transport.clone(),
                source_release: None,
                requested_from_yyyymmdd: None,
                requested_to_yyyymmdd: None,
                started_at_unix_ms: 3_100,
            })
            .await
            .unwrap();
        mirror
            .commit_batch(CommitBatchInput::test_only(CommitBatchParts {
                batch_id: next_batch,
                proof_contract_version: 1,
                outcome: RunOutcome::Completed,
                verification: VerificationState::Verified,
                completed_at_unix_ms: 3_500,
                record_counts_sha256: None,
                snapshot_sha256: Some("b".repeat(64)),
                expected_checkpoint_before: Some(first_checkpoint),
                checkpoint_after: Some("full:later".to_string()),
                freshness_target_seconds: 300,
                gap_codes: Vec::new(),
                warning_codes: Vec::new(),
            }))
            .await
            .unwrap();

        let connector = FakeConnector {
            batch: Mutex::new(VecDeque::new()),
            company: plan.company.clone(),
            requests: Mutex::new(Vec::new()),
        };
        let recovered = FullSnapshotEngine::new(&mirror, &store, &connector)
            .resolve_closed_batch(&plan, pending_state, proof)
            .await
            .expect("historical immutable receipt must recover independently of checkpoint head");
        assert_eq!(recovered.receipt.proof_id, Some(first_receipt.proof_id));
        assert_eq!(recovered.state.progress.phase, SnapshotPhase::Completed);
        assert!(connector.requests.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn completed_window_attempt_recovers_before_state_save_without_refetching_extraction() {
        let (_, mirror, store, mut plan) = setup().await;
        plan.resume_key = "resume-completed-window-attempt".to_string();
        plan.run_id = "run-completed-window-attempt".to_string();
        let source = core_groups(2);
        let connector = ReportConnector {
            inner: FakeConnector {
                batch: Mutex::new(VecDeque::from([Ok(source.clone()), Ok(source)])),
                company: plan.company.clone(),
                requests: Mutex::new(Vec::new()),
            },
        };
        let crash_store = FailBeforeFirstCompletedWindowSaveStore {
            inner: store.clone(),
            failed: AtomicBool::new(false),
        };

        let error = FullSnapshotEngine::new(&mirror, &crash_store, &connector)
            .run(&plan, &AtomicCancellation::default())
            .await
            .expect_err("crash after the normalized attempt commits but before state save");
        assert!(matches!(
            error,
            SnapshotError::StateInvariant("injected_crash_after_window_attempt_completion")
        ));
        assert_eq!(connector.inner.requests.lock().unwrap().len(), 1);

        let persisted = store.load(&plan.resume_key).await.unwrap().unwrap();
        let progress = persisted.windows.get(&plan.windows[0].id).unwrap();
        assert_eq!(progress.phase, WindowPhase::Staging);
        let attempt = progress
            .stage_attempt
            .as_ref()
            .expect("durable attempt ref");
        let receipt = mirror
            .load_latest_completed_window_receipt(&attempt.batch_id, &attempt.window_id)
            .await
            .unwrap()
            .expect("the normalized SQLite attempt completed before the crash");
        assert_eq!(receipt.attempt_id, attempt.attempt_id);

        let resumed = FullSnapshotEngine::new(&mirror, &store, &connector)
            .run(&plan, &AtomicCancellation::default())
            .await
            .expect("resume consumes the local receipt and only performs the stability reread");
        assert!(resumed.state.progress.phase.is_terminal());
        assert_eq!(connector.inner.requests.lock().unwrap().len(), 2);
        let completed = resumed.state.windows.get(&plan.windows[0].id).unwrap();
        assert_eq!(completed.phase, WindowPhase::Complete);
        assert!(completed.stage_attempt.is_none());
        assert_eq!(
            completed
                .stage_receipt
                .as_ref()
                .map(|stored| stored.attempt.attempt_id.as_str()),
            Some(receipt.attempt_id.as_str())
        );
    }

    #[tokio::test]
    async fn cancellation_after_receipt_recovery_preserves_provenance_gap_and_count() {
        let (_, mirror, store, mut plan) = setup().await;
        plan.resume_key = "resume-recovered-unavailable-provenance".to_string();
        plan.run_id = "run-recovered-unavailable-provenance".to_string();
        let source = core_groups(2);
        let connector = ReportConnector {
            inner: FakeConnector {
                batch: Mutex::new(VecDeque::from([Ok(source)])),
                company: plan.company.clone(),
                requests: Mutex::new(Vec::new()),
            },
        };
        let crash_store = FailBeforeFirstCompletedWindowSaveStore {
            inner: store.clone(),
            failed: AtomicBool::new(false),
        };

        FullSnapshotEngine::new(&mirror, &crash_store, &connector)
            .run(&plan, &AtomicCancellation::default())
            .await
            .expect_err("crash after unavailable-provenance receipt completion");
        let cancellation = AtomicCancellation::default();
        cancellation.cancel();
        let result = FullSnapshotEngine::new(&mirror, &store, &connector)
            .run(&plan, &cancellation)
            .await
            .expect("recovered receipt is terminalized locally");

        assert_eq!(result.state.progress.phase, SnapshotPhase::Cancelled);
        assert!(result
            .state
            .gap_codes
            .contains("record_provenance_unavailable"));
        assert_eq!(
            result.proof.record_counts["locally_staged.provenance_unavailable"],
            2
        );
        let ledger_receipt = mirror
            .historical_commit_receipt_for_batch(
                result.state.batch_id.as_deref().expect("terminal batch id"),
                &plan.run_id,
            )
            .await
            .expect("bound terminal receipt");
        assert_eq!(ledger_receipt.facts.provenance_unavailable_records, 2);
        assert_eq!(connector.inner.requests.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn partial_window_attempt_then_disappearance_fails_without_advancing_checkpoint() {
        let (_, mirror, store, mut plan) = setup().await;
        plan.resume_key = "resume-partial-window-disappearance".to_string();
        plan.run_id = "run-partial-window-disappearance".to_string();
        let previous_checkpoint = seed_verified_checkpoint(&mirror, &plan).await;
        let first = core_groups_with_provenance(MAX_WINDOW_STAGE_CHUNK + 1);
        let mut second = first.clone();
        let PackBatch::CoreAccounting(second_core) = &mut second.batch else {
            panic!("core fixture");
        };
        second_core.groups.remove(0);
        second
            .record_evidence
            .as_mut()
            .expect("synthetic provenance")
            .remove(0);
        let connector = ReportConnector {
            inner: FakeConnector {
                batch: Mutex::new(VecDeque::from([Ok(first), Ok(second)])),
                company: plan.company.clone(),
                requests: Mutex::new(Vec::new()),
            },
        };
        let crash_store = FailBeforeSecondStagingHeartbeatStore {
            inner: store.clone(),
            staging_heartbeats: Mutex::new(0),
        };

        let error = FullSnapshotEngine::new(&mirror, &crash_store, &connector)
            .run(&plan, &AtomicCancellation::default())
            .await
            .expect_err("crash after the first normalized membership chunk");
        assert!(matches!(
            error,
            SnapshotError::StateInvariant("injected_crash_after_partial_window_staging")
        ));
        assert_eq!(connector.inner.requests.lock().unwrap().len(), 1);
        let persisted = store.load(&plan.resume_key).await.unwrap().unwrap();
        assert_eq!(
            persisted.windows[&plan.windows[0].id].phase,
            WindowPhase::Staging
        );
        assert_eq!(
            mirror
                .freshness(
                    &plan.mirror_company_id,
                    pack_code(plan.pack),
                    Utc::now().timestamp_millis(),
                )
                .await
                .unwrap()
                .checkpoint_token,
            Some(previous_checkpoint.clone())
        );

        let result = FullSnapshotEngine::new(&mirror, &store, &connector)
            .run(&plan, &AtomicCancellation::default())
            .await
            .expect("disappearance is committed as a truthful terminal result");
        assert_eq!(result.state.progress.phase, SnapshotPhase::Failed);
        assert!(!result.receipt.checkpoint_advanced);
        assert!(result
            .state
            .gap_codes
            .contains("source_changed_during_resume"));
        assert_eq!(
            result.proof.record_counts["locally_staged.accepted"],
            (MAX_WINDOW_STAGE_CHUNK + 1) as u64
        );
        assert_eq!(result.proof.record_counts["locally_staged.rejected"], 0);
        let batch_id = result.state.batch_id.as_deref().expect("terminal batch id");
        let ledger_receipt = mirror
            .historical_commit_receipt_for_batch(batch_id, &plan.run_id)
            .await
            .expect("terminal ledger receipt");
        assert_eq!(
            ledger_receipt.facts.accepted_records,
            (MAX_WINDOW_STAGE_CHUNK + 1) as i64
        );
        assert_eq!(ledger_receipt.facts.rejected_records, 0);
        assert_eq!(connector.inner.requests.lock().unwrap().len(), 2);
        assert_eq!(
            mirror
                .freshness(
                    &plan.mirror_company_id,
                    pack_code(plan.pack),
                    Utc::now().timestamp_millis(),
                )
                .await
                .unwrap()
                .checkpoint_token,
            Some(previous_checkpoint)
        );
    }

    #[tokio::test]
    async fn normalized_staging_keeps_large_window_state_and_save_generation_compact() {
        let (_, mirror, store, mut plan) = setup().await;
        plan.resume_key = "resume-large-normalized-window".to_string();
        plan.run_id = "run-large-normalized-window".to_string();
        let record_count = (MAX_WINDOW_STAGE_CHUNK * 4) + 1;
        let source = core_groups(record_count);
        let connector = ReportConnector {
            inner: FakeConnector {
                batch: Mutex::new(VecDeque::from([Ok(source.clone()), Ok(source)])),
                company: plan.company.clone(),
                requests: Mutex::new(Vec::new()),
            },
        };
        let metrics_store = SaveMetricsStore {
            inner: store,
            saves: Mutex::new(0),
            max_state_json_bytes: Mutex::new(0),
        };

        let result = FullSnapshotEngine::new(&mirror, &metrics_store, &connector)
            .run(&plan, &AtomicCancellation::default())
            .await
            .expect("large normalized window remains resumable and bounded");
        let saves = *metrics_store.saves.lock().unwrap();
        let max_state_json_bytes = *metrics_store.max_state_json_bytes.lock().unwrap();
        assert!(result.state.progress.phase.is_terminal());
        assert_eq!(result.state.generation as usize, saves);
        assert!(
            saves < 32,
            "durable generations must follow phases, not {record_count} records: {saves}"
        );
        assert!(
            max_state_json_bytes < 128 * 1024,
            "record identities leaked into compact state: {max_state_json_bytes} bytes"
        );
        assert_eq!(connector.inner.requests.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn v4_nonterminal_state_remains_inspectable_but_cannot_resume() {
        let (_, mirror, store, plan) = setup().await;
        let mut legacy = DurableSnapshotState::new(&plan, Freshness::NeverVerified).unwrap();
        legacy.state_version = LEGACY_SNAPSHOT_STATE_VERSION_V4;
        store.save(&mut legacy).await.unwrap();

        let loaded = store.load(&plan.resume_key).await.unwrap().unwrap();
        assert_eq!(loaded.state_version, LEGACY_SNAPSHOT_STATE_VERSION_V4);
        loaded
            .validate_invariants()
            .expect("v4 remains readable for operator inspection");
        assert_eq!(store.load_recent(10).await.unwrap().len(), 1);
        assert!(matches!(
            loaded.recoverable_plan(),
            Err(SnapshotError::ResumePlanUnavailable)
        ));

        let connector = FakeConnector {
            batch: Mutex::new(VecDeque::new()),
            company: plan.company.clone(),
            requests: Mutex::new(Vec::new()),
        };
        assert!(matches!(
            FullSnapshotEngine::new(&mirror, &store, &connector)
                .run(&plan, &AtomicCancellation::default())
                .await,
            Err(SnapshotError::ResumePlanUnavailable)
        ));
        assert!(connector.requests.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn snapshot_without_reported_source_count_is_partial_and_idempotent() {
        let (pool, mirror, store, plan) = setup().await;
        let connector = FakeConnector {
            batch: Mutex::new(VecDeque::from([Ok(
                CanonicalPackWindow::without_source_count_evidence(PackBatch::CoreAccounting(
                    CoreAccountingBatch::default(),
                )),
            )])),
            company: plan.company.clone(),
            requests: Mutex::new(Vec::new()),
        };
        let cancellation = AtomicCancellation::default();
        let engine = FullSnapshotEngine::new(&mirror, &store, &connector);
        let first = engine.run(&plan, &cancellation).await.unwrap();
        assert_eq!(first.state.progress.phase, SnapshotPhase::Partial);
        assert!(!first.receipt.checkpoint_advanced);
        assert!(first
            .proof
            .gaps
            .iter()
            .any(|gap| gap.safe_reason_code == "source_count_unavailable"));
        assert!(first.state.gap_codes.contains("source_count_unavailable"));
        assert_eq!(
            first.state.gap_codes,
            first
                .proof
                .gaps
                .iter()
                .map(|gap| gap.safe_reason_code.clone())
                .collect()
        );
        let proof_id = first
            .receipt
            .proof_id
            .as_deref()
            .expect("partial snapshot stores a durable proof");
        let export = mirror
            .redacted_proof_export(&plan.mirror_company_id, proof_id, 10_000)
            .await
            .expect("hash-valid durable partial proof is exportable");
        assert!(export.json.contains("\"verification_state\": \"partial\""));
        assert!(export.json.contains("\"authenticity_claim\": \"none\""));
        assert!(!export.json.contains(&plan.company.display_name));
        assert!(!export.json.contains(&plan.company.identity.company_guid));
        assert!(!export.json.contains(&plan.run_id));
        assert!(!export.json.contains(proof_id));
        assert!(mirror
            .redacted_proof_export("wrong-company", proof_id, 10_000)
            .await
            .is_err());
        let second = engine.run(&plan, &cancellation).await.unwrap();
        assert_eq!(first.proof.snapshot_sha256, second.proof.snapshot_sha256);
        let freshness = mirror
            .freshness(
                &plan.mirror_company_id,
                pack_code(plan.pack),
                Utc::now().timestamp_millis(),
            )
            .await
            .unwrap();
        assert_eq!(freshness.checkpoint_token, None);
        let terminal_mutation = sqlx::query(
            "UPDATE tally_snapshot_run_states SET generation = generation + 1 \
             WHERE resume_key = ?1",
        )
        .bind(&plan.resume_key)
        .execute(&pool)
        .await;
        assert!(terminal_mutation.is_err());
        let terminal_delete =
            sqlx::query("DELETE FROM tally_snapshot_run_states WHERE resume_key = ?1")
                .bind(&plan.resume_key)
                .execute(&pool)
                .await;
        assert!(terminal_delete.is_err());
    }

    #[tokio::test]
    async fn parse_failure_and_cancellation_leave_previous_verified_checkpoint_active() {
        let (_pool, mirror, store, mut plan) = setup().await;
        let previous = seed_verified_checkpoint(&mirror, &plan).await;
        let cancellation = AtomicCancellation::default();

        plan.resume_key = "resume-failed".to_string();
        plan.run_id = "run-failed".to_string();
        let failing = FakeConnector {
            batch: Mutex::new(VecDeque::from([Err(TallyError::InvalidData {
                code: "synthetic_parse_failure".to_string(),
            })])),
            company: plan.company.clone(),
            requests: Mutex::new(Vec::new()),
        };
        let failed = FullSnapshotEngine::new(&mirror, &store, &failing)
            .run(&plan, &cancellation)
            .await
            .unwrap();
        assert_eq!(failed.state.progress.phase, SnapshotPhase::Failed);
        assert!(!failed.receipt.checkpoint_advanced);
        assert_eq!(
            mirror
                .freshness(
                    &plan.mirror_company_id,
                    pack_code(plan.pack),
                    Utc::now().timestamp_millis(),
                )
                .await
                .unwrap()
                .checkpoint_token,
            Some(previous.clone())
        );

        plan.resume_key = "resume-cancelled".to_string();
        plan.run_id = "run-cancelled".to_string();
        let cancelled = AtomicCancellation::default();
        cancelled.cancel();
        let connector = FakeConnector {
            batch: Mutex::new(VecDeque::new()),
            company: plan.company.clone(),
            requests: Mutex::new(Vec::new()),
        };
        let result = FullSnapshotEngine::new(&mirror, &store, &connector)
            .run(&plan, &cancelled)
            .await
            .unwrap();
        assert_eq!(result.state.progress.phase, SnapshotPhase::Cancelled);
        assert!(!result.receipt.checkpoint_advanced);
        assert_eq!(
            mirror
                .freshness(
                    &plan.mirror_company_id,
                    pack_code(plan.pack),
                    Utc::now().timestamp_millis(),
                )
                .await
                .unwrap()
                .checkpoint_token,
            Some(previous)
        );
    }

    #[tokio::test]
    async fn runtime_cancellation_during_source_stability_is_terminal() {
        let (_, mirror, store, plan) = setup().await;
        let cancellation = AtomicCancellation::default();
        let connector = RuntimeCancelledStabilityConnector {
            inner: FakeConnector {
                batch: Mutex::new(VecDeque::new()),
                company: plan.company.clone(),
                requests: Mutex::new(Vec::new()),
            },
            request_count: Mutex::new(0),
        };

        let result = FullSnapshotEngine::new(&mirror, &store, &connector)
            .run(&plan, &cancellation)
            .await
            .expect("cancellation is committed as a truthful terminal result");
        assert_eq!(result.state.progress.phase, SnapshotPhase::Cancelled);
        assert!(!cancellation.is_cancelled());
        assert_eq!(connector.inner.requests.lock().unwrap().len(), 2);
        assert!(!result.receipt.checkpoint_advanced);
        assert!(result
            .proof
            .gaps
            .iter()
            .any(|gap| gap.safe_reason_code == "run_cancelled"));
        assert_eq!(
            mirror
                .freshness(
                    &plan.mirror_company_id,
                    pack_code(plan.pack),
                    Utc::now().timestamp_millis(),
                )
                .await
                .unwrap()
                .checkpoint_token,
            None
        );
    }

    #[tokio::test]
    async fn runtime_cancellation_during_period_report_survives_commit_pending_resume() {
        let (_, mirror, store, plan) = setup().await;
        let cancellation = AtomicCancellation::default();
        let connector = RuntimeCancelledReportConnector {
            inner: FakeConnector {
                batch: Mutex::new(VecDeque::new()),
                company: plan.company.clone(),
                requests: Mutex::new(Vec::new()),
            },
        };
        let crash_store = FailAfterFirstCommitPendingStore {
            inner: store.clone(),
            failed: AtomicBool::new(false),
        };
        let error = FullSnapshotEngine::new(&mirror, &crash_store, &connector)
            .run(&plan, &cancellation)
            .await
            .expect_err("inject crash after the cancelled decision is durable");
        assert!(matches!(
            error,
            SnapshotError::StateInvariant("injected_crash_after_commit_pending")
        ));
        assert!(!cancellation.is_cancelled());
        let pending = store.load(&plan.resume_key).await.unwrap().unwrap();
        assert_eq!(pending.progress.phase, SnapshotPhase::CommitPending);
        assert_eq!(
            pending
                .pending_commit
                .as_ref()
                .and_then(|pending| pending.safe_reason_code.as_deref()),
            Some("run_cancelled")
        );

        let result = FullSnapshotEngine::new(&mirror, &store, &connector)
            .run(&plan, &cancellation)
            .await
            .expect("cancelled proof resumes without another source request");
        assert_eq!(result.state.progress.phase, SnapshotPhase::Cancelled);
        assert!(!result
            .state
            .gap_codes
            .contains("report_tie_out_unavailable"));
        assert!(result.state.gap_codes.contains("run_cancelled"));
        assert!(!result.receipt.checkpoint_advanced);
        assert_eq!(
            result.proof.outcome,
            bridge_tally_core::RunOutcome::Cancelled
        );
        let receipt = mirror
            .historical_commit_receipt_for_batch(
                result.state.batch_id.as_deref().unwrap(),
                &plan.run_id,
            )
            .await
            .unwrap();
        assert_eq!(receipt.facts.outcome, RunOutcome::Cancelled);
        assert_eq!(receipt.facts.gap_codes, vec!["run_cancelled"]);
    }

    #[tokio::test]
    async fn runtime_cancellation_during_end_probe_is_terminal() {
        let (_, mirror, store, plan) = setup().await;
        let cancellation = AtomicCancellation::default();
        let connector = RuntimeCancelledEndProbeConnector {
            inner: FakeConnector {
                batch: Mutex::new(VecDeque::new()),
                company: plan.company.clone(),
                requests: Mutex::new(Vec::new()),
            },
        };

        let result = FullSnapshotEngine::new(&mirror, &store, &connector)
            .run(&plan, &cancellation)
            .await
            .expect("runtime cancellation is committed as Cancelled");
        assert!(!cancellation.is_cancelled());
        assert_eq!(result.state.progress.phase, SnapshotPhase::Cancelled);
        assert_eq!(
            result.proof.outcome,
            bridge_tally_core::RunOutcome::Cancelled
        );
        assert!(!result.receipt.checkpoint_advanced);
        assert!(result.state.gap_codes.contains("run_cancelled"));
        assert!(!result
            .state
            .gap_codes
            .contains("end_profile_check_unavailable"));
    }

    #[tokio::test]
    async fn reviewed_tally_error_codes_are_precise_allowlisted_and_persisted() {
        let cases = [
            (
                TallyError::Protocol {
                    code: "response_truncated".to_string(),
                },
                "response_truncated",
            ),
            (
                TallyError::InvalidData {
                    code: "company_identity_mismatch".to_string(),
                },
                "company_identity_mismatch",
            ),
            (
                TallyError::Unsupported {
                    code: "endpoint_queue_deadline_exceeded".to_string(),
                },
                "endpoint_queue_deadline_exceeded",
            ),
            (
                TallyError::Protocol {
                    code: "source_supplied_sensitive_text".to_string(),
                },
                "tally_protocol_failed",
            ),
            (
                TallyError::InvalidData {
                    code: "source_supplied_sensitive_text".to_string(),
                },
                "response_parse_failed",
            ),
            (
                TallyError::Unsupported {
                    code: "source_supplied_sensitive_text".to_string(),
                },
                "capability_not_supported",
            ),
        ];

        for (index, (error, expected)) in cases.into_iter().enumerate() {
            let (_, mirror, store, mut plan) = setup().await;
            plan.resume_key = format!("resume-safe-code-{index}");
            plan.run_id = format!("run-safe-code-{index}");
            let connector = FakeConnector {
                batch: Mutex::new(VecDeque::from([Err(error)])),
                company: plan.company.clone(),
                requests: Mutex::new(Vec::new()),
            };

            let result = FullSnapshotEngine::new(&mirror, &store, &connector)
                .run(&plan, &AtomicCancellation::default())
                .await
                .expect("safe connector failure becomes a durable terminal result");
            assert_eq!(result.state.progress.phase, SnapshotPhase::Failed);
            assert_eq!(
                result.state.gap_codes,
                BTreeSet::from([expected.to_string()])
            );
            assert_eq!(result.proof.gaps.len(), 1);
            assert_eq!(result.proof.gaps[0].safe_reason_code, expected);
            let receipt = mirror
                .historical_commit_receipt_for_batch(
                    result.state.batch_id.as_deref().unwrap(),
                    &plan.run_id,
                )
                .await
                .unwrap();
            assert_eq!(receipt.facts.gap_codes, vec![expected]);
            assert!(!receipt
                .facts
                .gap_codes
                .iter()
                .any(|code| code.contains("sensitive")));
        }
    }

    #[tokio::test]
    async fn multi_leaf_source_stability_heartbeats_the_owned_lease() {
        let (_, mirror, store, mut plan) = setup().await;
        plan.resume_key = "resume-multi-leaf-heartbeat".to_string();
        plan.run_id = "run-multi-leaf-heartbeat".to_string();
        plan.windows = ["20260701", "20260702"]
            .into_iter()
            .map(|date| {
                PlannedWindow::deterministic(
                    CapabilityPackId::CoreAccounting,
                    ReadWindow {
                        from_yyyymmdd: date.to_string(),
                        to_yyyymmdd: date.to_string(),
                    },
                )
            })
            .collect();
        plan.capability_canary_window = Some(plan.windows[0].clone());
        let heartbeat_store = HeartbeatCountingStore {
            inner: store,
            heartbeats: Mutex::new(0),
        };
        let connector = FakeConnector {
            batch: Mutex::new(VecDeque::new()),
            company: plan.company.clone(),
            requests: Mutex::new(Vec::new()),
        };

        FullSnapshotEngine::new(&mirror, &heartbeat_store, &connector)
            .run(&plan, &AtomicCancellation::default())
            .await
            .expect("multi-leaf run retains its lease through stability and commit");
        assert_eq!(connector.requests.lock().unwrap().len(), 4);
        assert!(*heartbeat_store.heartbeats.lock().unwrap() >= 8);
    }

    #[tokio::test]
    async fn connector_await_renews_the_lease_while_a_call_is_in_flight() {
        let (_, mirror, store, plan) = setup().await;
        let mut state = DurableSnapshotState::new(&plan, Freshness::NeverVerified).unwrap();
        store.save(&mut state).await.unwrap();
        let heartbeat_store = HeartbeatCountingStore {
            inner: store,
            heartbeats: Mutex::new(0),
        };
        let connector = FakeConnector {
            batch: Mutex::new(VecDeque::new()),
            company: plan.company.clone(),
            requests: Mutex::new(Vec::new()),
        };
        let engine = FullSnapshotEngine::new(&mirror, &heartbeat_store, &connector);

        let result = engine
            .await_connector(&state, &AtomicCancellation::default(), async {
                tokio::time::sleep(Duration::from_millis(90)).await;
                Ok::<_, TallyError>(())
            })
            .await
            .unwrap();
        assert!(matches!(result, ConnectorAwait::Completed(Ok(()))));
        assert!(*heartbeat_store.heartbeats.lock().unwrap() >= 3);
    }

    #[tokio::test]
    async fn expired_lease_cannot_be_revived_by_heartbeat_or_state_save() {
        let (pool, _, store, plan) = setup().await;
        let mut state = DurableSnapshotState::new(&plan, Freshness::NeverVerified).unwrap();
        store.save(&mut state).await.unwrap();
        sqlx::query(
            "UPDATE tally_snapshot_run_states SET lease_expires_at_unix_ms = ?1 \
             WHERE resume_key = ?2",
        )
        .bind(Utc::now().timestamp_millis().saturating_sub(1))
        .bind(&plan.resume_key)
        .execute(&pool)
        .await
        .unwrap();

        assert!(matches!(
            store.heartbeat(&state).await,
            Err(SnapshotError::LeaseUnavailable)
        ));
        state.warning_codes.insert("must_not_persist".to_string());
        assert!(matches!(
            store.save(&mut state).await,
            Err(SnapshotError::StateConflict)
        ));
        let contender =
            SqliteSnapshotStateStore::for_worker(pool, "synthetic-contending-worker".to_string())
                .unwrap();
        assert!(contender.claim(&plan.resume_key).await.unwrap());
    }

    #[tokio::test]
    async fn durable_store_rejects_corruption_and_plan_drift() {
        let (pool, _mirror, store, mut plan) = setup().await;
        let mut state = DurableSnapshotState::new(&plan, Freshness::NeverVerified).unwrap();
        store.save(&mut state).await.unwrap();
        let original = plan.clone();
        plan.source_transport = "json_ex".to_string();
        let loaded = store.load(&plan.resume_key).await.unwrap().unwrap();
        store
            .heartbeat(&loaded)
            .await
            .expect("the owning worker can renew the exact loaded generation");
        assert_eq!(loaded.recoverable_plan().unwrap(), original);
        assert_eq!(
            store
                .load_by_run_id(&loaded.run_id)
                .await
                .unwrap()
                .unwrap()
                .resume_key,
            loaded.resume_key
        );
        assert_eq!(store.load_recent(10).await.unwrap().len(), 1);
        let mut legacy = loaded.clone();
        legacy.plan = None;
        assert!(matches!(
            legacy.recoverable_plan(),
            Err(SnapshotError::ResumePlanUnavailable)
        ));
        let contender = SqliteSnapshotStateStore::for_worker(
            pool.clone(),
            "synthetic-contending-worker".to_string(),
        )
        .unwrap();
        assert!(matches!(
            contender.claim(&loaded.resume_key).await,
            Err(SnapshotError::LeaseUnavailable)
        ));
        let mut advanced = loaded.clone();
        let mut stale = loaded.clone();
        advanced.warning_codes.insert("cas_advanced".to_string());
        store.save(&mut advanced).await.unwrap();
        stale.warning_codes.insert("cas_stale".to_string());
        assert!(matches!(
            store.save(&mut stale).await,
            Err(SnapshotError::StateConflict)
        ));
        let loaded = store.load(&plan.resume_key).await.unwrap().unwrap();
        assert!(loaded.warning_codes.contains("cas_advanced"));
        assert!(!loaded.warning_codes.contains("cas_stale"));
        assert!(matches!(
            loaded.assert_resumable_with(&plan),
            Err(SnapshotError::ResumePlanMismatch)
        ));
        let mut changed_references = original.clone();
        changed_references.external_references = ExternalReferenceCatalog::Complete {
            company_ids: BTreeSet::new(),
            voucher_ids: BTreeSet::new(),
            ledger_ids: BTreeSet::new(),
        };
        assert!(matches!(
            loaded.assert_resumable_with(&changed_references),
            Err(SnapshotError::ResumePlanMismatch)
        ));
        let mut changed_start = original.clone();
        changed_start.started_at_unix_ms += 1;
        assert!(matches!(
            loaded.assert_resumable_with(&changed_start),
            Err(SnapshotError::ResumePlanMismatch)
        ));
        let mut changed_freshness = original.clone();
        changed_freshness.freshness_target_seconds += 1;
        assert!(matches!(
            loaded.assert_resumable_with(&changed_freshness),
            Err(SnapshotError::ResumePlanMismatch)
        ));
        let mut malformed_filter = original;
        malformed_filter.windows[0].filters_sha256 = CanonicalText::parse("not-a-digest").unwrap();
        assert!(matches!(
            malformed_filter.validate(),
            Err(SnapshotError::InvalidPlan("windows"))
        ));
        let mut duplicate_plan = loaded.recoverable_plan().unwrap();
        duplicate_plan.resume_key = "duplicate-resume-key".to_string();
        let mut duplicate_state =
            DurableSnapshotState::new(&duplicate_plan, Freshness::NeverVerified).unwrap();
        assert!(store.save(&mut duplicate_state).await.is_err());
        assert_eq!(
            store
                .load_by_run_id(&loaded.run_id)
                .await
                .unwrap()
                .unwrap()
                .resume_key,
            loaded.resume_key
        );
        let identity_mutation = sqlx::query(
            "UPDATE tally_snapshot_run_states SET run_id = 'different-run' \
             WHERE resume_key = ?1",
        )
        .bind(&plan.resume_key)
        .execute(&pool)
        .await;
        assert!(identity_mutation.is_err());
        sqlx::query("UPDATE tally_snapshot_run_states SET row_sha256 = ?1 WHERE resume_key = ?2")
            .bind("0".repeat(64))
            .bind(&plan.resume_key)
            .execute(&pool)
            .await
            .unwrap();
        assert!(matches!(
            store.load(&plan.resume_key).await,
            Err(SnapshotError::CorruptState)
        ));
        let state_sha256: String = sqlx::query_scalar(
            "SELECT state_sha256 FROM tally_snapshot_run_states WHERE resume_key = ?1",
        )
        .bind(&plan.resume_key)
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query("UPDATE tally_snapshot_run_states SET row_sha256 = ?1 WHERE resume_key = ?2")
            .bind(snapshot_state_row_sha256(
                &loaded.resume_key,
                &loaded.run_id,
                loaded.generation,
                &state_sha256,
            ))
            .bind(&plan.resume_key)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "UPDATE tally_snapshot_run_states SET state_json = '{\"corrupt\":true}' \
             WHERE resume_key = ?1",
        )
        .bind(&plan.resume_key)
        .execute(&pool)
        .await
        .unwrap();
        assert!(matches!(
            store.load(&plan.resume_key).await,
            Err(SnapshotError::CorruptState)
        ));
    }
}
