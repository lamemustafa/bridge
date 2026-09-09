pub(crate) mod agent_read_request;
pub(crate) mod approved_import;
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
    OutstandingsCurrencyAssertion, OutstandingsLoadResult, OutstandingsPartialReason,
    OutstandingsReadStrategy, TallyRuntime, TallySessionSnapshot, TallyTelemetryPreviewExport,
    UnallocatedParty,
};
pub use xml_parser::{TallyCompany, TallyImportResult, TallyLedger, TallyVoucher};

/// A complete company tuple that a fresh Company collection has matched once.
///
/// The fields are intentionally private: a bare GUID cannot authorize a
/// company-scoped read after a year-end split.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedCompanyIdentity {
    display_name: String,
    company_guid: String,
    company_number: ObservedCompanyNumber,
    books_from_yyyymmdd: bridge_tally_core::TallyDate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifiedCompanyIdentityError {
    InvalidCompanyNumber,
    InvalidBooksFrom,
    Missing,
    DuplicateTuple,
    DisplayScopeAmbiguous,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ObservedCompanyNumber(String);

impl ObservedCompanyNumber {
    fn parse(value: String) -> Result<Self, VerifiedCompanyIdentityError> {
        if !validators::is_valid_company_number(&value) {
            return Err(VerifiedCompanyIdentityError::InvalidCompanyNumber);
        }
        Ok(Self(value))
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

impl VerifiedCompanyIdentity {
    /// Produces an identity only after the exact observed tuple is unique and
    /// no same-GUID book can collide with Tally's display-name scope.
    /// Constructs a library-facing read identity from a complete observed
    /// tuple. The exact tuple must occur once and no same-GUID book may share
    /// its display scope; callers cannot turn a bare GUID into a capability.
    pub fn from_observed_companies(
        display_name: String,
        company_guid: String,
        company_number: String,
        books_from_yyyymmdd: String,
        companies: &[TallyCompany],
    ) -> Result<Self, VerifiedCompanyIdentityError> {
        let company_number = ObservedCompanyNumber::parse(company_number)?;
        let books_from_yyyymmdd = bridge_tally_core::TallyDate::parse(books_from_yyyymmdd)
            .map_err(|_| VerifiedCompanyIdentityError::InvalidBooksFrom)?;
        let identity = Self {
            display_name,
            // All company-list and extent comparisons use ASCII-insensitive
            // GUID semantics. Keep the stored value canonical so derived
            // equality preserves that same contract across a read bracket.
            company_guid: company_guid.to_ascii_lowercase(),
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
            company_guid: company_guid.into().to_ascii_lowercase(),
            company_number: ObservedCompanyNumber::parse("1".to_string())
                .expect("fixed fixture company number is valid"),
            books_from_yyyymmdd: bridge_tally_core::TallyDate::parse("20260401")
                .expect("fixed fixture date is valid"),
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

    pub(crate) fn company_number(&self) -> &str {
        self.company_number.as_str()
    }

    pub(crate) fn books_from_yyyymmdd(&self) -> &str {
        self.books_from_yyyymmdd.as_str()
    }

    pub(crate) fn company_book_extent_expectation(
        &self,
    ) -> Result<
        bridge_tally_protocol::outstandings_shared::CompanyBookExtentExpectation,
        bridge_tally_protocol::outstandings_shared::CompanyBookExtentExpectationError,
    > {
        bridge_tally_protocol::outstandings_shared::CompanyBookExtentExpectation::new(
            self.display_name.clone(),
            self.company_guid.clone(),
            self.company_number.as_str().to_owned(),
            self.books_from_yyyymmdd.as_str().to_owned(),
        )
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

#[cfg(test)]
mod tests {
    use super::*;

    const CAPTURED_EXTENT_V2: &str = include_str!(
        "../../crates/bridge-tally-protocol/tests/fixtures/agent/native-company-book-extents-with-number.utf8.xml"
    );
    const CAPTURED_NAME: &str = "BRIDGE PROBE B SANDBOX";
    const CAPTURED_GUID: &str = "ec4454ae-5c4c-4bfa-b3b0-68182a749689";
    const CAPTURED_NUMBER: &str = "100005";
    const CAPTURED_BOOKS_FROM: &str = "20250401";

    fn captured_identity(xml: &str, guid: &str) -> VerifiedCompanyIdentity {
        let companies = bridge_tally_protocol::parse_companies_from_collection(xml)
            .expect("captured CompanyBookExtentV2 response parses as a Company collection");
        VerifiedCompanyIdentity::from_observed_companies(
            CAPTURED_NAME.to_string(),
            guid.to_string(),
            CAPTURED_NUMBER.to_string(),
            CAPTURED_BOOKS_FROM.to_string(),
            &companies,
        )
        .expect("captured full tuple remains uniquely observed")
    }

    #[test]
    fn verified_identity_canonicalizes_captured_guid_case_for_closing_comparison() {
        let target_start = CAPTURED_EXTENT_V2
            .find(&format!(r#"<COMPANY NAME="{CAPTURED_NAME}""#))
            .expect("captured target company row exists");
        let target_end = target_start
            + CAPTURED_EXTENT_V2[target_start..]
                .find("</COMPANY>")
                .expect("captured target company row closes")
            + "</COMPANY>".len();
        let target = &CAPTURED_EXTENT_V2[target_start..target_end];
        let upper_guid = CAPTURED_GUID.to_ascii_uppercase();
        let changed_target = target.replacen(CAPTURED_GUID, &upper_guid, 1);
        assert_ne!(
            changed_target, target,
            "captured GUID case mutation must apply"
        );
        let closing = CAPTURED_EXTENT_V2.replacen(target, &changed_target, 1);
        assert_ne!(
            closing, CAPTURED_EXTENT_V2,
            "captured response mutation must apply"
        );

        assert_eq!(
            captured_identity(CAPTURED_EXTENT_V2, CAPTURED_GUID),
            captured_identity(&closing, &upper_guid),
            "GUID casing alone must not make a verified closing identity drift"
        );
    }
}
