//! Sealed, read-only orchestration for the first synthetic write canary.
//!
//! This module deliberately stops after recording preflight evidence. It has
//! no import request, retry, receipt, or dispatch API.

use anyhow::{bail, Result};
use bridge_tally_protocol::xml_read_profiles::{
    ValidatedCanaryLedgerName, ValidatedCompanyName, ValidatedIdentityQuerySha256,
};
use bridge_tally_write::{
    verify_fixture_canary_preflight, FixtureCanaryPreflightEvidence, PreparedFixtureCanary,
    FIXTURE_CANARY_LEDGER_NAME,
};
use chrono::Utc;

use crate::{
    db::tally_mirror::{
        ActiveWriteCanaryPayloadBindingInput, ActiveWriteCanaryPreflightEvidenceInput,
        BeginWriteCanaryDispatchInput, BeginWriteCanaryPreflightInput, TallyMirrorRepository,
        WriteCanaryDispatchAttemptRef, WriteCanaryPreflightEvidenceInput,
        WriteCanaryPreflightEvidenceRef,
    },
    tally::{TallyConfig, TallyRuntime},
};

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

/// The final local, no-send claim before a future import coordinator is even
/// considered. This remains deliberately transport-free.
pub(crate) struct SealedCanaryDispatchClaimRequest {
    pub ledger_name: ValidatedCanaryLedgerName,
    pub identity_query_sha256: ValidatedIdentityQuerySha256,
    pub dispatch: BeginWriteCanaryDispatchInput,
}

/// Claims the durable preflight slot, performs exactly one sealed read, and
/// persists only the resulting digests. Any failure leaves the claim consumed
/// and cannot progress to a write.
pub(crate) async fn run_sealed_canary_preflight(
    repository: &TallyMirrorRepository,
    runtime: &TallyRuntime,
    request: SealedCanaryPreflightRequest,
    prepared: &PreparedFixtureCanary,
) -> Result<WriteCanaryPreflightEvidenceRef> {
    let binding = &request.binding.binding;
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
    Ok(repository
        .record_write_canary_preflight_evidence(WriteCanaryPreflightEvidenceInput {
            attempt_id: attempt.id,
            readback_state_sha256: evidence.readback_state_digest().as_hex().to_owned(),
            identity_coverage_sha256: evidence.identity_coverage_digest().as_hex().to_owned(),
            verified_at_unix_ms: Utc::now().timestamp_millis(),
        })
        .await?)
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
