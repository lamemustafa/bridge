//! Portable, fail-closed incremental-sync policy for Bridge Tally integrations.
//!
//! This crate performs deterministic policy calculations; it does not authenticate
//! database or protocol evidence. The production runtime must obtain capability and
//! checkpoint facts through its sealed repository verifier before calling `plan_sync`.

use bridge_tally_core::{
    CapabilityPackId, CapabilityState, EvidenceConfidence, PackSchemaVersion, TransportId,
    VerificationState,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Every dimension that can change the meaning or ordering of a Tally change
/// identifier. Checkpoints are reusable only under exact equality.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct IncrementalScope {
    /// Stable Bridge lineage for the Tally origin; never a raw endpoint.
    pub source_lineage: String,
    pub company_guid: String,
    pub company_fingerprint: String,
    pub object_type: String,
    pub capability_profile_version: u16,
    pub product: String,
    pub release: String,
    pub mode: String,
    pub transport: TransportId,
    pub pack: CapabilityPackId,
    pub pack_schema_version: PackSchemaVersion,
    /// Stable name of the exact query and canonical mapping.
    pub query_profile: String,
    /// Canonical lowercase SHA-256 of every filter that changes feed membership.
    pub filters_sha256: String,
    /// Versioned date/overlap-window policy; never inferred from a cursor alone.
    pub date_window_policy: String,
}

impl IncrementalScope {
    pub fn is_exact(&self) -> bool {
        self.capability_profile_version > 0
            && [
                self.source_lineage.as_str(),
                self.company_guid.as_str(),
                self.company_fingerprint.as_str(),
                self.object_type.as_str(),
                self.product.as_str(),
                self.release.as_str(),
                self.mode.as_str(),
                self.query_profile.as_str(),
                self.date_window_policy.as_str(),
            ]
            .into_iter()
            .all(|value| {
                !value.trim().is_empty()
                    && value.len() <= 512
                    && !value.chars().any(char::is_control)
            })
            && self.filters_sha256.len() == 64
            && self
                .filters_sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeIdentifierSemantics {
    /// Verified monotonic identifier scoped to one exact Tally object type.
    MonotonicPerObject,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct IncrementalCapabilityObservation {
    pub scope: IncrementalScope,
    pub state: CapabilityState,
    pub confidence: EvidenceConfidence,
    pub identifier_semantics: ChangeIdentifierSemantics,
    /// True only after the exact query profile was observed to support an
    /// inclusive lower bound, which makes overlap reads safe.
    pub inclusive_lower_bound_observed: bool,
    /// True only when the exact response contract exposes a source high
    /// watermark independently of the maximum identifier in returned rows.
    pub explicit_source_high_watermark_observed: bool,
}

impl IncrementalCapabilityObservation {
    fn proves_incremental_for(&self, scope: &IncrementalScope) -> bool {
        self.scope == *scope
            && scope.is_exact()
            && self.state == CapabilityState::Supported
            && self.confidence == EvidenceConfidence::Observed
            && self.identifier_semantics == ChangeIdentifierSemantics::MonotonicPerObject
            && self.inclusive_lower_bound_observed
            && self.explicit_source_high_watermark_observed
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct IncrementalCheckpoint {
    pub scope: IncrementalScope,
    pub high_watermark: u64,
    pub established_by_verified_full_snapshot: bool,
    pub established_by_proof_sha256: String,
    pub last_transition_proof_sha256: String,
    pub last_identity_sweep_unix_ms: i64,
    pub invalidated_reason: Option<CheckpointInvalidationReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub struct IncrementalPolicy {
    /// Number of identifiers re-read before the checkpoint. Inclusive queries
    /// therefore start at `checkpoint - overlap`, saturating at zero.
    pub overlap_identifiers: u64,
    pub identity_sweep_interval_ms: i64,
}

impl Default for IncrementalPolicy {
    fn default() -> Self {
        Self {
            overlap_identifiers: 128,
            identity_sweep_interval_ms: 24 * 60 * 60 * 1_000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FullSnapshotReason {
    ScopeIncomplete,
    CapabilityNotObserved,
    CapabilityUnsupported,
    CapabilityNotObservedAtRuntime,
    CapabilityScopeDrift,
    NoVerifiedCheckpoint,
    CheckpointScopeDrift,
    CheckpointInvalidated,
    InvalidPolicy,
    ReceiptScopeMismatch,
    SourceHighWatermarkMissing,
    InvalidProofReceipt,
}

impl FullSnapshotReason {
    pub const fn safe_warning_code(self) -> &'static str {
        match self {
            Self::ScopeIncomplete => "incremental_scope_incomplete_full_snapshot_required",
            Self::CapabilityNotObserved => "incremental_capability_unknown_full_snapshot_required",
            Self::CapabilityUnsupported => {
                "incremental_capability_unsupported_full_snapshot_required"
            }
            Self::CapabilityNotObservedAtRuntime => {
                "incremental_capability_not_observed_full_snapshot_required"
            }
            Self::CapabilityScopeDrift => {
                "incremental_capability_scope_drift_full_snapshot_required"
            }
            Self::NoVerifiedCheckpoint => "verified_full_snapshot_checkpoint_required",
            Self::CheckpointScopeDrift => {
                "incremental_checkpoint_scope_drift_full_snapshot_required"
            }
            Self::CheckpointInvalidated => "incremental_checkpoint_invalid_full_snapshot_required",
            Self::InvalidPolicy => "incremental_policy_invalid_full_snapshot_required",
            Self::ReceiptScopeMismatch => {
                "incremental_receipt_scope_mismatch_full_snapshot_required"
            }
            Self::SourceHighWatermarkMissing => {
                "incremental_source_high_watermark_missing_full_snapshot_required"
            }
            Self::InvalidProofReceipt => "incremental_proof_receipt_invalid_full_snapshot_required",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedFullSnapshotReceipt {
    scope: IncrementalScope,
    verification: VerificationState,
    proof_sha256: String,
    observed_source_high_watermark: Option<u64>,
    completed_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedIncrementalReceipt {
    scope: IncrementalScope,
    checkpoint_before: u64,
    verification: VerificationState,
    proof_sha256: String,
    observed_source_high_watermark: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncPlan {
    FullSnapshot {
        reason: FullSnapshotReason,
        warning_code: &'static str,
    },
    Incremental {
        from_change_id_inclusive: u64,
        checkpoint_before: u64,
        identity_sweep_required: bool,
    },
}

pub fn plan_sync(
    policy: IncrementalPolicy,
    scope: &IncrementalScope,
    capability: Option<&IncrementalCapabilityObservation>,
    checkpoint: Option<&IncrementalCheckpoint>,
    now_unix_ms: i64,
) -> SyncPlan {
    let full = |reason: FullSnapshotReason| SyncPlan::FullSnapshot {
        reason,
        warning_code: reason.safe_warning_code(),
    };

    if policy.identity_sweep_interval_ms <= 0 {
        return full(FullSnapshotReason::InvalidPolicy);
    }
    if !scope.is_exact() {
        return full(FullSnapshotReason::ScopeIncomplete);
    }

    let Some(capability) = capability else {
        return full(FullSnapshotReason::CapabilityNotObserved);
    };
    if capability.scope != *scope {
        return full(FullSnapshotReason::CapabilityScopeDrift);
    }
    if capability.state == CapabilityState::Unsupported {
        return full(FullSnapshotReason::CapabilityUnsupported);
    }
    if !capability.proves_incremental_for(scope) {
        return full(FullSnapshotReason::CapabilityNotObservedAtRuntime);
    }

    let Some(checkpoint) = checkpoint else {
        return full(FullSnapshotReason::NoVerifiedCheckpoint);
    };
    if checkpoint.scope != *scope {
        return full(FullSnapshotReason::CheckpointScopeDrift);
    }
    if checkpoint.invalidated_reason.is_some() {
        return full(FullSnapshotReason::CheckpointInvalidated);
    }
    if !checkpoint.established_by_verified_full_snapshot {
        return full(FullSnapshotReason::NoVerifiedCheckpoint);
    }
    if !is_lower_sha256(&checkpoint.established_by_proof_sha256)
        || !is_lower_sha256(&checkpoint.last_transition_proof_sha256)
    {
        return full(FullSnapshotReason::InvalidProofReceipt);
    }

    SyncPlan::Incremental {
        from_change_id_inclusive: checkpoint
            .high_watermark
            .saturating_sub(policy.overlap_identifiers),
        checkpoint_before: checkpoint.high_watermark,
        identity_sweep_required: identity_sweep_due(
            checkpoint.last_identity_sweep_unix_ms,
            now_unix_ms,
            policy.identity_sweep_interval_ms,
        ),
    }
}

pub fn establish_checkpoint_from_full_snapshot(
    scope: IncrementalScope,
    capability: &IncrementalCapabilityObservation,
    receipt: VerifiedFullSnapshotReceipt,
) -> Result<IncrementalCheckpoint, FullSnapshotReason> {
    if receipt.scope != scope {
        return Err(FullSnapshotReason::ReceiptScopeMismatch);
    }
    if receipt.verification != VerificationState::Verified {
        return Err(FullSnapshotReason::NoVerifiedCheckpoint);
    }
    if !is_lower_sha256(&receipt.proof_sha256) || receipt.completed_at_unix_ms <= 0 {
        return Err(FullSnapshotReason::InvalidProofReceipt);
    }
    let observed_high_watermark = receipt
        .observed_source_high_watermark
        .ok_or(FullSnapshotReason::SourceHighWatermarkMissing)?;
    if !capability.proves_incremental_for(&scope) {
        return Err(FullSnapshotReason::CapabilityNotObservedAtRuntime);
    }
    Ok(IncrementalCheckpoint {
        scope,
        high_watermark: observed_high_watermark,
        established_by_verified_full_snapshot: true,
        established_by_proof_sha256: receipt.proof_sha256.clone(),
        last_transition_proof_sha256: receipt.proof_sha256,
        last_identity_sweep_unix_ms: receipt.completed_at_unix_ms,
        invalidated_reason: None,
    })
}

fn identity_sweep_due(last_sweep_unix_ms: i64, now_unix_ms: i64, interval_ms: i64) -> bool {
    now_unix_ms < last_sweep_unix_ms
        || now_unix_ms.saturating_sub(last_sweep_unix_ms) >= interval_ms
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointInvalidationReason {
    IdentifierRegressedOrReset,
    SourceHighWatermarkMissing,
    IncrementalResponseUnverified,
    ReceiptScopeMismatch,
    InvalidProofReceipt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointTransition {
    Advanced {
        from: u64,
        to: u64,
    },
    Unchanged {
        at: u64,
    },
    Invalidated {
        at: u64,
        reason: CheckpointInvalidationReason,
    },
}

/// Advance only from a fully verified incremental response and an explicit
/// source high-watermark. Record IDs inside the overlap are intentionally not
/// used to detect regression.
pub fn apply_incremental_high_watermark(
    checkpoint: &mut IncrementalCheckpoint,
    receipt: &VerifiedIncrementalReceipt,
) -> CheckpointTransition {
    let invalidate = |checkpoint: &mut IncrementalCheckpoint,
                      reason: CheckpointInvalidationReason| {
        checkpoint.invalidated_reason = Some(reason);
        CheckpointTransition::Invalidated {
            at: checkpoint.high_watermark,
            reason,
        }
    };

    if let Some(reason) = checkpoint.invalidated_reason {
        return CheckpointTransition::Invalidated {
            at: checkpoint.high_watermark,
            reason,
        };
    }

    if receipt.scope != checkpoint.scope || receipt.checkpoint_before != checkpoint.high_watermark {
        return invalidate(
            checkpoint,
            CheckpointInvalidationReason::ReceiptScopeMismatch,
        );
    }
    if !is_lower_sha256(&receipt.proof_sha256) {
        return invalidate(
            checkpoint,
            CheckpointInvalidationReason::InvalidProofReceipt,
        );
    }
    if receipt.verification != VerificationState::Verified {
        return invalidate(
            checkpoint,
            CheckpointInvalidationReason::IncrementalResponseUnverified,
        );
    }
    let Some(observed) = receipt.observed_source_high_watermark else {
        return invalidate(
            checkpoint,
            CheckpointInvalidationReason::SourceHighWatermarkMissing,
        );
    };
    if observed < checkpoint.high_watermark {
        return invalidate(
            checkpoint,
            CheckpointInvalidationReason::IdentifierRegressedOrReset,
        );
    }
    if observed == checkpoint.high_watermark {
        checkpoint.last_transition_proof_sha256 = receipt.proof_sha256.clone();
        return CheckpointTransition::Unchanged { at: observed };
    }

    let from = checkpoint.high_watermark;
    checkpoint.high_watermark = observed;
    checkpoint.last_transition_proof_sha256 = receipt.proof_sha256.clone();
    CheckpointTransition::Advanced { from, to: observed }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalChange {
    pub stable_identity: String,
    pub change_id: u64,
    pub canonical_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct AmbiguousChange {
    pub change_id: u64,
    pub reason: ChangeRejectionReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeRejectionReason {
    InvalidIdentity,
    InvalidCanonicalSha256,
    ConflictingPayload,
}

/// Deduplicate overlap records deterministically. The newest change identifier
/// wins; identical replayed records collapse; conflicting payloads at the same
/// identity and change identifier fail closed.
pub fn deduplicate_changes(
    changes: impl IntoIterator<Item = CanonicalChange>,
) -> Result<Vec<CanonicalChange>, AmbiguousChange> {
    let mut deduplicated = BTreeMap::<String, CanonicalChange>::new();
    for change in changes {
        if !valid_identity(&change.stable_identity) {
            return Err(AmbiguousChange {
                change_id: change.change_id,
                reason: ChangeRejectionReason::InvalidIdentity,
            });
        }
        if !is_lower_sha256(&change.canonical_sha256) {
            return Err(AmbiguousChange {
                change_id: change.change_id,
                reason: ChangeRejectionReason::InvalidCanonicalSha256,
            });
        }
        match deduplicated.get(&change.stable_identity) {
            None => {
                deduplicated.insert(change.stable_identity.clone(), change);
            }
            Some(existing) if change.change_id > existing.change_id => {
                deduplicated.insert(change.stable_identity.clone(), change);
            }
            Some(existing) if change.change_id < existing.change_id => {}
            Some(existing) if change.canonical_sha256 == existing.canonical_sha256 => {}
            Some(_) => {
                return Err(AmbiguousChange {
                    change_id: change.change_id,
                    reason: ChangeRejectionReason::ConflictingPayload,
                });
            }
        }
    }
    Ok(deduplicated.into_values().collect())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeletionCapabilityObservation {
    scope: IncrementalScope,
    rule_id: String,
    state: CapabilityState,
    confidence: EvidenceConfidence,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExplicitTombstone {
    pub stable_identity: String,
    pub rule_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncrementalReconciliation {
    pub upserted_identities: BTreeSet<String>,
    pub deleted_identities: BTreeSet<String>,
    /// Existing records not mentioned by the feed remain present. This set is
    /// explicit so callers cannot accidentally interpret absence as deletion.
    pub retained_absent_identities: BTreeSet<String>,
    pub rejected_tombstones: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IncrementalReconciliationError {
    InvalidIdentity,
    InvalidDeletionRule,
    AmbiguousChange {
        change_id: u64,
        reason: ChangeRejectionReason,
    },
    ConflictingAction,
}

pub fn reconcile_incremental(
    scope: &IncrementalScope,
    existing_identities: &BTreeSet<String>,
    changes: &[CanonicalChange],
    tombstones: &[ExplicitTombstone],
    deletion_capability: Option<&DeletionCapabilityObservation>,
) -> Result<IncrementalReconciliation, IncrementalReconciliationError> {
    for identity in existing_identities {
        if !valid_identity(identity) {
            return Err(IncrementalReconciliationError::InvalidIdentity);
        }
    }
    let deduplicated = deduplicate_changes(changes.iter().cloned()).map_err(|error| {
        IncrementalReconciliationError::AmbiguousChange {
            change_id: error.change_id,
            reason: error.reason,
        }
    })?;
    let upserted_identities = deduplicated
        .iter()
        .map(|change| change.stable_identity.clone())
        .collect::<BTreeSet<_>>();

    let mut deleted_identities = BTreeSet::new();
    let mut rejected_tombstones = BTreeSet::new();
    for tombstone in tombstones {
        if !valid_identity(&tombstone.stable_identity) {
            return Err(IncrementalReconciliationError::InvalidIdentity);
        }
        if !valid_rule_id(&tombstone.rule_id) {
            return Err(IncrementalReconciliationError::InvalidDeletionRule);
        }
        let proven = deletion_capability.is_some_and(|capability| {
            capability.scope == *scope
                && capability.state == CapabilityState::Supported
                && capability.confidence == EvidenceConfidence::Observed
                && !capability.rule_id.trim().is_empty()
                && capability.rule_id == tombstone.rule_id
        });
        if proven {
            deleted_identities.insert(tombstone.stable_identity.clone());
        } else {
            rejected_tombstones.insert(tombstone.stable_identity.clone());
        }
    }
    if upserted_identities
        .intersection(&deleted_identities)
        .next()
        .is_some()
    {
        return Err(IncrementalReconciliationError::ConflictingAction);
    }

    let retained_absent_identities = existing_identities
        .difference(&upserted_identities)
        .filter(|identity| !deleted_identities.contains(*identity))
        .cloned()
        .collect();

    Ok(IncrementalReconciliation {
        upserted_identities,
        deleted_identities,
        retained_absent_identities,
        rejected_tombstones,
    })
}

fn valid_identity(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 512 && !value.chars().any(char::is_control)
}

fn valid_rule_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

fn is_lower_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(release: &str, query_profile: &str) -> IncrementalScope {
        IncrementalScope {
            source_lineage: "source-lineage".to_string(),
            company_guid: "company-guid".to_string(),
            company_fingerprint: "company-fingerprint".to_string(),
            object_type: "voucher".to_string(),
            capability_profile_version: 1,
            product: "tally_prime".to_string(),
            release: release.to_string(),
            mode: "education".to_string(),
            transport: TransportId::XmlHttp,
            pack: CapabilityPackId::CoreAccounting,
            pack_schema_version: PackSchemaVersion { major: 1, minor: 0 },
            query_profile: query_profile.to_string(),
            filters_sha256: "a".repeat(64),
            date_window_policy: "change_id_overlap_v1".to_string(),
        }
    }

    fn observed_capability(scope: IncrementalScope) -> IncrementalCapabilityObservation {
        IncrementalCapabilityObservation {
            scope,
            state: CapabilityState::Supported,
            confidence: EvidenceConfidence::Observed,
            identifier_semantics: ChangeIdentifierSemantics::MonotonicPerObject,
            inclusive_lower_bound_observed: true,
            explicit_source_high_watermark_observed: true,
        }
    }

    fn checkpoint(scope: IncrementalScope, high_watermark: u64) -> IncrementalCheckpoint {
        IncrementalCheckpoint {
            scope,
            high_watermark,
            established_by_verified_full_snapshot: true,
            established_by_proof_sha256: "c".repeat(64),
            last_transition_proof_sha256: "c".repeat(64),
            last_identity_sweep_unix_ms: 1_000,
            invalidated_reason: None,
        }
    }

    fn full_receipt(
        scope: IncrementalScope,
        verification: VerificationState,
        high_watermark: Option<u64>,
    ) -> VerifiedFullSnapshotReceipt {
        VerifiedFullSnapshotReceipt {
            scope,
            verification,
            proof_sha256: "d".repeat(64),
            observed_source_high_watermark: high_watermark,
            completed_at_unix_ms: 1_000,
        }
    }

    #[test]
    fn overlap_start_saturates_and_never_exceeds_checkpoint() {
        let scope = scope("7.0", "voucher-v1");
        let capability = observed_capability(scope.clone());
        for checkpoint_value in 0..=256 {
            for overlap in [0, 1, 7, 128, u64::MAX] {
                let plan = plan_sync(
                    IncrementalPolicy {
                        overlap_identifiers: overlap,
                        identity_sweep_interval_ms: 10_000,
                    },
                    &scope,
                    Some(&capability),
                    Some(&checkpoint(scope.clone(), checkpoint_value)),
                    1_001,
                );
                let SyncPlan::Incremental {
                    from_change_id_inclusive,
                    checkpoint_before,
                    ..
                } = plan
                else {
                    panic!("observed exact capability should permit incremental sync");
                };
                assert_eq!(checkpoint_before, checkpoint_value);
                assert_eq!(
                    from_change_id_inclusive,
                    checkpoint_value.saturating_sub(overlap)
                );
                assert!(from_change_id_inclusive <= checkpoint_before);
            }
        }
    }

    #[test]
    fn any_scope_drift_forces_honest_full_snapshot_fallback() {
        let original = scope("7.0", "voucher-v1");
        let capability = observed_capability(original.clone());
        let checkpoint = checkpoint(original.clone(), 42);
        let mut object_changed = original.clone();
        object_changed.object_type = "ledger".to_string();
        let mut profile_changed = original.clone();
        profile_changed.capability_profile_version = 2;
        let mut transport_changed = original.clone();
        transport_changed.transport = TransportId::JsonEx;
        let mut schema_changed = original.clone();
        schema_changed.pack_schema_version.minor = 1;
        let mut company_changed = original.clone();
        company_changed.company_guid = "different-company".to_string();
        let mut pack_changed = original.clone();
        pack_changed.pack = CapabilityPackId::BillsAndPayments;
        let mut lineage_changed = original.clone();
        lineage_changed.source_lineage = "different-source-lineage".to_string();
        let mut filters_changed = original.clone();
        filters_changed.filters_sha256 = "b".repeat(64);
        let mut window_policy_changed = original.clone();
        window_policy_changed.date_window_policy = "different-policy".to_string();
        for changed in [
            scope("7.1", "voucher-v1"),
            scope("7.0", "voucher-v2"),
            object_changed,
            profile_changed,
            transport_changed,
            schema_changed,
            company_changed,
            pack_changed,
            lineage_changed,
            filters_changed,
            window_policy_changed,
        ] {
            let plan = plan_sync(
                IncrementalPolicy::default(),
                &changed,
                Some(&capability),
                Some(&checkpoint),
                2_000,
            );
            assert!(matches!(
                plan,
                SyncPlan::FullSnapshot {
                    reason: FullSnapshotReason::CapabilityScopeDrift,
                    ..
                }
            ));
        }
    }

    #[test]
    fn documented_or_inferred_capability_is_not_treated_as_observed() {
        let scope = scope("7.0", "voucher-v1");
        for confidence in [
            EvidenceConfidence::Documented,
            EvidenceConfidence::Inferred,
            EvidenceConfidence::Unknown,
        ] {
            let mut capability = observed_capability(scope.clone());
            capability.confidence = confidence;
            assert!(matches!(
                plan_sync(
                    IncrementalPolicy::default(),
                    &scope,
                    Some(&capability),
                    Some(&checkpoint(scope.clone(), 42)),
                    2_000,
                ),
                SyncPlan::FullSnapshot {
                    reason: FullSnapshotReason::CapabilityNotObservedAtRuntime,
                    ..
                }
            ));
        }
        for missing_protocol_fact in [
            {
                let mut capability = observed_capability(scope.clone());
                capability.inclusive_lower_bound_observed = false;
                capability
            },
            {
                let mut capability = observed_capability(scope.clone());
                capability.explicit_source_high_watermark_observed = false;
                capability
            },
        ] {
            assert!(matches!(
                plan_sync(
                    IncrementalPolicy::default(),
                    &scope,
                    Some(&missing_protocol_fact),
                    Some(&checkpoint(scope.clone(), 42)),
                    2_000,
                ),
                SyncPlan::FullSnapshot {
                    reason: FullSnapshotReason::CapabilityNotObservedAtRuntime,
                    ..
                }
            ));
        }
    }

    #[test]
    fn malformed_filter_hash_or_missing_profile_dimension_forces_full_snapshot() {
        let valid = scope("7.0", "voucher-v1");
        let mut invalid_hash = valid.clone();
        invalid_hash.filters_sha256 = "not-a-sha256".to_string();
        let mut missing_profile = valid.clone();
        missing_profile.capability_profile_version = 0;
        let mut missing_window_policy = valid;
        missing_window_policy.date_window_policy.clear();
        for invalid in [invalid_hash, missing_profile, missing_window_policy] {
            assert!(matches!(
                plan_sync(IncrementalPolicy::default(), &invalid, None, None, 2_000),
                SyncPlan::FullSnapshot {
                    reason: FullSnapshotReason::ScopeIncomplete,
                    ..
                }
            ));
        }
    }

    #[test]
    fn periodic_identity_sweep_is_due_at_interval_and_on_clock_regression() {
        let scope = scope("7.0", "voucher-v1");
        let capability = observed_capability(scope.clone());
        let checkpoint = checkpoint(scope.clone(), 42);
        for (now, expected) in [(999, true), (1_000, false), (10_999, false), (11_000, true)] {
            let SyncPlan::Incremental {
                identity_sweep_required,
                ..
            } = plan_sync(
                IncrementalPolicy {
                    overlap_identifiers: 5,
                    identity_sweep_interval_ms: 10_000,
                },
                &scope,
                Some(&capability),
                Some(&checkpoint),
                now,
            )
            else {
                panic!("valid incremental plan");
            };
            assert_eq!(identity_sweep_required, expected, "now={now}");
        }
    }

    #[test]
    fn identifier_regression_or_missing_proof_invalidates_checkpoint() {
        for observed in [Some(41), None] {
            let scope = scope("7.0", "voucher-v1");
            let mut checkpoint = checkpoint(scope.clone(), 42);
            let receipt = VerifiedIncrementalReceipt {
                scope,
                checkpoint_before: 42,
                verification: VerificationState::Verified,
                proof_sha256: "e".repeat(64),
                observed_source_high_watermark: observed,
            };
            let transition = apply_incremental_high_watermark(&mut checkpoint, &receipt);
            assert!(matches!(
                transition,
                CheckpointTransition::Invalidated { .. }
            ));
            assert!(checkpoint.invalidated_reason.is_some());
        }
    }

    #[test]
    fn overlap_deduplication_is_idempotent_and_order_independent() {
        let input = vec![
            CanonicalChange {
                stable_identity: "a".to_string(),
                change_id: 10,
                canonical_sha256: "1".repeat(64),
            },
            CanonicalChange {
                stable_identity: "b".to_string(),
                change_id: 11,
                canonical_sha256: "2".repeat(64),
            },
            CanonicalChange {
                stable_identity: "a".to_string(),
                change_id: 12,
                canonical_sha256: "3".repeat(64),
            },
            CanonicalChange {
                stable_identity: "b".to_string(),
                change_id: 11,
                canonical_sha256: "2".repeat(64),
            },
        ];
        let mut reversed = input.clone();
        reversed.reverse();

        let first = deduplicate_changes(input).expect("unambiguous records");
        let second = deduplicate_changes(reversed).expect("unambiguous records");
        assert_eq!(first, second);
        assert_eq!(deduplicate_changes(first.clone()).unwrap(), first);
        assert_eq!(first[0].canonical_sha256, "3".repeat(64));
    }

    #[test]
    fn same_change_id_with_different_content_fails_closed() {
        let result = deduplicate_changes([
            CanonicalChange {
                stable_identity: "voucher-1".to_string(),
                change_id: 7,
                canonical_sha256: "a".repeat(64),
            },
            CanonicalChange {
                stable_identity: "voucher-1".to_string(),
                change_id: 7,
                canonical_sha256: "b".repeat(64),
            },
        ]);
        assert_eq!(
            result,
            Err(AmbiguousChange {
                change_id: 7,
                reason: ChangeRejectionReason::ConflictingPayload,
            })
        );
    }

    #[test]
    fn absence_never_deletes_and_only_proven_tombstones_are_accepted() {
        let scope = scope("7.0", "voucher-v1");
        let existing = ["unchanged", "edited", "deleted"]
            .into_iter()
            .map(str::to_string)
            .collect();
        let changes = [CanonicalChange {
            stable_identity: "edited".to_string(),
            change_id: 44,
            canonical_sha256: "a".repeat(64),
        }];
        let tombstones = [ExplicitTombstone {
            stable_identity: "deleted".to_string(),
            rule_id: "explicit-deleted-collection-v1".to_string(),
        }];

        let without_proof =
            reconcile_incremental(&scope, &existing, &changes, &tombstones, None).unwrap();
        assert!(without_proof.deleted_identities.is_empty());
        assert_eq!(
            without_proof.retained_absent_identities,
            ["deleted", "unchanged"]
                .into_iter()
                .map(str::to_string)
                .collect()
        );

        let deletion_capability = DeletionCapabilityObservation {
            scope: scope.clone(),
            rule_id: "explicit-deleted-collection-v1".to_string(),
            state: CapabilityState::Supported,
            confidence: EvidenceConfidence::Observed,
        };
        let proven = reconcile_incremental(
            &scope,
            &existing,
            &changes,
            &tombstones,
            Some(&deletion_capability),
        )
        .unwrap();
        assert_eq!(
            proven.deleted_identities,
            ["deleted"].into_iter().map(str::to_string).collect()
        );
        assert_eq!(
            proven.retained_absent_identities,
            ["unchanged"].into_iter().map(str::to_string).collect()
        );
    }

    #[test]
    fn only_verified_full_snapshot_can_establish_incremental_checkpoint() {
        let scope = scope("7.0", "voucher-v1");
        let capability = observed_capability(scope.clone());
        for verification in [VerificationState::Partial, VerificationState::Unverified] {
            assert_eq!(
                establish_checkpoint_from_full_snapshot(
                    scope.clone(),
                    &capability,
                    full_receipt(scope.clone(), verification, Some(42)),
                ),
                Err(FullSnapshotReason::NoVerifiedCheckpoint)
            );
        }
        assert_eq!(
            establish_checkpoint_from_full_snapshot(
                scope.clone(),
                &capability,
                full_receipt(scope, VerificationState::Verified, Some(42)),
            )
            .unwrap()
            .high_watermark,
            42
        );
    }

    #[test]
    fn receipt_scope_or_checkpoint_drift_invalidates_authority() {
        let expected_scope = scope("7.0", "voucher-v1");
        let capability = observed_capability(expected_scope.clone());
        let different_scope = scope("7.1", "voucher-v1");
        assert_eq!(
            establish_checkpoint_from_full_snapshot(
                expected_scope.clone(),
                &capability,
                full_receipt(
                    different_scope.clone(),
                    VerificationState::Verified,
                    Some(42)
                ),
            ),
            Err(FullSnapshotReason::ReceiptScopeMismatch)
        );

        let mut checkpoint = checkpoint(expected_scope, 42);
        let transition = apply_incremental_high_watermark(
            &mut checkpoint,
            &VerifiedIncrementalReceipt {
                scope: different_scope,
                checkpoint_before: 41,
                verification: VerificationState::Verified,
                proof_sha256: "f".repeat(64),
                observed_source_high_watermark: Some(43),
            },
        );
        assert_eq!(
            transition,
            CheckpointTransition::Invalidated {
                at: 42,
                reason: CheckpointInvalidationReason::ReceiptScopeMismatch,
            }
        );
    }

    #[test]
    fn one_identity_cannot_be_upserted_and_deleted_in_the_same_delta() {
        let scope = scope("7.0", "voucher-v1");
        let identity = "same-identity";
        let changes = [CanonicalChange {
            stable_identity: identity.to_string(),
            change_id: 44,
            canonical_sha256: "a".repeat(64),
        }];
        let tombstones = [ExplicitTombstone {
            stable_identity: identity.to_string(),
            rule_id: "deleted_v1".to_string(),
        }];
        let deletion_capability = DeletionCapabilityObservation {
            scope: scope.clone(),
            rule_id: "deleted_v1".to_string(),
            state: CapabilityState::Supported,
            confidence: EvidenceConfidence::Observed,
        };
        assert_eq!(
            reconcile_incremental(
                &scope,
                &BTreeSet::new(),
                &changes,
                &tombstones,
                Some(&deletion_capability),
            ),
            Err(IncrementalReconciliationError::ConflictingAction)
        );
    }

    #[test]
    fn reconciliation_cannot_bypass_overlap_deduplication() {
        let scope = scope("7.0", "voucher-v1");
        let existing = BTreeSet::new();
        let conflicting = [
            CanonicalChange {
                stable_identity: "voucher-1".to_string(),
                change_id: 7,
                canonical_sha256: "a".repeat(64),
            },
            CanonicalChange {
                stable_identity: "voucher-1".to_string(),
                change_id: 7,
                canonical_sha256: "b".repeat(64),
            },
        ];
        assert_eq!(
            reconcile_incremental(&scope, &existing, &conflicting, &[], None),
            Err(IncrementalReconciliationError::AmbiguousChange {
                change_id: 7,
                reason: ChangeRejectionReason::ConflictingPayload,
            })
        );

        let ordered_overlap = [
            CanonicalChange {
                stable_identity: "voucher-1".to_string(),
                change_id: 9,
                canonical_sha256: "c".repeat(64),
            },
            CanonicalChange {
                stable_identity: "voucher-1".to_string(),
                change_id: 8,
                canonical_sha256: "b".repeat(64),
            },
        ];
        let result = reconcile_incremental(&scope, &existing, &ordered_overlap, &[], None)
            .expect("newest unambiguous overlap record wins");
        assert_eq!(
            result.upserted_identities,
            ["voucher-1"].into_iter().map(str::to_string).collect()
        );
    }

    #[test]
    fn invalidated_checkpoint_is_terminal_for_later_receipts() {
        let scope = scope("7.0", "voucher-v1");
        let mut checkpoint = checkpoint(scope.clone(), 42);
        checkpoint.invalidated_reason =
            Some(CheckpointInvalidationReason::IdentifierRegressedOrReset);
        let prior_proof = checkpoint.last_transition_proof_sha256.clone();
        let transition = apply_incremental_high_watermark(
            &mut checkpoint,
            &VerifiedIncrementalReceipt {
                scope,
                checkpoint_before: 42,
                verification: VerificationState::Verified,
                proof_sha256: "f".repeat(64),
                observed_source_high_watermark: Some(43),
            },
        );
        assert_eq!(
            transition,
            CheckpointTransition::Invalidated {
                at: 42,
                reason: CheckpointInvalidationReason::IdentifierRegressedOrReset,
            }
        );
        assert_eq!(checkpoint.high_watermark, 42);
        assert_eq!(checkpoint.last_transition_proof_sha256, prior_proof);
    }
}
