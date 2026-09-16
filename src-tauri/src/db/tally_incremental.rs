use sqlx::Row;

use crate::db::tally_mirror::{MirrorError, TallyMirrorRepository};

// This module previously also carried a full incremental-sync capability/checkpoint evidence
// pipeline (`IncrementalScope`, `plan_sync`, checkpoint establishment and invalidation, overlap
// deduplication, and their `tally_mirror` persistence methods), backed by the former
// `bridge-tally-incremental` crate. Folding that crate in and narrowing its visibility away from
// `pub` (2026-08) made the whole pipeline visible to `dead_code` for the first time, and it turned
// out to have zero callers anywhere in the binary outside its own tests: `incremental_readiness`
// is never invoked by any Tauri command (the only live entry point,
// `incremental_foundation_evidence` below, is a read-only count query that does not need it), and
// nothing ever calls `save_incremental_capability_observation` to populate the evidence it would
// read. It was deleted rather than kept behind `#[allow(dead_code)]`: it had no caller anywhere,
// including tests that exercised it for reasons other than testing it, so it met the "no caller
// anywhere" bar for removal rather than the "load-bearing but not yet wired" bar for keeping it.
// If incremental sync is revived, reintroduce the policy module fresh against the live call site
// that will actually invoke it, rather than resurrecting unreachable code.

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct IncrementalFoundationEvidence {
    pub execution_enabled: bool,
    pub affirmative_exact_capability_receipts: i64,
    pub establishment_receipts: i64,
    pub active_checkpoint_heads: i64,
    pub state: &'static str,
    pub fallback_warning_code: &'static str,
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
}

#[cfg(test)]
#[path = "tally_incremental_tests.rs"]
mod tests;
