//! Closed, read-only XML request profiles shared by the native app and portable tools.
//!
//! The public profile API accepts only validated company and date inputs. The
//! compatibility renderers are intentionally hidden from generated
//! documentation; they preserve the native app's existing string-based
//! function signatures while still exposing only fixed export profiles.

use std::fmt;

use sha2::{Digest, Sha256};

const TEMPLATE_COMPANY: &str = "BRIDGE TEMPLATE COMPANY";
const TEMPLATE_FROM: &str = "20000101";
const TEMPLATE_TO: &str = "20000102";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadProfileValidationError {
    CompanyInvalid,
    DateInvalid,
    DateRangeInvalid,
}

impl fmt::Display for ReadProfileValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::CompanyInvalid => "read profile company input was invalid",
            Self::DateInvalid => "read profile date input was invalid",
            Self::DateRangeInvalid => "read profile date range was invalid",
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
    LedgersV1,
    VouchersV2,
    VouchersV3,
}

impl ReadOnlyProfileId {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CompanyListV1 => "company_list_v1",
            Self::LedgersV1 => "ledgers_v1",
            Self::VouchersV2 => "vouchers_v2",
            Self::VouchersV3 => "vouchers_v3",
        }
    }

    /// SHA-256 of the exact request template rendered with fixed safe
    /// sentinels in every dynamic slot. This changes if any emitted byte in the
    /// fixed profile changes, but is independent of a live company or range.
    pub fn template_sha256(self) -> String {
        let template = match self {
            Self::CompanyListV1 => render_company_list(),
            Self::LedgersV1 => render_ledgers(TEMPLATE_COMPANY),
            Self::VouchersV2 => render_vouchers(TEMPLATE_COMPANY, TEMPLATE_FROM, TEMPLATE_TO),
            Self::VouchersV3 => {
                render_selected_vouchers(TEMPLATE_COMPANY, TEMPLATE_FROM, TEMPLATE_TO)
            }
        };
        sha256_hex(template.as_bytes())
    }
}

#[derive(Debug, Clone, Copy)]
pub enum ReadOnlyProfile<'a> {
    CompanyListV1,
    LedgersV1 {
        company: &'a ValidatedCompanyName,
    },
    VouchersV2 {
        company: &'a ValidatedCompanyName,
        range: &'a ValidatedDateRange,
    },
    VouchersV3 {
        company: &'a ValidatedCompanyName,
        range: &'a ValidatedDateRange,
    },
}

impl ReadOnlyProfile<'_> {
    pub fn id(self) -> ReadOnlyProfileId {
        match self {
            Self::CompanyListV1 => ReadOnlyProfileId::CompanyListV1,
            Self::LedgersV1 { .. } => ReadOnlyProfileId::LedgersV1,
            Self::VouchersV2 { .. } => ReadOnlyProfileId::VouchersV2,
            Self::VouchersV3 { .. } => ReadOnlyProfileId::VouchersV3,
        }
    }

    pub fn template_sha256(self) -> String {
        self.id().template_sha256()
    }

    pub fn render(self) -> String {
        match self {
            Self::CompanyListV1 => render_company_list(),
            Self::LedgersV1 { company } => render_ledgers(company.as_str()),
            Self::VouchersV2 { company, range } => {
                render_vouchers(company.as_str(), range.from_yyyymmdd(), range.to_yyyymmdd())
            }
            Self::VouchersV3 { company, range } => render_selected_vouchers(
                company.as_str(),
                range.from_yyyymmdd(),
                range.to_yyyymmdd(),
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
mod tests {
    use super::*;

    fn profiles<'a>(
        company: &'a ValidatedCompanyName,
        range: &'a ValidatedDateRange,
    ) -> [ReadOnlyProfile<'a>; 4] {
        [
            ReadOnlyProfile::CompanyListV1,
            ReadOnlyProfile::LedgersV1 { company },
            ReadOnlyProfile::VouchersV2 { company, range },
            ReadOnlyProfile::VouchersV3 { company, range },
        ]
    }

    #[test]
    fn validated_inputs_reject_arbitrary_or_invalid_values() {
        for company in ["", "  ", "line\nbreak", "\u{0}"] {
            assert_eq!(
                ValidatedCompanyName::new(company),
                Err(ReadProfileValidationError::CompanyInvalid)
            );
        }
        assert_eq!(
            ValidatedCompanyName::new("x".repeat(256)),
            Err(ReadProfileValidationError::CompanyInvalid)
        );
        for (from, to, expected) in [
            (
                "2026-01-01",
                "20260102",
                ReadProfileValidationError::DateInvalid,
            ),
            (
                "20260229",
                "20260301",
                ReadProfileValidationError::DateInvalid,
            ),
            (
                "20260402",
                "20260401",
                ReadProfileValidationError::DateRangeInvalid,
            ),
        ] {
            assert_eq!(ValidatedDateRange::new(from, to), Err(expected));
        }
        assert!(ValidatedDateRange::new("20240229", "20240229").is_ok());
    }

    #[test]
    fn closed_profiles_emit_exports_only_and_escape_dynamic_values() {
        let company = ValidatedCompanyName::new("BRIDGE & <SYNTHETIC> \"BOOK\"").unwrap();
        let range = ValidatedDateRange::new("20260401", "20260430").unwrap();
        for profile in profiles(&company, &range) {
            let request = profile.render();
            let upper = request.to_ascii_uppercase();
            assert!(upper.contains("<TALLYREQUEST>EXPORT</TALLYREQUEST>"));
            for forbidden in ["IMPORT", "CREATE", "ALTER", "DELETE"] {
                assert!(!upper.contains(&format!("<TALLYREQUEST>{forbidden}")));
            }
        }
        let ledger = ReadOnlyProfile::LedgersV1 { company: &company }.render();
        assert!(ledger.contains("BRIDGE &amp; &lt;SYNTHETIC&gt; &quot;BOOK&quot;"));
        assert!(!ledger.contains("BRIDGE & <SYNTHETIC>"));

        let injection =
            ValidatedCompanyName::new("X</SVCURRENTCOMPANY><TALLYREQUEST>IMPORT</TALLYREQUEST>")
                .unwrap();
        let escaped = ReadOnlyProfile::LedgersV1 {
            company: &injection,
        }
        .render();
        assert_eq!(escaped.matches("<TALLYREQUEST>").count(), 1);
        assert!(!escaped.contains("<TALLYREQUEST>IMPORT"));
        assert!(escaped.contains("&lt;/SVCURRENTCOMPANY&gt;"));
    }

    #[test]
    fn profile_ids_and_template_hashes_are_stable() {
        let expected = [
            (
                ReadOnlyProfileId::CompanyListV1,
                "d5c134051e1d298a278e27284fbb5ab1a9d00e0006a70f9777c4e38cebbb16de",
            ),
            (
                ReadOnlyProfileId::LedgersV1,
                "aec4ffa397fde63e82ead885f70e1327d2b5f542d7ee167e291a2e86524c17b0",
            ),
            (
                ReadOnlyProfileId::VouchersV2,
                "efd9b5f5148afff213090f57bd6bd5d3f58db6d1112ec27ef118dd29eac50385",
            ),
            (
                ReadOnlyProfileId::VouchersV3,
                "2e68f0ab8e57ded8cc1948b6785598e2f1e0947fcc431975d15fd63131df478d",
            ),
        ];
        for (profile, digest) in expected {
            assert_eq!(profile.template_sha256(), digest);
            assert_eq!(profile.template_sha256().len(), 64);
        }
    }

    #[test]
    fn compatibility_renderers_preserve_validated_profile_bytes() {
        let company = ValidatedCompanyName::new("BRIDGE SYNTHETIC BOOK").unwrap();
        let range = ValidatedDateRange::new("20260401", "20260430").unwrap();
        assert_eq!(
            compatibility::company_list_request(),
            ReadOnlyProfile::CompanyListV1.render()
        );
        assert_eq!(
            compatibility::ledgers_request(company.as_str()),
            ReadOnlyProfile::LedgersV1 { company: &company }.render()
        );
        assert_eq!(
            compatibility::vouchers_request(
                company.as_str(),
                range.from_yyyymmdd(),
                range.to_yyyymmdd(),
            ),
            ReadOnlyProfile::VouchersV2 {
                company: &company,
                range: &range,
            }
            .render()
        );
        assert_eq!(
            compatibility::selected_vouchers_request(
                company.as_str(),
                range.from_yyyymmdd(),
                range.to_yyyymmdd(),
            ),
            ReadOnlyProfile::VouchersV3 {
                company: &company,
                range: &range,
            }
            .render()
        );
    }
}
