//! Runtime-feature-only coordinator for the sealed synthetic fixture canary.
//!
//! This is not a Tauri command or UI route. It remains unavailable in default
//! builds and accepts no generic payload or endpoint. When a future reviewed
//! command layer explicitly enables the non-default runtime feature, this
//! coordinator rechecks local admission and delegates exactly once to the
//! sealed runtime. It returns digest-only final-verdict metadata.

use crate::{
    db::tally_mirror::{
        BeginWriteCanaryDispatchInput, TallyMirrorRepository, WriteCanaryFinalVerdictRef,
    },
    tally::{
        canary_dispatch_admission::{
            admit_sealed_canary_runtime_dispatch, SealedCanaryRuntimeAdmissionRequest,
        },
        canary_preflight::{
            run_sealed_canary_runtime_dispatch, SealedCanaryRuntimeDispatchRequest,
        },
        write_sandbox::PreparedFixtureCanary,
        TallyConfig, TallyRuntime,
    },
};
use anyhow::{bail, Result};
use bridge_tally_protocol::xml_read_profiles::{
    ValidatedCanaryLedgerName, ValidatedCompanyName, ValidatedIdentityQuerySha256,
};

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
    use super::digest_only_result;
    use crate::db::tally_mirror::WriteCanaryFinalVerdictRef;

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
}
