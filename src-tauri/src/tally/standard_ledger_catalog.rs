//! Shared StandardLedgerCatalogV1 profile contract and guarded desktop read.
//!
//! The returned catalog retains ledger GUIDs only in memory. Callers may expose
//! names, but must never serialize the catalog or a binding directly.

use sha2::{Digest, Sha256};

use bridge_tally_protocol::{
    parse_standard_ledger_catalog_with_identities,
    xml_read_profiles::{ReadOnlyProfile, ValidatedCompanyName},
    StandardLedgerCatalog, StandardLedgerCatalogError,
};
use bridge_tally_transport::TallyTransportError;

use super::{
    agent_read_request::AgentReadRequest,
    connection::NativeReportPairDrift,
    runtime::{BracketStageTransportFailure, CompanyIdentityBracketError, TallyRuntime},
    TallyConfig, VerifiedCompanyIdentity,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StandardLedgerCatalogReadError {
    Transport,
    UnstableResponse,
    CompanyIdentityMismatch,
    DuplicateIdentity,
    BoundsViolation,
    MalformedResponse,
}

impl StandardLedgerCatalogReadError {
    pub(crate) const fn command_code(self) -> &'static str {
        match self {
            Self::Transport => "source_draft_catalogue_transport_failed",
            Self::UnstableResponse => "source_draft_catalogue_unstable",
            Self::CompanyIdentityMismatch => "source_draft_catalogue_identity_mismatch",
            Self::DuplicateIdentity => "source_draft_catalogue_duplicate_identity",
            Self::BoundsViolation => "source_draft_catalogue_bounds_invalid",
            Self::MalformedResponse => "source_draft_catalogue_malformed_response",
        }
    }
}

impl From<StandardLedgerCatalogError> for StandardLedgerCatalogReadError {
    fn from(error: StandardLedgerCatalogError) -> Self {
        match error {
            StandardLedgerCatalogError::MalformedResponse => Self::MalformedResponse,
            StandardLedgerCatalogError::CompanyIdentityMismatch => Self::CompanyIdentityMismatch,
            StandardLedgerCatalogError::DuplicateIdentity => Self::DuplicateIdentity,
            StandardLedgerCatalogError::BoundsViolation => Self::BoundsViolation,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct StandardLedgerCatalogRead {
    /// The parsed catalogue. The unparsed body is deliberately not retained:
    /// holding both invites a caller to reparse what this read already parsed.
    pub(crate) catalog: StandardLedgerCatalog,
    pub(crate) request_sha256: String,
    pub(crate) response_sha256: String,
    /// Both accepted, byte-identical paired catalogue bodies. Identity and
    /// health brackets are guards, not source bodies in this commitment.
    pub(crate) bytes: usize,
}

pub(crate) fn render_standard_ledger_catalog_request(company_name: &str) -> anyhow::Result<String> {
    let company = ValidatedCompanyName::new(company_name.to_owned())?;
    Ok(ReadOnlyProfile::StandardLedgerCatalogV1 { company: &company }.render())
}

pub(crate) fn admit_standard_ledger_catalog_request(
    request_xml: String,
) -> Result<AgentReadRequest, super::agent_read_request::AgentReadRequestError> {
    AgentReadRequest::parse(request_xml)
}

pub(crate) fn parse_standard_ledger_catalog_response(
    response_xml: &str,
    expected_company_name: &str,
    expected_company_guid: &str,
) -> Result<StandardLedgerCatalog, StandardLedgerCatalogError> {
    parse_standard_ledger_catalog_with_identities(
        response_xml,
        expected_company_name,
        expected_company_guid,
    )
}

pub(crate) async fn read_standard_ledger_catalog(
    runtime: &TallyRuntime,
    config: TallyConfig,
    identity: &VerifiedCompanyIdentity,
) -> Result<StandardLedgerCatalogRead, StandardLedgerCatalogReadError> {
    let request_xml = render_standard_ledger_catalog_request(identity.display_name())
        .map_err(|_| StandardLedgerCatalogReadError::BoundsViolation)?;
    let request = admit_standard_ledger_catalog_request(request_xml.clone())
        .map_err(|_| StandardLedgerCatalogReadError::MalformedResponse)?;
    let response = runtime
        .fetch_agent_read(config, identity, request)
        .await
        .map_err(classify_runtime_catalogue_error)?;
    let catalog = parse_standard_ledger_catalog_response(
        &response.body,
        identity.display_name(),
        identity.company_guid(),
    )
    .map_err(StandardLedgerCatalogReadError::from)?;
    Ok(StandardLedgerCatalogRead {
        catalog,
        request_sha256: sha256(&bridge_tally_protocol::encode_tally_xml_request_utf16le(
            &request_xml,
        )),
        response_sha256: response.encoded_sha256,
        bytes: response.encoded_bytes.saturating_mul(2),
    })
}

fn classify_runtime_catalogue_error(error: anyhow::Error) -> StandardLedgerCatalogReadError {
    // A `BracketStageTransportFailure` marks a transport fault from the identity
    // bracket around this read, not from the catalogue request itself -- see that
    // type. It must be caught before the variant-only split below, or a bracket
    // failure would inherit `BoundsViolation`/`MalformedResponse` and claim the
    // existing-ledger list is bad when the catalogue request never ran, or already
    // succeeded before the closing bracket failed.
    if error
        .chain()
        .any(|cause| cause.is::<BracketStageTransportFailure>())
    {
        return StandardLedgerCatalogReadError::Transport;
    }
    if let Some(transport_error) = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<TallyTransportError>())
    {
        return classify_transport_error(transport_error);
    }
    if error
        .chain()
        .any(|cause| cause.is::<NativeReportPairDrift>())
    {
        return StandardLedgerCatalogReadError::UnstableResponse;
    }
    if error
        .chain()
        .any(|cause| cause.is::<CompanyIdentityBracketError>())
    {
        return StandardLedgerCatalogReadError::CompanyIdentityMismatch;
    }
    StandardLedgerCatalogReadError::Transport
}

/// `TallyTransportError` is not `#[non_exhaustive]`, so this is written to stay exhaustive on
/// purpose: a variant added later must fail to compile here, not silently inherit `Transport`
/// through a catch-all the way `ResponseTooLarge` and `InvalidEncoding` used to. Those two are
/// response-side validation failures -- Tally was reached and answered, and the answer failed
/// validation -- so telling the operator to check connectivity and retry is wrong; a retry
/// reproduces the same answer. The dividing question is whether retrying could plausibly
/// succeed: `ResponseTruncated` and `ResponseReadFailed` are answers cut short in transit, which
/// a retry may well fix, so they stay `Transport` even though Tally did respond.
fn classify_transport_error(error: &TallyTransportError) -> StandardLedgerCatalogReadError {
    match error {
        TallyTransportError::EndpointInvalid { .. }
        | TallyTransportError::PolicyInvalid { .. }
        | TallyTransportError::ClientInitializationFailed
        | TallyTransportError::RequestTooLarge { .. }
        | TallyTransportError::ConnectionFailed
        | TallyTransportError::RequestTimedOut
        | TallyTransportError::RequestFailed
        | TallyTransportError::HttpStatus { .. }
        | TallyTransportError::ResponseTruncated
        | TallyTransportError::ResponseReadFailed => StandardLedgerCatalogReadError::Transport,
        TallyTransportError::ResponseTooLarge { .. } => {
            StandardLedgerCatalogReadError::BoundsViolation
        }
        // An encoding Bridge cannot decode and one Tally encoded wrongly are the
        // same situation to the operator: a complete answer that cannot be read,
        // reproduced exactly by retrying.
        TallyTransportError::UnsupportedContentEncoding
        | TallyTransportError::InvalidEncoding { .. } => {
            StandardLedgerCatalogReadError::MalformedResponse
        }
    }
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_read_failure_classifier_preserves_runtime_variants() {
        assert_eq!(
            classify_runtime_catalogue_error(anyhow::Error::new(
                TallyTransportError::ConnectionFailed
            )),
            StandardLedgerCatalogReadError::Transport
        );
        assert_eq!(
            classify_runtime_catalogue_error(anyhow::Error::new(NativeReportPairDrift)),
            StandardLedgerCatalogReadError::UnstableResponse
        );
        assert_eq!(
            classify_runtime_catalogue_error(anyhow::Error::new(
                CompanyIdentityBracketError::AbsentOrAmbiguous,
            )),
            StandardLedgerCatalogReadError::CompanyIdentityMismatch
        );
    }

    /// Every `TallyTransportError` variant, named explicitly rather than sampled, so a
    /// variant this test does not know about cannot pass silently -- the match in
    /// `classify_transport_error` would fail to compile first.
    #[test]
    fn catalog_read_failure_classifier_splits_transport_from_response_validation() {
        let request_side = [
            TallyTransportError::EndpointInvalid { code: "test" },
            TallyTransportError::PolicyInvalid { code: "test" },
            TallyTransportError::ClientInitializationFailed,
            TallyTransportError::RequestTooLarge { limit: 1 },
            TallyTransportError::ConnectionFailed,
            TallyTransportError::RequestTimedOut,
            TallyTransportError::RequestFailed,
            TallyTransportError::HttpStatus { status: 500 },
            TallyTransportError::ResponseTruncated,
            TallyTransportError::ResponseReadFailed,
        ];
        for variant in request_side {
            assert_eq!(
                classify_transport_error(&variant),
                StandardLedgerCatalogReadError::Transport,
                "expected {variant:?} to remain the transport code"
            );
        }

        assert_eq!(
            classify_transport_error(&TallyTransportError::ResponseTooLarge {
                limit: 1,
                declared_by_peer: true,
            }),
            StandardLedgerCatalogReadError::BoundsViolation
        );
        // Both encoding faults are complete answers Bridge cannot read, so both are
        // malformed responses rather than outages a retry might clear.
        for variant in [
            TallyTransportError::UnsupportedContentEncoding,
            TallyTransportError::InvalidEncoding { code: "test" },
        ] {
            assert_eq!(
                classify_transport_error(&variant),
                StandardLedgerCatalogReadError::MalformedResponse,
                "expected {variant:?} to report an unusable response, not an outage"
            );
        }

        // A genuine request-side failure -- Tally was never reached at all -- must still
        // surface as the transport code end to end, through the anyhow chain.
        assert_eq!(
            classify_runtime_catalogue_error(anyhow::Error::new(
                TallyTransportError::RequestFailed
            )),
            StandardLedgerCatalogReadError::Transport
        );
        assert_eq!(
            classify_runtime_catalogue_error(anyhow::Error::new(
                TallyTransportError::ResponseTooLarge {
                    limit: 1,
                    declared_by_peer: true,
                }
            )),
            StandardLedgerCatalogReadError::BoundsViolation
        );
        assert_eq!(
            classify_runtime_catalogue_error(anyhow::Error::new(
                TallyTransportError::InvalidEncoding { code: "test" }
            )),
            StandardLedgerCatalogReadError::MalformedResponse
        );
    }

    /// The whole point of `BracketStageTransportFailure`: the identical
    /// `TallyTransportError` variant must classify differently depending on which
    /// stage produced it. A bracket failure proves nothing about the catalogue
    /// response it never received (or received fine, before the closing bracket
    /// failed), so it must not inherit the catalogue request's bounds/malformed
    /// split -- see `BracketStageTransportFailure` and `classify_runtime_catalogue_error`.
    #[test]
    fn bracket_stage_transport_failures_stay_generic_while_catalogue_request_failures_split() {
        for variant in [
            TallyTransportError::ResponseTooLarge {
                limit: 1,
                declared_by_peer: true,
            },
            TallyTransportError::InvalidEncoding { code: "test" },
        ] {
            let bracket_failure =
                BracketStageTransportFailure::new(anyhow::Error::new(variant.clone()));
            assert_eq!(
                classify_runtime_catalogue_error(anyhow::Error::new(bracket_failure)),
                StandardLedgerCatalogReadError::Transport,
                "expected a bracket-stage {variant:?} to stay the generic transport code"
            );

            // The same variant, raised by the catalogue request itself rather than the
            // identity bracket, still gets the finer split.
            let expected = match variant {
                TallyTransportError::ResponseTooLarge { .. } => {
                    StandardLedgerCatalogReadError::BoundsViolation
                }
                TallyTransportError::InvalidEncoding { .. } => {
                    StandardLedgerCatalogReadError::MalformedResponse
                }
                _ => unreachable!(),
            };
            let description = format!("{variant:?}");
            assert_eq!(
                classify_runtime_catalogue_error(anyhow::Error::new(variant)),
                expected,
                "expected a catalogue-request {description} to keep its specific code"
            );
        }
    }

    #[test]
    fn catalog_read_error_codes_are_stable_and_distinct() {
        let errors = [
            StandardLedgerCatalogReadError::Transport,
            StandardLedgerCatalogReadError::UnstableResponse,
            StandardLedgerCatalogReadError::CompanyIdentityMismatch,
            StandardLedgerCatalogReadError::DuplicateIdentity,
            StandardLedgerCatalogReadError::BoundsViolation,
            StandardLedgerCatalogReadError::MalformedResponse,
        ];
        let codes = errors.map(StandardLedgerCatalogReadError::command_code);
        assert_eq!(
            codes,
            [
                "source_draft_catalogue_transport_failed",
                "source_draft_catalogue_unstable",
                "source_draft_catalogue_identity_mismatch",
                "source_draft_catalogue_duplicate_identity",
                "source_draft_catalogue_bounds_invalid",
                "source_draft_catalogue_malformed_response",
            ]
        );
    }
}
