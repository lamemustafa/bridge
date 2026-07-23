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
        connection::canonical_loopback_origin,
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

fn validate_preflight_config(config: &TallyConfig) -> Result<()> {
    canonical_loopback_origin(config).map(|_| ())
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
    // Reject malformed or non-loopback configuration before consuming the
    // irreversible one-time preflight claim.
    validate_preflight_config(&request.config)?;
    let PreparedSealedCanaryPreflight {
        company,
        expected_company_guid,
        ledger_name,
        identity_query_sha256,
        binding,
        prepared,
        ..
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
    Ok(digest_only_result(evidence.evidence))
}

#[cfg(test)]
mod tests {
    use super::{digest_only_result, validate_preflight_config};
    use crate::db::tally_mirror::WriteCanaryPreflightEvidenceRef;
    use crate::tally::TallyConfig;

    #[test]
    fn malformed_loopback_configuration_fails_before_preflight_claim() {
        assert!(validate_preflight_config(&TallyConfig {
            host: "example.invalid".to_owned(),
            port: 9000,
        })
        .is_err());
    }
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
