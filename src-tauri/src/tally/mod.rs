pub mod capability_packs;
pub mod connection;
pub mod connector;
#[cfg(feature = "voucher-scan")]
pub(crate) mod outstandings_runtime;
pub mod runtime;
// Crate-internal only: `tally::runtime` is the sole consumer.
mod runtime_control;
pub mod serial_queue;
pub mod tdl_engine;
pub mod validators;
pub mod xml_builder;
pub mod xml_parser;
// Crate-internal only: `tally::connector` and `tally::connection` are the sole consumers.
mod canonical_window;

pub use bridge_tally_core as core;
pub use connection::{
    ConnectionStatus, SelectedReadObservation, SelectedReadScopeEvidence, TallyClient, TallyConfig,
    TallyProbeResult, TallyProduct, SELECTED_LEDGER_QUERY_PROFILE_ID,
    SELECTED_VOUCHER_QUERY_PROFILE_ID,
};
pub(crate) use connector::core_snapshot_start_authorized_codes;
pub use connector::{
    company_source_identity, core_snapshot_start_authorized, source_lineage, RuntimeTallyConnector,
};
pub use runtime::{
    CachedProbeReservation, EndpointKey, ExposureDirection, OpenBillRow, OutstandingsAgeingAnchor,
    OutstandingsCurrencyAssertion, OutstandingsLoadResult, OutstandingsPartialReason, TallyRuntime,
    TallySessionSnapshot, TallyTelemetryPreviewExport, UnallocatedParty,
};
pub use xml_parser::{TallyCompany, TallyImportResult, TallyLedger, TallyVoucher};

/// A complete company tuple that a fresh Company collection has matched once.
///
/// The fields are intentionally private: a bare GUID cannot authorize a
/// company-scoped read after a year-end split.
#[derive(Debug, Clone)]
pub struct VerifiedCompanyIdentity {
    display_name: String,
    company_guid: String,
    company_number: String,
    books_from_yyyymmdd: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VerifiedCompanyIdentityError {
    Missing,
    DuplicateTuple,
    DisplayScopeAmbiguous,
}

impl VerifiedCompanyIdentity {
    /// Produces an identity only after the exact observed tuple is unique and
    /// no same-GUID book can collide with Tally's display-name scope.
    pub(crate) fn from_observed_companies(
        display_name: String,
        company_guid: String,
        company_number: String,
        books_from_yyyymmdd: String,
        companies: &[TallyCompany],
    ) -> Result<Self, VerifiedCompanyIdentityError> {
        let identity = Self {
            display_name,
            company_guid,
            company_number,
            books_from_yyyymmdd,
        };
        if companies
            .iter()
            .any(|company| identity.is_presentation_equivalent_guid_sibling(company))
        {
            return Err(VerifiedCompanyIdentityError::DisplayScopeAmbiguous);
        }
        match companies
            .iter()
            .filter(|company| identity.matches_observed_company(company))
            .count()
        {
            0 => Err(VerifiedCompanyIdentityError::Missing),
            1 => Ok(identity),
            _ => Err(VerifiedCompanyIdentityError::DuplicateTuple),
        }
    }

    #[cfg(test)]
    pub(crate) fn test_fixture(
        display_name: impl Into<String>,
        company_guid: impl Into<String>,
    ) -> Self {
        Self {
            display_name: display_name.into(),
            company_guid: company_guid.into(),
            company_number: "1".to_string(),
            books_from_yyyymmdd: "20260401".to_string(),
        }
    }

    #[cfg(feature = "live-calibration-harness")]
    pub fn live_calibration_harness_identity(
        display_name: impl Into<String>,
        company_guid: impl Into<String>,
    ) -> Self {
        let display_name = display_name.into();
        let company_guid = company_guid.into();
        Self::from_observed_companies(
            display_name.clone(),
            company_guid.clone(),
            "1".to_string(),
            "20260401".to_string(),
            &[TallyCompany {
                name: display_name,
                guid: Some(company_guid),
                company_number: Some("1".to_string()),
                books_from: Some("20260401".to_string()),
            }],
        )
        .expect("the fixed calibration fixture is one complete company tuple")
    }

    pub(crate) fn display_name(&self) -> &str {
        &self.display_name
    }

    pub(crate) fn company_guid(&self) -> &str {
        &self.company_guid
    }

    pub(crate) fn matches_observed_company(&self, company: &TallyCompany) -> bool {
        company.name == self.display_name
            && company
                .guid
                .as_deref()
                .is_some_and(|guid| guid.eq_ignore_ascii_case(&self.company_guid))
            && company.company_number.as_deref() == Some(self.company_number.as_str())
            && company.books_from.as_deref() == Some(self.books_from_yyyymmdd.as_str())
    }

    pub(crate) fn is_presentation_equivalent_guid_sibling(&self, company: &TallyCompany) -> bool {
        company
            .guid
            .as_deref()
            .is_some_and(|guid| guid.eq_ignore_ascii_case(&self.company_guid))
            && company
                .name
                .trim()
                .eq_ignore_ascii_case(self.display_name.trim())
            && !self.matches_observed_company(company)
    }
}
