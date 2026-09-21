//! Closed, read-only XML request profiles shared by the native app and portable tools.
//!
//! The public profile API accepts only validated company and date inputs. The
//! compatibility renderers are intentionally hidden from generated
//! documentation; they preserve the native app's existing string-based
//! function signatures while still exposing only fixed export profiles.

use std::fmt;

use sha2::{Digest, Sha256};

#[cfg(feature = "voucher-scan")]
use crate::outstandings::{
    render_empty_partition_witness_template, render_ledger_opening_coverage,
    render_outstandings_template, render_outstandings_vouchers, AlterIdRange, NarrowDateWindow,
};
#[cfg(feature = "voucher-scan")]
use crate::outstandings_shared::PinnedCompany;
use crate::outstandings_shared::{render_company_book_extent, render_company_book_extent_v2};
use crate::{
    encode_tally_xml_request_utf16le, BRIDGE_LEDGER_EXPORT_SCHEMA,
    BRIDGE_LEDGER_WRITE_READBACK_SCHEMA,
};

const TEMPLATE_COMPANY: &str = "BRIDGE TEMPLATE COMPANY";
const TEMPLATE_FROM: &str = "20000101";
const TEMPLATE_TO: &str = "20000102";
#[cfg(feature = "voucher-scan")]
const TEMPLATE_ALTER_ID_START: u64 = 0;
#[cfg(feature = "voucher-scan")]
const TEMPLATE_ALTER_ID_END: u64 = 1;
const TEMPLATE_CANARY_LEDGER: &str = "BRIDGE-CANARY-LEDGER-001";
const BRIDGE_CANARY_LEDGER_PREFIX: &str = "BRIDGE-CANARY-";
const TEMPLATE_IDENTITY_QUERY_SHA256: &str =
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadProfileValidationError {
    CompanyInvalid,
    DateInvalid,
    DateRangeInvalid,
    CanaryLedgerInvalid,
    IdentityQueryInvalid,
}

impl fmt::Display for ReadProfileValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::CompanyInvalid => "read profile company input was invalid",
            Self::DateInvalid => "read profile date input was invalid",
            Self::DateRangeInvalid => "read profile date range was invalid",
            Self::CanaryLedgerInvalid => "read profile canary ledger input was invalid",
            Self::IdentityQueryInvalid => "read profile identity query input was invalid",
        })
    }
}

impl std::error::Error for ReadProfileValidationError {}

#[derive(Clone, PartialEq, Eq)]
pub struct ValidatedCompanyName(String);

impl ValidatedCompanyName {
    pub fn new(value: impl Into<String>) -> Result<Self, ReadProfileValidationError> {
        let value = value.into();
        if value.trim().is_empty() || value.len() > 255 || value.chars().any(char::is_control) {
            return Err(ReadProfileValidationError::CompanyInvalid);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ValidatedCompanyName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ValidatedCompanyName([redacted])")
    }
}

/// A deliberately narrow name for a Bridge-generated write-canary ledger.
/// It is safe to place in the exact TDL filter formula used by the readback
/// profile and cannot represent an arbitrary operator-supplied ledger name.
#[derive(Clone, PartialEq, Eq)]
pub struct ValidatedCanaryLedgerName(String);

impl ValidatedCanaryLedgerName {
    pub fn new(value: impl Into<String>) -> Result<Self, ReadProfileValidationError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 128
            || value
                .strip_prefix(BRIDGE_CANARY_LEDGER_PREFIX)
                .is_none_or(str::is_empty)
            || !value.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b' ' | b'-' | b'_' | b'.')
            })
        {
            return Err(ReadProfileValidationError::CanaryLedgerInvalid);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ValidatedCanaryLedgerName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ValidatedCanaryLedgerName([redacted])")
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct ValidatedIdentityQuerySha256(String);

impl ValidatedIdentityQuerySha256 {
    pub fn new(value: impl Into<String>) -> Result<Self, ReadProfileValidationError> {
        let value = value.into();
        if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(ReadProfileValidationError::IdentityQueryInvalid);
        }
        Ok(Self(value.to_ascii_lowercase()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ValidatedIdentityQuerySha256 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ValidatedIdentityQuerySha256([redacted])")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedDateRange {
    from_yyyymmdd: String,
    to_yyyymmdd: String,
}

impl ValidatedDateRange {
    pub fn new(
        from_yyyymmdd: impl Into<String>,
        to_yyyymmdd: impl Into<String>,
    ) -> Result<Self, ReadProfileValidationError> {
        let from_yyyymmdd = from_yyyymmdd.into();
        let to_yyyymmdd = to_yyyymmdd.into();
        if !valid_yyyymmdd(&from_yyyymmdd) || !valid_yyyymmdd(&to_yyyymmdd) {
            return Err(ReadProfileValidationError::DateInvalid);
        }
        if from_yyyymmdd > to_yyyymmdd {
            return Err(ReadProfileValidationError::DateRangeInvalid);
        }
        Ok(Self {
            from_yyyymmdd,
            to_yyyymmdd,
        })
    }

    pub fn from_yyyymmdd(&self) -> &str {
        &self.from_yyyymmdd
    }

    pub fn to_yyyymmdd(&self) -> &str {
        &self.to_yyyymmdd
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadOnlyProfileId {
    CompanyListV1,
    CompanyListV2,
    CompanyBookExtentV1,
    CompanyBookExtentV2,
    #[cfg(feature = "voucher-scan")]
    LedgerOpeningCoverageV1,
    StandardLedgerIdentityV1,
    StandardLedgerCatalogV1,
    LedgersV1,
    LedgerCanaryReadbackV1,
    VouchersV2,
    VouchersV3,
    #[cfg(feature = "voucher-scan")]
    VoucherOutstandingsV1,
    #[cfg(feature = "voucher-scan")]
    VoucherEmptyPartitionWitnessV1,
    AuditCompanyObjectV1,
    AuditLedgersV1,
    AuditVouchersV1,
    AuditStockItemsV1,
}

impl ReadOnlyProfileId {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CompanyListV1 => "company_list_v1",
            Self::CompanyListV2 => "company_list_v2",
            Self::CompanyBookExtentV1 => "company_book_extent_v1",
            Self::CompanyBookExtentV2 => "company_book_extent_v2",
            #[cfg(feature = "voucher-scan")]
            Self::LedgerOpeningCoverageV1 => "ledger_opening_coverage_v1",
            Self::StandardLedgerIdentityV1 => "standard_ledger_identity_v1",
            Self::StandardLedgerCatalogV1 => "standard_ledger_catalog_v1",
            Self::LedgersV1 => "ledgers_v1",
            Self::LedgerCanaryReadbackV1 => "ledger_canary_readback_v1",
            Self::VouchersV2 => "vouchers_v2",
            Self::VouchersV3 => "vouchers_v3",
            #[cfg(feature = "voucher-scan")]
            Self::VoucherOutstandingsV1 => "voucher_outstandings_v1",
            #[cfg(feature = "voucher-scan")]
            Self::VoucherEmptyPartitionWitnessV1 => "voucher_empty_partition_witness_v1",
            Self::AuditCompanyObjectV1 => "audit_company_object_v1",
            Self::AuditLedgersV1 => "audit_ledgers_v1",
            Self::AuditVouchersV1 => "audit_vouchers_v1",
            Self::AuditStockItemsV1 => "audit_stock_items_v1",
        }
    }

    /// SHA-256 of the exact request template rendered with fixed safe
    /// sentinels in every dynamic slot. This changes if any emitted byte in the
    /// fixed profile changes, but is independent of a live company or range.
    pub fn template_sha256(self) -> String {
        let template = match self {
            Self::CompanyListV1 => render_company_list(),
            Self::CompanyListV2 => render_company_list_v2(),
            Self::CompanyBookExtentV1 => render_company_book_extent(TEMPLATE_COMPANY),
            Self::CompanyBookExtentV2 => render_company_book_extent_v2(TEMPLATE_COMPANY),
            #[cfg(feature = "voucher-scan")]
            Self::LedgerOpeningCoverageV1 => render_ledger_opening_coverage(TEMPLATE_COMPANY),
            Self::StandardLedgerIdentityV1 => render_standard_ledger_identity(TEMPLATE_COMPANY),
            Self::StandardLedgerCatalogV1 => render_standard_ledger_identity(TEMPLATE_COMPANY),
            Self::LedgersV1 => render_ledgers(TEMPLATE_COMPANY),
            Self::LedgerCanaryReadbackV1 => render_ledger_canary_readback(
                TEMPLATE_COMPANY,
                TEMPLATE_CANARY_LEDGER,
                TEMPLATE_IDENTITY_QUERY_SHA256,
            ),
            Self::VouchersV2 => render_vouchers(TEMPLATE_COMPANY, TEMPLATE_FROM, TEMPLATE_TO),
            Self::VouchersV3 => {
                render_selected_vouchers(TEMPLATE_COMPANY, TEMPLATE_FROM, TEMPLATE_TO)
            }
            #[cfg(feature = "voucher-scan")]
            Self::VoucherOutstandingsV1 => render_outstandings_template(
                TEMPLATE_COMPANY,
                TEMPLATE_FROM,
                TEMPLATE_TO,
                TEMPLATE_ALTER_ID_START,
                TEMPLATE_ALTER_ID_END,
            ),
            #[cfg(feature = "voucher-scan")]
            Self::VoucherEmptyPartitionWitnessV1 => render_empty_partition_witness_template(
                TEMPLATE_COMPANY,
                TEMPLATE_FROM,
                TEMPLATE_TO,
            ),
            Self::AuditCompanyObjectV1 => render_audit_company_object(TEMPLATE_COMPANY),
            Self::AuditLedgersV1 => {
                render_audit_ledgers(TEMPLATE_COMPANY, TEMPLATE_FROM, TEMPLATE_TO)
            }
            Self::AuditVouchersV1 => {
                render_audit_vouchers(TEMPLATE_COMPANY, TEMPLATE_FROM, TEMPLATE_TO)
            }
            Self::AuditStockItemsV1 => {
                render_audit_stock_items(TEMPLATE_COMPANY, TEMPLATE_FROM, TEMPLATE_TO)
            }
        };
        sha256_hex(&encode_tally_xml_request_utf16le(&template))
    }
}

#[derive(Debug, Clone, Copy)]
pub enum ReadOnlyProfile<'a> {
    CompanyListV1,
    /// Tally's documented `Company` collection (`TYPE=Collection`), fetching
    /// identity plus fixed gateway product/mode evidence. Unlike
    /// `CompanyListV1`'s custom TDL report,
    /// this returns the ordinary shaped `HEADER/STATUS=1` success envelope,
    /// so it can satisfy the trust check instead of being parsed as an
    /// unverified direct report.
    CompanyListV2,
    #[cfg(feature = "voucher-scan")]
    LedgerOpeningCoverageV1 {
        company: &'a ValidatedCompanyName,
    },
    CompanyBookExtentV1 {
        company: &'a ValidatedCompanyName,
    },
    /// A versioned Company collection that includes the observed company
    /// number required to distinguish year-split sibling books.
    CompanyBookExtentV2 {
        company: &'a ValidatedCompanyName,
    },
    /// A narrow compatibility bootstrap for Tally responders that reject
    /// Bridge's custom report profile but accept the documented built-in
    /// `List of Ledgers` collection. Its output is parsed only for repeated
    /// computed company context; ledger data is discarded.
    StandardLedgerIdentityV1 {
        company: &'a ValidatedCompanyName,
    },
    /// A scoped compatibility catalog that returns only ledger names and
    /// parents after independently validating the repeated company context.
    /// It is not the custom Bridge ledger export and cannot qualify a sync.
    StandardLedgerCatalogV1 {
        company: &'a ValidatedCompanyName,
    },
    LedgersV1 {
        company: &'a ValidatedCompanyName,
    },
    /// A one-ledger export used only to verify a separately approved,
    /// Bridge-generated synthetic write canary. It is not a general ledger
    /// query and remains read-only.
    LedgerCanaryReadbackV1 {
        company: &'a ValidatedCompanyName,
        ledger_name: &'a ValidatedCanaryLedgerName,
        identity_query_sha256: &'a ValidatedIdentityQuerySha256,
    },
    VouchersV2 {
        company: &'a ValidatedCompanyName,
        range: &'a ValidatedDateRange,
    },
    VouchersV3 {
        company: &'a ValidatedCompanyName,
        range: &'a ValidatedDateRange,
    },
    #[cfg(feature = "voucher-scan")]
    VoucherOutstandingsV1 {
        company: &'a PinnedCompany,
        window: &'a NarrowDateWindow,
        alter_id_range: AlterIdRange,
    },
    #[cfg(feature = "voucher-scan")]
    VoucherEmptyPartitionWitnessV1 {
        company: &'a PinnedCompany,
        window: &'a NarrowDateWindow,
    },
    /// The `company` part of a tally-read v1 read: a single-object export of
    /// the one named company. See [`AUDIT_COMPANY_FETCH`].
    AuditCompanyObjectV1 {
        company: &'a ValidatedCompanyName,
    },
    /// The `ledgers` part of a tally-read v1 read, for the audit period.
    AuditLedgersV1 {
        company: &'a ValidatedCompanyName,
        period: &'a ValidatedDateRange,
    },
    /// One `vouchers` part of a tally-read v1 read: one date window.
    AuditVouchersV1 {
        company: &'a ValidatedCompanyName,
        window: &'a ValidatedDateRange,
    },
    /// The `stock_items` part of a tally-read v1 read, for the audit period.
    AuditStockItemsV1 {
        company: &'a ValidatedCompanyName,
        period: &'a ValidatedDateRange,
    },
}

impl ReadOnlyProfile<'_> {
    pub fn id(self) -> ReadOnlyProfileId {
        match self {
            Self::CompanyListV1 => ReadOnlyProfileId::CompanyListV1,
            Self::CompanyListV2 => ReadOnlyProfileId::CompanyListV2,
            Self::CompanyBookExtentV1 { .. } => ReadOnlyProfileId::CompanyBookExtentV1,
            Self::CompanyBookExtentV2 { .. } => ReadOnlyProfileId::CompanyBookExtentV2,
            #[cfg(feature = "voucher-scan")]
            Self::LedgerOpeningCoverageV1 { .. } => ReadOnlyProfileId::LedgerOpeningCoverageV1,
            Self::StandardLedgerIdentityV1 { .. } => ReadOnlyProfileId::StandardLedgerIdentityV1,
            Self::StandardLedgerCatalogV1 { .. } => ReadOnlyProfileId::StandardLedgerCatalogV1,
            Self::LedgersV1 { .. } => ReadOnlyProfileId::LedgersV1,
            Self::LedgerCanaryReadbackV1 { .. } => ReadOnlyProfileId::LedgerCanaryReadbackV1,
            Self::VouchersV2 { .. } => ReadOnlyProfileId::VouchersV2,
            Self::VouchersV3 { .. } => ReadOnlyProfileId::VouchersV3,
            #[cfg(feature = "voucher-scan")]
            Self::VoucherOutstandingsV1 { .. } => ReadOnlyProfileId::VoucherOutstandingsV1,
            #[cfg(feature = "voucher-scan")]
            Self::VoucherEmptyPartitionWitnessV1 { .. } => {
                ReadOnlyProfileId::VoucherEmptyPartitionWitnessV1
            }
            Self::AuditCompanyObjectV1 { .. } => ReadOnlyProfileId::AuditCompanyObjectV1,
            Self::AuditLedgersV1 { .. } => ReadOnlyProfileId::AuditLedgersV1,
            Self::AuditVouchersV1 { .. } => ReadOnlyProfileId::AuditVouchersV1,
            Self::AuditStockItemsV1 { .. } => ReadOnlyProfileId::AuditStockItemsV1,
        }
    }

    pub fn template_sha256(self) -> String {
        self.id().template_sha256()
    }

    pub fn render(self) -> String {
        match self {
            Self::CompanyListV1 => render_company_list(),
            Self::CompanyListV2 => render_company_list_v2(),
            Self::CompanyBookExtentV1 { company } => render_company_book_extent(company.as_str()),
            Self::CompanyBookExtentV2 { company } => {
                render_company_book_extent_v2(company.as_str())
            }
            #[cfg(feature = "voucher-scan")]
            Self::LedgerOpeningCoverageV1 { company } => {
                render_ledger_opening_coverage(company.as_str())
            }
            Self::StandardLedgerIdentityV1 { company } => {
                render_standard_ledger_identity(company.as_str())
            }
            Self::StandardLedgerCatalogV1 { company } => {
                render_standard_ledger_identity(company.as_str())
            }
            Self::LedgersV1 { company } => render_ledgers(company.as_str()),
            Self::LedgerCanaryReadbackV1 {
                company,
                ledger_name,
                identity_query_sha256,
            } => render_ledger_canary_readback(
                company.as_str(),
                ledger_name.as_str(),
                identity_query_sha256.as_str(),
            ),
            Self::VouchersV2 { company, range } => {
                render_vouchers(company.as_str(), range.from_yyyymmdd(), range.to_yyyymmdd())
            }
            Self::VouchersV3 { company, range } => render_selected_vouchers(
                company.as_str(),
                range.from_yyyymmdd(),
                range.to_yyyymmdd(),
            ),
            #[cfg(feature = "voucher-scan")]
            Self::VoucherOutstandingsV1 {
                company,
                window,
                alter_id_range,
            } => render_outstandings_vouchers(company, window, alter_id_range),
            #[cfg(feature = "voucher-scan")]
            Self::VoucherEmptyPartitionWitnessV1 { company, window } => {
                crate::outstandings::voucher_empty_partition_witness_request(company, window)
                    .into_xml()
            }
            Self::AuditCompanyObjectV1 { company } => render_audit_company_object(company.as_str()),
            Self::AuditLedgersV1 { company, period } => render_audit_ledgers(
                company.as_str(),
                period.from_yyyymmdd(),
                period.to_yyyymmdd(),
            ),
            Self::AuditVouchersV1 { company, window } => render_audit_vouchers(
                company.as_str(),
                window.from_yyyymmdd(),
                window.to_yyyymmdd(),
            ),
            Self::AuditStockItemsV1 { company, period } => render_audit_stock_items(
                company.as_str(),
                period.from_yyyymmdd(),
                period.to_yyyymmdd(),
            ),
        }
    }
}

/// Compatibility seam for the native app's existing function signatures.
/// These functions still expose only the three fixed read-only profiles and
/// XML-escape every dynamic value; no caller-provided XML can be dispatched.
#[doc(hidden)]
pub mod compatibility {
    pub fn company_list_request() -> String {
        super::render_company_list()
    }

    pub fn ledgers_request(company: &str) -> String {
        super::render_ledgers(company)
    }

    pub fn standard_ledger_identity_request(company: &str) -> String {
        super::render_standard_ledger_identity(company)
    }

    pub fn standard_ledger_catalog_request(company: &str) -> String {
        super::render_standard_ledger_identity(company)
    }

    pub fn vouchers_request(company: &str, from: &str, to: &str) -> String {
        super::render_vouchers(company, from, to)
    }

    pub fn selected_vouchers_request(company: &str, from: &str, to: &str) -> String {
        super::render_selected_vouchers(company, from, to)
    }
}

fn render_company_list() -> String {
    r#"
<ENVELOPE>
    <HEADER>
        <VERSION>1</VERSION>
        <TALLYREQUEST>Export</TALLYREQUEST>
        <TYPE>Data</TYPE>
        <ID>Company Report</ID>
    </HEADER>
    <BODY>
        <DESC>
            <STATICVARIABLES>
                <SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT>
            </STATICVARIABLES>
            <TDL>
                <TDLMESSAGE>
                    <REPORT NAME="Company Report">
                        <FORMS>Company Form</FORMS>
                        <TITLE>"Company Details"</TITLE>
                    </REPORT>
                    <FORM NAME="Company Form">
                        <TOPPARTS>Company Part</TOPPARTS>
                        <HEIGHT>100% Page</HEIGHT>
                        <WIDTH>100% Page</WIDTH>
                    </FORM>
                    <PART NAME="Company Part">
                        <TOPLINES>Company Header, Company Details</TOPLINES>
                        <REPEAT>Company Details : CompanyCollection</REPEAT>
                        <SCROLLED>Vertical</SCROLLED>
                        <COMMONBORDERS>Yes</COMMONBORDERS>
                    </PART>
                    <LINE NAME="Company Header">
                        <LEFTFIELDS>
                            Company Name Header, Company GUID Header
                        </LEFTFIELDS>
                    </LINE>
                    <FIELD NAME="Company Name Header"><SET>"Company Name"</SET></FIELD>
                    <FIELD NAME="Company GUID Header"><SET>"Company GUID"</SET></FIELD>
                    <LINE NAME="Company Details">
                        <LEFTFIELDS>
                            Company Name Field, Company GUID Field
                        </LEFTFIELDS>
                        <XMLTAG>"CompanyInfo"</XMLTAG>
                    </LINE>
                    <FIELD NAME="Company Name Field"><SET>$Name</SET></FIELD>
                    <FIELD NAME="Company GUID Field"><SET>$GUID</SET></FIELD>
                    <COLLECTION NAME="CompanyCollection">
                        <TYPE>Company</TYPE>
                        <FETCH>Name, GUID</FETCH>
                    </COLLECTION>
                </TDLMESSAGE>
            </TDL>
        </DESC>
    </BODY>
</ENVELOPE>
"#
    .trim()
    .to_string()
}

/// Tally's documented `Company` collection. It returns company identity plus
/// endpoint-wide product/mode facts from fixed, no-space `$$LicenseInfo`
/// functions. Those facts are parsed separately so a missing capability field
/// cannot make ordinary company discovery unavailable.
/// Unlike `render_company_list`'s custom TDL report — which Tally answers
/// with a bare `<ENVELOPE><COMPANYINFO>...` document carrying no
/// `HEADER`/`STATUS` at all — a `TYPE=Collection` export returns the
/// ordinary shaped success envelope, so its response can satisfy the same
/// `HEADER/STATUS=1` trust check every other export profile requires.
// Product, mode, tier and release are observed endpoint facts; see
// TALLY_PROTOCOL_REFERENCE.md §3.1 for the captured qualification scope.
fn render_company_list_v2() -> String {
    r#"
<ENVELOPE>
    <HEADER>
        <VERSION>1</VERSION>
        <TALLYREQUEST>Export</TALLYREQUEST>
        <TYPE>Collection</TYPE>
        <ID>BridgeCompanyExtent</ID>
    </HEADER>
    <BODY>
        <DESC>
            <STATICVARIABLES>
                <SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT>
            </STATICVARIABLES>
            <TDL>
                <TDLMESSAGE>
                    <COLLECTION NAME="BridgeCompanyExtent" ISMODIFY="No" ISINITIALIZE="Yes">
                        <TYPE>Company</TYPE>
                        <NATIVEMETHOD>NAME</NATIVEMETHOD>
                        <NATIVEMETHOD>GUID</NATIVEMETHOD>
                        <NATIVEMETHOD>COMPANYNUMBER</NATIVEMETHOD>
                        <NATIVEMETHOD>BOOKSFROM</NATIVEMETHOD>
                        <NATIVEMETHOD>PRODUCTNAME</NATIVEMETHOD>
                        <COMPUTE>EduMode : $$LicenseInfo:IsEducationalMode</COMPUTE>
                        <COMPUTE>Silver : $$LicenseInfo:IsSilver</COMPUTE>
                        <COMPUTE>Gold : $$LicenseInfo:IsGold</COMPUTE>
                        <COMPUTE>BridgeRelease : @@VersionReleaseString</COMPUTE>
                    </COLLECTION>
                </TDLMESSAGE>
            </TDL>
        </DESC>
    </BODY>
</ENVELOPE>
"#
    .trim()
    .to_string()
}

fn render_standard_ledger_identity(company: &str) -> String {
    format!(
        r#"
<ENVELOPE>
    <HEADER>
        <VERSION>1</VERSION>
        <TALLYREQUEST>EXPORT</TALLYREQUEST>
        <TYPE>COLLECTION</TYPE>
        <ID>List of Ledgers</ID>
    </HEADER>
    <BODY>
        <DESC>
            <STATICVARIABLES>
                <SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT>
                <SVCURRENTCOMPANY>{}</SVCURRENTCOMPANY>
            </STATICVARIABLES>
            <TDL>
                <TDLMESSAGE>
                    <COLLECTION NAME="List of Ledgers" ISMODIFY="Yes">
                        <NATIVEMETHOD>Name</NATIVEMETHOD>
                        <NATIVEMETHOD>GUID</NATIVEMETHOD>
                        <NATIVEMETHOD>Parent</NATIVEMETHOD>
                        <COMPUTE>BRIDGECOMPANYGUID:$GUID:Company:##SVCurrentCompany</COMPUTE>
                        <COMPUTE>BRIDGECOMPANYNAME:##SVCurrentCompany</COMPUTE>
                    </COLLECTION>
                </TDLMESSAGE>
            </TDL>
        </DESC>
    </BODY>
</ENVELOPE>
"#,
        xml_escape(company)
    )
    .trim()
    .to_string()
}

fn render_ledgers(company: &str) -> String {
    format!(
        r#"
<ENVELOPE>
    <HEADER>
        <VERSION>1</VERSION>
        <TALLYREQUEST>EXPORT</TALLYREQUEST>
        <TYPE>DATA</TYPE>
        <ID>BRIDGE Ledger Export V1</ID>
    </HEADER>
    <BODY>
        <DESC>
            <STATICVARIABLES>
                <SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT>
                <SVCURRENTCOMPANY>{}</SVCURRENTCOMPANY>
            </STATICVARIABLES>
            <TDL>
                <TDLMESSAGE>
                    <REPORT NAME="BRIDGE Ledger Export V1">
                        <FORMS>BRIDGE Ledger Export Form V1</FORMS>
                        <PLAINXML>Yes</PLAINXML>
                    </REPORT>
                    <FORM NAME="BRIDGE Ledger Export Form V1">
                        <TOPPARTS>BRIDGE Ledger Context Part V1, BRIDGE Ledger Rows Part V1</TOPPARTS>
                    </FORM>
                    <PART NAME="BRIDGE Ledger Context Part V1">
                        <TOPLINES>BRIDGE Ledger Context Line V1</TOPLINES>
                    </PART>
                    <PART NAME="BRIDGE Ledger Rows Part V1">
                        <TOPLINES>BRIDGE Ledger Row Line V1</TOPLINES>
                        <REPEAT>BRIDGE Ledger Row Line V1 : BRIDGE Ledger Collection V1</REPEAT>
                    </PART>
                    <LINE NAME="BRIDGE Ledger Context Line V1">
                        <LEFTFIELDS>BRIDGE Ledger Schema V1, BRIDGE Ledger Object Type V1, BRIDGE Ledger Company Name V1, BRIDGE Ledger Company GUID V1, BRIDGE Ledger Record Count V1</LEFTFIELDS>
                        <XMLTAG>"COMPANYCONTEXT"</XMLTAG>
                    </LINE>
                    <FIELD NAME="BRIDGE Ledger Schema V1">
                        <SET>"bridge.tally.ledgers/1"</SET>
                        <XMLTAG>"SCHEMA"</XMLTAG>
                    </FIELD>
                    <FIELD NAME="BRIDGE Ledger Object Type V1">
                        <SET>"LEDGER"</SET>
                        <XMLTAG>"OBJECTTYPE"</XMLTAG>
                    </FIELD>
                    <FIELD NAME="BRIDGE Ledger Company Name V1">
                        <SET>##SVCurrentCompany</SET>
                        <XMLTAG>"NAME"</XMLTAG>
                    </FIELD>
                    <FIELD NAME="BRIDGE Ledger Company GUID V1">
                        <SET>$GUID:Company:##SVCurrentCompany</SET>
                        <XMLTAG>"GUID"</XMLTAG>
                    </FIELD>
                    <FIELD NAME="BRIDGE Ledger Record Count V1">
                        <SET>$$NumItems:BRIDGE Ledger Collection V1</SET>
                        <XMLTAG>"RECORDCOUNT"</XMLTAG>
                    </FIELD>
                    <LINE NAME="BRIDGE Ledger Row Line V1">
                        <LEFTFIELDS>BRIDGE Ledger Parent V1, BRIDGE Ledger GSTIN V1, BRIDGE Ledger Opening Balance V1</LEFTFIELDS>
                        <XMLTAG>"LEDGER"</XMLTAG>
                        <XMLATTR>"NAME" : $Name</XMLATTR>
                        <XMLATTR>"GUID" : $GUID</XMLATTR>
                        <XMLATTR>"REMOTEID" : $RemoteID</XMLATTR>
                        <XMLATTR>"MASTERID" : $MasterID</XMLATTR>
                        <XMLATTR>"ALTERID" : $AlterID</XMLATTR>
                    </LINE>
                    <FIELD NAME="BRIDGE Ledger Parent V1">
                        <SET>$Parent</SET>
                        <XMLTAG>"PARENT"</XMLTAG>
                    </FIELD>
                    <FIELD NAME="BRIDGE Ledger GSTIN V1">
                        <SET>$PartyGSTIN</SET>
                        <XMLTAG>"PARTYGSTIN"</XMLTAG>
                    </FIELD>
                    <FIELD NAME="BRIDGE Ledger Opening Balance V1">
                        <SET>$OpeningBalance</SET>
                        <XMLTAG>"OPENINGBALANCE"</XMLTAG>
                    </FIELD>
                    <COLLECTION ISMODIFY="No" ISFIXED="No" ISINITIALIZE="No" ISOPTION="No" ISINTERNAL="No" NAME="BRIDGE Ledger Collection V1">
                        <TYPE>Ledger</TYPE>
                        <FETCH>Name, GUID, RemoteID, MasterID, AlterID, Parent, PartyGSTIN, OpeningBalance</FETCH>
                    </COLLECTION>
                </TDLMESSAGE>
            </TDL>
        </DESC>
    </BODY>
</ENVELOPE>
"#,
        xml_escape(company)
    )
    .trim()
    .to_string()
}

fn render_ledger_canary_readback(
    company: &str,
    ledger_name: &str,
    identity_query_sha256: &str,
) -> String {
    let collection_filter = r#"                        <FILTERS>BRIDGE Ledger Exact Canary Name V1</FILTERS>
"#;
    let filter_formula = r#"                    <SYSTEM TYPE="Formulae" NAME="BRIDGE Ledger Exact Canary Name V1">$Name = "__BRIDGE_CANARY_LEDGER_NAME__"</SYSTEM>
"#;
    render_ledgers(company)
        .replace("BRIDGE Ledger Export V1", "BRIDGE Ledger Canary Readback V1")
        .replace(
            BRIDGE_LEDGER_EXPORT_SCHEMA,
            BRIDGE_LEDGER_WRITE_READBACK_SCHEMA,
        )
        .replacen(
            "                        <XMLTAG>\"COMPANYCONTEXT\"</XMLTAG>",
            &format!(
                "                        <XMLTAG>\"COMPANYCONTEXT\"</XMLTAG>\n                        <XMLATTR>\"QUERYIDENTITYSETSHA256\" : \"{identity_query_sha256}\"</XMLATTR>",
            ),
            1,
        )
        .replacen(
            "                        <TYPE>Ledger</TYPE>",
            &format!("                        <TYPE>Ledger</TYPE>\n{collection_filter}"),
            1,
        )
        .replacen(
            "                </TDLMESSAGE>",
            &format!(
                "{}                </TDLMESSAGE>",
                filter_formula.replace("__BRIDGE_CANARY_LEDGER_NAME__", ledger_name),
            ),
            1,
        )
}

fn render_vouchers(company: &str, from: &str, to: &str) -> String {
    format!(
        r#"
<ENVELOPE>
    <HEADER>
        <VERSION>1</VERSION>
        <TALLYREQUEST>EXPORT</TALLYREQUEST>
        <TYPE>DATA</TYPE>
        <ID>BRIDGE Voucher Export V2</ID>
    </HEADER>
    <BODY>
        <DESC>
            <STATICVARIABLES>
                <SVCURRENTCOMPANY>{}</SVCURRENTCOMPANY>
                <SVFROMDATE TYPE="Date">{}</SVFROMDATE>
                <SVTODATE TYPE="Date">{}</SVTODATE>
                <SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT>
            </STATICVARIABLES>
            <TDL>
                <TDLMESSAGE>
                    <REPORT NAME="BRIDGE Voucher Export V2">
                        <FORMS>BRIDGE Voucher Export Form V2</FORMS>
                        <PLAINXML>Yes</PLAINXML>
                    </REPORT>
                    <FORM NAME="BRIDGE Voucher Export Form V2">
                        <TOPPARTS>BRIDGE Voucher Context Part V1, BRIDGE Voucher Rows Part V1</TOPPARTS>
                    </FORM>
                    <PART NAME="BRIDGE Voucher Context Part V1">
                        <TOPLINES>BRIDGE Voucher Context Line V1</TOPLINES>
                    </PART>
                    <PART NAME="BRIDGE Voucher Rows Part V1">
                        <TOPLINES>BRIDGE Voucher Row Line V1</TOPLINES>
                        <REPEAT>BRIDGE Voucher Row Line V1 : BRIDGE Voucher Collection V1</REPEAT>
                    </PART>
                    <LINE NAME="BRIDGE Voucher Context Line V1">
                        <LEFTFIELDS>BRIDGE Voucher Schema V1, BRIDGE Voucher Object Type V1, BRIDGE Voucher Company Name V1, BRIDGE Voucher Company GUID V1, BRIDGE Voucher Record Count V1</LEFTFIELDS>
                        <XMLTAG>"COMPANYCONTEXT"</XMLTAG>
                    </LINE>
                    <FIELD NAME="BRIDGE Voucher Schema V1">
                        <SET>"bridge.tally.vouchers/2"</SET>
                        <XMLTAG>"SCHEMA"</XMLTAG>
                    </FIELD>
                    <FIELD NAME="BRIDGE Voucher Object Type V1">
                        <SET>"VOUCHER"</SET>
                        <XMLTAG>"OBJECTTYPE"</XMLTAG>
                    </FIELD>
                    <FIELD NAME="BRIDGE Voucher Company Name V1">
                        <SET>##SVCurrentCompany</SET>
                        <XMLTAG>"NAME"</XMLTAG>
                    </FIELD>
                    <FIELD NAME="BRIDGE Voucher Company GUID V1">
                        <SET>$GUID:Company:##SVCurrentCompany</SET>
                        <XMLTAG>"GUID"</XMLTAG>
                    </FIELD>
                    <FIELD NAME="BRIDGE Voucher Record Count V1">
                        <SET>$$NumItems:BRIDGE Voucher Collection V1</SET>
                        <XMLTAG>"RECORDCOUNT"</XMLTAG>
                    </FIELD>
                    <LINE NAME="BRIDGE Voucher Row Line V1">
                        <LEFTFIELDS>BRIDGE Voucher Date V1, BRIDGE Voucher Type V1, BRIDGE Voucher Number V1, BRIDGE Voucher Cancelled V1, BRIDGE Voucher Optional V2, BRIDGE Voucher Ledger Entry Count V1</LEFTFIELDS>
                        <XMLTAG>"VOUCHER"</XMLTAG>
                        <XMLATTR>"REMOTEID" : $RemoteID</XMLATTR>
                        <XMLATTR>"GUID" : $GUID</XMLATTR>
                        <XMLATTR>"MASTERID" : $MasterID</XMLATTR>
                        <XMLATTR>"ALTERID" : $AlterID</XMLATTR>
                        <EXPLODE>BRIDGE Voucher Ledger Entries Part V1 : Yes</EXPLODE>
                    </LINE>
                    <FIELD NAME="BRIDGE Voucher Date V1">
                        <SET>$Date</SET>
                        <XMLTAG>"DATE"</XMLTAG>
                    </FIELD>
                    <FIELD NAME="BRIDGE Voucher Type V1">
                        <SET>$VoucherTypeName</SET>
                        <XMLTAG>"VOUCHERTYPENAME"</XMLTAG>
                    </FIELD>
                    <FIELD NAME="BRIDGE Voucher Number V1">
                        <SET>$VoucherNumber</SET>
                        <XMLTAG>"VOUCHERNUMBER"</XMLTAG>
                    </FIELD>
                    <FIELD NAME="BRIDGE Voucher Cancelled V1">
                        <SET>$IsCancelled</SET>
                        <XMLTAG>"ISCANCELLED"</XMLTAG>
                    </FIELD>
                    <FIELD NAME="BRIDGE Voucher Optional V2">
                        <SET>$IsOptional</SET>
                        <TYPE>Logical</TYPE>
                        <XMLTAG>"ISOPTIONAL"</XMLTAG>
                    </FIELD>
                    <FIELD NAME="BRIDGE Voucher Ledger Entry Count V1">
                        <SET>$$NumItems:AllLedgerEntries</SET>
                        <XMLTAG>"LEDGERENTRYCOUNT"</XMLTAG>
                    </FIELD>
                    <PART NAME="BRIDGE Voucher Ledger Entries Part V1">
                        <TOPLINES>BRIDGE Voucher Ledger Entry Row V1</TOPLINES>
                        <REPEAT>BRIDGE Voucher Ledger Entry Row V1 : AllLedgerEntries</REPEAT>
                        <XMLTAG>"LEDGERENTRIES"</XMLTAG>
                    </PART>
                    <LINE NAME="BRIDGE Voucher Ledger Entry Row V1">
                        <LEFTFIELDS>BRIDGE Voucher Ledger Entry Index V1, BRIDGE Voucher Ledger Entry Name V1, BRIDGE Voucher Ledger Entry Amount V1, BRIDGE Voucher Ledger Entry Deemed Positive V1</LEFTFIELDS>
                        <XMLTAG>"LEDGERENTRY"</XMLTAG>
                    </LINE>
                    <FIELD NAME="BRIDGE Voucher Ledger Entry Index V1">
                        <SET>$$Line</SET>
                        <XMLTAG>"ENTRYINDEX"</XMLTAG>
                    </FIELD>
                    <FIELD NAME="BRIDGE Voucher Ledger Entry Name V1">
                        <SET>$LedgerName</SET>
                        <XMLTAG>"LEDGERNAME"</XMLTAG>
                    </FIELD>
                    <FIELD NAME="BRIDGE Voucher Ledger Entry Amount V1">
                        <SET>$Amount</SET>
                        <TYPE>Amount</TYPE>
                        <FORMAT>"No Symbol, No Comma"</FORMAT>
                        <XMLTAG>"AMOUNT"</XMLTAG>
                    </FIELD>
                    <FIELD NAME="BRIDGE Voucher Ledger Entry Deemed Positive V1">
                        <SET>$IsDeemedPositive</SET>
                        <TYPE>Logical</TYPE>
                        <XMLTAG>"ISDEEMEDPOSITIVE"</XMLTAG>
                    </FIELD>
                    <COLLECTION NAME="BRIDGE Voucher Collection V1" ISMODIFY="No" ISFIXED="No" ISINITIALIZE="No" ISOPTION="No" ISINTERNAL="No">
                        <TYPE>Voucher</TYPE>
                        <FETCH>RemoteID, GUID, MasterID, AlterID, Date, VoucherTypeName, VoucherNumber, IsCancelled, AllLedgerEntries.*</FETCH>
                    </COLLECTION>
                </TDLMESSAGE>
            </TDL>
        </DESC>
    </BODY>
</ENVELOPE>
"#,
        xml_escape(company),
        xml_escape(from),
        xml_escape(to)
    )
    .trim()
    .to_string()
}

fn render_selected_vouchers(company: &str, from: &str, to: &str) -> String {
    let request = render_vouchers(company, from, to)
        .replace("BRIDGE Voucher Export V2", "BRIDGE Voucher Export V3")
        .replace("bridge.tally.vouchers/2", "bridge.tally.vouchers/3")
        .replace(
            "BRIDGE Voucher Company GUID V1, BRIDGE Voucher Record Count V1",
            "BRIDGE Voucher Company GUID V1, BRIDGE Voucher From Date V3, BRIDGE Voucher To Date V3, BRIDGE Voucher Record Count V1",
        );
    let record_count_field = r#"                    <FIELD NAME="BRIDGE Voucher Record Count V1">"#;
    let window_fields = r#"                    <FIELD NAME="BRIDGE Voucher From Date V3">
                        <SET>$$String:##SVFromDate:"YYYYMMDD"</SET>
                        <XMLTAG>"FROMDATE"</XMLTAG>
                    </FIELD>
                    <FIELD NAME="BRIDGE Voucher To Date V3">
                        <SET>$$String:##SVToDate:"YYYYMMDD"</SET>
                        <XMLTAG>"TODATE"</XMLTAG>
                    </FIELD>
"#;
    request.replacen(
        record_count_field,
        &format!("{window_fields}{record_count_field}"),
        1,
    )
}

/// FETCHLIST of the tally-read v1 `company` part (`AuditCompanyObjectV1`).
///
/// A `Company` *collection* returns every loaded company whatever
/// `SVCURRENTCOMPANY` says (protocol reference §12a.7), and both consumers
/// take the first `COMPANY` carrying a GUID, so the part is a single-object
/// export instead (protocol reference §9.11a). That shape was measured with
/// `FETCH *` only (565 tags). This explicit list is narrower on purpose: the
/// part is stored, and nothing else in the company definition is read:
/// `GUID` and `BOOKSFROM` (both engines), `ISINTEGRATED` (the Python stock
/// test), and `NAME`, which no engine reads but which evidences which company
/// Tally resolved the `ID` to. Each was present in the `FETCH *` captures of
/// the audit books; whether an explicit `FETCHLIST` narrows an Object export
/// is unmeasured, so admission checks the fields the consumers need, not the
/// absence of the rest.
///
/// Never add `ORIGINALNAME` or any field outside this list without a new
/// profile id.
pub const AUDIT_COMPANY_FETCH: [&str; 4] = ["GUID", "NAME", "BOOKSFROM", "ISINTEGRATED"];

/// FETCH of the tally-read v1 `ledgers` part (`AuditLedgersV1`).
///
/// The fields the audit consumers were recorded reading (Lane B's recorded
/// Python reads and the crate's traced reads of the same parts, 2026-09-21),
/// plus `ALTERID` for the part's `alter_id_max`. It is deliberately narrower
/// than the party-master workbook's list: no bank details, IFSC, e-mail,
/// phone or address, because no audit consumer reads them and the read is
/// stored (plan D-C). Adding a field needs a new profile id.
/// `LEDGSTREGDETAILS.LIST` fetches the whole registration sub-list (the
/// consumers read `APPLICABLEFROM` and `GSTIN` from it); it carries the
/// dated GST registration history; the flat `PARTYGSTIN` was blank for
/// ledgers whose GSTIN lives only there (measured 2026-09-21 on licensed
/// Silver 7.1 with this exact FETCH token in a `Ledger` collection).
pub const AUDIT_LEDGER_FETCH: &str = "NAME, GUID, MASTERID, ALTERID, PARENT, OPENINGBALANCE, \
ISBILLWISEON, PARTYGSTIN, INCOMETAXNUMBER, LEDGSTREGDETAILS.LIST";

/// FETCH of one tally-read v1 `vouchers` part (`AuditVouchersV1`): the
/// fields both engines were recorded reading (2026-09-21). `@REMOTEID` and
/// `@VCHTYPE` are attributes Tally emits on every voucher unasked.
///
/// - `ISOPTIONAL`, `ISCANCELLED` and `ISPOSTDATED` are load-bearing: Tally
///   emits no field the FETCH does not name, and a voucher missing any of the
///   three is UNKNOWN to the engine, which then refuses the book.
/// - Quantities, rates and amounts on goods lines are fetched at every level
///   the Python engine falls back through, including
///   `ALLLEDGERENTRIES.INVENTORYALLOCATIONS`: an absent quantity there reads
///   as no quantity and the stock movement is silently dropped.
/// - No bill allocations. That holds only while no audit test reads bill
///   types; the agent's `ALLLEDGERENTRIES.*` shape exists for readers that do.
/// - **Unmeasured:** Bridge ships one-level dotted paths
///   (`ALLLEDGERENTRIES.LEDGERNAME`); the two-level
///   `ALLLEDGERENTRIES.INVENTORYALLOCATIONS.*` and the `ALLINVENTORYENTRIES.*`
///   and `INVENTORYENTRIES{IN,OUT}.*` paths have not been sent to Tally by
///   Bridge. Live qualification must show they return the nested values on an
///   inventory book with batches before this profile reads a client book.
pub const AUDIT_VOUCHER_FETCH: &str = "GUID,MASTERID,ALTERID,DATE,VOUCHERTYPENAME,\
VOUCHERNUMBER,REFERENCE,PARTYLEDGERNAME,PARTYGSTIN,NARRATION,ISOPTIONAL,ISCANCELLED,ISPOSTDATED,\
ALLLEDGERENTRIES.LEDGERNAME,ALLLEDGERENTRIES.AMOUNT,ALLLEDGERENTRIES.ISDEEMEDPOSITIVE,\
ALLLEDGERENTRIES.INVENTORYALLOCATIONS.STOCKITEMNAME,ALLLEDGERENTRIES.INVENTORYALLOCATIONS.BILLEDQTY,\
ALLLEDGERENTRIES.INVENTORYALLOCATIONS.ACTUALQTY,ALLLEDGERENTRIES.INVENTORYALLOCATIONS.RATE,\
ALLLEDGERENTRIES.INVENTORYALLOCATIONS.AMOUNT,\
ALLINVENTORYENTRIES.STOCKITEMNAME,ALLINVENTORYENTRIES.BILLEDQTY,ALLINVENTORYENTRIES.ACTUALQTY,\
ALLINVENTORYENTRIES.RATE,ALLINVENTORYENTRIES.AMOUNT,\
INVENTORYENTRIESIN.STOCKITEMNAME,INVENTORYENTRIESIN.ACTUALQTY,INVENTORYENTRIESIN.AMOUNT,\
INVENTORYENTRIESOUT.STOCKITEMNAME,INVENTORYENTRIESOUT.ACTUALQTY,INVENTORYENTRIESOUT.AMOUNT";

/// FETCH of the tally-read v1 `stock_items` part (`AuditStockItemsV1`).
///
/// The fields the Python stock tests were recorded reading from a
/// `stock_items` part (2026-09-21), plus `ALTERID` for the part's
/// `alter_id_max`. `CLOSINGRATE` is read only from `stock_summary` parts and is
/// not fetched here. The request carries the audit period on the inference
/// that closing figures are period-dependent; that is unmeasured.
pub const AUDIT_STOCK_ITEM_FETCH: &str = "NAME, GUID, ALTERID, PARENT, BASEUNITS, \
OPENINGBALANCE, OPENINGVALUE, CLOSINGBALANCE, CLOSINGVALUE";

fn render_audit_company_object(company: &str) -> String {
    let company = xml_escape(company);
    let fetch = AUDIT_COMPANY_FETCH
        .iter()
        .map(|field| format!("<FETCH>{field}</FETCH>"))
        .collect::<String>();
    format!(
        r#"<ENVELOPE><HEADER><VERSION>1</VERSION><TALLYREQUEST>Export</TALLYREQUEST><TYPE>Object</TYPE><SUBTYPE>Company</SUBTYPE><ID TYPE="Name">{company}</ID></HEADER><BODY><DESC><STATICVARIABLES><SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT><SVCURRENTCOMPANY>{company}</SVCURRENTCOMPANY></STATICVARIABLES><FETCHLIST>{fetch}</FETCHLIST></DESC></BODY></ENVELOPE>"#
    )
}

fn render_audit_ledgers(company: &str, from: &str, to: &str) -> String {
    format!(
        r#"<ENVELOPE><HEADER><VERSION>1</VERSION><TALLYREQUEST>Export</TALLYREQUEST><TYPE>Collection</TYPE><ID>Bridge Audit Ledgers</ID></HEADER><BODY><DESC><STATICVARIABLES><SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT><SVCURRENTCOMPANY>{company}</SVCURRENTCOMPANY><SVFROMDATE TYPE="Date">{from}</SVFROMDATE><SVTODATE TYPE="Date">{to}</SVTODATE></STATICVARIABLES><TDL><TDLMESSAGE><COLLECTION NAME="Bridge Audit Ledgers" ISMODIFY="No"><TYPE>Ledger</TYPE><FETCH>{AUDIT_LEDGER_FETCH}</FETCH></COLLECTION></TDLMESSAGE></TDL></DESC></BODY></ENVELOPE>"#,
        company = xml_escape(company),
        from = xml_escape(from),
        to = xml_escape(to),
    )
}

/// The agent's windowed voucher envelope (literal `$Date` bounds, protocol
/// reference §5.3) with [`AUDIT_VOUCHER_FETCH`]. The app crate holds a test
/// that this is byte-identical to its own windowed renderer apart from the
/// FETCH, so the request is the qualified window shape. The response-side
/// checks keyed to that shape (`window_not_honoured`, part admission) are not
/// inherited by this renderer; the caller that sends it must apply them.
fn render_audit_vouchers(company: &str, from: &str, to: &str) -> String {
    format!(
        "<ENVELOPE><HEADER><VERSION>1</VERSION><TALLYREQUEST>Export</TALLYREQUEST><TYPE>Collection</TYPE><ID>Bridge Agent Vouchers</ID></HEADER><BODY><DESC><STATICVARIABLES><SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT><SVCURRENTCOMPANY>{company}</SVCURRENTCOMPANY><SVFROMDATE TYPE=\"Date\">{from}</SVFROMDATE><SVTODATE TYPE=\"Date\">{to}</SVTODATE></STATICVARIABLES><TDL><TDLMESSAGE><SYSTEM TYPE=\"Formulae\" NAME=\"BridgeAgentWindow\">$Date &gt;= $$Date:\"{from}\" AND $Date &lt;= $$Date:\"{to}\"</SYSTEM><COLLECTION NAME=\"Bridge Agent Vouchers\" ISMODIFY=\"No\"><TYPE>Voucher</TYPE><FETCH>{AUDIT_VOUCHER_FETCH}</FETCH><FILTERS>BridgeAgentWindow</FILTERS></COLLECTION></TDLMESSAGE></TDL></DESC></BODY></ENVELOPE>",
        company = xml_escape(company),
        from = xml_escape(from),
        to = xml_escape(to),
    )
}

fn render_audit_stock_items(company: &str, from: &str, to: &str) -> String {
    format!(
        r#"<ENVELOPE><HEADER><VERSION>1</VERSION><TALLYREQUEST>Export</TALLYREQUEST><TYPE>Collection</TYPE><ID>Bridge Audit Stock Items</ID></HEADER><BODY><DESC><STATICVARIABLES><SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT><SVCURRENTCOMPANY>{company}</SVCURRENTCOMPANY><SVFROMDATE TYPE="Date">{from}</SVFROMDATE><SVTODATE TYPE="Date">{to}</SVTODATE></STATICVARIABLES><TDL><TDLMESSAGE><COLLECTION NAME="Bridge Audit Stock Items" ISMODIFY="No"><TYPE>StockItem</TYPE><FETCH>{AUDIT_STOCK_ITEM_FETCH}</FETCH></COLLECTION></TDLMESSAGE></TDL></DESC></BODY></ENVELOPE>"#,
        company = xml_escape(company),
        from = xml_escape(from),
        to = xml_escape(to),
    )
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn valid_yyyymmdd(value: &str) -> bool {
    if value.len() != 8 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    let year = value[0..4].parse::<u16>().ok();
    let month = value[4..6].parse::<u8>().ok();
    let day = value[6..8].parse::<u8>().ok();
    let (Some(year), Some(month), Some(day)) = (year, month, day) else {
        return false;
    };
    if year == 0 || !(1..=12).contains(&month) {
        return false;
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let maximum_day = match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    (1..=maximum_day).contains(&day)
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        use fmt::Write as _;
        write!(&mut output, "{byte:02x}").expect("writing to a String cannot fail");
    }
    output
}

#[cfg(test)]
#[path = "xml_read_profiles_tests.rs"]
mod tests;
