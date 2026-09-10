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
    pub(crate) catalog: StandardLedgerCatalog,
    pub(crate) body: String,
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
        body: response.body,
        request_sha256: sha256(&bridge_tally_protocol::encode_tally_xml_request_utf16le(
            &request_xml,
        )),
        response_sha256: response.encoded_sha256,
        bytes: response.encoded_bytes.saturating_mul(2),
    })
}

fn classify_runtime_catalogue_error(error: anyhow::Error) -> StandardLedgerCatalogReadError {
    if error
        .chain()
        .any(|cause| cause.downcast_ref::<TallyTransportError>().is_some())
    {
        return StandardLedgerCatalogReadError::Transport;
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
