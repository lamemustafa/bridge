//! Private coordinator for the one sealed synthetic-canary preflight read.
//!
//! This has no public Tauri command, UI route, import path, retry, or
//! dispatch capability. It binds an already prepared fixed canary to the
//! existing one-time durable preflight-read claim; no write can occur here.

use crate::{
    db::tally_mirror::{
        BeginWriteCanaryPreflightInput, TallyMirrorRepository, WriteCanaryPreflightEvidenceRef,
    },
    tally::{
        canary_preflight::{run_sealed_canary_preflight, SealedCanaryPreflightRequest},
        canary_preflight_preparation::PreparedSealedCanaryPreflight,
        TallyConfig, TallyRuntime,
    },
};
use anyhow::Result;
use chrono::Utc;

/// The exact local preparation plus validated loopback configuration required to
/// consume the sealed preflight-read claim. The preparation is non-cloneable
/// in practice because its opaque capsule moves into this request.
pub(crate) struct SealedCanaryPreflightReadCoordinatorRequest {
    pub config: TallyConfig,
    pub preparation: PreparedSealedCanaryPreflight,
}

/// Digest-only metadata for a completed exact readback. No XML, payload,
/// target, response, or dispatch authority crosses this boundary.
pub(crate) struct SealedCanaryPreflightReadCoordinatorResult {
    pub evidence_id: String,
    pub verified_at_unix_ms: i64,
}

fn digest_only_result(
    evidence: WriteCanaryPreflightEvidenceRef,
) -> SealedCanaryPreflightReadCoordinatorResult {
    SealedCanaryPreflightReadCoordinatorResult {
        evidence_id: evidence.id,
        verified_at_unix_ms: evidence.verified_at_unix_ms,
    }
}

/// Claims the one durable preflight-read slot and delegates to the sealed
/// readback routine. Any error after the claim is terminal for this fixture;
/// this coordinator deliberately provides neither retry nor dispatch access.
pub(crate) async fn run_prepared_sealed_canary_preflight_read(
    repository: &TallyMirrorRepository,
    runtime: &TallyRuntime,
    request: SealedCanaryPreflightReadCoordinatorRequest,
) -> Result<SealedCanaryPreflightReadCoordinatorResult> {
    let PreparedSealedCanaryPreflight {
        company,
        expected_company_guid,
        ledger_name,
        identity_query_sha256,
        binding,
        prepared,
    } = request.preparation;
    let evidence = run_sealed_canary_preflight(
        repository,
        runtime,
        SealedCanaryPreflightRequest {
            config: request.config,
            company,
            ledger_name,
            identity_query_sha256,
            expected_company_guid,
            binding: BeginWriteCanaryPreflightInput {
                binding,
                started_at_unix_ms: Utc::now().timestamp_millis(),
            },
        },
        &prepared,
    )
    .await?;
    Ok(digest_only_result(evidence))
}

#[cfg(test)]
mod tests {
    use super::digest_only_result;
    use crate::db::tally_mirror::WriteCanaryPreflightEvidenceRef;

    #[test]
    fn coordinator_result_contains_only_evidence_metadata() {
        let result = digest_only_result(WriteCanaryPreflightEvidenceRef {
            id: "synthetic-preflight-evidence".to_owned(),
            attempt_id: "synthetic-preflight-attempt".to_owned(),
            verified_at_unix_ms: 1_000,
        });
        assert_eq!(result.evidence_id, "synthetic-preflight-evidence");
        assert_eq!(result.verified_at_unix_ms, 1_000);
    }
}
