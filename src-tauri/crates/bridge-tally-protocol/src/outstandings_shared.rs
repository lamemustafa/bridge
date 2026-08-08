//! The outstandings report contract, plus the company-identity/book-extent
//! read both outstandings read strategies pin against, before they diverge.
//!
//! Two independent strategies produce an [`OutstandingsReport`]:
//!
//! - `native_outstandings` -- always compiled -- reads Tally's own `Bills
//!   Receivable`/`Bills Payable` and `List of Ledgers` reports directly.
//! - `outstandings` -- compiled only under the `voucher-scan` feature --
//!   scans every voucher in a date/AlterID-partitioned wildcard fetch and
//!   derives outstandings from bill allocations.
//!
//! Both begin by pinning the same verified company identity and book extent
//! (`PinnedCompany`, `CompanyBookExtent`, via `parse_company_book_extent`),
//! and both end by producing the same report shape (`OutstandingsReport` and
//! its constituents). None of that is scan machinery -- no date
//! partitioning, no AlterID segmentation, no wildcard voucher fetch -- so it
//! lives here, ungated, rather than inside `outstandings`. Gating
//! `outstandings` wholesale would have taken this out with it and broken the
//! native path, which is the live product path today.
//!
//! Everything scan-specific (`DateWindow`, `AlterIdRange`, `Voucher`,
//! `LedgerEntry`, `BillAllocation`, segment/witness completeness proofs, the
//! wildcard voucher request) stays in `outstandings`, gated behind
//! `voucher-scan`.

use std::{fmt, sync::Arc};

use bridge_tally_primitives::{ExactDecimal, TallyDate};
use serde::{Deserialize, Serialize};

use crate::tolerant_xml::sanitize_invalid_numeric_references;
use crate::xml_read_profiles::ValidatedCompanyName;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutstandingsError {
    InvalidDateWindow,
    InvalidAlterIdRange,
    InvalidCompanyIdentity,
    CompanyIdentityMismatch,
    InvalidResponse(&'static str),
    InvalidAmount,
    ArithmeticOverflow,
}

impl fmt::Display for OutstandingsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidDateWindow => "outstandings date window is invalid",
            Self::InvalidAlterIdRange => "outstandings AlterID range is invalid",
            Self::InvalidCompanyIdentity => "outstandings company identity is invalid",
            Self::CompanyIdentityMismatch => "Tally returned a different company identity",
            Self::InvalidResponse(code) => code,
            Self::InvalidAmount => "Tally returned an invalid amount",
            Self::ArithmeticOverflow => "outstandings arithmetic exceeded the exact-decimal bound",
        })
    }
}

impl std::error::Error for OutstandingsError {}

#[derive(Clone, PartialEq, Eq)]
pub struct PinnedCompany {
    name: ValidatedCompanyName,
    guid: Arc<str>,
}

impl PinnedCompany {
    pub(crate) fn verified(
        name: ValidatedCompanyName,
        guid: String,
    ) -> Result<Self, OutstandingsError> {
        if guid.trim() != guid
            || guid.is_empty()
            || guid.len() > 255
            || guid.chars().any(char::is_control)
        {
            return Err(OutstandingsError::InvalidCompanyIdentity);
        }
        Ok(Self {
            name,
            guid: Arc::from(guid),
        })
    }

    pub fn name(&self) -> &str {
        self.name.as_str()
    }

    pub fn guid(&self) -> &str {
        &self.guid
    }
}

impl fmt::Debug for PinnedCompany {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PinnedCompany([verified identity])")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoucherAlterIdHighWater(u64);

impl VoucherAlterIdHighWater {
    pub fn parse(value: &str) -> Result<Self, OutstandingsError> {
        let value = value
            .trim()
            .parse::<u64>()
            .map_err(|_| OutstandingsError::InvalidResponse("company_altvchid_invalid"))?;
        Ok(Self(value))
    }

    pub fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompanyBookExtent {
    company: PinnedCompany,
    books_from: TallyDate,
    last_voucher_date: TallyDate,
    voucher_alter_id_high_water: Option<VoucherAlterIdHighWater>,
}

impl CompanyBookExtent {
    pub(crate) fn new(
        company: PinnedCompany,
        books_from: TallyDate,
        last_voucher_date: TallyDate,
        voucher_alter_id_high_water: Option<VoucherAlterIdHighWater>,
    ) -> Self {
        Self {
            company,
            books_from,
            last_voucher_date,
            voucher_alter_id_high_water,
        }
    }

    pub fn company(&self) -> &PinnedCompany {
        &self.company
    }
    pub fn books_from(&self) -> &TallyDate {
        &self.books_from
    }
    pub fn last_voucher_date(&self) -> &TallyDate {
        &self.last_voucher_date
    }
    pub fn voucher_alter_id_high_water(&self) -> Option<VoucherAlterIdHighWater> {
        self.voucher_alter_id_high_water
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AgeingBuckets {
    pub days_0_30: ExactDecimal,
    pub days_31_60: ExactDecimal,
    pub days_61_90: ExactDecimal,
    pub days_90_plus: ExactDecimal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AgeingBillCounts {
    pub days_0_30: usize,
    pub days_31_60: usize,
    pub days_61_90: usize,
    pub days_90_plus: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PartyOutstanding {
    pub party: String,
    pub receivable: ExactDecimal,
    pub payable: ExactDecimal,
    pub outstanding_total: ExactDecimal,
    /// `None` means this party's open exposure is entirely On Account, which
    /// has no bill reference and therefore no truthful bill age.
    /// TALLY_PROTOCOL_REFERENCE.md §12a.2 records that On Account is not aged;
    /// §12a.4 records that Tally strips its name.
    pub oldest_bill_age_days: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OutstandingsReport {
    pub company_name: String,
    pub as_of_yyyymmdd: String,
    pub receivable_total: ExactDecimal,
    pub payable_total: ExactDecimal,
    /// At least one observed receivable On Account allocation is included in
    /// `receivable_total` but cannot be assigned a truthful bill age.
    /// TALLY_PROTOCOL_REFERENCE.md §12a.2 records that On Account is not aged.
    pub has_unaged_receivable: bool,
    pub ageing: AgeingBuckets,
    pub open_receivable_bill_count: usize,
    pub ageing_bill_counts: AgeingBillCounts,
    pub top_parties: Vec<PartyOutstanding>,
    pub source_voucher_count: usize,
    pub source_bytes: usize,
}

// --- Wire scaffold + parsing for the paired `CompanyBookExtentV1` read. ---
//
// `outstandings::wire` re-exports the generic `Envelope`/`Header`/`Body`/
// `Data`/`Value` scaffold from here rather than duplicating it, so its own
// scan-only collections (vouchers, ledger openings) keep using the same
// names they always had.

#[derive(Deserialize)]
pub(crate) struct Envelope<T> {
    #[serde(rename = "HEADER")]
    pub(crate) header: Header,
    #[serde(rename = "BODY")]
    pub(crate) body: Body<T>,
}

#[derive(Deserialize)]
pub(crate) struct Header {
    #[serde(rename = "STATUS")]
    pub(crate) status: String,
}

#[derive(Deserialize)]
pub(crate) struct Body<T> {
    #[serde(rename = "DATA")]
    pub(crate) data: Data<T>,
}

#[derive(Deserialize)]
pub(crate) struct Data<T> {
    #[serde(rename = "COLLECTION")]
    pub(crate) collection: T,
}

#[derive(Default, Deserialize)]
pub(crate) struct Value {
    #[serde(rename = "$text", default)]
    pub(crate) text: String,
}

#[derive(Deserialize)]
struct CompanyCollection {
    #[serde(rename = "COMPANY", default)]
    companies: Vec<RawCompany>,
}

#[derive(Deserialize)]
struct RawCompany {
    #[serde(rename = "@NAME")]
    attribute_name: String,
    #[serde(rename = "NAME")]
    name: Value,
    #[serde(rename = "GUID")]
    guid: Value,
    #[serde(rename = "BOOKSFROM")]
    books_from: Value,
    #[serde(rename = "LASTVOUCHERDATE")]
    last_voucher_date: Value,
    #[serde(rename = "ALTVCHID", default)]
    alter_voucher_id: Option<Value>,
}

pub fn parse_company_book_extent(
    xml: &str,
    expected_name: &str,
    expected_guid: &str,
) -> Result<CompanyBookExtent, OutstandingsError> {
    require_complete_envelope(xml)?;
    let sanitized = sanitize_invalid_numeric_references(xml);
    let parsed: Envelope<CompanyCollection> = quick_xml::de::from_str(&sanitized)
        .map_err(|_| OutstandingsError::InvalidResponse("company_extent_xml_invalid"))?;
    require_success(&parsed.header)?;
    let mut matching = parsed
        .body
        .data
        .collection
        .companies
        .into_iter()
        .filter(|raw| raw.guid.text.trim().eq_ignore_ascii_case(expected_guid));
    let raw = matching
        .next()
        .ok_or(OutstandingsError::CompanyIdentityMismatch)?;
    if matching.next().is_some() {
        return Err(OutstandingsError::InvalidResponse(
            "company_identity_ambiguous",
        ));
    }
    let name = required(raw.name.text, "company_name_missing")?;
    let guid = required(raw.guid.text, "company_guid_missing")?;
    if raw.attribute_name != name
        || name != expected_name
        || !guid.eq_ignore_ascii_case(expected_guid)
    {
        return Err(OutstandingsError::CompanyIdentityMismatch);
    }
    let name =
        ValidatedCompanyName::new(name).map_err(|_| OutstandingsError::InvalidCompanyIdentity)?;
    let company = PinnedCompany::verified(name, guid)?;
    let books_from = parse_date(raw.books_from.text)?;
    let last_voucher_date = parse_date(raw.last_voucher_date.text)?;
    let voucher_alter_id_high_water = raw
        .alter_voucher_id
        .map(|value| VoucherAlterIdHighWater::parse(&value.text))
        .transpose()?;
    if books_from > last_voucher_date {
        return Err(OutstandingsError::InvalidResponse(
            "company_extent_reversed",
        ));
    }
    Ok(CompanyBookExtent::new(
        company,
        books_from,
        last_voucher_date,
        voucher_alter_id_high_water,
    ))
}

fn require_complete_envelope(xml: &str) -> Result<(), OutstandingsError> {
    if !xml.trim_end().ends_with("</ENVELOPE>") {
        return Err(OutstandingsError::InvalidResponse("response_truncated"));
    }
    Ok(())
}

fn require_success(header: &Header) -> Result<(), OutstandingsError> {
    if header.status.trim() == "1" {
        Ok(())
    } else {
        Err(OutstandingsError::InvalidResponse(
            "tally_status_not_success",
        ))
    }
}

fn required(value: String, code: &'static str) -> Result<String, OutstandingsError> {
    let value = value.trim().to_string();
    if value.is_empty() {
        Err(OutstandingsError::InvalidResponse(code))
    } else {
        Ok(value)
    }
}

fn parse_date(value: String) -> Result<TallyDate, OutstandingsError> {
    TallyDate::parse(value.trim().to_string())
        .map_err(|_| OutstandingsError::InvalidResponse("tally_date_invalid"))
}

// --- Request rendering for the paired `CompanyBookExtentV1` read. ---

const COMPANY_EXTENT_COLLECTION_NAME: &str = "BridgeCompanyBookExtentV1";
const COMPANY_EXTENT_FETCH: &str = "Name, GUID, BooksFrom, LastVoucherDate, ALTVCHID";

pub(crate) fn render_company_book_extent(company: &str) -> String {
    format!(
        r#"<ENVELOPE>
  <HEADER><VERSION>1</VERSION><TALLYREQUEST>Export</TALLYREQUEST><TYPE>Collection</TYPE><ID>{collection}</ID></HEADER>
  <BODY><DESC>
    <STATICVARIABLES><SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT><SVCURRENTCOMPANY>{company}</SVCURRENTCOMPANY></STATICVARIABLES>
    <TDL><TDLMESSAGE><COLLECTION NAME="{collection}" ISMODIFY="No"><TYPE>Company</TYPE><FETCH>{fetch}</FETCH></COLLECTION></TDLMESSAGE></TDL>
  </DESC></BODY>
</ENVELOPE>"#,
        collection = COMPANY_EXTENT_COLLECTION_NAME,
        company = xml_escape(company),
        fetch = COMPANY_EXTENT_FETCH,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn company_book_extent_template_has_only_the_verified_fetch_list() {
        let xml = render_company_book_extent("Synthetic & Company");
        assert!(xml.contains("<ID>BridgeCompanyBookExtentV1</ID>"));
        assert!(xml.contains("ALTVCHID"));
        assert!(xml.contains("Synthetic &amp; Company"));
        assert!(!xml.contains("<COMPUTE>"));
        assert!(!xml.contains("$$NumItems"));
    }
}
