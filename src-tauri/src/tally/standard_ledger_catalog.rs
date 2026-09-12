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
    connection::{NativeReportPairDrift, PairedNativeReportResponseFailure},
    runtime::{CompanyIdentityBracketError, TallyRuntime},
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
    // The bounds/malformed split below applies only when a
    // `PairedNativeReportResponseFailure` marks the error chain -- i.e. the
    // failure came from one of the two POST responses the paired catalogue
    // request itself makes. Everything untagged -- the identity bracket
    // around this read, both health checks between and after the pair, and
    // any stage added later -- falls through to the conservative `Transport`
    // code at the bottom of this function. This is inverted from tagging
    // every non-response stage on purpose: enumerating stages to exclude is
    // unbounded, since there is always another stage, while a positive
    // marker on the one response actually being classified means a stage
    // added later inherits the safe code automatically instead of silently
    // inheriting a confidently wrong one.
    if let Some(transport_error) = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<PairedNativeReportResponseFailure>())
        .and_then(PairedNativeReportResponseFailure::transport_error)
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
        // surface as the transport code end to end, through the anyhow chain, whether or
        // not it is tagged as a paired-report response (see the marker-gating test below
        // for the untagged/tagged response-side contrast this end-to-end path exists for).
        assert_eq!(
            classify_runtime_catalogue_error(anyhow::Error::new(
                TallyTransportError::RequestFailed
            )),
            StandardLedgerCatalogReadError::Transport
        );
    }

    /// The whole point of `PairedNativeReportResponseFailure`: the identical
    /// `TallyTransportError` variant must classify differently depending on whether it
    /// is marked as a paired native-report response failure. An untagged transport error
    /// -- what a bracket read or one of the two health checks around the pair now
    /// produces -- proves nothing about the shape of a catalogue response, since it
    /// either predates that response or never touched it, so it must not inherit the
    /// bounds/malformed split; only the marker earns that split. See
    /// `PairedNativeReportResponseFailure` and `classify_runtime_catalogue_error`.
    #[test]
    fn catalogue_response_marker_gates_the_bounds_malformed_split() {
        for variant in [
            TallyTransportError::ResponseTooLarge {
                limit: 1,
                declared_by_peer: true,
            },
            TallyTransportError::InvalidEncoding { code: "test" },
            TallyTransportError::UnsupportedContentEncoding,
        ] {
            // Untagged: what a bracket read or a health-check failure now looks like.
            // Must stay the generic transport code even though the variant matches a
            // response-side split further down.
            assert_eq!(
                classify_runtime_catalogue_error(anyhow::Error::new(variant.clone())),
                StandardLedgerCatalogReadError::Transport,
                "expected an untagged {variant:?} to stay the generic transport code"
            );

            // The same variant, tagged as having come from a paired-report response,
            // gets the finer split.
            let expected = match variant {
                TallyTransportError::ResponseTooLarge { .. } => {
                    StandardLedgerCatalogReadError::BoundsViolation
                }
                TallyTransportError::InvalidEncoding { .. }
                | TallyTransportError::UnsupportedContentEncoding => {
                    StandardLedgerCatalogReadError::MalformedResponse
                }
                _ => unreachable!(),
            };
            let description = format!("{variant:?}");
            let tagged = PairedNativeReportResponseFailure::new(anyhow::Error::new(variant));
            assert_eq!(
                classify_runtime_catalogue_error(anyhow::Error::new(tagged)),
                expected,
                "expected a paired-report-tagged {description} to get its specific code"
            );
        }

        // A genuine request-side variant, tagged as though it were a paired-report
        // response, must still surface as the generic transport code: the marker only
        // gates the split, and `classify_transport_error` itself keeps request-side
        // variants at `Transport` regardless of tagging.
        let tagged_request_failure = PairedNativeReportResponseFailure::new(anyhow::Error::new(
            TallyTransportError::RequestFailed,
        ));
        assert_eq!(
            classify_runtime_catalogue_error(anyhow::Error::new(tagged_request_failure)),
            StandardLedgerCatalogReadError::Transport
        );
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
