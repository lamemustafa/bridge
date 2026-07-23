//! Runtime-feature-only coordinator for the sealed synthetic fixture canary.
//!
//! This is not a Tauri command or UI route. It remains unavailable in default
//! builds and accepts no generic payload or endpoint. When a future reviewed
//! command layer explicitly enables the non-default runtime feature, this
//! coordinator rechecks local admission and delegates exactly once to the
//! sealed runtime. It returns digest-only final-verdict metadata.

use crate::{
    db::tally_mirror::{
        BeginWriteCanaryDispatchInput, BeginWriteCanaryPreflightInput, TallyMirrorRepository,
        WriteCanaryFinalVerdictRef,
    },
    tally::{
        canary_dispatch_admission::{
            admit_sealed_canary_runtime_dispatch, SealedCanaryRuntimeAdmissionRequest,
        },
        canary_preflight::{
            run_sealed_canary_preflight, run_sealed_canary_runtime_dispatch,
            SealedCanaryPreflightRequest, SealedCanaryRuntimeDispatchRequest,
        },
        canary_preflight_preparation::{
            prepare_sealed_canary_preflight, PrepareSealedCanaryPreflightRequest,
            PreparedSealedCanaryPreflight,
        },
        connection::canonical_loopback_origin,
        write_sandbox::PreparedFixtureCanary,
        TallyConfig, TallyRuntime,
    },
};
use anyhow::{bail, Result};
use bridge_tally_protocol::xml_read_profiles::{
    ValidatedCanaryLedgerName, ValidatedCompanyName, ValidatedIdentityQuerySha256,
};
use chrono::Utc;

/// Exact, sealed inputs for the only runtime-capable coordinator. It has no
/// raw XML fields, generic request body, retry settings, or caller-selected
/// dispatch operation.
pub(crate) struct SealedCanaryRuntimeCoordinatorRequest {
    pub company_id: String,
    pub config: TallyConfig,
    pub company: ValidatedCompanyName,
    pub ledger_name: ValidatedCanaryLedgerName,
    pub identity_query_sha256: ValidatedIdentityQuerySha256,
    pub expected_company_guid: String,
    pub dispatch: BeginWriteCanaryDispatchInput,
    pub prepared: PreparedFixtureCanary,
}

/// The only source-derived input accepted by the feature-gated end-to-end
/// coordinator. The fixed canary is created inside the coordinator from the
/// enrolled local company pin; callers cannot supply XML, payload fields,
/// evidence digests, an endpoint override, a retry policy, or dispatch data.
pub(crate) struct SealedCanaryRuntimeSequenceRequest {
    pub config: TallyConfig,
    pub preparation: PrepareSealedCanaryPreflightRequest,
}

/// Safe result of an already completed sealed dispatch. It contains neither
/// a Tally response nor a readback, payload, target, or digest value.
pub(crate) struct SealedCanaryRuntimeCoordinatorResult {
    pub final_verdict_id: String,
    pub recorded_at_unix_ms: i64,
}

fn digest_only_result(verdict: WriteCanaryFinalVerdictRef) -> SealedCanaryRuntimeCoordinatorResult {
    SealedCanaryRuntimeCoordinatorResult {
        final_verdict_id: verdict.id,
        recorded_at_unix_ms: verdict.recorded_at_unix_ms,
    }
}

fn validate_runtime_sequence_config(config: &TallyConfig) -> Result<()> {
    canonical_loopback_origin(config).map(|_| ())
}

fn into_preflight_request(
    config: TallyConfig,
    preparation: &PreparedSealedCanaryPreflight,
) -> SealedCanaryPreflightRequest {
    SealedCanaryPreflightRequest {
        config,
        company: preparation.company.clone(),
        ledger_name: preparation.ledger_name.clone(),
        identity_query_sha256: preparation.identity_query_sha256.clone(),
        expected_company_guid: preparation.expected_company_guid.clone(),
        binding: BeginWriteCanaryPreflightInput {
            binding: preparation.binding.clone(),
            started_at_unix_ms: Utc::now().timestamp_millis(),
        },
    }
}

/// Runs the closed synthetic-canary sequence after its non-default feature has
/// been explicitly enabled by a future reviewed command boundary. It validates
/// the loopback endpoint before reserving anything, prepares the fixed canary,
/// performs the exact one-time preflight read, rechecks admission, and then
/// delegates once to the sealed runtime. It never retries a read or import.
///
/// This remains crate-private and is not a Tauri command or UI route.
pub(crate) async fn run_sealed_canary_runtime_sequence(
    repository: &TallyMirrorRepository,
    runtime: &TallyRuntime,
    request: SealedCanaryRuntimeSequenceRequest,
) -> Result<SealedCanaryRuntimeCoordinatorResult> {
    let canonical_origin = canonical_loopback_origin(&request.config)?;
    let preparation =
        prepare_sealed_canary_preflight(repository, request.preparation, &canonical_origin).await?;
    if preparation.canonical_origin != canonical_origin {
        bail!("sealed_canary_runtime_sequence_origin_mismatch");
    }
    let preflight = run_sealed_canary_preflight(
        repository,
        runtime,
        into_preflight_request(request.config.clone(), &preparation),
        &preparation.prepared,
    )
    .await?;
    run_admitted_sealed_canary_runtime_dispatch(
        repository,
        runtime,
        SealedCanaryRuntimeCoordinatorRequest {
            company_id: preparation.binding.company_id,
            config: request.config,
            company: preparation.company,
            ledger_name: preparation.ledger_name,
            identity_query_sha256: preparation.identity_query_sha256,
            expected_company_guid: preparation.expected_company_guid,
            dispatch: BeginWriteCanaryDispatchInput {
                evidence: preflight.active_evidence,
                claimed_at_unix_ms: Utc::now().timestamp_millis(),
            },
            prepared: preparation.prepared,
        },
    )
    .await
}

/// Rechecks local admission immediately before calling the sealed one-send
/// runtime. The sealed runtime owns the durable claim, single import, exact
/// readback, and digest-only verdict record; this coordinator adds no retry or
/// recovery send behavior.
pub(crate) async fn run_admitted_sealed_canary_runtime_dispatch(
    repository: &TallyMirrorRepository,
    runtime: &TallyRuntime,
    request: SealedCanaryRuntimeCoordinatorRequest,
) -> Result<SealedCanaryRuntimeCoordinatorResult> {
    let admission = admit_sealed_canary_runtime_dispatch(
        repository,
        SealedCanaryRuntimeAdmissionRequest {
            company_id: request.company_id,
            ledger_name: request.ledger_name.clone(),
            identity_query_sha256: request.identity_query_sha256.clone(),
            evidence: request.dispatch.evidence.clone(),
        },
        &request.prepared,
    )
    .await?;
    if admission.preflight_evidence.id != request.dispatch.evidence.evidence_id
        || admission.preflight_evidence.attempt_id != request.dispatch.evidence.attempt_id
    {
        bail!("sealed_canary_runtime_coordinator_admission_mismatch");
    }

    let verdict = run_sealed_canary_runtime_dispatch(
        repository,
        runtime,
        SealedCanaryRuntimeDispatchRequest {
            config: request.config,
            company: request.company,
            ledger_name: request.ledger_name,
            identity_query_sha256: request.identity_query_sha256,
            expected_company_guid: request.expected_company_guid,
            dispatch: request.dispatch,
        },
        request.prepared,
    )
    .await?;
    Ok(digest_only_result(verdict))
}

#[cfg(test)]
mod tests {
    use super::{digest_only_result, validate_runtime_sequence_config};
    use crate::db::tally_mirror::WriteCanaryFinalVerdictRef;
    use crate::tally::TallyConfig;

    #[test]
    fn coordinator_result_contains_only_final_verdict_metadata() {
        let result = digest_only_result(WriteCanaryFinalVerdictRef {
            id: "synthetic-final-verdict".to_string(),
            dispatch_attempt_id: "synthetic-dispatch-attempt".to_string(),
            recorded_at_unix_ms: 1_000,
        });
        assert_eq!(result.final_verdict_id, "synthetic-final-verdict");
        assert_eq!(result.recorded_at_unix_ms, 1_000);
    }

    #[test]
    fn malformed_loopback_configuration_fails_before_runtime_sequence() {
        assert!(validate_runtime_sequence_config(&TallyConfig {
            host: "example.invalid".to_owned(),
            port: 9000,
        })
        .is_err());
    }
}
