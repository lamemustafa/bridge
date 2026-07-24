//! Sealed orchestration for the first synthetic write canary.
//!
//! Ordinary builds stop after digest-only preflight evidence. The separately
//! disabled runtime feature adds one closed, one-send coordinator with no UI
//! command, generic payload API, or retry behavior.

use crate::tally::write_sandbox::{
    verify_fixture_canary_preflight, FixtureCanaryPostDispatchObservation,
    FixtureCanaryPreflightEvidence, PreparedFixtureCanary, FIXTURE_CANARY_LEDGER_NAME,
};
use anyhow::{bail, Result};
use bridge_tally_protocol::xml_read_profiles::{
    ValidatedCanaryLedgerName, ValidatedCompanyName, ValidatedIdentityQuerySha256,
};
use chrono::Utc;
use sha2::{Digest, Sha256};

#[cfg(feature = "fixture-canary-runtime-dispatch")]
use crate::db::tally_mirror::ActiveWriteCanaryDispatchAttemptInput;

use crate::{
    db::tally_mirror::{
        ActiveWriteCanaryPayloadBindingInput, ActiveWriteCanaryPreflightEvidenceInput,
        BeginWriteCanaryDispatchInput, BeginWriteCanaryPreflightInput, TallyMirrorRepository,
        WriteCanaryDispatchAttemptRef, WriteCanaryFinalVerdictInput, WriteCanaryFinalVerdictRef,
        WriteCanaryPreflightEvidenceInput, WriteCanaryPreflightEvidenceRef,
    },
    tally::{connection::canonical_loopback_origin, TallyConfig, TallyRuntime},
};

fn sealed_target_binding_sha256(
    config: &TallyConfig,
    expected_company_guid: &str,
) -> Result<(String, String)> {
    let endpoint = canonical_loopback_origin(config)?;
    let endpoint_sha256 = sha256_hex(endpoint.as_bytes());
    let company_sha256 = sha256_hex(expected_company_guid.to_ascii_lowercase().as_bytes());
    Ok((endpoint_sha256, company_sha256))
}

fn sha256_hex(value: &[u8]) -> String {
    Sha256::digest(value)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Every value required for one sealed, serial preflight read. This stays
/// crate-private until a separately reviewed command layer exposes it.
pub(crate) struct SealedCanaryPreflightRequest {
    pub config: TallyConfig,
    pub company: ValidatedCompanyName,
    pub ledger_name: ValidatedCanaryLedgerName,
    pub identity_query_sha256: ValidatedIdentityQuerySha256,
    pub expected_company_guid: String,
    pub binding: BeginWriteCanaryPreflightInput,
}

/// Exact, digest-only evidence required before a future canary import may be
/// considered. It intentionally contains no Tally configuration, transport,
/// import payload, retry policy, or dispatch capability.
pub(crate) struct SealedCanaryPreflightEvidenceGateRequest {
    pub ledger_name: ValidatedCanaryLedgerName,
    pub identity_query_sha256: ValidatedIdentityQuerySha256,
    pub evidence: ActiveWriteCanaryPreflightEvidenceInput,
}

/// The private handoff from an exact preflight read to the feature-gated
/// runtime sequence. It retains the complete durable evidence commitment so a
/// later local admission can verify it exactly; it has no import payload,
/// transport handle, or raw Tally response.
pub(crate) struct SealedCanaryPreflightCompletion {
    pub evidence: WriteCanaryPreflightEvidenceRef,
    pub active_evidence: ActiveWriteCanaryPreflightEvidenceInput,
}

/// The final local, no-send claim before a future import coordinator is even
/// considered. This remains deliberately transport-free.
pub(crate) struct SealedCanaryDispatchClaimRequest {
    pub ledger_name: ValidatedCanaryLedgerName,
    pub identity_query_sha256: ValidatedIdentityQuerySha256,
    pub dispatch: BeginWriteCanaryDispatchInput,
}

/// A final local, digest-only record request. The caller supplies only the
/// durable commitments plus an already parsed semantic observation; it has no
/// Tally configuration, raw XML, payload, or transport capability.
pub(crate) struct SealedCanaryFinalVerdictRequest {
    pub ledger_name: ValidatedCanaryLedgerName,
    pub identity_query_sha256: ValidatedIdentityQuerySha256,
    pub verdict: WriteCanaryFinalVerdictInput,
}

/// All inputs needed for the closed, one-send synthetic canary runtime. This
/// is crate-private and feature-gated; there is intentionally no Tauri command
/// or UI route that can invoke it in this change.
#[cfg(feature = "fixture-canary-runtime-dispatch")]
pub(crate) struct SealedCanaryRuntimeDispatchRequest {
    pub config: TallyConfig,
    pub company: ValidatedCompanyName,
    pub ledger_name: ValidatedCanaryLedgerName,
    pub identity_query_sha256: ValidatedIdentityQuerySha256,
    pub expected_company_guid: String,
    pub dispatch: BeginWriteCanaryDispatchInput,
}

/// Terminal truth classification for the one-send sealed runtime. Errors before
/// its durable dispatch claim prove that no import was sent; after that claim a
/// final verdict may be absent even if Tally received the one permitted import.
#[cfg(feature = "fixture-canary-runtime-dispatch")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SealedCanaryRuntimeDispatchError {
    PreDispatch,
    OutcomeUnknown,
}

/// Claims the durable preflight slot, performs exactly one sealed read, and
/// persists only the resulting digests. Any failure leaves the claim consumed
/// and cannot progress to a write.
pub(crate) async fn run_sealed_canary_preflight(
    repository: &TallyMirrorRepository,
    runtime: &TallyRuntime,
    request: SealedCanaryPreflightRequest,
    prepared: &PreparedFixtureCanary,
) -> Result<SealedCanaryPreflightCompletion> {
    let binding = request.binding.binding.clone();
    if binding.wire_sha256 != prepared.wire_digest().as_hex()
        || binding.intended_state_sha256 != prepared.intended_state_digest().as_hex()
        || binding.identity_query_sha256 != prepared.identity_query_digest().as_hex()
        || request.identity_query_sha256.as_str() != prepared.identity_query_digest().as_hex()
        || request.ledger_name.as_str() != FIXTURE_CANARY_LEDGER_NAME
    {
        bail!("sealed_canary_preflight_binding_mismatch");
    }

    let attempt = repository
        .begin_write_canary_preflight(request.binding)
        .await?;
    let (canonical_endpoint_sha256, company_identity_sha256) =
        sealed_target_binding_sha256(&request.config, &request.expected_company_guid)?;
    let readback = runtime
        .fetch_ledger_canary_readback(
            request.config,
            request.company,
            request.ledger_name,
            request.identity_query_sha256,
            request.expected_company_guid,
        )
        .await?;
    let evidence: FixtureCanaryPreflightEvidence =
        verify_fixture_canary_preflight(prepared, readback.as_xml())?;
    let active_evidence = ActiveWriteCanaryPreflightEvidenceInput {
        binding,
        attempt_id: attempt.id.clone(),
        evidence_id: String::new(),
        readback_state_sha256: evidence.readback_state_digest().as_hex().to_owned(),
        identity_coverage_sha256: evidence.identity_coverage_digest().as_hex().to_owned(),
        canonical_endpoint_sha256,
        company_identity_sha256,
    };
    let persisted = repository
        .record_write_canary_preflight_evidence(WriteCanaryPreflightEvidenceInput {
            attempt_id: attempt.id,
            readback_state_sha256: active_evidence.readback_state_sha256.clone(),
            identity_coverage_sha256: active_evidence.identity_coverage_sha256.clone(),
            canonical_endpoint_sha256: active_evidence.canonical_endpoint_sha256.clone(),
            company_identity_sha256: active_evidence.company_identity_sha256.clone(),
            verified_at_unix_ms: Utc::now().timestamp_millis(),
        })
        .await?;
    Ok(SealedCanaryPreflightCompletion {
        active_evidence: ActiveWriteCanaryPreflightEvidenceInput {
            evidence_id: persisted.id.clone(),
            ..active_evidence
        },
        evidence: persisted,
    })
}

/// Rechecks that the supplied immutable preflight evidence is still bound to
/// the exact prepared fixture canary and active enrollment. This is a
/// read-only gate for a future separately reviewed import coordinator; it
/// cannot contact Tally or construct an import request.
pub(crate) async fn verify_sealed_canary_preflight_evidence(
    repository: &TallyMirrorRepository,
    request: SealedCanaryPreflightEvidenceGateRequest,
    prepared: &PreparedFixtureCanary,
) -> Result<WriteCanaryPreflightEvidenceRef> {
    let binding: &ActiveWriteCanaryPayloadBindingInput = &request.evidence.binding;
    if binding.wire_sha256 != prepared.wire_digest().as_hex()
        || binding.intended_state_sha256 != prepared.intended_state_digest().as_hex()
        || binding.identity_query_sha256 != prepared.identity_query_digest().as_hex()
        || request.identity_query_sha256.as_str() != prepared.identity_query_digest().as_hex()
        || request.ledger_name.as_str() != FIXTURE_CANARY_LEDGER_NAME
    {
        bail!("sealed_canary_preflight_evidence_binding_mismatch");
    }

    Ok(repository
        .active_write_canary_preflight_evidence(request.evidence)
        .await?)
}

/// Consumes one immutable, evidence-gated dispatch claim. It has no Tally
/// configuration or payload and therefore cannot create a request or write.
pub(crate) async fn claim_sealed_canary_dispatch(
    repository: &TallyMirrorRepository,
    request: SealedCanaryDispatchClaimRequest,
    prepared: &PreparedFixtureCanary,
) -> Result<WriteCanaryDispatchAttemptRef> {
    let binding = &request.dispatch.evidence.binding;
    if binding.wire_sha256 != prepared.wire_digest().as_hex()
        || binding.intended_state_sha256 != prepared.intended_state_digest().as_hex()
        || binding.identity_query_sha256 != prepared.identity_query_digest().as_hex()
        || request.identity_query_sha256.as_str() != prepared.identity_query_digest().as_hex()
        || request.ledger_name.as_str() != FIXTURE_CANARY_LEDGER_NAME
    {
        bail!("sealed_canary_dispatch_claim_binding_mismatch");
    }
    Ok(repository
        .begin_write_canary_dispatch_attempt(request.dispatch)
        .await?)
}

/// Validates the target commitment, claims the one durable dispatch slot, then
/// acquires an exclusive lease and revalidates the pinned GUID immediately
/// before the sole import. That lease remains held through the final readback,
/// so no Bridge read can interleave between identity verification and import.
/// There is no retry or resend. Errors through the claim are pre-dispatch;
/// every later failure is an unknown outcome.
#[cfg(feature = "fixture-canary-runtime-dispatch")]
pub(crate) async fn run_sealed_canary_runtime_dispatch(
    repository: &TallyMirrorRepository,
    runtime: &TallyRuntime,
    request: SealedCanaryRuntimeDispatchRequest,
    prepared: PreparedFixtureCanary,
) -> std::result::Result<WriteCanaryFinalVerdictRef, SealedCanaryRuntimeDispatchError> {
    let (canonical_endpoint_sha256, company_identity_sha256) =
        sealed_target_binding_sha256(&request.config, &request.expected_company_guid)
            .map_err(|_| SealedCanaryRuntimeDispatchError::PreDispatch)?;
    if request.dispatch.evidence.canonical_endpoint_sha256 != canonical_endpoint_sha256
        || request.dispatch.evidence.company_identity_sha256 != company_identity_sha256
    {
        return Err(SealedCanaryRuntimeDispatchError::PreDispatch);
    }
    let dispatch_claim = claim_sealed_canary_dispatch(
        repository,
        SealedCanaryDispatchClaimRequest {
            ledger_name: request.ledger_name.clone(),
            identity_query_sha256: request.identity_query_sha256.clone(),
            dispatch: request.dispatch.clone(),
        },
        &prepared,
    )
    .await
    .map_err(|_| SealedCanaryRuntimeDispatchError::PreDispatch)?;

    let dispatch_lease = runtime
        .begin_verified_canary_dispatch(
            request.config.clone(),
            request.company.clone(),
            request.ledger_name.clone(),
            request.identity_query_sha256.clone(),
            request.expected_company_guid.clone(),
        )
        .await
        .map_err(|_| SealedCanaryRuntimeDispatchError::OutcomeUnknown)?;
    let receipt = runtime
        .dispatch_fixture_canary_once_under_dispatch(
            &dispatch_lease,
            &request.config,
            prepared
                .seal_for_dispatch()
                .map_err(|_| SealedCanaryRuntimeDispatchError::OutcomeUnknown)?,
        )
        .await
        .map_err(|_| SealedCanaryRuntimeDispatchError::OutcomeUnknown)?;
    receipt
        .validate_receipt()
        .map_err(|_| SealedCanaryRuntimeDispatchError::OutcomeUnknown)?;
    let readback = runtime
        .fetch_ledger_canary_readback_under_dispatch(
            &dispatch_lease,
            request.config,
            request.company,
            request.ledger_name,
            request.identity_query_sha256,
            request.expected_company_guid,
        )
        .await
        .map_err(|_| SealedCanaryRuntimeDispatchError::OutcomeUnknown)?;
    let observation = receipt
        .observe_with_readback(readback.as_xml())
        .map_err(|_| SealedCanaryRuntimeDispatchError::OutcomeUnknown)?;
    let verdict = WriteCanaryFinalVerdictInput {
        dispatch: ActiveWriteCanaryDispatchAttemptInput {
            evidence: request.dispatch.evidence,
            dispatch_attempt_id: dispatch_claim.id,
            claimed_at_unix_ms: dispatch_claim.claimed_at_unix_ms,
        },
        import_response_sha256: observation.import_response_digest().as_hex().to_owned(),
        readback_state_sha256: observation.readback_state_digest().as_hex().to_owned(),
        identity_coverage_sha256: observation.identity_coverage_digest().as_hex().to_owned(),
        recorded_at_unix_ms: Utc::now().timestamp_millis(),
    };
    repository
        .record_write_canary_final_verdict(verdict)
        .await
        .map_err(|_| SealedCanaryRuntimeDispatchError::OutcomeUnknown)
}

/// Correlates an exact portable observation to one durable dispatch claim and
/// stores only its digests. This coordinator cannot make a Tally request: a
/// separately reviewed dispatch path must create the observation first.
pub(crate) async fn record_sealed_canary_final_verdict(
    repository: &TallyMirrorRepository,
    request: SealedCanaryFinalVerdictRequest,
    prepared: &PreparedFixtureCanary,
    observation: &FixtureCanaryPostDispatchObservation,
) -> Result<WriteCanaryFinalVerdictRef> {
    let binding = &request.verdict.dispatch.evidence.binding;
    if binding.wire_sha256 != prepared.wire_digest().as_hex()
        || binding.intended_state_sha256 != prepared.intended_state_digest().as_hex()
        || binding.identity_query_sha256 != prepared.identity_query_digest().as_hex()
        || request.identity_query_sha256.as_str() != prepared.identity_query_digest().as_hex()
        || request.ledger_name.as_str() != FIXTURE_CANARY_LEDGER_NAME
        || request.verdict.import_response_sha256 != observation.import_response_digest().as_hex()
        || request.verdict.readback_state_sha256 != observation.readback_state_digest().as_hex()
        || request.verdict.identity_coverage_sha256
            != observation.identity_coverage_digest().as_hex()
    {
        bail!("sealed_canary_final_verdict_binding_mismatch");
    }
    Ok(repository
        .record_write_canary_final_verdict(request.verdict)
        .await?)
}

#[cfg(test)]
mod tests {
    use super::sealed_target_binding_sha256;
    use crate::tally::TallyConfig;

    #[test]
    fn sealed_target_binding_is_canonical_and_target_specific() {
        let loopback = TallyConfig {
            host: "127.0.0.1".to_string(),
            port: 9000,
        };
        let localhost = TallyConfig {
            host: "localhost".to_string(),
            port: 9000,
        };
        let different_port = TallyConfig {
            host: "127.0.0.1".to_string(),
            port: 9001,
        };

        let binding = sealed_target_binding_sha256(&loopback, "fixture-company-guid")
            .expect("hash synthetic target binding");
        assert_eq!(
            binding,
            sealed_target_binding_sha256(&localhost, "FIXTURE-COMPANY-GUID")
                .expect("canonical aliases and GUID case share one binding")
        );
        assert_ne!(
            binding,
            sealed_target_binding_sha256(&different_port, "fixture-company-guid")
                .expect("a changed endpoint must not share authority")
        );
        assert_ne!(
            binding,
            sealed_target_binding_sha256(&loopback, "another-fixture-company-guid")
                .expect("a changed company must not share authority")
        );
    }
}
