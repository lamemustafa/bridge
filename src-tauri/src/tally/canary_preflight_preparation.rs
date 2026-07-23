//! Private, no-network preparation of the fixed synthetic fixture canary.
//!
//! This module has no Tauri command, runtime, transport, endpoint, retry, XML
//! accessor, or configuration input. It reserves then binds the one fixed
//! canary locally; a future readback or dispatch boundary must be reviewed
//! separately before it can use the resulting durable commitment.

use crate::{
    db::tally_mirror::{
        ActiveWriteCanaryPayloadBindingInput, SnapshotSourcePin, TallyMirrorRepository,
        WriteCanaryPayloadBindingInput, WriteCanaryReservationInput, WriteCanaryReservationRef,
    },
    tally::write_sandbox::{
        authorize_fixture_canary, fixture_canary_ledger_mutation,
        prepare_fixture_canary_ledger_import, preview_ledger_import,
        FixtureCanaryAuthorizationRequest, IdempotencyRegistry, PreparedFixtureCanary,
        SyntheticCompany, FIXTURE_CANARY_LEDGER_NAME, FIXTURE_CANARY_MAPPING_VERSION,
    },
};
use anyhow::{bail, Result};
use bridge_tally_protocol::xml_read_profiles::{
    ValidatedCanaryLedgerName, ValidatedCompanyName, ValidatedIdentityQuerySha256,
};
use chrono::Utc;

/// Operator assertions required before any durable canary preparation work.
///
/// This is crate-private so no UI or command can turn it into a generic import
/// request. The eventual command boundary must construct it from a separately
/// reviewed, explicit user interaction.
pub(crate) struct PrepareSealedCanaryPreflightRequest {
    pub company_id: String,
    pub review_commitment_sha256: String,
    pub explicit_opt_in: bool,
    pub synthetic_company_confirmed: bool,
    pub backup_guidance_acknowledged: bool,
}

/// A private, non-serializable preparation result. The opaque prepared capsule
/// intentionally has no transport or import API in this build.
pub(crate) struct PreparedSealedCanaryPreflight {
    pub company: ValidatedCompanyName,
    pub expected_company_guid: String,
    pub ledger_name: ValidatedCanaryLedgerName,
    pub identity_query_sha256: ValidatedIdentityQuerySha256,
    pub binding: ActiveWriteCanaryPayloadBindingInput,
    pub prepared: PreparedFixtureCanary,
}

fn validate_operator_assertions(request: &PrepareSealedCanaryPreflightRequest) -> Result<()> {
    if !request.explicit_opt_in {
        bail!("sealed_canary_preflight_explicit_opt_in_required");
    }
    if !request.synthetic_company_confirmed {
        bail!("sealed_canary_preflight_synthetic_company_required");
    }
    if !request.backup_guidance_acknowledged {
        bail!("sealed_canary_preflight_backup_acknowledgement_required");
    }
    Ok(())
}

fn materialize_fixed_preflight(
    request: &PrepareSealedCanaryPreflightRequest,
    pin: &SnapshotSourcePin,
    reservation: &WriteCanaryReservationRef,
) -> Result<PreparedSealedCanaryPreflight> {
    if pin.company_id != request.company_id {
        bail!("sealed_canary_preflight_persisted_company_mismatch");
    }

    let company = SyntheticCompany::new(pin.display_name.clone(), pin.company_guid.clone())?;
    let mutation = fixture_canary_ledger_mutation()?;
    let preview = preview_ledger_import(&company, &[mutation], FIXTURE_CANARY_MAPPING_VERSION)?;
    let authorization = authorize_fixture_canary(FixtureCanaryAuthorizationRequest {
        explicit_opt_in: request.explicit_opt_in,
        synthetic_company_confirmed: request.synthetic_company_confirmed,
        company_guid: pin.company_guid.clone(),
        backup_guidance_acknowledged: request.backup_guidance_acknowledged,
        review_commitment_sha256: request.review_commitment_sha256.clone(),
        reservation_id: reservation.id.clone(),
        reservation_payload_sha256: reservation.reservation_payload_sha256.clone(),
        approved_wire_sha256: preview.wire_digest().as_hex().to_owned(),
        approved_intended_state_sha256: preview.intended_state_digest().as_hex().to_owned(),
        approved_identity_query_sha256: preview.identity_query_digest().as_hex().to_owned(),
        idempotency_key: format!("fixture-canary:{}", reservation.id),
    })?;
    let prepared = prepare_fixture_canary_ledger_import(
        company,
        authorization,
        &mut IdempotencyRegistry::default(),
    )?;
    let binding = ActiveWriteCanaryPayloadBindingInput {
        company_id: request.company_id.clone(),
        review_commitment_sha256: request.review_commitment_sha256.clone(),
        reservation_id: reservation.id.clone(),
        reservation_payload_sha256: reservation.reservation_payload_sha256.clone(),
        wire_sha256: prepared.wire_digest().as_hex().to_owned(),
        intended_state_sha256: prepared.intended_state_digest().as_hex().to_owned(),
        identity_query_sha256: prepared.identity_query_digest().as_hex().to_owned(),
    };

    Ok(PreparedSealedCanaryPreflight {
        company: ValidatedCompanyName::new(pin.display_name.clone())?,
        expected_company_guid: pin.company_guid.clone(),
        ledger_name: ValidatedCanaryLedgerName::new(FIXTURE_CANARY_LEDGER_NAME)?,
        identity_query_sha256: ValidatedIdentityQuerySha256::new(
            binding.identity_query_sha256.clone(),
        )?,
        binding,
        prepared,
    })
}

/// Reserves and binds the fixed canary for a previously enrolled synthetic
/// fixture. It performs no Tally interaction: both repository calls are local
/// durable-state operations, and the returned capsule remains private.
pub(crate) async fn prepare_sealed_canary_preflight(
    repository: &TallyMirrorRepository,
    request: PrepareSealedCanaryPreflightRequest,
) -> Result<PreparedSealedCanaryPreflight> {
    validate_operator_assertions(&request)?;
    let pin = repository.snapshot_source_pin(&request.company_id).await?;
    let reservation = repository
        .reserve_write_canary(WriteCanaryReservationInput {
            company_id: request.company_id.clone(),
            review_commitment_sha256: request.review_commitment_sha256.clone(),
            reserved_at_unix_ms: Utc::now().timestamp_millis(),
        })
        .await?;
    let preparation = materialize_fixed_preflight(&request, &pin, &reservation)?;
    repository
        .bind_write_canary_payload(WriteCanaryPayloadBindingInput {
            company_id: preparation.binding.company_id.clone(),
            review_commitment_sha256: preparation.binding.review_commitment_sha256.clone(),
            reservation_id: preparation.binding.reservation_id.clone(),
            reservation_payload_sha256: preparation.binding.reservation_payload_sha256.clone(),
            wire_sha256: preparation.binding.wire_sha256.clone(),
            intended_state_sha256: preparation.binding.intended_state_sha256.clone(),
            identity_query_sha256: preparation.binding.identity_query_sha256.clone(),
            bound_at_unix_ms: Utc::now().timestamp_millis(),
        })
        .await?;

    Ok(preparation)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preparation_derives_binding_only_from_the_fixed_canary() {
        let request = PrepareSealedCanaryPreflightRequest {
            company_id: "synthetic-company".to_owned(),
            review_commitment_sha256: "a".repeat(64),
            explicit_opt_in: true,
            synthetic_company_confirmed: true,
            backup_guidance_acknowledged: true,
        };
        let pin = SnapshotSourcePin {
            company_id: request.company_id.clone(),
            endpoint_id: "synthetic-endpoint".to_owned(),
            canonical_origin: "synthetic-origin".to_owned(),
            display_name: "Synthetic Fixture Company".to_owned(),
            company_guid: "synthetic-company-guid".to_owned(),
        };
        let reservation = WriteCanaryReservationRef {
            id: "synthetic-reservation".to_owned(),
            enrollment_id: "synthetic-enrollment".to_owned(),
            reservation_payload_sha256: "b".repeat(64),
            reserved_at_unix_ms: 1_000,
        };

        let preparation = materialize_fixed_preflight(&request, &pin, &reservation)
            .expect("fixed synthetic fixture must prepare");

        assert_eq!(preparation.company.as_str(), pin.display_name);
        assert_eq!(preparation.expected_company_guid, pin.company_guid);
        assert_eq!(preparation.ledger_name.as_str(), FIXTURE_CANARY_LEDGER_NAME);
        assert_eq!(
            preparation.identity_query_sha256.as_str(),
            preparation.prepared.identity_query_digest().as_hex(),
        );
        assert_eq!(preparation.binding.company_id, request.company_id);
        assert_eq!(
            preparation.binding.review_commitment_sha256,
            request.review_commitment_sha256,
        );
        assert_eq!(preparation.binding.reservation_id, reservation.id);
        assert_eq!(
            preparation.binding.reservation_payload_sha256,
            reservation.reservation_payload_sha256,
        );
        assert_eq!(
            preparation.binding.wire_sha256,
            preparation.prepared.wire_digest().as_hex(),
        );
        assert_eq!(
            preparation.binding.intended_state_sha256,
            preparation.prepared.intended_state_digest().as_hex(),
        );
    }
}
