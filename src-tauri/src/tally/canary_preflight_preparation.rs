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
    pub canonical_origin: String,
    pub company: ValidatedCompanyName,
    pub expected_company_guid: String,
    pub ledger_name: ValidatedCanaryLedgerName,
    pub identity_query_sha256: ValidatedIdentityQuerySha256,
    pub binding: ActiveWriteCanaryPayloadBindingInput,
    pub prepared: PreparedFixtureCanary,
}

/// Every source-derived value that can reject preparation is validated before
/// the irreversible one-time reservation is consumed.
struct ValidatedFixedCanary {
    canonical_origin: String,
    company: SyntheticCompany,
    company_name: ValidatedCompanyName,
    expected_company_guid: String,
    ledger_name: ValidatedCanaryLedgerName,
    identity_query_sha256: ValidatedIdentityQuerySha256,
    wire_sha256: String,
    intended_state_sha256: String,
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
    if request.review_commitment_sha256.len() != 64
        || !request
            .review_commitment_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        bail!("sealed_canary_preflight_review_commitment_invalid");
    }
    Ok(())
}

fn validate_fixed_canary(pin: &SnapshotSourcePin) -> Result<ValidatedFixedCanary> {
    let company = SyntheticCompany::new(pin.display_name.clone(), pin.company_guid.clone())?;
    let mutation = fixture_canary_ledger_mutation()?;
    let preview = preview_ledger_import(&company, &[mutation], FIXTURE_CANARY_MAPPING_VERSION)?;
    Ok(ValidatedFixedCanary {
        canonical_origin: pin.canonical_origin.clone(),
        company,
        company_name: ValidatedCompanyName::new(pin.display_name.clone())?,
        expected_company_guid: pin.company_guid.clone(),
        ledger_name: ValidatedCanaryLedgerName::new(FIXTURE_CANARY_LEDGER_NAME)?,
        identity_query_sha256: ValidatedIdentityQuerySha256::new(
            preview.identity_query_digest().as_hex(),
        )?,
        wire_sha256: preview.wire_digest().as_hex().to_owned(),
        intended_state_sha256: preview.intended_state_digest().as_hex().to_owned(),
    })
}

fn source_pin_matches_origin(pin: &SnapshotSourcePin, expected_canonical_origin: &str) -> bool {
    pin.canonical_origin == expected_canonical_origin
}
fn materialize_fixed_preflight(
    request: &PrepareSealedCanaryPreflightRequest,
    fixed_canary: ValidatedFixedCanary,
    reservation: &WriteCanaryReservationRef,
) -> Result<PreparedSealedCanaryPreflight> {
    let authorization = authorize_fixture_canary(FixtureCanaryAuthorizationRequest {
        explicit_opt_in: request.explicit_opt_in,
        synthetic_company_confirmed: request.synthetic_company_confirmed,
        company_guid: fixed_canary.expected_company_guid.clone(),
        backup_guidance_acknowledged: request.backup_guidance_acknowledged,
        review_commitment_sha256: request.review_commitment_sha256.clone(),
        reservation_id: reservation.id.clone(),
        reservation_payload_sha256: reservation.reservation_payload_sha256.clone(),
        approved_wire_sha256: fixed_canary.wire_sha256,
        approved_intended_state_sha256: fixed_canary.intended_state_sha256,
        approved_identity_query_sha256: fixed_canary.identity_query_sha256.as_str().to_owned(),
        idempotency_key: format!("fixture-canary:{}", reservation.id),
    })?;
    let prepared = prepare_fixture_canary_ledger_import(
        fixed_canary.company,
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
        canonical_origin: fixed_canary.canonical_origin,
        company: fixed_canary.company_name,
        expected_company_guid: fixed_canary.expected_company_guid,
        ledger_name: fixed_canary.ledger_name,
        identity_query_sha256: fixed_canary.identity_query_sha256,
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
    expected_canonical_origin: &str,
) -> Result<PreparedSealedCanaryPreflight> {
    validate_operator_assertions(&request)?;
    let pin = repository.snapshot_source_pin(&request.company_id).await?;
    if pin.company_id != request.company_id {
        bail!("sealed_canary_preflight_persisted_company_mismatch");
    }
    if !source_pin_matches_origin(&pin, expected_canonical_origin) {
        bail!("sealed_canary_preflight_origin_mismatch");
    }
    // Validate every caller- and source-dependent fixed payload input before
    // consuming the fixture's irreversible one-time reservation.
    let fixed_canary = validate_fixed_canary(&pin)?;
    let reservation = repository
        .reserve_write_canary(WriteCanaryReservationInput {
            company_id: request.company_id.clone(),
            review_commitment_sha256: request.review_commitment_sha256.clone(),
            reserved_at_unix_ms: Utc::now().timestamp_millis(),
        })
        .await?;
    let preparation = materialize_fixed_preflight(&request, fixed_canary, &reservation)?;
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

    fn request() -> PrepareSealedCanaryPreflightRequest {
        PrepareSealedCanaryPreflightRequest {
            company_id: "synthetic-company".to_owned(),
            review_commitment_sha256: "a".repeat(64),
            explicit_opt_in: true,
            synthetic_company_confirmed: true,
            backup_guidance_acknowledged: true,
        }
    }

    fn pin(company_id: String) -> SnapshotSourcePin {
        SnapshotSourcePin {
            company_id,
            endpoint_id: "synthetic-endpoint".to_owned(),
            canonical_origin: "synthetic-origin".to_owned(),
            display_name: "Synthetic Fixture Company".to_owned(),
            company_guid: "synthetic-company-guid".to_owned(),
        }
    }

    fn reservation() -> WriteCanaryReservationRef {
        WriteCanaryReservationRef {
            id: "synthetic-reservation".to_owned(),
            enrollment_id: "synthetic-enrollment".to_owned(),
            reservation_payload_sha256: "b".repeat(64),
            reserved_at_unix_ms: 1_000,
        }
    }

    #[test]
    fn preparation_derives_binding_only_from_the_fixed_canary() {
        let request = request();
        let pin = pin(request.company_id.clone());
        let fixed_canary = validate_fixed_canary(&pin).expect("fixed source validates");
        let preparation = materialize_fixed_preflight(&request, fixed_canary, &reservation())
            .expect("fixed synthetic fixture must prepare");

        assert_eq!(preparation.canonical_origin, pin.canonical_origin);
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
        assert_eq!(preparation.binding.reservation_id, reservation().id);
        assert_eq!(
            preparation.binding.reservation_payload_sha256,
            reservation().reservation_payload_sha256,
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

    #[test]
    fn source_values_that_cannot_materialize_fail_before_reservation() {
        let request = request();
        let mut oversized_name = pin(request.company_id.clone());
        oversized_name.display_name = "n".repeat(256);
        assert!(validate_fixed_canary(&oversized_name).is_err());

        let mut oversized_guid = pin(request.company_id);
        oversized_guid.company_guid = "g".repeat(256);
        assert!(validate_fixed_canary(&oversized_guid).is_err());
    }

    #[test]
    fn invalid_review_commitment_fails_before_reservation() {
        let mut request = request();
        request.review_commitment_sha256 = "not-a-sha256".to_owned();
        assert!(validate_operator_assertions(&request).is_err());
    }

    #[test]
    fn source_pin_origin_must_match_before_reservation() {
        let source = pin("synthetic-company".to_owned());
        assert!(source_pin_matches_origin(&source, &source.canonical_origin));
        assert!(!source_pin_matches_origin(&source, "http://127.0.0.1:9001"));
    }
}
