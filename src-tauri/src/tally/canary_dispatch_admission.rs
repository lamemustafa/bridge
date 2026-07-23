//! Disabled application-level admission for the sealed fixture-canary runtime.
//!
//! This module is deliberately not a Tauri command and contains no endpoint,
//! runtime, transport, import, payload-construction, persistence, or retry
//! capability. It only checks that an already prepared synthetic fixture is
//! tied to active, exact durable preflight evidence and an active reviewed
//! fixture enrollment. A future dispatch boundary must remain separately
//! reviewed and revalidate every condition immediately before any send.

use crate::{
    db::tally_mirror::{
        ActiveWriteCanaryPreflightEvidenceInput, TallyMirrorRepository,
        WriteCanaryPreflightEvidenceRef, WriteFixtureEnrollmentStatus,
    },
    tally::{
        canary_preflight::{
            verify_sealed_canary_preflight_evidence, SealedCanaryPreflightEvidenceGateRequest,
        },
        write_sandbox::PreparedFixtureCanary,
    },
};
use anyhow::{bail, Result};
use bridge_tally_protocol::xml_read_profiles::{
    ValidatedCanaryLedgerName, ValidatedIdentityQuerySha256,
};

/// Local-only inputs for the disabled admission seam. The caller supplies
/// immutable evidence previously committed by the sealed preflight; this type
/// deliberately has no Tally configuration, company display data, XML, or
/// dispatch/payload material.
pub(crate) struct SealedCanaryRuntimeAdmissionRequest {
    pub company_id: String,
    pub ledger_name: ValidatedCanaryLedgerName,
    pub identity_query_sha256: ValidatedIdentityQuerySha256,
    pub evidence: ActiveWriteCanaryPreflightEvidenceInput,
}

/// Read-only admission evidence. This is not a dispatch token and cannot be
/// used to construct, send, repeat, or record a Tally import.
pub(crate) struct SealedCanaryRuntimeAdmission {
    pub preflight_evidence: WriteCanaryPreflightEvidenceRef,
}

fn has_active_reviewed_fixture_enrollment(status: &WriteFixtureEnrollmentStatus) -> bool {
    status.fixture_state == "active" && status.candidate_gate == "enrolled"
}

/// Confirms active reviewed enrollment and exact durable preflight evidence
/// for the already prepared synthetic fixture. This performs database reads
/// only; the future runtime-dispatch path must repeat its own final checks
/// before it is allowed to contact Tally.
pub(crate) async fn admit_sealed_canary_runtime_dispatch(
    repository: &TallyMirrorRepository,
    request: SealedCanaryRuntimeAdmissionRequest,
    prepared: &PreparedFixtureCanary,
) -> Result<SealedCanaryRuntimeAdmission> {
    if request.company_id != request.evidence.binding.company_id {
        bail!("sealed_canary_runtime_admission_company_mismatch");
    }

    let enrollment = repository
        .write_fixture_enrollment_status(&request.company_id)
        .await?;
    if !has_active_reviewed_fixture_enrollment(&enrollment) {
        bail!("sealed_canary_runtime_admission_not_enrolled");
    }

    let evidence_id = request.evidence.evidence_id.clone();
    let attempt_id = request.evidence.attempt_id.clone();
    let preflight_evidence = verify_sealed_canary_preflight_evidence(
        repository,
        SealedCanaryPreflightEvidenceGateRequest {
            ledger_name: request.ledger_name,
            identity_query_sha256: request.identity_query_sha256,
            evidence: request.evidence,
        },
        prepared,
    )
    .await?;
    if preflight_evidence.id != evidence_id || preflight_evidence.attempt_id != attempt_id {
        bail!("sealed_canary_runtime_admission_evidence_mismatch");
    }

    Ok(SealedCanaryRuntimeAdmission { preflight_evidence })
}
#[cfg(test)]
mod tests {
    use super::has_active_reviewed_fixture_enrollment;
    use crate::db::tally_mirror::WriteFixtureEnrollmentStatus;

    fn status(
        fixture_state: &'static str,
        candidate_gate: &'static str,
    ) -> WriteFixtureEnrollmentStatus {
        WriteFixtureEnrollmentStatus {
            fixture_state,
            enrolled_at_unix_ms: None,
            revoked_at_unix_ms: None,
            candidate_gate,
            write_capability: "unknown",
        }
    }

    #[test]
    fn admission_requires_an_exact_active_reviewed_fixture_enrollment() {
        assert!(has_active_reviewed_fixture_enrollment(&status(
            "active", "enrolled"
        )));
        assert!(!has_active_reviewed_fixture_enrollment(&status(
            "not_enrolled",
            "not_enrolled"
        )));
        assert!(!has_active_reviewed_fixture_enrollment(&status(
            "revoked",
            "not_enrolled"
        )));
        assert!(!has_active_reviewed_fixture_enrollment(&status(
            "active",
            "not_enrolled"
        )));
    }
}
