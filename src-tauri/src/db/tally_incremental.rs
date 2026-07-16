use bridge_tally_core::{CapabilityPackId, CapabilityState, EvidenceConfidence, TransportId};
use bridge_tally_incremental::{
    plan_sync, ChangeIdentifierSemantics, IncrementalCapabilityObservation, IncrementalCheckpoint,
    IncrementalPolicy, IncrementalScope, SyncPlan,
};
use sha2::{Digest, Sha256};
use sqlx::Row;
use uuid::Uuid;

use crate::db::tally_mirror::{MirrorError, TallyMirrorRepository};
use crate::sync::snapshot::{SnapshotPhase, SqliteSnapshotStateStore};
use crate::tally::company_source_identity;

/// Sealed authority receipt. Only the future protocol verifier in this module may construct one;
/// callers cannot turn a generic pack passport into observed incremental evidence.
#[derive(Debug, Clone)]
pub struct VerifiedIncrementalCanaryReceipt {
    company_id: String,
    capability_snapshot_id: String,
    canary_contract_version: u16,
    response_sha256: String,
    observation: IncrementalCapabilityObservation,
    observed_at_unix_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredIncrementalCapability {
    pub id: String,
    pub observation: IncrementalCapabilityObservation,
    pub observed_at_unix_ms: i64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct IncrementalReadiness {
    pub scope_sha256: String,
    pub plan: SyncPlan,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct IncrementalFoundationEvidence {
    pub execution_enabled: bool,
    pub affirmative_exact_capability_receipts: i64,
    pub establishment_receipts: i64,
    pub active_checkpoint_heads: i64,
    pub state: &'static str,
    pub fallback_warning_code: &'static str,
}

#[derive(Debug, serde::Serialize)]
struct EstablishmentReceiptHashInput {
    verifier_contract_version: i64,
    receipt_id: String,
    scope_sha256: String,
    capability_observation_id: String,
    proof_id: String,
    proof_sha256: String,
    batch_id: String,
    snapshot_plan_sha256: String,
    source_response_sha256: String,
    coverage_manifest_sha256: String,
    source_high_watermark_decimal: String,
    max_observed_alter_id_decimal: Option<String>,
    source_record_count: i64,
    accepted_record_count: i64,
    deduplicated_record_count: i64,
    numeric_alter_id_count: i64,
    rejected_record_count: i64,
    duplicate_identity_count: i64,
    missing_identity_count: i64,
    out_of_scope_record_count: i64,
    created_at_unix_ms: i64,
}

impl TallyMirrorRepository {
    /// Read-only, count-only operator evidence. It cannot authorize or start an incremental read.
    pub async fn incremental_foundation_evidence(
        &self,
        company_id: &str,
    ) -> Result<IncrementalFoundationEvidence, MirrorError> {
        if company_id.trim().is_empty()
            || company_id.len() > 128
            || company_id.chars().any(char::is_control)
        {
            return Err(MirrorError::InvalidInput("company_id"));
        }
        let row = sqlx::query(
            "SELECT \
               (SELECT COUNT(*) FROM tally_incremental_capability_observations AS capability \
                WHERE capability.company_id = ?1 AND capability.capability_state = 'supported' \
                  AND capability.confidence = 'observed' \
                  AND capability.identifier_semantics = 'monotonic_per_object' \
                  AND capability.inclusive_lower_bound_observed = 1 \
                  AND capability.explicit_source_high_watermark_observed = 1) \
                 AS capability_count, \
               (SELECT COUNT(*) FROM tally_incremental_establishment_receipts AS receipt \
                JOIN tally_incremental_capability_observations AS capability \
                  ON capability.id = receipt.capability_observation_id \
                WHERE capability.company_id = ?1) AS receipt_count, \
               (SELECT COUNT(*) FROM tally_incremental_checkpoint_heads AS head \
                JOIN tally_incremental_establishment_receipts AS receipt \
                  ON receipt.id = head.establishment_receipt_id \
                JOIN tally_incremental_capability_observations AS capability \
                  ON capability.id = receipt.capability_observation_id \
                WHERE capability.company_id = ?1 AND head.generation = 1 \
                  AND head.state = 'active') AS head_count",
        )
        .bind(company_id)
        .fetch_one(&self.pool)
        .await?;
        let affirmative_exact_capability_receipts: i64 = row.try_get("capability_count")?;
        let establishment_receipts: i64 = row.try_get("receipt_count")?;
        let active_checkpoint_heads: i64 = row.try_get("head_count")?;
        let state = if affirmative_exact_capability_receipts == 0 {
            "exact_capability_not_observed"
        } else if establishment_receipts == 0 || active_checkpoint_heads == 0 {
            "verified_establishment_missing"
        } else {
            "execution_not_enabled"
        };
        Ok(IncrementalFoundationEvidence {
            execution_enabled: false,
            affirmative_exact_capability_receipts,
            establishment_receipts,
            active_checkpoint_heads,
            state,
            fallback_warning_code: "incremental_execution_disabled_full_snapshot_required",
        })
    }

    /// Persist only exact-profile evidence. This is intentionally not exposed as a Tauri command:
    /// the future object-specific canary owns authority to call it.
    pub async fn save_incremental_capability_observation(
        &self,
        receipt: VerifiedIncrementalCanaryReceipt,
    ) -> Result<StoredIncrementalCapability, MirrorError> {
        validate_scope(&receipt.observation.scope)?;
        if receipt.company_id.trim().is_empty()
            || receipt.capability_snapshot_id.trim().is_empty()
            || receipt.canary_contract_version == 0
            || !is_lower_sha256(&receipt.response_sha256)
            || receipt.observed_at_unix_ms <= 0
        {
            return Err(MirrorError::InvalidInput("incremental_authority"));
        }
        let pin = self.snapshot_source_pin(&receipt.company_id).await?;
        let expected_lineage = format!("tally_xml_http:{}", pin.canonical_origin);
        let expected_identity = company_source_identity(&expected_lineage, &pin.company_guid);
        if receipt.observation.scope.source_lineage != expected_identity.bridge_source_lineage
            || !receipt
                .observation
                .scope
                .company_guid
                .eq_ignore_ascii_case(&expected_identity.company_guid)
            || receipt.observation.scope.company_fingerprint
                != expected_identity.observed_fingerprint
        {
            return Err(MirrorError::InvalidInput("incremental_company_scope"));
        }
        if !self
            .capability_snapshot_matches_plan(
                &receipt.capability_snapshot_id,
                &receipt.company_id,
                receipt.observation.scope.capability_profile_version,
                &receipt.observation.scope.product,
                Some(&receipt.observation.scope.release),
                Some(&receipt.observation.scope.mode),
            )
            .await?
        {
            return Err(MirrorError::InvalidInput("incremental_capability_profile"));
        }

        let scope_json = serde_json::to_string(&receipt.observation.scope)?;
        let scope_sha256 = incremental_scope_sha256(&receipt.observation.scope)?;
        let id = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO tally_incremental_capability_observations(\
               id, scope_sha256, scope_json, capability_snapshot_id, company_id, \
               verifier_contract_version, response_sha256, capability_state, confidence, \
               identifier_semantics, inclusive_lower_bound_observed, \
               explicit_source_high_watermark_observed, observed_at_unix_ms\
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        )
        .bind(&id)
        .bind(scope_sha256)
        .bind(scope_json)
        .bind(receipt.capability_snapshot_id)
        .bind(receipt.company_id)
        .bind(i64::from(receipt.canary_contract_version))
        .bind(receipt.response_sha256)
        .bind(capability_state_code(receipt.observation.state))
        .bind(confidence_code(receipt.observation.confidence))
        .bind(identifier_semantics_code(
            receipt.observation.identifier_semantics,
        ))
        .bind(i64::from(
            receipt.observation.inclusive_lower_bound_observed,
        ))
        .bind(i64::from(
            receipt.observation.explicit_source_high_watermark_observed,
        ))
        .bind(receipt.observed_at_unix_ms)
        .execute(&self.pool)
        .await?;
        Ok(StoredIncrementalCapability {
            id,
            observation: receipt.observation,
            observed_at_unix_ms: receipt.observed_at_unix_ms,
        })
    }

    pub async fn load_incremental_capability(
        &self,
        scope: &IncrementalScope,
    ) -> Result<Option<StoredIncrementalCapability>, MirrorError> {
        validate_scope(scope)?;
        let row = sqlx::query(
            "SELECT id, scope_json, capability_state, confidence, identifier_semantics, \
               inclusive_lower_bound_observed, explicit_source_high_watermark_observed, \
               observed_at_unix_ms \
             FROM tally_incremental_capability_observations WHERE scope_sha256 = ?1 \
             ORDER BY observed_at_unix_ms DESC, id DESC LIMIT 1",
        )
        .bind(incremental_scope_sha256(scope)?)
        .fetch_optional(&self.pool)
        .await?;
        let stored = row.map(decode_capability_row).transpose()?;
        if stored
            .as_ref()
            .is_some_and(|stored| stored.observation.scope != *scope)
        {
            return Err(MirrorError::VerificationInvariant);
        }
        Ok(stored)
    }

    pub async fn load_incremental_checkpoint(
        &self,
        scope: &IncrementalScope,
    ) -> Result<Option<IncrementalCheckpoint>, MirrorError> {
        validate_scope(scope)?;
        let scope_sha256 = incremental_scope_sha256(scope)?;
        let head_count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_incremental_checkpoint_heads WHERE scope_sha256 = ?1",
        )
        .bind(&scope_sha256)
        .fetch_one(&self.pool)
        .await?;
        if head_count == 0 {
            return Ok(None);
        }
        if head_count != 1 {
            return Err(MirrorError::VerificationInvariant);
        }
        let row = sqlx::query(
            "SELECT head.scope_sha256, head.scope_json, head.high_watermark_decimal, \
               head.generation, head.state, head.established_at_unix_ms, \
               receipt.id AS receipt_id, receipt.scope_sha256 AS receipt_scope_sha256, \
               receipt.scope_json AS receipt_scope_json, \
               receipt.capability_observation_id, receipt.proof_id, receipt.proof_sha256, \
               receipt.batch_id, receipt.snapshot_plan_sha256, \
               receipt.source_response_sha256, receipt.coverage_manifest_sha256, \
               receipt.source_high_watermark_decimal, receipt.max_observed_alter_id_decimal, \
               receipt.source_record_count, receipt.accepted_record_count, \
               receipt.deduplicated_record_count, receipt.numeric_alter_id_count, \
               receipt.rejected_record_count, receipt.duplicate_identity_count, \
               receipt.missing_identity_count, receipt.out_of_scope_record_count, \
               receipt.verifier_contract_version AS receipt_verifier_contract_version, \
               receipt.receipt_sha256, receipt.created_at_unix_ms, proof.run_id, \
               CASE WHEN \
                 head.establishment_receipt_id = receipt.id AND \
                 head.scope_sha256 = receipt.scope_sha256 AND \
                 head.scope_json = receipt.scope_json AND \
                 head.high_watermark_decimal = receipt.source_high_watermark_decimal AND \
                 head.generation = 1 AND head.state = 'active' AND \
                 head.established_at_unix_ms = receipt.created_at_unix_ms AND \
                 capability.id = receipt.capability_observation_id AND \
                 capability.scope_sha256 = receipt.scope_sha256 AND \
                 capability.scope_json = receipt.scope_json AND \
                 capability.company_id = proof.company_id AND \
                 capability.capability_snapshot_id = proof.capability_snapshot_id AND \
                 capability.verifier_contract_version > 0 AND \
                 length(capability.response_sha256) = 64 AND \
                 capability.response_sha256 NOT GLOB '*[^0-9a-f]*' AND \
                 capability.capability_state = 'supported' AND capability.confidence = 'observed' AND \
                 capability.identifier_semantics = 'monotonic_per_object' AND \
                 capability.inclusive_lower_bound_observed = 1 AND \
                 capability.explicit_source_high_watermark_observed = 1 AND \
                 proof.id = receipt.proof_id AND proof.entry_sha256 = receipt.proof_sha256 AND \
                 proof.batch_id = receipt.batch_id AND proof.outcome = 'completed' AND \
                 proof.verification_state = 'verified' AND proof.completed_at_unix_ms IS NOT NULL AND \
                 proof.snapshot_sha256 IS NOT NULL AND proof.gap_codes_json = '[]' AND \
                 proof.warning_codes_json = '[]' AND proof.rejected_records = 0 AND \
                 proof.accepted_records = receipt.accepted_record_count AND \
                 batch.id = receipt.batch_id AND batch.run_id = proof.run_id AND \
                 batch.capability_snapshot_id = proof.capability_snapshot_id AND \
                 batch.company_id = proof.company_id AND batch.pack_id = proof.pack_id AND \
                 batch.pack_id = json_extract(receipt.scope_json, '$.pack') AND \
                 batch.pack_schema_major = json_extract(receipt.scope_json, '$.pack_schema_version.major') AND \
                 batch.pack_schema_minor = json_extract(receipt.scope_json, '$.pack_schema_version.minor') AND \
                 batch.source_transport = json_extract(receipt.scope_json, '$.transport') AND \
                 batch.source_release = json_extract(receipt.scope_json, '$.release') AND \
                 batch.state = 'verified' AND batch.snapshot_sha256 = proof.snapshot_sha256 AND \
                 batch.accepted_records = receipt.accepted_record_count AND batch.rejected_records = 0 AND \
                 snapshot.id = proof.capability_snapshot_id AND \
                 snapshot.profile_version = json_extract(receipt.scope_json, '$.capability_profile_version') AND \
                 snapshot.product = json_extract(receipt.scope_json, '$.product') AND \
                 snapshot.release = json_extract(receipt.scope_json, '$.release') AND \
                 snapshot.mode = json_extract(receipt.scope_json, '$.mode') AND \
                 snapshot.mode_confidence = 'observed' AND company.id = proof.company_id AND \
                 company.endpoint_id = snapshot.endpoint_id AND \
                 company.company_guid = json_extract(receipt.scope_json, '$.company_guid') COLLATE NOCASE AND \
                 company.identity_confidence = 'observed' AND durable.run_id = proof.run_id AND \
                 durable.row_sha256 IS NOT NULL AND durable.plan_sha256 = receipt.snapshot_plan_sha256 AND \
                 json_extract(durable.state_json, '$.plan_sha256') = receipt.snapshot_plan_sha256 AND \
                 json_extract(durable.state_json, '$.batch_id') = receipt.batch_id AND \
                 json_extract(durable.state_json, '$.progress.phase') = 'completed' AND \
                 json_extract(durable.state_json, '$.commit_receipt.proof_id') = receipt.proof_id AND \
                 json_extract(durable.state_json, '$.commit_receipt.proof_sha256') = receipt.proof_sha256 AND \
                 EXISTS (SELECT 1 FROM json_each(durable.state_json, '$.plan.windows') AS window \
                   WHERE json_extract(window.value, '$.query_profile') = json_extract(receipt.scope_json, '$.query_profile') \
                     AND json_extract(window.value, '$.filters_sha256') = json_extract(receipt.scope_json, '$.filters_sha256')) \
               THEN 1 ELSE 0 END AS relationally_valid \
             FROM tally_incremental_checkpoint_heads AS head \
             JOIN tally_incremental_establishment_receipts AS receipt \
               ON receipt.id = head.establishment_receipt_id \
             JOIN tally_incremental_capability_observations AS capability \
               ON capability.id = receipt.capability_observation_id \
             JOIN tally_proof_ledger AS proof ON proof.id = receipt.proof_id \
             JOIN tally_observation_batches AS batch ON batch.id = receipt.batch_id \
             JOIN tally_capability_snapshots AS snapshot ON snapshot.id = proof.capability_snapshot_id \
             JOIN tally_companies AS company ON company.id = proof.company_id \
             JOIN tally_snapshot_run_states AS durable ON durable.run_id = proof.run_id \
             WHERE head.scope_sha256 = ?1",
        )
        .bind(&scope_sha256)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(MirrorError::VerificationInvariant)?;
        if row.try_get::<i64, _>("relationally_valid")? != 1 {
            return Err(MirrorError::VerificationInvariant);
        }
        let stored_scope: IncrementalScope =
            serde_json::from_str(&row.try_get::<String, _>("scope_json")?)?;
        let receipt_scope: IncrementalScope =
            serde_json::from_str(&row.try_get::<String, _>("receipt_scope_json")?)?;
        if stored_scope != *scope
            || receipt_scope != *scope
            || row.try_get::<String, _>("scope_sha256")? != scope_sha256
            || row.try_get::<String, _>("receipt_scope_sha256")? != scope_sha256
        {
            return Err(MirrorError::VerificationInvariant);
        }
        let receipt = decode_establishment_receipt_hash_input(&row)?;
        if sha256_establishment_receipt(&receipt)? != row.try_get::<String, _>("receipt_sha256")? {
            return Err(MirrorError::VerificationInvariant);
        }
        let batch_id: String = row.try_get("batch_id")?;
        let run_id: String = row.try_get("run_id")?;
        let proof_id: String = row.try_get("proof_id")?;
        let proof_sha256: String = row.try_get("proof_sha256")?;
        let durable = SqliteSnapshotStateStore::new(self.pool_clone())
            .load_by_run_id(&run_id)
            .await
            .map_err(|_| MirrorError::VerificationInvariant)?
            .ok_or(MirrorError::VerificationInvariant)?;
        let durable_receipt = durable
            .commit_receipt
            .as_ref()
            .ok_or(MirrorError::VerificationInvariant)?;
        if !durable.row_integrity_bound
            || durable.progress.phase != SnapshotPhase::Completed
            || durable.plan_sha256 != row.try_get::<String, _>("snapshot_plan_sha256")?
            || durable.batch_id.as_deref() != Some(batch_id.as_str())
            || durable_receipt.proof_id.as_deref() != Some(proof_id.as_str())
            || durable_receipt.proof_sha256.as_deref() != Some(proof_sha256.as_str())
            || !durable_receipt.checkpoint_advanced
        {
            return Err(MirrorError::VerificationInvariant);
        }
        let generic_proof = self
            .historical_commit_receipt_for_batch(&batch_id, &run_id)
            .await?;
        if generic_proof.proof_id != proof_id || generic_proof.proof_sha256 != proof_sha256 {
            return Err(MirrorError::VerificationInvariant);
        }
        let high_watermark_decimal: String = row.try_get("high_watermark_decimal")?;
        let high_watermark = high_watermark_decimal
            .parse::<u64>()
            .map_err(|_| MirrorError::VerificationInvariant)?;
        Ok(Some(IncrementalCheckpoint {
            scope: stored_scope,
            high_watermark,
            established_by_verified_full_snapshot: true,
            established_by_proof_sha256: proof_sha256.clone(),
            last_transition_proof_sha256: proof_sha256,
            last_identity_sweep_unix_ms: row.try_get("established_at_unix_ms")?,
            invalidated_reason: None,
        }))
    }

    /// Current runtime entry point: it can prove eligibility, but it never starts a delta read.
    /// Missing evidence produces the portable policy's explicit full-snapshot warning.
    pub async fn incremental_readiness(
        &self,
        scope: &IncrementalScope,
        policy: IncrementalPolicy,
        now_unix_ms: i64,
    ) -> Result<IncrementalReadiness, MirrorError> {
        let capability = self.load_incremental_capability(scope).await?;
        let checkpoint = self.load_incremental_checkpoint(scope).await?;
        Ok(IncrementalReadiness {
            scope_sha256: incremental_scope_sha256(scope)?,
            plan: plan_sync(
                policy,
                scope,
                capability.as_ref().map(|stored| &stored.observation),
                checkpoint.as_ref(),
                now_unix_ms,
            ),
        })
    }
}

pub fn incremental_scope_sha256(scope: &IncrementalScope) -> Result<String, MirrorError> {
    validate_scope(scope)?;
    let bytes = serde_json::to_vec(scope)?;
    let mut digest = Sha256::new();
    digest.update(b"bridge-tally-incremental-scope-v1\0");
    digest.update(bytes);
    Ok(hex_digest(digest.finalize()))
}

fn decode_establishment_receipt_hash_input(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<EstablishmentReceiptHashInput, MirrorError> {
    Ok(EstablishmentReceiptHashInput {
        verifier_contract_version: row.try_get("receipt_verifier_contract_version")?,
        receipt_id: row.try_get("receipt_id")?,
        scope_sha256: row.try_get("receipt_scope_sha256")?,
        capability_observation_id: row.try_get("capability_observation_id")?,
        proof_id: row.try_get("proof_id")?,
        proof_sha256: row.try_get("proof_sha256")?,
        batch_id: row.try_get("batch_id")?,
        snapshot_plan_sha256: row.try_get("snapshot_plan_sha256")?,
        source_response_sha256: row.try_get("source_response_sha256")?,
        coverage_manifest_sha256: row.try_get("coverage_manifest_sha256")?,
        source_high_watermark_decimal: row.try_get("source_high_watermark_decimal")?,
        max_observed_alter_id_decimal: row.try_get("max_observed_alter_id_decimal")?,
        source_record_count: row.try_get("source_record_count")?,
        accepted_record_count: row.try_get("accepted_record_count")?,
        deduplicated_record_count: row.try_get("deduplicated_record_count")?,
        numeric_alter_id_count: row.try_get("numeric_alter_id_count")?,
        rejected_record_count: row.try_get("rejected_record_count")?,
        duplicate_identity_count: row.try_get("duplicate_identity_count")?,
        missing_identity_count: row.try_get("missing_identity_count")?,
        out_of_scope_record_count: row.try_get("out_of_scope_record_count")?,
        created_at_unix_ms: row.try_get("created_at_unix_ms")?,
    })
}

fn sha256_establishment_receipt(
    receipt: &EstablishmentReceiptHashInput,
) -> Result<String, MirrorError> {
    let mut digest = Sha256::new();
    digest.update(b"bridge-tally-incremental-establishment-v1\0");
    digest.update(serde_json::to_vec(receipt)?);
    Ok(hex_digest(digest.finalize()))
}

fn hex_digest(bytes: impl AsRef<[u8]>) -> String {
    bytes
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn validate_scope(scope: &IncrementalScope) -> Result<(), MirrorError> {
    if !scope.is_exact()
        || scope.pack != CapabilityPackId::CoreAccounting
        || scope.transport != TransportId::XmlHttp
    {
        return Err(MirrorError::InvalidInput("incremental_scope"));
    }
    Ok(())
}

fn is_lower_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn decode_capability_row(
    row: sqlx::sqlite::SqliteRow,
) -> Result<StoredIncrementalCapability, MirrorError> {
    let scope: IncrementalScope = serde_json::from_str(&row.try_get::<String, _>("scope_json")?)?;
    validate_scope(&scope)?;
    Ok(StoredIncrementalCapability {
        id: row.try_get("id")?,
        observation: IncrementalCapabilityObservation {
            scope,
            state: parse_capability_state(&row.try_get::<String, _>("capability_state")?)?,
            confidence: parse_confidence(&row.try_get::<String, _>("confidence")?)?,
            identifier_semantics: parse_identifier_semantics(
                &row.try_get::<String, _>("identifier_semantics")?,
            )?,
            inclusive_lower_bound_observed: row
                .try_get::<i64, _>("inclusive_lower_bound_observed")?
                == 1,
            explicit_source_high_watermark_observed: row
                .try_get::<i64, _>("explicit_source_high_watermark_observed")?
                == 1,
        },
        observed_at_unix_ms: row.try_get("observed_at_unix_ms")?,
    })
}

fn capability_state_code(value: CapabilityState) -> &'static str {
    match value {
        CapabilityState::Supported => "supported",
        CapabilityState::Unsupported => "unsupported",
        CapabilityState::Unknown => "unknown",
        CapabilityState::NotConfigured => "not_configured",
    }
}

fn parse_capability_state(value: &str) -> Result<CapabilityState, MirrorError> {
    match value {
        "supported" => Ok(CapabilityState::Supported),
        "unsupported" => Ok(CapabilityState::Unsupported),
        "unknown" => Ok(CapabilityState::Unknown),
        "not_configured" => Ok(CapabilityState::NotConfigured),
        _ => Err(MirrorError::VerificationInvariant),
    }
}

fn confidence_code(value: EvidenceConfidence) -> &'static str {
    match value {
        EvidenceConfidence::Documented => "documented",
        EvidenceConfidence::Observed => "observed",
        EvidenceConfidence::Inferred => "inferred",
        EvidenceConfidence::Unknown => "unknown",
    }
}

fn parse_confidence(value: &str) -> Result<EvidenceConfidence, MirrorError> {
    match value {
        "documented" => Ok(EvidenceConfidence::Documented),
        "observed" => Ok(EvidenceConfidence::Observed),
        "inferred" => Ok(EvidenceConfidence::Inferred),
        "unknown" => Ok(EvidenceConfidence::Unknown),
        _ => Err(MirrorError::VerificationInvariant),
    }
}

fn identifier_semantics_code(value: ChangeIdentifierSemantics) -> &'static str {
    match value {
        ChangeIdentifierSemantics::MonotonicPerObject => "monotonic_per_object",
        ChangeIdentifierSemantics::Unknown => "unknown",
    }
}

fn parse_identifier_semantics(value: &str) -> Result<ChangeIdentifierSemantics, MirrorError> {
    match value {
        "monotonic_per_object" => Ok(ChangeIdentifierSemantics::MonotonicPerObject),
        "unknown" => Ok(ChangeIdentifierSemantics::Unknown),
        _ => Err(MirrorError::VerificationInvariant),
    }
}

#[cfg(test)]
mod tests {
    use bridge_tally_core::{CapabilityPackId, PackSchemaVersion, TransportId};
    use sqlx::sqlite::SqlitePoolOptions;

    use super::*;
    use crate::db::tally_mirror::{
        BeginBatchInput, CapabilityItemInput, CapabilityKind, CapabilitySnapshotInput,
        CapabilityState as MirrorCapabilityState, CompanyInput, Confidence, RunOutcome,
        SourceIdentityInput, VerificationState as MirrorVerificationState,
    };
    use crate::sync::reconciliation::{CommitBatchInput, CommitBatchParts};

    async fn setup() -> (TallyMirrorRepository, String, String, IncrementalScope) {
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
            .expect("connect synthetic mirror");
        let repository = TallyMirrorRepository::new(pool);
        repository
            .migrate()
            .await
            .expect("migrate synthetic mirror");
        let snapshot = repository
            .save_capability_snapshot(CapabilitySnapshotInput {
                canonical_origin: "http://127.0.0.1:9000".to_string(),
                observed_at_unix_ms: 1_000,
                profile_version: 1,
                product: "TallyPrime".to_string(),
                release: Some("7.0".to_string()),
                mode: Some("Education".to_string()),
                mode_confidence: Confidence::Observed,
                items: vec![
                    CapabilityItemInput {
                        kind: CapabilityKind::Transport,
                        key: "xml_http".to_string(),
                        state: MirrorCapabilityState::Supported,
                        confidence: Confidence::Observed,
                        safe_reason_code: None,
                    },
                    CapabilityItemInput {
                        kind: CapabilityKind::Pack,
                        key: "core_accounting".to_string(),
                        state: MirrorCapabilityState::Supported,
                        confidence: Confidence::Observed,
                        safe_reason_code: None,
                    },
                ],
            })
            .await
            .expect("persist synthetic capability profile");
        let company_guid = "synthetic-company-guid";
        let company = repository
            .upsert_company(CompanyInput {
                endpoint_id: snapshot.endpoint_id,
                display_name: "Synthetic Company".to_string(),
                identity: SourceIdentityInput {
                    guid: Some(company_guid.to_string()),
                    confidence: Some(Confidence::Observed),
                    ..Default::default()
                },
                observed_at_unix_ms: 1_000,
            })
            .await
            .expect("persist synthetic company");
        let lineage = "tally_xml_http:http://127.0.0.1:9000";
        let identity = company_source_identity(lineage, company_guid);
        let scope = IncrementalScope {
            source_lineage: identity.bridge_source_lineage,
            company_guid: identity.company_guid,
            company_fingerprint: identity.observed_fingerprint,
            object_type: "voucher".to_string(),
            capability_profile_version: 1,
            product: "TallyPrime".to_string(),
            release: "7.0".to_string(),
            mode: "Education".to_string(),
            transport: TransportId::XmlHttp,
            pack: CapabilityPackId::CoreAccounting,
            pack_schema_version: PackSchemaVersion { major: 1, minor: 0 },
            query_profile: "core_voucher_incremental_v1".to_string(),
            filters_sha256: "a".repeat(64),
            date_window_policy: "change_id_overlap_v1".to_string(),
        };
        (repository, snapshot.id, company.id, scope)
    }

    #[tokio::test]
    async fn exact_observation_is_immutable_and_still_requires_verified_full_checkpoint() {
        let (repository, snapshot_id, company_id, scope) = setup().await;
        let generic_only = repository
            .incremental_readiness(&scope, IncrementalPolicy::default(), 1_500)
            .await
            .expect("generic capability passport is inspectable");
        assert!(matches!(
            generic_only.plan,
            SyncPlan::FullSnapshot {
                reason: bridge_tally_incremental::FullSnapshotReason::CapabilityNotObserved,
                ..
            }
        ));
        let stored = repository
            .save_incremental_capability_observation(VerifiedIncrementalCanaryReceipt {
                company_id: company_id.clone(),
                capability_snapshot_id: snapshot_id,
                canary_contract_version: 1,
                response_sha256: "1".repeat(64),
                observation: IncrementalCapabilityObservation {
                    scope: scope.clone(),
                    state: CapabilityState::Supported,
                    confidence: EvidenceConfidence::Observed,
                    identifier_semantics: ChangeIdentifierSemantics::MonotonicPerObject,
                    inclusive_lower_bound_observed: true,
                    explicit_source_high_watermark_observed: true,
                },
                observed_at_unix_ms: 2_000,
            })
            .await
            .expect("persist exact synthetic incremental evidence");
        let loaded = repository
            .load_incremental_capability(&scope)
            .await
            .expect("load exact capability")
            .expect("capability exists");
        assert_eq!(loaded, stored);
        let foundation = repository
            .incremental_foundation_evidence(&company_id)
            .await
            .expect("read count-only incremental evidence");
        assert!(!foundation.execution_enabled);
        assert_eq!(foundation.affirmative_exact_capability_receipts, 1);
        assert_eq!(foundation.establishment_receipts, 0);
        assert_eq!(foundation.active_checkpoint_heads, 0);
        assert_eq!(foundation.state, "verified_establishment_missing");
        let readiness = repository
            .incremental_readiness(&scope, IncrementalPolicy::default(), 3_000)
            .await
            .expect("plan readiness");
        assert!(matches!(
            readiness.plan,
            SyncPlan::FullSnapshot {
                reason: bridge_tally_incremental::FullSnapshotReason::NoVerifiedCheckpoint,
                ..
            }
        ));
        let mutation = sqlx::query(
            "UPDATE tally_incremental_capability_observations SET confidence = 'unknown' \
             WHERE id = ?1",
        )
        .bind(stored.id)
        .execute(&repository.pool)
        .await;
        assert!(mutation.is_err(), "capability evidence must be immutable");
    }

    #[tokio::test]
    async fn company_or_filter_scope_drift_cannot_reuse_incremental_authority() {
        let (repository, snapshot_id, company_id, scope) = setup().await;
        let observation = |scope| IncrementalCapabilityObservation {
            scope,
            state: CapabilityState::Supported,
            confidence: EvidenceConfidence::Observed,
            identifier_semantics: ChangeIdentifierSemantics::MonotonicPerObject,
            inclusive_lower_bound_observed: true,
            explicit_source_high_watermark_observed: true,
        };
        let original = repository
            .save_incremental_capability_observation(VerifiedIncrementalCanaryReceipt {
                company_id: company_id.clone(),
                capability_snapshot_id: snapshot_id.clone(),
                canary_contract_version: 1,
                response_sha256: "1".repeat(64),
                observation: observation(scope.clone()),
                observed_at_unix_ms: 2_000,
            })
            .await
            .expect("persist original exact scope");
        let mut changed_filter = scope.clone();
        changed_filter.filters_sha256 = "b".repeat(64);
        let changed = repository
            .save_incremental_capability_observation(VerifiedIncrementalCanaryReceipt {
                company_id: company_id.clone(),
                capability_snapshot_id: snapshot_id.clone(),
                canary_contract_version: 1,
                response_sha256: "2".repeat(64),
                observation: observation(changed_filter),
                observed_at_unix_ms: 2_001,
            })
            .await
            .expect("a new exact filter scope gets separate evidence");
        assert_ne!(
            incremental_scope_sha256(&original.observation.scope).unwrap(),
            incremental_scope_sha256(&changed.observation.scope).unwrap()
        );

        let mut wrong_company = scope;
        wrong_company.company_guid = "different-guid".to_string();
        assert!(matches!(
            repository
                .save_incremental_capability_observation(VerifiedIncrementalCanaryReceipt {
                    company_id,
                    capability_snapshot_id: snapshot_id,
                    canary_contract_version: 1,
                    response_sha256: "3".repeat(64),
                    observation: observation(wrong_company),
                    observed_at_unix_ms: 2_001,
                })
                .await,
            Err(MirrorError::InvalidInput("incremental_company_scope"))
        ));
    }

    #[tokio::test]
    async fn generic_pack_proof_cannot_self_attest_incremental_establishment() {
        let (repository, snapshot_id, company_id, scope) = setup().await;
        let capability = repository
            .save_incremental_capability_observation(VerifiedIncrementalCanaryReceipt {
                company_id: company_id.clone(),
                capability_snapshot_id: snapshot_id.clone(),
                canary_contract_version: 1,
                response_sha256: "1".repeat(64),
                observation: IncrementalCapabilityObservation {
                    scope: scope.clone(),
                    state: CapabilityState::Supported,
                    confidence: EvidenceConfidence::Observed,
                    identifier_semantics: ChangeIdentifierSemantics::MonotonicPerObject,
                    inclusive_lower_bound_observed: true,
                    explicit_source_high_watermark_observed: true,
                },
                observed_at_unix_ms: 2_000,
            })
            .await
            .expect("persist sealed canary evidence");
        let batch_id = repository
            .begin_batch(BeginBatchInput {
                run_id: "incremental-foundation-proof".to_string(),
                capability_snapshot_id: snapshot_id,
                company_id: company_id.clone(),
                pack_id: "core_accounting".to_string(),
                pack_schema_major: 1,
                pack_schema_minor: 0,
                source_transport: "xml_http".to_string(),
                source_release: Some("7.0".to_string()),
                requested_from_yyyymmdd: None,
                requested_to_yyyymmdd: None,
                started_at_unix_ms: 2_100,
            })
            .await
            .expect("begin synthetic verified full snapshot");
        let proof = repository
            .commit_batch(CommitBatchInput::test_only(CommitBatchParts {
                batch_id: batch_id.clone(),
                proof_contract_version: 1,
                outcome: RunOutcome::Completed,
                verification: MirrorVerificationState::Verified,
                completed_at_unix_ms: 3_000,
                record_counts_sha256: None,
                snapshot_sha256: Some("2".repeat(64)),
                expected_checkpoint_before: None,
                checkpoint_after: Some("42".to_string()),
                freshness_target_seconds: 60,
                gap_codes: vec![],
                warning_codes: vec![],
            }))
            .await
            .expect("commit proof-bound full snapshot");
        let scope_sha256 = incremental_scope_sha256(&scope).unwrap();
        let scope_json = serde_json::to_string(&scope).unwrap();

        for watermark in ["01", "18446744073709551616", "42"] {
            let insert = sqlx::query(
                "INSERT INTO tally_incremental_establishment_receipts(\
                   id, scope_sha256, scope_json, capability_observation_id, proof_id, \
                   proof_sha256, batch_id, snapshot_plan_sha256, source_response_sha256, \
                   coverage_manifest_sha256, source_high_watermark_decimal, \
                   max_observed_alter_id_decimal, source_record_count, accepted_record_count, \
                   deduplicated_record_count, numeric_alter_id_count, rejected_record_count, \
                   duplicate_identity_count, missing_identity_count, out_of_scope_record_count, \
                   verifier_contract_version, receipt_sha256, created_at_unix_ms\
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, NULL, \
                   0, 0, 0, 0, 0, 0, 0, 0, 1, ?12, 3000)",
            )
            .bind(Uuid::new_v4().to_string())
            .bind(&scope_sha256)
            .bind(&scope_json)
            .bind(&capability.id)
            .bind(&proof.proof_id)
            .bind(&proof.proof_sha256)
            .bind(&batch_id)
            .bind("3".repeat(64))
            .bind("4".repeat(64))
            .bind("5".repeat(64))
            .bind(watermark)
            .bind("6".repeat(64))
            .execute(&repository.pool)
            .await;
            assert!(
                insert.is_err(),
                "generic proof and caller hashes must never establish authority"
            );
        }
        assert!(repository
            .load_incremental_checkpoint(&scope)
            .await
            .expect("missing authority falls back safely")
            .is_none());
        assert!(sqlx::query(
            "INSERT INTO tally_incremental_checkpoint_heads(\
               scope_sha256, scope_json, establishment_receipt_id, high_watermark_decimal, \
               generation, state, established_at_unix_ms\
             ) VALUES (?1, ?2, 'missing-receipt', '42', 1, 'active', 3000)",
        )
        .bind(scope_sha256)
        .bind(scope_json)
        .execute(&repository.pool)
        .await
        .is_err());
    }
}
