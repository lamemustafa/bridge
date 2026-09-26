//! Portable decoding and strict application-level parsing for Tally responses.
//!
//! This crate has no HTTP, database, native-library, or Tauri dependency. HTTP
//! success must be checked separately. Every parser that produces durable or
//! qualification evidence requires Tally application `STATUS=1`; interactive
//! company discovery additionally accepts one strict, direct report shape for
//! documented compatibility.
//!
//! Tally's responses carry numeric character references that XML 1.0 forbids,
//! `&#4;` above all. Before its native collection parsers read a response, this
//! crate marks each one with [`mark_forbidden_numeric_references`]: the
//! reference becomes the literal text U+FFFD `#` *n* `;` (so `&#4; Primary`
//! reads as [`TALLY_SANITIZED_ROOT_MARKER`] followed by ` Primary`), and a
//! U+FFFD already in the text that could be mistaken for a marker becomes
//! U+FFFD `#65533;`, which keeps the rewrite reversible. Raw characters are not
//! touched. The rule is recorded in `docs/tally/TALLY_PROTOCOL_REFERENCE.md`
//! §1.1(d).

use std::{
    collections::{HashMap, HashSet},
    fmt::Write as _,
};

use quick_xml::{events::Event, name::QName, Reader};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Admission of the tally-read v1 `company` part (audit_read plan step 3).
pub mod audit_company_part;
#[cfg(feature = "bills-native-outstandings-probe")]
pub mod bills_native_outstandings_probe;
#[cfg(feature = "bills-payments-observation-parser")]
pub mod bills_payments_observation;
pub mod group_ancestry;
pub mod gst_registration;
mod import_outcome;
#[cfg(feature = "india-tax-observation-parser")]
pub mod india_tax_observation;
#[cfg(feature = "jsonex-parser")]
pub mod jsonex;
#[cfg(feature = "jsonex-request-builder")]
pub mod jsonex_request;
mod native_ledger_collection;
pub mod native_outstandings;
pub mod native_trial_balance;
/// The legacy voucher-scan outstandings path: date/AlterID-partitioned
/// wildcard voucher fetch, segment/witness completeness proofs, and bill
/// computation from voucher allocations. Superseded by `native_outstandings`,
/// which is always compiled. Gated because it cannot execute in a shipped
/// build: the only non-test constructor for its width calibration is behind
/// `live-calibration-harness`, which implies this feature.
#[cfg(feature = "voucher-scan")]
pub mod outstandings;
/// The outstandings report contract and the company-identity/book-extent
/// read shared by `outstandings` and `native_outstandings`. Deliberately
/// ungated: `native_outstandings` depends on it, so gating it with
/// `outstandings` would break the always-on native path. See the module docs
/// for why this is scoped the way it is.
pub mod outstandings_shared;
mod standard_ledger_catalog;
mod text_encoding;
mod tolerant_xml;
pub mod xml_read_profiles;

pub use import_outcome::{
    parse_import_evidence, parse_import_outcome, parse_import_result, ParsedImportEvidence,
    TallyImportApplicationStatus, TallyImportCounterPresence, TallyImportOutcome,
    TallyImportResult, TallyLineError, MAX_TALLY_LINE_ERRORS, MAX_TALLY_LINE_ERROR_BYTES,
    MAX_TALLY_LINE_ERROR_CHARS,
};
pub use native_ledger_collection::{
    parse_native_ledger_source_records_with_evidence,
    parse_native_party_ledger_master_records_with_evidence, GstDutyHead, GstDutyHeadObservation,
    NativeLedgerAmountError, PartyLedgerMasterFields, PartyLedgerMasterRecord,
};
pub use standard_ledger_catalog::{
    parse_standard_ledger_catalog, parse_standard_ledger_catalog_with_identities,
    parse_standard_ledger_identity_observation, StandardLedgerCatalog,
    StandardLedgerCatalogBinding, StandardLedgerCatalogError, StandardLedgerIdentityObservation,
    MAX_STANDARD_LEDGER_IDENTITY_ROWS,
};
pub use text_encoding::{
    decode_tally_text_bytes_limited, decode_tally_xml_response_bytes_limited, decode_xml_bytes,
    decode_xml_bytes_limited, encode_tally_xml_request_utf16le,
    validate_tally_xml_response_content_type, DecodedTallyText, ExpectedTallyTextEncoding,
    StreamDecodedTallyText, TallyTextDecodeError, TallyTextEncoding, TallyTextStreamDecoder,
};
pub use tolerant_xml::mark_forbidden_numeric_references;

pub const BRIDGE_LEDGER_EXPORT_SCHEMA: &str = "bridge.tally.ledgers/1";
pub const BRIDGE_LEDGER_WRITE_READBACK_SCHEMA: &str = "bridge.tally.ledger-write-readback/1";
pub const BRIDGE_GROUP_EXPORT_SCHEMA: &str = "bridge.tally.groups/1";
pub const BRIDGE_VOUCHER_TYPE_EXPORT_SCHEMA: &str = "bridge.tally.voucher-types/1";
pub const BRIDGE_VOUCHER_EXPORT_SCHEMA: &str = "bridge.tally.vouchers/2";
pub const BRIDGE_SELECTED_VOUCHER_EXPORT_SCHEMA: &str = "bridge.tally.vouchers/3";
pub const BRIDGE_LEDGER_PERIOD_BALANCE_SCHEMA: &str = "bridge.tally.ledger-period-balances/1";
pub const MAX_INTERACTIVE_DISCOVERY_COMPANIES: usize = 100;

/// The sanitized representation of Tally's U+0004 metadata prefix.
///
/// [`mark_forbidden_numeric_references`] produces this exact form for an
/// illegal `&#4;` reference; literal U+FFFD source text that could collide with
/// it remains distinguishable as `U+FFFD#65533;`.
pub const TALLY_SANITIZED_ROOT_MARKER: &str = "\u{fffd}#4;";

/// Whether decoded Tally text names the reserved top-level root.
///
/// Tally writes its root as `&#4; Primary`, and every Bridge decoder reads
/// that as [`TALLY_SANITIZED_ROOT_MARKER`] followed by ` Primary`
/// (`docs/tally/TALLY_PROTOCOL_REFERENCE.md` §1.1(d)). Only that marked form
/// is the root: after trimming, the text must start with the marker, and the
/// rest, trimmed again, must be `Primary` (ASCII case ignored). A bare
/// `Primary` names a group a user called that, and a chain walks through it
/// like any other group. The marker also prefixes Tally's other reserved
/// values (`&#4; Resave`, `&#4; Not Applicable`), which are not the root.
pub fn is_tally_reserved_root(value: &str) -> bool {
    value
        .trim()
        .strip_prefix(TALLY_SANITIZED_ROOT_MARKER)
        .is_some_and(|rest| rest.trim().eq_ignore_ascii_case("primary"))
}

#[derive(Debug, Deserialize, Serialize)]
pub struct TallyEnvelope<T> {
    #[serde(rename = "BODY")]
    pub body: Option<T>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct TallyCompany {
    pub name: String,
    pub guid: Option<String>,
    /// Tally's data-folder number, observed as one component of the company
    /// identity tuple. It is not independently stable enough to be a key.
    pub company_number: Option<String>,
    /// The book opening date returned by the Company collection, in Tally's
    /// canonical `YYYYMMDD` form.
    pub books_from: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct TallyLedger {
    pub name: String,
    /// Preserve whether Tally returned the parent group rather than rendering
    /// an unobserved group as an empty workbook cell.
    pub parent: PartyLedgerMasterFieldObservation,
    /// The shared ledger read requests PARTYGSTIN. Preserve whether Tally
    /// omitted it instead of collapsing that fact into an empty GSTIN.
    pub party_gstin: PartyLedgerMasterFieldObservation,
    pub opening_balance: Option<String>,
}

/// What Tally actually exposed for one requested party/ledger export field.
///
/// `NotObserved` is deliberately not an empty value: a collection response
/// cannot distinguish an unset field from one unavailable in this Tally build.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(untagged)]
pub enum PartyLedgerMasterFieldObservation {
    Returned(String),
    #[default]
    NotObserved,
}

impl PartyLedgerMasterFieldObservation {
    pub const NOT_OBSERVED_WORKBOOK_TEXT: &str = "Not observed";
    pub const NOT_OBSERVED_WORKBOOK_DISCLOSURE: &str = "“Not observed” means this Tally response did not return the requested field; it does not establish whether that field is unset in this book or unavailable in this Tally build. Bridge never manufactures master data.";

    pub fn workbook_text(&self) -> &str {
        match self {
            Self::Returned(value) => value,
            Self::NotObserved => Self::NOT_OBSERVED_WORKBOOK_TEXT,
        }
    }

    /// The observed text, including an explicitly returned empty value.
    pub fn returned_text(&self) -> Option<&str> {
        match self {
            Self::Returned(value) => Some(value),
            Self::NotObserved => None,
        }
    }

    /// The legacy hierarchy value: explicit empty and unobserved values do
    /// not name a parent. Presentation must use `workbook_text` instead.
    pub fn nonempty_returned_text(&self) -> Option<&str> {
        self.returned_text().filter(|value| !value.is_empty())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct TallyLedgerPeriodBalance {
    pub opening_balance: String,
    pub closing_balance: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct LedgerPeriodBalanceContext {
    pub company_guid: String,
    pub from_yyyymmdd: String,
    pub to_yyyymmdd: String,
    /// Bridge's requested comparison profile echoed by the custom report.
    /// This is request binding, not source-observed proof of Tally semantics.
    pub ordinary_books_requested: bool,
    pub source_record_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ParsedLedgerPeriodBalanceReport {
    pub context: LedgerPeriodBalanceContext,
    pub records: Vec<ParsedSourceRecord<TallyLedgerPeriodBalance>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct TallyNamedMaster {
    pub name: String,
    pub parent: PartyLedgerMasterFieldObservation,
    /// Tally's `RESERVEDNAME` attribute: the immutable identity of a
    /// predefined group (e.g. `"Sundry Debtors"`), untouched by renaming the
    /// group's `name`. Distinguished three ways by readers that populate it:
    /// `Some(non-empty)` is a trustworthy predefined identity; `Some("")` is
    /// Tally's own explicit signal that the row is a user-created master,
    /// definitively not predefined; `None` means this reader never captured
    /// the attribute at all (either an older capture, or a master kind this
    /// crate does not read `RESERVEDNAME` for), carrying no signal either
    /// way. Readers that do not parse `RESERVEDNAME` leave this `None`.
    #[serde(default)]
    pub reserved_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct TallyLedgerEntry {
    pub entry_index: u64,
    pub ledger_name: String,
    pub amount: String,
    pub is_deemed_positive: bool,
    pub raw_source_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct TallyVoucher {
    pub id: Option<String>,
    pub date: Option<String>,
    pub voucher_type: Option<String>,
    pub voucher_number: Option<String>,
    pub party_ledger_name: Option<String>,
    pub cancelled: Option<bool>,
    pub optional: Option<bool>,
    pub ledger_entry_count: Option<u64>,
    pub ledger_entries: Vec<TallyLedgerEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TallyExportStatus {
    Success,
    Failure,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
pub struct CompanyContextEvidence {
    pub name: Option<String>,
    pub guid: Option<String>,
    /// Echo of the exact requested identity set for scoped write readbacks.
    pub query_identity_set_sha256: Option<String>,
    /// Bridge request-binding echoes; they are not independent proof that Tally honored filters.
    pub requested_from_yyyymmdd: Option<String>,
    pub requested_to_yyyymmdd: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct DuplicateIdentityEvidence {
    pub identity_sha256: String,
    pub occurrences: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
pub struct ExportEvidence {
    pub company_context: Option<CompanyContextEvidence>,
    pub schema: Option<String>,
    pub object_type: Option<String>,
    /// A count Bridge obtained by parsing rows. It is not a Tally-reported
    /// count and must never be promoted to source-count evidence.
    pub observed_record_count: Option<u64>,
    pub source_record_count: Option<u64>,
    pub identified_record_count: u64,
    pub duplicate_identities: Vec<DuplicateIdentityEvidence>,
    /// Native ledger collections carry a master GUID on each row rather than
    /// a report-envelope company GUID. These counters retain the observed
    /// binding evidence without assuming every imported master has the local
    /// company prefix.
    pub company_guid_prefix_match_count: u64,
    pub company_guid_prefix_mismatch_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ParsedExport<T> {
    pub records: Vec<T>,
    pub evidence: ExportEvidence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ParsedSourceIdentityKind {
    Guid,
    RemoteId,
    MasterId,
    /// The source response carries no durable master identifier. Callers may
    /// use a documented deterministic fallback only where the domain makes
    /// that identity safe.
    Fallback,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
pub struct ParsedSourceIdentities {
    pub guid: Option<String>,
    pub remote_id: Option<String>,
    pub master_id: Option<String>,
}

/// A parsed row plus source evidence that must survive canonicalisation. The hash covers the
/// exact XML record fragment consumed by the row parser, never a reconstructed representation.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ParsedSourceRecord<T> {
    pub record: T,
    pub source_id: Option<String>,
    pub identity_kind: Option<ParsedSourceIdentityKind>,
    pub identities: ParsedSourceIdentities,
    pub alter_id: Option<String>,
    pub raw_source_sha256: String,
}
pub fn parse_xml<T>(xml: &str) -> anyhow::Result<T>
where
    T: for<'de> Deserialize<'de>,
{
    Ok(quick_xml::de::from_str(xml)?)
}

pub fn parse_companies(xml: &str) -> anyhow::Result<Vec<TallyCompany>> {
    Ok(parse_companies_with_evidence(xml)?.records)
}

pub fn parse_companies_with_evidence(xml: &str) -> anyhow::Result<ParsedExport<TallyCompany>> {
    validate_export_response(xml)?;
    let evidence = scan_export_evidence(xml)?;
    let records = parse_company_rows(xml)?;
    Ok(ParsedExport { records, evidence })
}

/// Parses the observed direct company-report form for an interactive setup
/// probe. This must not be used for qualification, synchronization, or other
/// evidence that requires Tally's shaped `HEADER/STATUS=1` success response.
pub fn parse_companies_for_interactive_discovery(xml: &str) -> anyhow::Result<Vec<TallyCompany>> {
    validate_company_list_response(xml)?;
    parse_company_rows_with_limit(xml, Some(MAX_INTERACTIVE_DISCOVERY_COMPANIES))
}

/// Parses Tally's documented `Company` collection shape returned by
/// `ReadOnlyProfile::CompanyListV2` (`<DATA><COLLECTION><COMPANY NAME="..">`
/// rows, each carrying a nested `<GUID>` element). Unlike
/// `parse_companies_for_interactive_discovery`, this requires the ordinary
/// `HEADER/STATUS=1` export success envelope — the trust check is satisfied,
/// not bypassed.
///
/// Company rows are read only from beneath `ENVELOPE/BODY/DATA/COLLECTION`.
/// A real response also carries a `BODY/DESC/CMPINFO` block of bare counter
/// elements, including a `<COMPANY>0</COMPANY>` row count; because that block
/// sits outside `DATA`, it is never mistaken for a company row.
pub fn parse_companies_from_collection(xml: &str) -> anyhow::Result<Vec<TallyCompany>> {
    validate_export_response(xml)?;
    parse_company_collection_rows(xml)
}

/// Product and licence-mode facts returned by the fixed `CompanyListV2`
/// collection. The collection repeats endpoint-wide facts for every loaded
/// company, so this parser requires all rows to agree before exposing them.
/// Missing, empty or disagreeing optional release fields leave release unknown
/// without discarding the independently agreed product and licence facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompanyGatewayCapabilityObservation {
    pub product: String,
    pub release: Option<String>,
    pub educational_mode: bool,
    pub silver: bool,
    pub gold: bool,
}

/// Parses the gateway capability fields from `CompanyListV2`. A successful
/// company listing remains usable if this stricter parser fails: callers must
/// retain mode-agnostic behaviour and report the unavailable evidence rather
/// than treating an unproven licence mode as licensed.
pub fn parse_company_gateway_capability_observation(
    xml: &str,
) -> anyhow::Result<CompanyGatewayCapabilityObservation> {
    validate_export_response(xml)?;
    let mut reader = configured_reader(xml);
    let mut path = Vec::<Vec<u8>>::new();
    let mut observation: Option<CompanyGatewayCapabilityObservation> = None;
    let mut release_agrees = true;
    loop {
        match reader.read_event()? {
            Event::Start(element)
                if path_eq(&path, &[b"ENVELOPE", b"BODY", b"DATA", b"COLLECTION"])
                    && element.name().as_ref().eq_ignore_ascii_case(b"COMPANY") =>
            {
                let parsed = parse_company_gateway_capability_row(&mut reader, &element)?;
                if let Some(previous) = &observation {
                    release_agrees &= previous.release == parsed.release;
                    if previous.product != parsed.product
                        || previous.educational_mode != parsed.educational_mode
                        || previous.silver != parsed.silver
                        || previous.gold != parsed.gold
                    {
                        anyhow::bail!(
                            "company collection reported inconsistent gateway capabilities"
                        );
                    }
                } else {
                    observation = Some(parsed);
                }
            }
            Event::Start(element) => path.push(element.name().as_ref().to_ascii_uppercase()),
            Event::End(element) => pop_expected_path(&mut path, element.name().as_ref())?,
            Event::Eof => break,
            _ => {}
        }
    }
    if !path.is_empty() {
        anyhow::bail!("company capability response ended before its root closed");
    }
    let mut observation = observation
        .ok_or_else(|| anyhow::anyhow!("company capability response omitted company rows"))?;
    if !release_agrees {
        observation.release = None;
    }
    Ok(observation)
}

fn normalized_standard_value(value: &str, label: &str) -> anyhow::Result<String> {
    let value = value.trim();
    if value.is_empty() || value.len() > 255 || value.chars().any(char::is_control) {
        anyhow::bail!("standard ledger collection contained an invalid {label}");
    }
    Ok(value.to_string())
}

fn normalized_standard_company_guid(value: &str) -> anyhow::Result<String> {
    let value = value.trim();
    if value.is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
        anyhow::bail!("standard ledger collection contained an invalid company GUID");
    }
    Ok(value.to_string())
}

fn parse_company_rows(xml: &str) -> anyhow::Result<Vec<TallyCompany>> {
    parse_company_rows_with_limit(xml, None)
}

fn parse_company_rows_with_limit(
    xml: &str,
    max_records: Option<usize>,
) -> anyhow::Result<Vec<TallyCompany>> {
    let mut reader = configured_reader(xml);
    let mut records = Vec::new();
    loop {
        match reader.read_event()? {
            Event::Start(element)
                if element.name().as_ref().eq_ignore_ascii_case(b"COMPANYINFO") =>
            {
                if max_records.is_some_and(|limit| records.len() >= limit) {
                    anyhow::bail!(
                        "interactive discovery listing limit exceeded: Tally returned more than {MAX_INTERACTIVE_DISCOVERY_COMPANIES} local companies; the unverified listing was not retained"
                    );
                }
                records.push(parse_company_info(&mut reader)?);
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(records)
}

/// Reads `<COMPANY>` rows strictly from beneath `ENVELOPE/BODY/DATA/COLLECTION`.
/// This scoping is deliberate: `BODY/DESC/CMPINFO` also carries a bare
/// `<COMPANY>0</COMPANY>` object counter, and scanning for the element name
/// anywhere in the document would misread that counter as a company row.
fn parse_company_collection_rows(xml: &str) -> anyhow::Result<Vec<TallyCompany>> {
    let mut reader = configured_reader(xml);
    let mut path = Vec::<Vec<u8>>::new();
    let mut collection_seen = false;
    let mut records = Vec::new();
    loop {
        match reader.read_event()? {
            Event::Start(element)
                if path_eq(&path, &[b"ENVELOPE", b"BODY", b"DATA", b"COLLECTION"])
                    && element.name().as_ref().eq_ignore_ascii_case(b"COMPANY") =>
            {
                if records.len() >= MAX_INTERACTIVE_DISCOVERY_COMPANIES {
                    anyhow::bail!(
                        "company collection exceeded the safe row limit: Tally returned more than {MAX_INTERACTIVE_DISCOVERY_COMPANIES} companies"
                    );
                }
                records.push(parse_company_collection_row(&mut reader, &element)?);
            }
            Event::Empty(element)
                if path_eq(&path, &[b"ENVELOPE", b"BODY", b"DATA", b"COLLECTION"])
                    && element.name().as_ref().eq_ignore_ascii_case(b"COMPANY") =>
            {
                anyhow::bail!("company collection omitted the company GUID");
            }
            Event::Start(element) => {
                if path_eq(&path, &[b"ENVELOPE", b"BODY", b"DATA"])
                    && element.name().as_ref().eq_ignore_ascii_case(b"COLLECTION")
                {
                    collection_seen = true;
                }
                path.push(element.name().as_ref().to_ascii_uppercase());
            }
            Event::Empty(element)
                if path_eq(&path, &[b"ENVELOPE", b"BODY", b"DATA"])
                    && element.name().as_ref().eq_ignore_ascii_case(b"COLLECTION") =>
            {
                collection_seen = true;
            }
            Event::End(element) => pop_expected_path(&mut path, element.name().as_ref())?,
            Event::Eof => break,
            _ => {}
        }
    }
    if !path.is_empty() {
        anyhow::bail!("company collection response ended before its root closed");
    }
    if !collection_seen {
        anyhow::bail!("company collection response omitted BODY/DATA/COLLECTION");
    }
    Ok(records)
}

fn parse_company_collection_row(
    reader: &mut Reader<&[u8]>,
    element: &quick_xml::events::BytesStart<'_>,
) -> anyhow::Result<TallyCompany> {
    validate_only_attributes(element, &[b"NAME", b"RESERVEDNAME"])?;
    let name = attr_value(reader, element, b"NAME")
        .map(|value| normalized_standard_value(&value, "company name"))
        .transpose()?
        .ok_or_else(|| anyhow::anyhow!("company collection omitted the company name"))?;
    let row_name = element.name().as_ref().to_ascii_uppercase();
    let mut guid = None::<String>;
    let mut company_number = None::<String>;
    let mut books_from = None::<String>;
    loop {
        match reader.read_event()? {
            Event::Start(child) if child.name().as_ref().eq_ignore_ascii_case(b"GUID") => {
                validate_only_attributes(&child, &[b"TYPE"])?;
                if guid
                    .replace(normalized_standard_value(
                        &read_required_text(reader, child.name())?,
                        "company GUID",
                    )?)
                    .is_some()
                {
                    anyhow::bail!("company collection repeated the company GUID");
                }
            }
            Event::Empty(child) if child.name().as_ref().eq_ignore_ascii_case(b"GUID") => {
                anyhow::bail!("company collection contained an empty company GUID");
            }
            Event::Start(child) if child.name().as_ref().eq_ignore_ascii_case(b"COMPANYNUMBER") => {
                validate_only_attributes(&child, &[b"TYPE"])?;
                if company_number
                    .replace(normalized_standard_value(
                        &read_required_text(reader, child.name())?,
                        "company number",
                    )?)
                    .is_some()
                {
                    anyhow::bail!("company collection repeated the company number");
                }
            }
            Event::Empty(child) if child.name().as_ref().eq_ignore_ascii_case(b"COMPANYNUMBER") => {
                anyhow::bail!("company collection contained an empty company number");
            }
            Event::Start(child) if child.name().as_ref().eq_ignore_ascii_case(b"BOOKSFROM") => {
                validate_only_attributes(&child, &[b"TYPE"])?;
                if books_from
                    .replace(normalized_standard_value(
                        &read_required_text(reader, child.name())?,
                        "company books from",
                    )?)
                    .is_some()
                {
                    anyhow::bail!("company collection repeated the books-from date");
                }
            }
            Event::Empty(child) if child.name().as_ref().eq_ignore_ascii_case(b"BOOKSFROM") => {
                anyhow::bail!("company collection contained an empty books-from date");
            }
            Event::End(end) if end.name().as_ref().eq_ignore_ascii_case(&row_name) => break,
            // Tally echoes the company name as a CHILD element as well as the
            // row attribute, and may carry other descriptive fields. Skipping
            // an unrecognised child is safe here because identity comes from
            // the NAME attribute and the GUID element, both of which are
            // required below -- whereas rejecting the row outright made every
            // real response unparseable while a hand-written fixture passed.
            // (Measured 2026-08-07: the live row is
            // `<COMPANY NAME="..." RESERVEDNAME=""><NAME .../><GUID .../></COMPANY>`.)
            Event::Start(child) => {
                let name = child.name().as_ref().to_vec();
                reader.read_to_end(quick_xml::name::QName(&name).to_owned())?;
            }
            Event::Empty(_) => {}
            Event::Text(text) if !text.decode()?.trim().is_empty() => {
                anyhow::bail!("company collection row contained unexpected text")
            }
            Event::CData(_) | Event::DocType(_) | Event::PI(_) => {
                anyhow::bail!("company collection row contained a forbidden XML construct")
            }
            Event::Eof => anyhow::bail!("company collection row ended before COMPANY closed"),
            _ => {}
        }
    }
    let guid =
        guid.ok_or_else(|| anyhow::anyhow!("company collection omitted the company GUID"))?;
    Ok(TallyCompany {
        name,
        guid: Some(guid),
        company_number,
        books_from,
    })
}

fn parse_company_gateway_capability_row(
    reader: &mut Reader<&[u8]>,
    element: &quick_xml::events::BytesStart<'_>,
) -> anyhow::Result<CompanyGatewayCapabilityObservation> {
    validate_only_attributes(element, &[b"NAME", b"RESERVEDNAME"])?;
    let row_name = element.name().as_ref().to_ascii_uppercase();
    let mut product = None;
    let mut release = None;
    let mut educational_mode = None;
    let mut silver = None;
    let mut gold = None;
    loop {
        match reader.read_event()? {
            Event::Start(child) => {
                validate_only_attributes(&child, &[b"TYPE"])?;
                let child_name = child.name().as_ref().to_ascii_uppercase();
                if child_name == b"BRIDGERELEASE" {
                    let value = read_optional_text(reader, child.name())?
                        .map(|value| normalized_standard_value(&value, "release"))
                        .transpose()?;
                    set_once(&mut release, value)?;
                    continue;
                }
                let value = read_required_text(reader, child.name())?;
                match child_name.as_slice() {
                    b"PRODUCTNAME" => set_once(
                        &mut product,
                        normalized_standard_value(&value, "product name")?,
                    )?,
                    b"EDUMODE" => set_once(
                        &mut educational_mode,
                        parse_gateway_yes_no(&value, "educational mode")?,
                    )?,
                    b"SILVER" => {
                        set_once(&mut silver, parse_gateway_yes_no(&value, "Silver licence")?)?
                    }
                    b"GOLD" => set_once(&mut gold, parse_gateway_yes_no(&value, "Gold licence")?)?,
                    _ => {}
                }
            }
            Event::Empty(child) => {
                let child_name = child.name().as_ref().to_ascii_uppercase();
                if child_name == b"BRIDGERELEASE" {
                    validate_only_attributes(&child, &[b"TYPE"])?;
                    set_once(&mut release, None)?;
                }
                if matches!(
                    child_name.as_slice(),
                    b"PRODUCTNAME" | b"EDUMODE" | b"SILVER" | b"GOLD"
                ) {
                    anyhow::bail!(
                        "company capability collection contained an empty required field"
                    );
                }
            }
            Event::End(end) if end.name().as_ref().eq_ignore_ascii_case(&row_name) => break,
            Event::Text(text) if !text.decode()?.trim().is_empty() => {
                anyhow::bail!("company capability collection row contained unexpected text")
            }
            Event::CData(_) | Event::DocType(_) | Event::PI(_) => {
                anyhow::bail!(
                    "company capability collection row contained a forbidden XML construct"
                )
            }
            Event::Eof => {
                anyhow::bail!("company capability collection row ended before COMPANY closed")
            }
            _ => {}
        }
    }
    Ok(CompanyGatewayCapabilityObservation {
        release: release.flatten(),
        product: product
            .ok_or_else(|| anyhow::anyhow!("company capability collection omitted PRODUCTNAME"))?,
        educational_mode: educational_mode
            .ok_or_else(|| anyhow::anyhow!("company capability collection omitted EDUMODE"))?,
        silver: silver
            .ok_or_else(|| anyhow::anyhow!("company capability collection omitted SILVER"))?,
        gold: gold.ok_or_else(|| anyhow::anyhow!("company capability collection omitted GOLD"))?,
    })
}

/// Whether a `CompanyListV2` response may come from an Education-mode endpoint:
/// some `EDUMODE` field says anything other than `No`, including `Yes`, an
/// empty field or an unrecognised value.
///
/// Deliberately looser than [`parse_company_gateway_capability_observation`],
/// which fails the whole observation when any product or licence field is
/// empty, missing or inconsistent. A caller that restricts reads in Education
/// mode must not have that restriction switched off by a field it cannot parse,
/// so for this one question an uncertain `EDUMODE` counts as Education. A
/// response with no `EDUMODE` field at all returns `false`: nothing in it
/// speaks to the mode. A live Education instance was observed on 22 Sep 2026
/// reporting `EDUMODE=Yes` alongside `SILVER=Yes` and `GOLD=No` (bridge#581).
///
/// Only fields inside the collection's `DATA` are read, as the strict parser
/// reads rows only beneath `ENVELOPE/BODY/DATA/COLLECTION`: the `DESC/CMPINFO`
/// counter block is never taken for a company row.
pub fn company_list_may_be_in_educational_mode(xml: &str) -> bool {
    let mut reader = configured_reader(xml);
    let mut path = Vec::<Vec<u8>>::new();
    loop {
        let in_data = path.iter().any(|name| name.as_slice() == b"DATA");
        match reader.read_event() {
            Ok(Event::Start(element))
                if in_data && element.name().as_ref().eq_ignore_ascii_case(b"EDUMODE") =>
            {
                let licensed = reader
                    .read_text(element.name())
                    .ok()
                    .and_then(|text| {
                        text.decode()
                            .ok()
                            .map(|text| text.trim().eq_ignore_ascii_case("no"))
                    })
                    .unwrap_or(false);
                if !licensed {
                    return true;
                }
            }
            Ok(Event::Empty(element))
                if in_data && element.name().as_ref().eq_ignore_ascii_case(b"EDUMODE") =>
            {
                return true;
            }
            Ok(Event::Start(element)) => path.push(element.name().as_ref().to_ascii_uppercase()),
            Ok(Event::End(_)) => {
                path.pop();
            }
            Ok(Event::Eof) | Err(_) => return false,
            _ => {}
        }
    }
}

fn parse_gateway_yes_no(value: &str, label: &str) -> anyhow::Result<bool> {
    if value.eq_ignore_ascii_case("yes") {
        Ok(true)
    } else if value.eq_ignore_ascii_case("no") {
        Ok(false)
    } else {
        anyhow::bail!("company capability collection returned an invalid {label} flag")
    }
}

pub fn parse_group_source_records_with_evidence(
    xml: &str,
) -> anyhow::Result<ParsedExport<ParsedSourceRecord<TallyNamedMaster>>> {
    parse_named_master_source_records(xml, b"GROUP", BRIDGE_GROUP_EXPORT_SCHEMA, "GROUP")
}

pub fn parse_voucher_type_source_records_with_evidence(
    xml: &str,
) -> anyhow::Result<ParsedExport<ParsedSourceRecord<TallyNamedMaster>>> {
    parse_named_master_source_records(
        xml,
        b"VOUCHERTYPE",
        BRIDGE_VOUCHER_TYPE_EXPORT_SCHEMA,
        "VOUCHERTYPE",
    )
}

fn parse_named_master_source_records(
    xml: &str,
    element_name: &[u8],
    schema: &str,
    object_type: &str,
) -> anyhow::Result<ParsedExport<ParsedSourceRecord<TallyNamedMaster>>> {
    // Mark forbidden references before anything reads the text, as the native
    // collection parsers do (§1.1(d)), so `&#4; Primary` reads as the marker
    // and not as a raw U+0004. Row hashes still attest the original bytes.
    let sanitized = tolerant_xml::sanitize_invalid_numeric_references_with_provenance(xml);
    let text = sanitized.as_str();
    validate_export_response(text)?;
    let evidence = scan_export_evidence(text)?;
    let mut reader = configured_reader(text);
    let mut path = Vec::<Vec<u8>>::new();
    let mut records = Vec::new();
    loop {
        let record_start = reader.buffer_position() as usize;
        match reader.read_event()? {
            Event::Start(element)
                if is_supported_export_parent(&path)
                    && element.name().as_ref().eq_ignore_ascii_case(element_name) =>
            {
                validate_only_attributes(
                    &element,
                    &[b"NAME", b"GUID", b"REMOTEID", b"MASTERID", b"ALTERID"],
                )?;
                let identities = parsed_source_identities(&reader, &element)?;
                let (source_id, identity_kind) = preferred_identity(&identities);
                let alter_id = attr_value(&reader, &element, b"ALTERID");
                let name = attr_value(&reader, &element, b"NAME").unwrap_or_default();
                let record = parse_named_master(&mut reader, element_name, name)?;
                records.push(ParsedSourceRecord {
                    record,
                    source_id,
                    identity_kind,
                    identities,
                    alter_id,
                    raw_source_sha256: source_fragment_sha256_from_sanitized(
                        &sanitized,
                        record_start,
                        reader.buffer_position() as usize,
                    )?,
                });
            }
            Event::Empty(element)
                if is_supported_export_parent(&path)
                    && element.name().as_ref().eq_ignore_ascii_case(element_name) =>
            {
                validate_only_attributes(
                    &element,
                    &[b"NAME", b"GUID", b"REMOTEID", b"MASTERID", b"ALTERID"],
                )?;
                let identities = parsed_source_identities(&reader, &element)?;
                let (source_id, identity_kind) = preferred_identity(&identities);
                records.push(ParsedSourceRecord {
                    record: TallyNamedMaster {
                        name: attr_value(&reader, &element, b"NAME").unwrap_or_default(),
                        parent: PartyLedgerMasterFieldObservation::NotObserved,
                        reserved_name: None,
                    },
                    source_id,
                    identity_kind,
                    identities,
                    alter_id: attr_value(&reader, &element, b"ALTERID"),
                    raw_source_sha256: source_fragment_sha256_from_sanitized(
                        &sanitized,
                        record_start,
                        reader.buffer_position() as usize,
                    )?,
                });
            }
            Event::Start(element) => path.push(element.name().as_ref().to_ascii_uppercase()),
            Event::End(element) => pop_expected_path(&mut path, element.name().as_ref())?,
            Event::Eof => break,
            _ => {}
        }
    }
    validate_scoped_export(&evidence, schema, object_type, records.len())?;
    Ok(ParsedExport { records, evidence })
}

pub fn parse_ledgers(xml: &str) -> anyhow::Result<Vec<TallyLedger>> {
    Ok(parse_ledgers_with_evidence(xml)?.records)
}

pub fn parse_ledgers_with_evidence(xml: &str) -> anyhow::Result<ParsedExport<TallyLedger>> {
    let parsed = parse_ledger_source_records_with_evidence(xml)?;
    Ok(ParsedExport {
        records: parsed
            .records
            .into_iter()
            .map(|record| record.record)
            .collect(),
        evidence: parsed.evidence,
    })
}

pub fn parse_ledger_source_records_with_evidence(
    xml: &str,
) -> anyhow::Result<ParsedExport<ParsedSourceRecord<TallyLedger>>> {
    parse_ledger_source_records_for_schema(xml, BRIDGE_LEDGER_EXPORT_SCHEMA)
}

/// Why a native voucher-type, group or voucher collection was refused. The
/// class is decided where the parser fails, so a caller can say why without
/// reading parser text (bridge#676). A repeated identity across rows is not
/// here: it is kept as `duplicate_identities` evidence, not refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeCollectionError {
    /// The XML did not parse, or its envelope, nesting or `COLLECTION` was
    /// not the documented shape: Tally or the transport, not one master.
    MalformedResponse,
    /// `STATUS` was absent or not `1`.
    NotSuccess,
    /// One row was refused: empty, missing an identity or a required field,
    /// repeating a field, or carrying content the row grammar does not admit.
    RowUnusable,
    /// No row carried the requested company's GUID prefix.
    CompanyIdentityMismatch,
    /// A count passed its bound.
    BoundsViolation,
}

impl std::fmt::Display for NativeCollectionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::MalformedResponse => "native collection response was malformed",
            Self::NotSuccess => "native collection did not report success",
            Self::RowUnusable => "native collection held a row it could not use",
            Self::CompanyIdentityMismatch => {
                "native collection did not bind to the requested company"
            }
            Self::BoundsViolation => "native collection exceeded a safety bound",
        })
    }
}

impl std::error::Error for NativeCollectionError {}

impl NativeCollectionError {
    /// A stable, data-free name for the failure, for a refusal's `cause`
    /// (bridge#676). None of these names a row or the collection's object,
    /// which the refusal's own code already names.
    pub const fn safe_code(self) -> &'static str {
        match self {
            Self::MalformedResponse => "native_collection_malformed_response",
            Self::NotSuccess => "native_collection_not_success",
            Self::RowUnusable => "native_collection_row_unusable",
            Self::CompanyIdentityMismatch => "native_collection_identity_mismatch",
            Self::BoundsViolation => "native_collection_bounds_exceeded",
        }
    }

    /// A row parser reports through `anyhow`. An XML, escape or encoding
    /// error inside the row, or a response that ends before the row closes, is
    /// the response, not the row's content. Anything else the row parser
    /// refuses is the row. An attribute error is flattened to text by the
    /// shared attribute helpers, so it still reads as the row.
    fn from_row(error: &anyhow::Error) -> Self {
        if error.chain().any(|cause| {
            cause.is::<quick_xml::Error>()
                || cause.is::<quick_xml::escape::EscapeError>()
                || cause.is::<quick_xml::encoding::EncodingError>()
                || cause.is::<RowCutOff>()
        }) {
            Self::MalformedResponse
        } else {
            Self::RowUnusable
        }
    }
}

/// A native collection row that the response ended inside. quick-xml returns
/// end-of-input with elements still open, so a truncated response reaches the
/// row parser's end-of-input arm. This marks it as the response's fault, by
/// type (bridge#676).
#[derive(Debug)]
struct RowCutOff;

impl std::fmt::Display for RowCutOff {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("native collection row ended before it closed")
    }
}

impl std::error::Error for RowCutOff {}

/// Parses a native `List of VoucherTypes` collection. Like native ledgers,
/// the collection has no envelope company context, so at least one row must
/// bind through its observed master GUID prefix. Foreign rows are retained as
/// mismatch evidence because imported masters may carry another prefix.
pub fn parse_native_voucher_type_source_records_with_evidence(
    xml: &str,
    expected_company_guid: &str,
) -> Result<ParsedExport<ParsedSourceRecord<TallyNamedMaster>>, NativeCollectionError> {
    parse_native_collection_with_identity_evidence(
        xml,
        expected_company_guid,
        b"VOUCHERTYPE",
        "VOUCHERTYPE",
        parse_native_voucher_type_collection_row,
        false,
    )
}

/// Parses the native `List of Groups` collection used by the core window.
/// Group names are mutable display text; a selected read therefore requires
/// the observed GUID rather than deriving an identity from the name.
pub fn parse_native_group_source_records_with_evidence(
    xml: &str,
    expected_company_guid: &str,
) -> Result<ParsedExport<ParsedSourceRecord<TallyNamedMaster>>, NativeCollectionError> {
    parse_native_collection_with_identity_evidence(
        xml,
        expected_company_guid,
        b"GROUP",
        "group",
        parse_native_group_collection_row,
        false,
    )
}

/// Parses a native Voucher collection, retaining direct accounting-entry
/// amounts only. Nested allocations are deliberately skipped by the entry
/// parser, so a bill allocation's `AMOUNT` can never be miscounted as a
/// second ledger entry amount.
pub fn parse_native_voucher_source_records_with_evidence(
    xml: &str,
    expected_company_guid: &str,
) -> Result<ParsedExport<ParsedSourceRecord<TallyVoucher>>, NativeCollectionError> {
    let sanitized = tolerant_xml::sanitize_invalid_numeric_references_with_provenance(xml);
    let mut reader = configured_reader(sanitized.as_str());
    let mut path = Vec::<Vec<u8>>::new();
    let mut status_seen = false;
    let mut collection_seen = false;
    let mut records = Vec::new();
    let mut identities = HashMap::<String, u64>::new();
    let mut company_guid_prefix_match_count = 0_u64;
    let mut company_guid_prefix_mismatch_count = 0_u64;

    loop {
        let record_start = reader.buffer_position() as usize;
        match reader
            .read_event()
            .map_err(|_| NativeCollectionError::MalformedResponse)?
        {
            Event::Start(element) => {
                let name = element.name().as_ref().to_ascii_uppercase();
                if path.is_empty() && name != b"ENVELOPE" {
                    return Err(NativeCollectionError::MalformedResponse);
                }
                if path_eq(&path, &[b"ENVELOPE", b"HEADER"]) && name == b"STATUS" {
                    native_collection_status(&mut reader, &element, status_seen)?;
                    status_seen = true;
                    continue;
                }
                if path_eq(&path, &[b"ENVELOPE", b"BODY", b"DATA"]) && name == b"COLLECTION" {
                    collection_seen = true;
                }
                if path_eq(&path, &[b"ENVELOPE", b"BODY", b"DATA", b"COLLECTION"])
                    && name == b"VOUCHER"
                {
                    let (record, identities_for_row, alter_id) =
                        parse_native_voucher_collection_row(&mut reader, &element, &sanitized)
                            .map_err(|error| NativeCollectionError::from_row(&error))?;
                    let guid = identities_for_row
                        .guid
                        .as_deref()
                        .ok_or(NativeCollectionError::RowUnusable)?;
                    let remote_id = identities_for_row
                        .remote_id
                        .as_deref()
                        .ok_or(NativeCollectionError::RowUnusable)?;
                    if native_ledger_guid_has_company_prefix(guid, expected_company_guid)
                        && native_ledger_guid_has_company_prefix(remote_id, expected_company_guid)
                    {
                        company_guid_prefix_match_count = company_guid_prefix_match_count
                            .checked_add(1)
                            .ok_or(NativeCollectionError::BoundsViolation)?;
                    } else {
                        company_guid_prefix_mismatch_count = company_guid_prefix_mismatch_count
                            .checked_add(1)
                            .ok_or(NativeCollectionError::BoundsViolation)?;
                    }
                    record_identities_from_values("VOUCHER", &identities_for_row, &mut identities)
                        .map_err(|_| NativeCollectionError::BoundsViolation)?;
                    let record_end = reader.buffer_position() as usize;
                    records.push(ParsedSourceRecord {
                        record,
                        source_id: Some(guid.to_owned()),
                        identity_kind: Some(ParsedSourceIdentityKind::Guid),
                        identities: identities_for_row,
                        alter_id,
                        raw_source_sha256: source_fragment_sha256_from_sanitized(
                            &sanitized,
                            record_start,
                            record_end,
                        )
                        .map_err(|_| NativeCollectionError::MalformedResponse)?,
                    });
                    continue;
                }
                path.push(name);
            }
            Event::Empty(element) => {
                let name = element.name().as_ref().to_ascii_uppercase();
                if path_eq(&path, &[b"ENVELOPE", b"BODY", b"DATA"]) && name == b"COLLECTION" {
                    collection_seen = true;
                } else if path_eq(&path, &[b"ENVELOPE", b"BODY", b"DATA", b"COLLECTION"])
                    && name == b"VOUCHER"
                {
                    return Err(NativeCollectionError::RowUnusable);
                }
            }
            Event::End(element) => pop_expected_path(&mut path, element.name().as_ref())
                .map_err(|_| NativeCollectionError::MalformedResponse)?,
            Event::Eof => break,
            _ => {}
        }
    }
    native_collection_export(
        NativeCollectionState {
            path,
            status_seen,
            collection_seen,
            records,
            identities,
            company_guid_prefix_match_count,
            company_guid_prefix_mismatch_count,
        },
        true,
    )
}

fn parse_native_collection_with_identity_evidence<T>(
    xml: &str,
    expected_company_guid: &str,
    element_name: &[u8],
    object_type: &str,
    parse_row: impl Fn(
        &mut Reader<&[u8]>,
        &quick_xml::events::BytesStart<'_>,
    ) -> anyhow::Result<(T, ParsedSourceIdentities, Option<String>)>,
    allow_empty_without_row_identity: bool,
) -> Result<ParsedExport<ParsedSourceRecord<T>>, NativeCollectionError> {
    let sanitized = tolerant_xml::sanitize_invalid_numeric_references_with_provenance(xml);
    let mut reader = configured_reader(sanitized.as_str());
    let mut path = Vec::<Vec<u8>>::new();
    let mut status_seen = false;
    let mut collection_seen = false;
    let mut records = Vec::new();
    let mut identities = HashMap::<String, u64>::new();
    let mut company_guid_prefix_match_count = 0_u64;
    let mut company_guid_prefix_mismatch_count = 0_u64;

    loop {
        let record_start = reader.buffer_position() as usize;
        match reader
            .read_event()
            .map_err(|_| NativeCollectionError::MalformedResponse)?
        {
            Event::Start(element) => {
                let name = element.name().as_ref().to_ascii_uppercase();
                if path.is_empty() && name != b"ENVELOPE" {
                    return Err(NativeCollectionError::MalformedResponse);
                }
                if path_eq(&path, &[b"ENVELOPE", b"HEADER"]) && name == b"STATUS" {
                    native_collection_status(&mut reader, &element, status_seen)?;
                    status_seen = true;
                    continue;
                }
                if path_eq(&path, &[b"ENVELOPE", b"BODY", b"DATA"]) && name == b"COLLECTION" {
                    collection_seen = true;
                }
                if path_eq(&path, &[b"ENVELOPE", b"BODY", b"DATA", b"COLLECTION"])
                    && name.as_slice().eq_ignore_ascii_case(element_name)
                {
                    let (record, identities_for_row, alter_id) =
                        parse_row(&mut reader, &element)
                            .map_err(|error| NativeCollectionError::from_row(&error))?;
                    let guid = identities_for_row
                        .guid
                        .as_deref()
                        .ok_or(NativeCollectionError::RowUnusable)?;
                    if native_ledger_guid_has_company_prefix(guid, expected_company_guid) {
                        company_guid_prefix_match_count = company_guid_prefix_match_count
                            .checked_add(1)
                            .ok_or(NativeCollectionError::BoundsViolation)?;
                    } else {
                        company_guid_prefix_mismatch_count = company_guid_prefix_mismatch_count
                            .checked_add(1)
                            .ok_or(NativeCollectionError::BoundsViolation)?;
                    }
                    record_identities_from_values(
                        object_type,
                        &identities_for_row,
                        &mut identities,
                    )
                    .map_err(|_| NativeCollectionError::BoundsViolation)?;
                    let record_end = reader.buffer_position() as usize;
                    records.push(ParsedSourceRecord {
                        record,
                        source_id: Some(guid.to_owned()),
                        identity_kind: Some(ParsedSourceIdentityKind::Guid),
                        identities: identities_for_row,
                        alter_id,
                        raw_source_sha256: source_fragment_sha256_from_sanitized(
                            &sanitized,
                            record_start,
                            record_end,
                        )
                        .map_err(|_| NativeCollectionError::MalformedResponse)?,
                    });
                    continue;
                }
                path.push(name);
            }
            Event::Empty(element) => {
                let name = element.name().as_ref().to_ascii_uppercase();
                if path_eq(&path, &[b"ENVELOPE", b"BODY", b"DATA"]) && name == b"COLLECTION" {
                    collection_seen = true;
                } else if path_eq(&path, &[b"ENVELOPE", b"BODY", b"DATA", b"COLLECTION"])
                    && name.as_slice().eq_ignore_ascii_case(element_name)
                {
                    return Err(NativeCollectionError::RowUnusable);
                }
            }
            Event::End(element) => pop_expected_path(&mut path, element.name().as_ref())
                .map_err(|_| NativeCollectionError::MalformedResponse)?,
            Event::Eof => break,
            _ => {}
        }
    }
    native_collection_export(
        NativeCollectionState {
            path,
            status_seen,
            collection_seen,
            records,
            identities,
            company_guid_prefix_match_count,
            company_guid_prefix_mismatch_count,
        },
        allow_empty_without_row_identity,
    )
}

struct NativeCollectionState<T> {
    path: Vec<Vec<u8>>,
    status_seen: bool,
    collection_seen: bool,
    records: Vec<ParsedSourceRecord<T>>,
    identities: HashMap<String, u64>,
    company_guid_prefix_match_count: u64,
    company_guid_prefix_mismatch_count: u64,
}

/// The collection's one `STATUS`, which must read `1`. A second `STATUS` or
/// one whose text cannot be read is the response's shape, not Tally's answer.
fn native_collection_status(
    reader: &mut Reader<&[u8]>,
    element: &quick_xml::events::BytesStart<'_>,
    status_seen: bool,
) -> Result<(), NativeCollectionError> {
    if status_seen {
        return Err(NativeCollectionError::MalformedResponse);
    }
    let status = read_required_text(reader, element.name())
        .map_err(|_| NativeCollectionError::MalformedResponse)?;
    if status != "1" {
        return Err(NativeCollectionError::NotSuccess);
    }
    Ok(())
}

fn native_collection_export<T>(
    state: NativeCollectionState<T>,
    allow_empty_without_row_identity: bool,
) -> Result<ParsedExport<ParsedSourceRecord<T>>, NativeCollectionError> {
    if !state.path.is_empty() {
        return Err(NativeCollectionError::MalformedResponse);
    }
    if !state.status_seen {
        return Err(NativeCollectionError::NotSuccess);
    }
    if !state.collection_seen {
        return Err(NativeCollectionError::MalformedResponse);
    }
    if state.company_guid_prefix_match_count == 0
        && (!allow_empty_without_row_identity || !state.records.is_empty())
    {
        return Err(NativeCollectionError::CompanyIdentityMismatch);
    }
    let mut duplicate_identities = state
        .identities
        .into_iter()
        .filter(|(_, occurrences)| *occurrences > 1)
        .map(|(identity, occurrences)| DuplicateIdentityEvidence {
            identity_sha256: sha256_hex(identity.as_bytes()),
            occurrences,
        })
        .collect::<Vec<_>>();
    duplicate_identities.sort_by(|left, right| left.identity_sha256.cmp(&right.identity_sha256));
    let source_record_count =
        u64::try_from(state.records.len()).map_err(|_| NativeCollectionError::BoundsViolation)?;
    Ok(ParsedExport {
        records: state.records,
        evidence: ExportEvidence {
            observed_record_count: Some(source_record_count),
            identified_record_count: source_record_count,
            duplicate_identities,
            company_guid_prefix_match_count: state.company_guid_prefix_match_count,
            company_guid_prefix_mismatch_count: state.company_guid_prefix_mismatch_count,
            ..ExportEvidence::default()
        },
    })
}

fn parse_native_voucher_type_collection_row(
    reader: &mut Reader<&[u8]>,
    element: &quick_xml::events::BytesStart<'_>,
) -> anyhow::Result<(TallyNamedMaster, ParsedSourceIdentities, Option<String>)> {
    validate_only_attributes(element, &[b"NAME", b"RESERVEDNAME"])?;
    let name = attr_value(reader, element, b"NAME")
        .ok_or_else(|| anyhow::anyhow!("native voucher type row omitted NAME"))?;
    let mut record = TallyNamedMaster {
        name,
        parent: PartyLedgerMasterFieldObservation::NotObserved,
        reserved_name: None,
    };
    let mut identities = ParsedSourceIdentities::default();
    let mut alter_id = None;
    let mut parent_seen = false;
    let mut guid_seen = false;
    let mut master_id_seen = false;
    let mut alter_id_seen = false;
    loop {
        match reader.read_event()? {
            Event::Start(child) => match child.name().as_ref().to_ascii_uppercase().as_slice() {
                b"GUID" => {
                    validate_only_attributes(&child, &[b"TYPE"])?;
                    if std::mem::replace(&mut guid_seen, true) {
                        anyhow::bail!("native voucher type row repeated GUID");
                    }
                    identities.guid = validated_optional_identifier(Some(read_required_text(
                        reader,
                        child.name(),
                    )?))?
                    .map(|guid| guid.to_ascii_lowercase());
                }
                b"MASTERID" => {
                    validate_only_attributes(&child, &[b"TYPE"])?;
                    if std::mem::replace(&mut master_id_seen, true) {
                        anyhow::bail!("native voucher type row repeated MASTERID");
                    }
                    identities.master_id = validated_optional_identifier(Some(
                        read_required_text(reader, child.name())?,
                    ))?;
                }
                b"ALTERID" => {
                    validate_only_attributes(&child, &[b"TYPE"])?;
                    if std::mem::replace(&mut alter_id_seen, true) {
                        anyhow::bail!("native voucher type row repeated ALTERID");
                    }
                    alter_id = validated_optional_identifier(Some(read_required_text(
                        reader,
                        child.name(),
                    )?))?;
                }
                b"PARENT" => {
                    validate_only_attributes(&child, &[b"TYPE"])?;
                    if std::mem::replace(&mut parent_seen, true) {
                        anyhow::bail!("native voucher type row repeated PARENT");
                    }
                    record.parent = PartyLedgerMasterFieldObservation::Returned(
                        read_identifier_text(reader, child.name())?.unwrap_or_default(),
                    );
                }
                _ => {
                    let child_name = child.name().as_ref().to_vec();
                    reader.read_to_end(QName(&child_name).to_owned())?;
                }
            },
            Event::Empty(child) => match child.name().as_ref().to_ascii_uppercase().as_slice() {
                b"GUID" | b"MASTERID" | b"ALTERID" | b"PARENT" => {
                    anyhow::bail!("native voucher type row omitted a required field");
                }
                _ => {}
            },
            Event::End(end) if end.name().as_ref().eq_ignore_ascii_case(b"VOUCHERTYPE") => break,
            Event::Text(text) if !text.decode()?.trim().is_empty() => {
                anyhow::bail!("native voucher type row contained unexpected text");
            }
            Event::Eof => return Err(RowCutOff.into()),
            _ => {}
        }
    }
    if !guid_seen || identities.guid.is_none() {
        anyhow::bail!("native voucher type row omitted GUID");
    }
    if !master_id_seen || identities.master_id.is_none() {
        anyhow::bail!("native voucher type row omitted MASTERID");
    }
    if !alter_id_seen || alter_id.is_none() {
        anyhow::bail!("native voucher type row omitted ALTERID");
    }
    if !parent_seen || record.parent.returned_text().is_none() {
        anyhow::bail!("native voucher type row omitted PARENT");
    }
    Ok((record, identities, alter_id))
}

fn parse_native_group_collection_row(
    reader: &mut Reader<&[u8]>,
    element: &quick_xml::events::BytesStart<'_>,
) -> anyhow::Result<(TallyNamedMaster, ParsedSourceIdentities, Option<String>)> {
    validate_only_attributes(element, &[b"NAME", b"RESERVEDNAME"])?;
    let name = attr_value(reader, element, b"NAME")
        .ok_or_else(|| anyhow::anyhow!("native group row omitted NAME"))?;
    let mut record = TallyNamedMaster {
        name,
        parent: PartyLedgerMasterFieldObservation::NotObserved,
        reserved_name: None,
    };
    let mut identities = ParsedSourceIdentities::default();
    let mut alter_id = None;
    let mut parent_seen = false;
    let mut guid_seen = false;
    let mut master_id_seen = false;
    let mut alter_id_seen = false;
    loop {
        match reader.read_event()? {
            Event::Start(child) => match child.name().as_ref().to_ascii_uppercase().as_slice() {
                b"GUID" => {
                    validate_only_attributes(&child, &[b"TYPE"])?;
                    if std::mem::replace(&mut guid_seen, true) {
                        anyhow::bail!("native group row repeated GUID");
                    }
                    identities.guid = validated_optional_identifier(Some(read_required_text(
                        reader,
                        child.name(),
                    )?))?
                    .map(|guid| guid.to_ascii_lowercase());
                }
                b"MASTERID" => {
                    validate_only_attributes(&child, &[b"TYPE"])?;
                    if std::mem::replace(&mut master_id_seen, true) {
                        anyhow::bail!("native group row repeated MASTERID");
                    }
                    identities.master_id = validated_optional_identifier(Some(
                        read_required_text(reader, child.name())?,
                    ))?;
                }
                b"ALTERID" => {
                    validate_only_attributes(&child, &[b"TYPE"])?;
                    if std::mem::replace(&mut alter_id_seen, true) {
                        anyhow::bail!("native group row repeated ALTERID");
                    }
                    alter_id = validated_optional_identifier(Some(read_required_text(
                        reader,
                        child.name(),
                    )?))?;
                }
                b"PARENT" => {
                    validate_only_attributes(&child, &[b"TYPE"])?;
                    if std::mem::replace(&mut parent_seen, true) {
                        anyhow::bail!("native group row repeated PARENT");
                    }
                    record.parent = PartyLedgerMasterFieldObservation::Returned(
                        read_identifier_text(reader, child.name())?.unwrap_or_default(),
                    );
                }
                _ => {
                    let child_name = child.name().as_ref().to_vec();
                    reader.read_to_end(QName(&child_name).to_owned())?;
                }
            },
            Event::Empty(child) => match child.name().as_ref().to_ascii_uppercase().as_slice() {
                b"GUID" | b"MASTERID" | b"ALTERID" | b"PARENT" => {
                    anyhow::bail!("native group row omitted a required field");
                }
                _ => {}
            },
            Event::End(end) if end.name().as_ref().eq_ignore_ascii_case(b"GROUP") => break,
            Event::Text(text) if !text.decode()?.trim().is_empty() => {
                anyhow::bail!("native group row contained unexpected text");
            }
            Event::Eof => return Err(RowCutOff.into()),
            _ => {}
        }
    }
    if !guid_seen || identities.guid.is_none() {
        anyhow::bail!("native group row omitted GUID");
    }
    if !master_id_seen || identities.master_id.is_none() {
        anyhow::bail!("native group row omitted MASTERID");
    }
    if !alter_id_seen || alter_id.is_none() {
        anyhow::bail!("native group row omitted ALTERID");
    }
    if !parent_seen || record.parent.returned_text().is_none() {
        anyhow::bail!("native group row omitted PARENT");
    }
    Ok((record, identities, alter_id))
}

fn parse_native_voucher_collection_row(
    reader: &mut Reader<&[u8]>,
    element: &quick_xml::events::BytesStart<'_>,
    sanitized: &tolerant_xml::SanitizedXml<'_>,
) -> anyhow::Result<(TallyVoucher, ParsedSourceIdentities, Option<String>)> {
    validate_only_attributes(element, &[b"REMOTEID", b"VCHKEY", b"VCHTYPE", b"OBJVIEW"])?;
    let remote_id = attr_value(reader, element, b"REMOTEID")
        .ok_or_else(|| anyhow::anyhow!("native voucher row omitted REMOTEID"))?;
    let mut voucher = TallyVoucher {
        id: None,
        date: None,
        voucher_type: None,
        voucher_number: None,
        party_ledger_name: None,
        cancelled: None,
        optional: None,
        ledger_entry_count: None,
        ledger_entries: Vec::new(),
    };
    let mut identities = ParsedSourceIdentities {
        remote_id: validated_optional_identifier(Some(remote_id))?,
        ..ParsedSourceIdentities::default()
    };
    let mut alter_id = None;
    let mut seen = HashSet::new();
    loop {
        let entry_start = reader.buffer_position() as usize;
        match reader.read_event()? {
            Event::Start(child) => match child.name().as_ref().to_ascii_uppercase().as_slice() {
                b"DATE" => {
                    validate_only_attributes(&child, &[b"TYPE"])?;
                    mark_unique_field(&mut seen, "DATE", "native voucher")?;
                    voucher.date = Some(read_required_text(reader, child.name())?);
                }
                b"GUID" => {
                    validate_only_attributes(&child, &[b"TYPE"])?;
                    mark_unique_field(&mut seen, "GUID", "native voucher")?;
                    identities.guid = validated_optional_identifier(Some(read_required_text(
                        reader,
                        child.name(),
                    )?))?
                    .map(|guid| guid.to_ascii_lowercase());
                }
                b"MASTERID" => {
                    validate_only_attributes(&child, &[b"TYPE"])?;
                    mark_unique_field(&mut seen, "MASTERID", "native voucher")?;
                    identities.master_id = validated_optional_identifier(Some(
                        read_required_text(reader, child.name())?,
                    ))?;
                }
                b"ALTERID" => {
                    validate_only_attributes(&child, &[b"TYPE"])?;
                    mark_unique_field(&mut seen, "ALTERID", "native voucher")?;
                    alter_id = validated_optional_identifier(Some(read_required_text(
                        reader,
                        child.name(),
                    )?))?;
                }
                b"VOUCHERTYPENAME" => {
                    validate_only_attributes(&child, &[b"TYPE"])?;
                    mark_unique_field(&mut seen, "VOUCHERTYPENAME", "native voucher")?;
                    voucher.voucher_type =
                        Some(read_required_identifier_text(reader, child.name())?);
                }
                b"VOUCHERNUMBER" => {
                    validate_only_attributes(&child, &[b"TYPE"])?;
                    mark_unique_field(&mut seen, "VOUCHERNUMBER", "native voucher")?;
                    voucher.voucher_number = read_optional_text(reader, child.name())?;
                }
                b"ISCANCELLED" => {
                    validate_only_attributes(&child, &[b"TYPE"])?;
                    mark_unique_field(&mut seen, "ISCANCELLED", "native voucher")?;
                    voucher.cancelled = Some(parse_tally_boolean(&read_required_text(
                        reader,
                        child.name(),
                    )?)?);
                }
                b"ISOPTIONAL" => {
                    validate_only_attributes(&child, &[b"TYPE"])?;
                    mark_unique_field(&mut seen, "ISOPTIONAL", "native voucher")?;
                    voucher.optional = Some(parse_tally_boolean(&read_required_text(
                        reader,
                        child.name(),
                    )?)?);
                }
                b"ALLLEDGERENTRIES.LIST" => {
                    validate_only_attributes(&child, &[])?;
                    let mut entry = parse_native_voucher_ledger_entry(reader)?;
                    entry.raw_source_sha256 = source_fragment_sha256_from_sanitized(
                        sanitized,
                        entry_start,
                        reader.buffer_position() as usize,
                    )?;
                    let entry_index = u64::try_from(voucher.ledger_entries.len())
                        .map_err(|_| anyhow::anyhow!("native voucher entry count overflow"))?
                        .checked_add(1)
                        .ok_or_else(|| anyhow::anyhow!("native voucher entry count overflow"))?;
                    entry.entry_index = entry_index;
                    voucher.ledger_entries.push(entry);
                }
                _ => {
                    let child_name = child.name().as_ref().to_vec();
                    reader.read_to_end(QName(&child_name).to_owned())?;
                }
            },
            Event::Empty(child) => match child.name().as_ref().to_ascii_uppercase().as_slice() {
                b"DATE"
                | b"GUID"
                | b"MASTERID"
                | b"ALTERID"
                | b"VOUCHERTYPENAME"
                | b"ISCANCELLED"
                | b"ISOPTIONAL"
                | b"ALLLEDGERENTRIES.LIST" => {
                    anyhow::bail!("native voucher row omitted a required field");
                }
                b"VOUCHERNUMBER" => {
                    mark_unique_field(&mut seen, "VOUCHERNUMBER", "native voucher")?;
                }
                _ => {}
            },
            Event::End(end) if end.name().as_ref().eq_ignore_ascii_case(b"VOUCHER") => break,
            Event::Text(text) if !text.decode()?.trim().is_empty() => {
                anyhow::bail!("native voucher row contained unexpected text");
            }
            Event::Eof => return Err(RowCutOff.into()),
            _ => {}
        }
    }
    if identities.guid.is_none() || identities.master_id.is_none() || alter_id.is_none() {
        anyhow::bail!("native voucher row omitted durable identity");
    }
    if voucher.date.is_none()
        || voucher.voucher_type.is_none()
        || voucher.cancelled.is_none()
        || voucher.optional.is_none()
    {
        anyhow::bail!("native voucher row omitted a required accounting field");
    }
    voucher.ledger_entry_count = Some(
        u64::try_from(voucher.ledger_entries.len())
            .map_err(|_| anyhow::anyhow!("native voucher entry count overflow"))?,
    );
    voucher.id = identities.guid.clone();
    Ok((voucher, identities, alter_id))
}

fn parse_native_voucher_ledger_entry(
    reader: &mut Reader<&[u8]>,
) -> anyhow::Result<TallyLedgerEntry> {
    let mut ledger_name = None;
    let mut amount = None;
    let mut is_deemed_positive = None;
    let mut seen = HashSet::new();
    loop {
        match reader.read_event()? {
            Event::Start(child) => match child.name().as_ref().to_ascii_uppercase().as_slice() {
                b"LEDGERNAME" => {
                    validate_only_attributes(&child, &[b"TYPE"])?;
                    mark_unique_field(&mut seen, "LEDGERNAME", "native voucher ledger entry")?;
                    ledger_name = Some(read_required_identifier_text(reader, child.name())?);
                }
                b"AMOUNT" => {
                    validate_only_attributes(&child, &[b"TYPE"])?;
                    mark_unique_field(&mut seen, "AMOUNT", "native voucher ledger entry")?;
                    let value = read_required_text(reader, child.name())?;
                    bridge_tally_primitives::ExactDecimal::parse(value.clone())?;
                    amount = Some(value);
                }
                b"ISDEEMEDPOSITIVE" => {
                    validate_only_attributes(&child, &[b"TYPE"])?;
                    mark_unique_field(
                        &mut seen,
                        "ISDEEMEDPOSITIVE",
                        "native voucher ledger entry",
                    )?;
                    is_deemed_positive = Some(parse_tally_boolean(&read_required_text(
                        reader,
                        child.name(),
                    )?)?);
                }
                _ => {
                    // This consumes nested allocations as a subtree. In
                    // particular, `BILLALLOCATIONS.LIST/AMOUNT` can never
                    // reach the direct-entry `AMOUNT` branch above.
                    let child_name = child.name().as_ref().to_vec();
                    reader.read_to_end(QName(&child_name).to_owned())?;
                }
            },
            Event::Empty(child) => match child.name().as_ref().to_ascii_uppercase().as_slice() {
                b"LEDGERNAME" | b"AMOUNT" | b"ISDEEMEDPOSITIVE" => {
                    anyhow::bail!("native voucher ledger entry omitted a required field");
                }
                _ => {}
            },
            Event::End(end)
                if end
                    .name()
                    .as_ref()
                    .eq_ignore_ascii_case(b"ALLLEDGERENTRIES.LIST") =>
            {
                break;
            }
            Event::Text(text) if !text.decode()?.trim().is_empty() => {
                anyhow::bail!("native voucher ledger entry contained unexpected text");
            }
            Event::Eof => return Err(RowCutOff.into()),
            _ => {}
        }
    }
    Ok(TallyLedgerEntry {
        entry_index: 0,
        ledger_name: ledger_name
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow::anyhow!("native voucher ledger entry omitted ledger name"))?,
        amount: amount
            .ok_or_else(|| anyhow::anyhow!("native voucher ledger entry omitted amount"))?,
        is_deemed_positive: is_deemed_positive
            .ok_or_else(|| anyhow::anyhow!("native voucher ledger entry omitted sign evidence"))?,
        raw_source_sha256: String::new(),
    })
}

/// Tests the observed company prefix and nonempty master suffix of a Tally GUID.
pub fn master_guid_belongs_to_company(master_guid: &str, company_guid: &str) -> bool {
    // Bind the response to the pinned company, not just the request.
    //
    // `SVCURRENTCOMPANY` selects by NAME. If a second loaded company shares the
    // selected name, or the name binding shifts mid-scan, Tally can return that
    // other company's vouchers while the paired company collection still finds
    // the expected GUID among all loaded companies -- so date checks, AlterID
    // range checks, and the closing extent all pass, and another company's
    // financial data is published under the pinned name.
    //
    // TALLY_PROTOCOL_REFERENCE.md §9.11 records that every master GUID begins
    // with its company GUID; require the documented `-<master-id>` delimiter
    // as response identity evidence instead of accepting the bare company GUID.
    let Some(prefix) = master_guid.get(..company_guid.len()) else {
        return false;
    };
    let Some(suffix) = master_guid.get(company_guid.len()..) else {
        return false;
    };
    prefix.eq_ignore_ascii_case(company_guid)
        && suffix
            .strip_prefix('-')
            .is_some_and(|master_id| !master_id.is_empty())
}

pub(crate) fn native_ledger_guid_has_company_prefix(
    guid: &str,
    expected_company_guid: &str,
) -> bool {
    let Some(remainder) = guid.get(..expected_company_guid.len()) else {
        return false;
    };
    remainder.eq_ignore_ascii_case(expected_company_guid)
        && guid
            .as_bytes()
            .get(expected_company_guid.len())
            .is_some_and(|separator| *separator == b'-')
}

fn record_identities_from_values(
    object_type: &str,
    identities_for_row: &ParsedSourceIdentities,
    identities: &mut HashMap<String, u64>,
) -> anyhow::Result<()> {
    for (identity_kind, identity) in [
        ("guid", identities_for_row.guid.as_ref()),
        ("remote_id", identities_for_row.remote_id.as_ref()),
        ("master_id", identities_for_row.master_id.as_ref()),
    ] {
        let Some(identity) = identity else {
            continue;
        };
        let scoped_identity = format!("{object_type}\0{identity_kind}\0{identity}");
        let occurrences = identities.entry(scoped_identity).or_insert(0);
        *occurrences = occurrences
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("native ledger identity count overflow"))?;
    }
    Ok(())
}

pub fn parse_ledger_write_readback_with_evidence(
    xml: &str,
) -> anyhow::Result<ParsedExport<ParsedSourceRecord<TallyLedger>>> {
    let mut reader = configured_reader(xml);
    let mut path = Vec::<Vec<u8>>::new();
    let mut records = Vec::new();
    let mut context = None::<ParsedCompanyContext>;
    let mut header_seen = false;
    let mut body_seen = false;
    let mut status_seen = false;
    loop {
        let record_start = reader.buffer_position() as usize;
        match reader.read_event()? {
            Event::Start(element) => {
                let name = element.name().as_ref().to_ascii_uppercase();
                if matches!(
                    name.as_slice(),
                    b"ENVELOPE" | b"HEADER" | b"BODY" | b"VERSION" | b"STATUS"
                ) {
                    validate_only_attributes(&element, &[]).map_err(|_| {
                        anyhow::anyhow!("Tally export response attributes were invalid")
                    })?;
                }
                if path.is_empty() {
                    if name.as_slice() != b"ENVELOPE" {
                        anyhow::bail!("Tally write readback root was not ENVELOPE");
                    }
                    path.push(name);
                } else if path_eq(&path, &[b"ENVELOPE"]) && name.as_slice() == b"HEADER" {
                    if std::mem::replace(&mut header_seen, true) || body_seen {
                        anyhow::bail!("Tally write readback repeated or misplaced HEADER");
                    }
                    path.push(name);
                } else if path_eq(&path, &[b"ENVELOPE", b"HEADER"]) && name.as_slice() == b"STATUS"
                {
                    if std::mem::replace(&mut status_seen, true) {
                        anyhow::bail!("Tally write readback repeated STATUS");
                    }
                    if read_required_text(&mut reader, element.name())? != "1" {
                        anyhow::bail!("Tally write readback application STATUS was not successful");
                    }
                } else if path_eq(&path, &[b"ENVELOPE"]) && name.as_slice() == b"BODY" {
                    if !status_seen || std::mem::replace(&mut body_seen, true) {
                        anyhow::bail!("Tally write readback repeated or misplaced BODY");
                    }
                    path.push(name);
                } else if path_eq(&path, &[b"ENVELOPE", b"BODY"])
                    && name.as_slice() == b"COMPANYCONTEXT"
                {
                    if context.is_some() {
                        anyhow::bail!("Tally write readback repeated COMPANYCONTEXT");
                    }
                    context = Some(parse_company_context(&mut reader, &element, false)?);
                } else if path_eq(&path, &[b"ENVELOPE", b"BODY"]) && name.as_slice() == b"LEDGER" {
                    validate_only_attributes(
                        &element,
                        &[b"NAME", b"GUID", b"REMOTEID", b"MASTERID", b"ALTERID"],
                    )?;
                    let name = attr_value(&reader, &element, b"NAME");
                    let identities = parsed_source_identities(&reader, &element)?;
                    let (source_id, identity_kind) = preferred_identity(&identities);
                    let alter_id = attr_value(&reader, &element, b"ALTERID");
                    let record = parse_ledger_write_readback(&mut reader, name)?;
                    records.push(ParsedSourceRecord {
                        record,
                        identity_kind,
                        source_id,
                        identities,
                        alter_id,
                        raw_source_sha256: source_fragment_sha256(
                            xml,
                            record_start,
                            reader.buffer_position() as usize,
                        )?,
                    });
                } else {
                    anyhow::bail!(
                        "Tally write readback contained an element outside its exact profile"
                    );
                }
            }
            Event::Empty(element)
                if path_eq(&path, &[b"ENVELOPE", b"BODY"])
                    && element
                        .name()
                        .as_ref()
                        .eq_ignore_ascii_case(b"COMPANYCONTEXT") =>
            {
                if context.is_some() {
                    anyhow::bail!("Tally write readback repeated COMPANYCONTEXT");
                }
                context = Some(parse_company_context(&mut reader, &element, true)?);
            }
            Event::Empty(_) => {
                anyhow::bail!(
                    "Tally write readback contained an empty element outside its exact profile"
                );
            }
            Event::End(element) => {
                let Some(expected) = path.pop() else {
                    anyhow::bail!("Tally write readback closed an unexpected element");
                };
                if !element.name().as_ref().eq_ignore_ascii_case(&expected) {
                    anyhow::bail!("Tally write readback closed an unexpected element");
                }
            }
            Event::Text(text) if !text.decode()?.trim().is_empty() => {
                anyhow::bail!("Tally write readback contained unexpected text");
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if !path.is_empty() || !header_seen || !status_seen || !body_seen {
        anyhow::bail!("Tally write readback ended before its root closed");
    }
    let context =
        context.ok_or_else(|| anyhow::anyhow!("Tally write readback omitted COMPANYCONTEXT"))?;
    let mut identities = HashMap::<String, u64>::new();
    let mut identified_record_count = 0_u64;
    for record in &records {
        let mut identified = false;
        for identity in [
            record.identities.guid.as_ref(),
            record.identities.remote_id.as_ref(),
            record.identities.master_id.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            *identities.entry(identity.clone()).or_insert(0) += 1;
            identified = true;
        }
        identified_record_count += u64::from(identified);
    }
    let duplicate_identities = identities
        .into_iter()
        .filter(|(_, occurrences)| *occurrences > 1)
        .map(|(identity, occurrences)| DuplicateIdentityEvidence {
            identity_sha256: sha256_hex(identity.as_bytes()),
            occurrences,
        })
        .collect();
    let evidence = ExportEvidence {
        company_context: Some(context.company),
        schema: context.schema,
        object_type: context.object_type,
        source_record_count: context.source_record_count,
        identified_record_count,
        duplicate_identities,
        ..ExportEvidence::default()
    };
    validate_scoped_export(
        &evidence,
        BRIDGE_LEDGER_WRITE_READBACK_SCHEMA,
        "LEDGER",
        records.len(),
    )?;
    Ok(ParsedExport { records, evidence })
}

fn parse_ledger_source_records_for_schema(
    xml: &str,
    expected_schema: &str,
) -> anyhow::Result<ParsedExport<ParsedSourceRecord<TallyLedger>>> {
    validate_export_response(xml)?;
    let evidence = scan_export_evidence(xml)?;
    let mut reader = configured_reader(xml);
    let mut path = Vec::<Vec<u8>>::new();
    let mut records = Vec::new();
    loop {
        let record_start = reader.buffer_position() as usize;
        match reader.read_event()? {
            Event::Start(element)
                if is_supported_export_parent(&path)
                    && element.name().as_ref().eq_ignore_ascii_case(b"LEDGER") =>
            {
                validate_only_attributes(
                    &element,
                    &[b"NAME", b"GUID", b"REMOTEID", b"MASTERID", b"ALTERID"],
                )?;
                let name = attr_value(&reader, &element, b"NAME");
                let identities = parsed_source_identities(&reader, &element)?;
                let (source_id, identity_kind) = preferred_identity(&identities);
                let alter_id = attr_value(&reader, &element, b"ALTERID");
                let record = parse_ledger(&mut reader, name)?;
                records.push(ParsedSourceRecord {
                    record,
                    identity_kind,
                    source_id,
                    identities,
                    alter_id,
                    raw_source_sha256: source_fragment_sha256(
                        xml,
                        record_start,
                        reader.buffer_position() as usize,
                    )?,
                });
            }
            Event::Empty(element)
                if is_supported_export_parent(&path)
                    && element.name().as_ref().eq_ignore_ascii_case(b"LEDGER") =>
            {
                validate_only_attributes(
                    &element,
                    &[b"NAME", b"GUID", b"REMOTEID", b"MASTERID", b"ALTERID"],
                )?;
                let identities = parsed_source_identities(&reader, &element)?;
                let (source_id, identity_kind) = preferred_identity(&identities);
                let alter_id = attr_value(&reader, &element, b"ALTERID");
                records.push(ParsedSourceRecord {
                    record: TallyLedger {
                        name: attr_value(&reader, &element, b"NAME").unwrap_or_default(),
                        parent: PartyLedgerMasterFieldObservation::NotObserved,
                        party_gstin: PartyLedgerMasterFieldObservation::NotObserved,
                        opening_balance: None,
                    },
                    identity_kind,
                    source_id,
                    identities,
                    alter_id,
                    raw_source_sha256: source_fragment_sha256(
                        xml,
                        record_start,
                        reader.buffer_position() as usize,
                    )?,
                });
            }
            Event::Start(element) => path.push(element.name().as_ref().to_ascii_uppercase()),
            Event::End(element) => pop_expected_path(&mut path, element.name().as_ref())?,
            Event::Eof => break,
            _ => {}
        }
    }
    validate_scoped_export(&evidence, expected_schema, "LEDGER", records.len())?;
    Ok(ParsedExport { records, evidence })
}

pub fn parse_ledger_period_balance_report(
    xml: &str,
) -> anyhow::Result<ParsedLedgerPeriodBalanceReport> {
    validate_export_response(xml)?;
    let mut reader = configured_reader(xml);
    let mut context = None;
    let mut records = Vec::new();
    loop {
        let record_start = reader.buffer_position() as usize;
        match reader.read_event()? {
            Event::Start(element)
                if element
                    .name()
                    .as_ref()
                    .eq_ignore_ascii_case(b"COMPANYCONTEXT") =>
            {
                let parsed = parse_ledger_period_context(&mut reader, &element, false)?;
                if context.replace(parsed).is_some() {
                    anyhow::bail!("Tally period report repeated its context");
                }
            }
            Event::Empty(element)
                if element
                    .name()
                    .as_ref()
                    .eq_ignore_ascii_case(b"COMPANYCONTEXT") =>
            {
                let parsed = parse_ledger_period_context(&mut reader, &element, true)?;
                if context.replace(parsed).is_some() {
                    anyhow::bail!("Tally period report repeated its context");
                }
            }
            Event::Start(element)
                if element
                    .name()
                    .as_ref()
                    .eq_ignore_ascii_case(b"LEDGERPERIODBALANCE") =>
            {
                validate_only_attributes(
                    &element,
                    &[b"GUID", b"REMOTEID", b"MASTERID", b"ALTERID"],
                )?;
                let identities = parsed_source_identities(&reader, &element)?;
                let (source_id, identity_kind) = preferred_identity(&identities);
                let alter_id = attr_value(&reader, &element, b"ALTERID");
                let record = parse_ledger_period_balance(&mut reader)?;
                records.push(ParsedSourceRecord {
                    record,
                    source_id,
                    identity_kind,
                    identities,
                    alter_id,
                    raw_source_sha256: source_fragment_sha256(
                        xml,
                        record_start,
                        reader.buffer_position() as usize,
                    )?,
                });
            }
            Event::Eof => break,
            _ => {}
        }
    }
    let context = context.ok_or_else(|| anyhow::anyhow!("Tally period report omitted context"))?;
    if context.source_record_count != records.len() as u64 {
        anyhow::bail!("Tally period report count did not match parsed rows");
    }
    let mut identities = HashMap::new();
    for record in &records {
        let source_id = record
            .source_id
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("Tally period report row omitted stable identity"))?;
        if identities.insert(source_id, ()).is_some() {
            anyhow::bail!("Tally period report repeated a stable identity");
        }
    }
    Ok(ParsedLedgerPeriodBalanceReport { context, records })
}

fn parse_ledger_period_context(
    reader: &mut Reader<&[u8]>,
    element: &quick_xml::events::BytesStart<'_>,
    is_empty: bool,
) -> anyhow::Result<LedgerPeriodBalanceContext> {
    let mut fields = HashMap::<&'static str, String>::new();
    for attribute in element.attributes().with_checks(true) {
        let attribute = attribute
            .map_err(|_| anyhow::anyhow!("Tally period context contained malformed attributes"))?;
        let key = period_context_field(attribute.key.as_ref())
            .ok_or_else(|| anyhow::anyhow!("Tally period context contained an unexpected field"))?;
        let value = attribute
            .decoded_and_normalized_value(quick_xml::XmlVersion::Implicit1_0, reader.decoder())
            .map_err(|_| anyhow::anyhow!("Tally period context contained invalid text"))?;
        insert_period_context_field(&mut fields, key, value.trim())?;
    }
    if !is_empty {
        loop {
            match reader.read_event()? {
                Event::Start(child) => {
                    let key = period_context_field(child.name().as_ref()).ok_or_else(|| {
                        anyhow::anyhow!("Tally period context contained an unexpected field")
                    })?;
                    let value = read_required_text(reader, child.name()).map_err(|_| {
                        anyhow::anyhow!("Tally period context contained an empty or invalid value")
                    })?;
                    insert_period_context_field(&mut fields, key, &value)?;
                }
                Event::Empty(_) => {
                    anyhow::bail!("Tally period context contained an empty field");
                }
                Event::Text(text) if !text.decode()?.trim().is_empty() => {
                    anyhow::bail!("Tally period context contained unexpected text");
                }
                Event::End(end) if end.name().as_ref().eq_ignore_ascii_case(b"COMPANYCONTEXT") => {
                    break;
                }
                Event::Eof => anyhow::bail!("Tally period context ended before closing"),
                _ => {}
            }
        }
    }

    let required = |key: &'static str| {
        fields
            .get(key)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Tally period context omitted {key}"))
    };
    let schema = required("SCHEMA")?;
    let object_type = required("OBJECTTYPE")?;
    if schema != BRIDGE_LEDGER_PERIOD_BALANCE_SCHEMA || object_type != "LEDGERPERIODBALANCE" {
        anyhow::bail!("Tally period report scope was invalid");
    }
    Ok(LedgerPeriodBalanceContext {
        company_guid: required("GUID")?,
        from_yyyymmdd: required("FROMDATE")?,
        to_yyyymmdd: required("TODATE")?,
        ordinary_books_requested: parse_tally_boolean(&required("ORDINARYBOOKSREQUESTED")?)?,
        source_record_count: required("RECORDCOUNT")?
            .parse::<u64>()
            .map_err(|_| anyhow::anyhow!("Tally period report count was invalid"))?,
    })
}

fn period_context_field(name: &[u8]) -> Option<&'static str> {
    [
        "SCHEMA",
        "OBJECTTYPE",
        "GUID",
        "FROMDATE",
        "TODATE",
        "ORDINARYBOOKSREQUESTED",
        "RECORDCOUNT",
    ]
    .into_iter()
    .find(|field| name.eq_ignore_ascii_case(field.as_bytes()))
}

fn insert_period_context_field(
    fields: &mut HashMap<&'static str, String>,
    key: &'static str,
    value: &str,
) -> anyhow::Result<()> {
    if value.is_empty() {
        anyhow::bail!("Tally period context contained an empty field");
    }
    if fields.insert(key, value.to_string()).is_some() {
        anyhow::bail!("Tally period context repeated {key}");
    }
    Ok(())
}

pub fn parse_vouchers(xml: &str) -> anyhow::Result<Vec<TallyVoucher>> {
    Ok(parse_vouchers_with_evidence(xml)?.records)
}

pub fn parse_vouchers_with_evidence(xml: &str) -> anyhow::Result<ParsedExport<TallyVoucher>> {
    let parsed = parse_voucher_source_records_with_evidence(xml)?;
    Ok(ParsedExport {
        records: parsed
            .records
            .into_iter()
            .map(|record| record.record)
            .collect(),
        evidence: parsed.evidence,
    })
}

pub fn parse_voucher_source_records_with_evidence(
    xml: &str,
) -> anyhow::Result<ParsedExport<ParsedSourceRecord<TallyVoucher>>> {
    parse_voucher_source_records_for_schema(xml, BRIDGE_VOUCHER_EXPORT_SCHEMA)
}

pub fn parse_selected_voucher_source_records_with_evidence(
    xml: &str,
) -> anyhow::Result<ParsedExport<ParsedSourceRecord<TallyVoucher>>> {
    parse_voucher_source_records_for_schema(xml, BRIDGE_SELECTED_VOUCHER_EXPORT_SCHEMA)
}

fn parse_voucher_source_records_for_schema(
    xml: &str,
    expected_schema: &str,
) -> anyhow::Result<ParsedExport<ParsedSourceRecord<TallyVoucher>>> {
    validate_export_response(xml)?;
    let evidence = scan_export_evidence(xml)?;
    let mut reader = configured_reader(xml);
    let mut path = Vec::<Vec<u8>>::new();
    let mut records = Vec::new();
    loop {
        let record_start = reader.buffer_position() as usize;
        match reader.read_event()? {
            Event::Start(element)
                if is_supported_export_parent(&path)
                    && element.name().as_ref().eq_ignore_ascii_case(b"VOUCHER") =>
            {
                validate_only_attributes(
                    &element,
                    &[b"GUID", b"REMOTEID", b"MASTERID", b"ALTERID"],
                )?;
                let identities = parsed_source_identities(&reader, &element)?;
                let (source_id, identity_kind) = preferred_identity(&identities);
                let alter_id = attr_value(&reader, &element, b"ALTERID");
                let record = parse_voucher(&mut reader, source_id.clone(), xml)?;
                records.push(ParsedSourceRecord {
                    record,
                    source_id,
                    identity_kind,
                    identities,
                    alter_id,
                    raw_source_sha256: source_fragment_sha256(
                        xml,
                        record_start,
                        reader.buffer_position() as usize,
                    )?,
                });
            }
            Event::Empty(element)
                if is_supported_export_parent(&path)
                    && element.name().as_ref().eq_ignore_ascii_case(b"VOUCHER") =>
            {
                validate_only_attributes(
                    &element,
                    &[b"GUID", b"REMOTEID", b"MASTERID", b"ALTERID"],
                )?;
                let identities = parsed_source_identities(&reader, &element)?;
                let (source_id, identity_kind) = preferred_identity(&identities);
                let alter_id = attr_value(&reader, &element, b"ALTERID");
                records.push(ParsedSourceRecord {
                    record: TallyVoucher {
                        id: source_id.clone(),
                        date: None,
                        voucher_type: None,
                        voucher_number: None,
                        party_ledger_name: None,
                        cancelled: None,
                        optional: None,
                        ledger_entry_count: None,
                        ledger_entries: Vec::new(),
                    },
                    source_id,
                    identity_kind,
                    identities,
                    alter_id,
                    raw_source_sha256: source_fragment_sha256(
                        xml,
                        record_start,
                        reader.buffer_position() as usize,
                    )?,
                });
            }
            Event::Start(element) => path.push(element.name().as_ref().to_ascii_uppercase()),
            Event::End(element) => pop_expected_path(&mut path, element.name().as_ref())?,
            Event::Eof => break,
            _ => {}
        }
    }
    validate_scoped_export(&evidence, expected_schema, "VOUCHER", records.len())?;
    Ok(ParsedExport { records, evidence })
}

pub fn verify_company_context(
    evidence: &ExportEvidence,
    expected_guid: &str,
) -> anyhow::Result<()> {
    if expected_guid.trim().is_empty() {
        anyhow::bail!("Expected Tally company identity is missing");
    }
    match evidence
        .company_context
        .as_ref()
        .and_then(|context| context.guid.as_deref())
    {
        Some(actual) if actual.eq_ignore_ascii_case(expected_guid) => Ok(()),
        Some(_) => anyhow::bail!("Tally response company context did not match the request"),
        None => anyhow::bail!("Tally response did not include verifiable company context"),
    }
}

pub fn verify_selected_voucher_window_context(
    evidence: &ExportEvidence,
    expected_from_yyyymmdd: &str,
    expected_to_yyyymmdd: &str,
) -> anyhow::Result<()> {
    let company = evidence
        .company_context
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Tally response did not include selected window context"))?;
    if company.requested_from_yyyymmdd.as_deref() != Some(expected_from_yyyymmdd)
        || company.requested_to_yyyymmdd.as_deref() != Some(expected_to_yyyymmdd)
    {
        anyhow::bail!("Tally response selected window context did not match the request");
    }
    Ok(())
}

/// Enforces the narrow response skeleton used as selected-read capability evidence. Compatibility
/// parsers remain intentionally separate and may accept broader Tally wrapper shapes.
pub fn validate_exact_selected_export_structure(
    xml: &str,
    expected_primary_row: &str,
) -> anyhow::Result<()> {
    let expected_primary_row = expected_primary_row.as_bytes().to_ascii_uppercase();
    if ![b"LEDGER".as_slice(), b"VOUCHER".as_slice()]
        .iter()
        .any(|candidate| *candidate == expected_primary_row)
    {
        anyhow::bail!("Selected Tally read primary row type was invalid");
    }
    let mut reader = configured_reader(xml);
    let mut path = Vec::<Vec<u8>>::new();
    loop {
        match reader.read_event()? {
            Event::Start(element) => {
                let name = element.name().as_ref().to_ascii_uppercase();
                if !selected_structure_child_allowed(&path, &name, &expected_primary_row) {
                    anyhow::bail!("Selected Tally read contained an unexpected structural element");
                }
                validate_selected_wrapper_attributes(&element, &name)?;
                path.push(name);
            }
            Event::Empty(element) => {
                let name = element.name().as_ref().to_ascii_uppercase();
                if !selected_structure_child_allowed(&path, &name, &expected_primary_row) {
                    anyhow::bail!("Selected Tally read contained an unexpected empty element");
                }
                validate_selected_wrapper_attributes(&element, &name)?;
            }
            Event::End(element) => pop_expected_path(&mut path, element.name().as_ref())?,
            Event::Text(text)
                if !text.decode()?.trim().is_empty()
                    && !selected_structure_text_allowed(&path, &expected_primary_row) =>
            {
                anyhow::bail!("Selected Tally read contained unexpected structural text");
            }
            Event::CData(text) if !text.decode()?.trim().is_empty() => {
                anyhow::bail!("Selected Tally read contained unexpected CDATA");
            }
            Event::DocType(_) | Event::PI(_) => {
                anyhow::bail!("Selected Tally read contained a forbidden XML construct");
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if !path.is_empty() {
        anyhow::bail!("Selected Tally read ended before its structure closed");
    }
    Ok(())
}

fn validate_selected_wrapper_attributes(
    element: &quick_xml::events::BytesStart<'_>,
    name: &[u8],
) -> anyhow::Result<()> {
    if matches!(
        name,
        b"ENVELOPE" | b"HEADER" | b"BODY" | b"DATA" | b"COLLECTION" | b"VERSION" | b"STATUS"
    ) {
        validate_only_attributes(element, &[])
            .map_err(|_| anyhow::anyhow!("Selected Tally read wrapper attributes were invalid"))?;
    }
    Ok(())
}

fn selected_structure_child_allowed(path: &[Vec<u8>], name: &[u8], expected: &[u8]) -> bool {
    if path
        .iter()
        .any(|part| part == expected || part.as_slice() == b"COMPANYCONTEXT")
    {
        return true;
    }
    match path {
        [] => name == b"ENVELOPE",
        [envelope] if envelope.as_slice() == b"ENVELOPE" => {
            matches!(name, b"HEADER" | b"BODY")
        }
        [envelope, header]
            if envelope.as_slice() == b"ENVELOPE" && header.as_slice() == b"HEADER" =>
        {
            matches!(name, b"VERSION" | b"STATUS")
        }
        [envelope, body] if envelope.as_slice() == b"ENVELOPE" && body.as_slice() == b"BODY" => {
            name == b"DATA" || name == b"COMPANYCONTEXT" || name == expected
        }
        [envelope, body, data]
            if envelope.as_slice() == b"ENVELOPE"
                && body.as_slice() == b"BODY"
                && data.as_slice() == b"DATA" =>
        {
            name == b"COMPANYCONTEXT" || name == b"COLLECTION" || name == expected
        }
        [envelope, body, data, collection]
            if envelope.as_slice() == b"ENVELOPE"
                && body.as_slice() == b"BODY"
                && data.as_slice() == b"DATA"
                && collection.as_slice() == b"COLLECTION" =>
        {
            name == expected
        }
        _ => false,
    }
}

fn selected_structure_text_allowed(path: &[Vec<u8>], expected: &[u8]) -> bool {
    path.iter().any(|part| {
        part == expected
            || part.as_slice() == b"COMPANYCONTEXT"
            || matches!(part.as_slice(), b"VERSION" | b"STATUS")
    })
}

fn path_eq(path: &[Vec<u8>], expected: &[&[u8]]) -> bool {
    path.len() == expected.len()
        && path
            .iter()
            .zip(expected)
            .all(|(actual, expected)| actual.as_slice() == *expected)
}

/// `ENVELOPE`, `HEADER`, `BODY`, `VERSION` and `STATUS` are protocol element names
/// only in the envelope's own frame -- the root, its direct children, and the
/// header's fields. Tally reuses the same spellings for ordinary data: a real
/// trading book returns
/// `VOUCHER/ALLLEDGERENTRIES.LIST/BANKALLOCATIONS.LIST/STATUS` on every Payment,
/// Receipt and Contra voucher that carries a bank allocation, and reading that
/// bank field as a second, misplaced protocol status refused the whole response.
///
/// `ENVELOPE/BODY` stays inside the frame deliberately. A `STATUS` placed there
/// is still refused as misplaced, because Tally does not emit data at that depth
/// and the existing guarantee is worth more than the extra permissiveness.
/// Everything below it -- `DESC`, `DATA`, and their descendants -- is data, and
/// is judged by position rather than by name.
fn in_protocol_region(path: &[Vec<u8>]) -> bool {
    path.is_empty()
        || path_eq(path, &[b"ENVELOPE"])
        || path_eq(path, &[b"ENVELOPE", b"HEADER"])
        || path_eq(path, &[b"ENVELOPE", b"BODY"])
}

pub fn export_status(xml: &str) -> anyhow::Result<TallyExportStatus> {
    let mut reader = configured_reader(xml);
    let mut path = Vec::<Vec<u8>>::new();
    let mut saw_envelope = false;
    let mut envelope_closed = false;
    let mut header_seen = false;
    let mut body_seen = false;
    let mut version = None;
    let mut status = None;
    loop {
        match reader.read_event()? {
            Event::Start(element) => {
                let name = element.name().as_ref().to_ascii_uppercase();
                if in_protocol_region(&path)
                    && matches!(
                        name.as_slice(),
                        b"ENVELOPE" | b"HEADER" | b"BODY" | b"VERSION" | b"STATUS"
                    )
                {
                    validate_only_attributes(&element, &[]).map_err(|_| {
                        anyhow::anyhow!("Tally export response attributes were invalid")
                    })?;
                }
                if path.is_empty() {
                    if !element.name().as_ref().eq_ignore_ascii_case(b"ENVELOPE") {
                        anyhow::bail!("Tally response root must be ENVELOPE");
                    }
                    if saw_envelope || envelope_closed {
                        anyhow::bail!("Tally response contained multiple root elements");
                    }
                    saw_envelope = true;
                }
                if path_eq(&path, &[b"ENVELOPE"]) {
                    if !header_seen && name.as_slice() != b"HEADER" {
                        anyhow::bail!("Tally export response expected HEADER before BODY");
                    }
                    if header_seen && !body_seen && name.as_slice() != b"BODY" {
                        anyhow::bail!("Tally export response expected BODY after HEADER");
                    }
                    if body_seen {
                        anyhow::bail!("Tally export response contained an extra ENVELOPE child");
                    }
                }
                if !in_protocol_region(&path) {
                    // Body data. Record depth; judge nothing by element name.
                    path.push(name);
                } else if name.as_slice() == b"HEADER" {
                    if !path_eq(&path, &[b"ENVELOPE"]) || header_seen {
                        anyhow::bail!("Tally export response repeated or misplaced HEADER");
                    }
                    header_seen = true;
                    path.push(name);
                } else if name.as_slice() == b"BODY" {
                    if !path_eq(&path, &[b"ENVELOPE"]) || !header_seen || body_seen {
                        anyhow::bail!("Tally export response repeated or misplaced BODY");
                    }
                    body_seen = true;
                    path.push(name);
                } else if name.as_slice() == b"VERSION" {
                    if !path_eq(&path, &[b"ENVELOPE", b"HEADER"]) {
                        anyhow::bail!("Tally export response misplaced VERSION");
                    }
                    let value = read_required_text(&mut reader, element.name())?;
                    if version.replace(value).is_some() {
                        anyhow::bail!("Tally export response duplicated VERSION");
                    }
                } else if name.as_slice() == b"STATUS" {
                    if !path_eq(&path, &[b"ENVELOPE", b"HEADER"]) {
                        anyhow::bail!("Tally export response misplaced STATUS");
                    }
                    let value = read_required_text(&mut reader, element.name())?;
                    if status.replace(value).is_some() {
                        anyhow::bail!("Tally export response duplicated STATUS");
                    }
                } else if path_eq(&path, &[b"ENVELOPE", b"HEADER"]) {
                    anyhow::bail!("Tally export response contained an unexpected HEADER field");
                } else {
                    path.push(name);
                }
            }
            Event::Empty(element) => {
                let name = element.name().as_ref().to_ascii_uppercase();
                if in_protocol_region(&path)
                    && matches!(
                        name.as_slice(),
                        b"ENVELOPE" | b"HEADER" | b"BODY" | b"VERSION" | b"STATUS"
                    )
                {
                    validate_only_attributes(&element, &[]).map_err(|_| {
                        anyhow::anyhow!("Tally export response attributes were invalid")
                    })?;
                }
                if path.is_empty() {
                    if name.as_slice() != b"ENVELOPE" || saw_envelope || envelope_closed {
                        anyhow::bail!("Tally response root must be one ENVELOPE");
                    }
                    saw_envelope = true;
                    envelope_closed = true;
                } else if path_eq(&path, &[b"ENVELOPE"]) && name.as_slice() == b"BODY" {
                    if !header_seen || body_seen {
                        anyhow::bail!("Tally export response repeated or misplaced BODY");
                    }
                    body_seen = true;
                } else if path_eq(&path, &[b"ENVELOPE"]) {
                    anyhow::bail!("Tally export response contained an unexpected ENVELOPE child");
                } else if in_protocol_region(&path)
                    && matches!(name.as_slice(), b"HEADER" | b"VERSION" | b"STATUS")
                {
                    anyhow::bail!("Tally export response contained an empty critical header field");
                }
            }
            Event::End(element) => {
                let Some(expected) = path.pop() else {
                    anyhow::bail!("Tally response contained an unexpected closing element");
                };
                if !element.name().as_ref().eq_ignore_ascii_case(&expected) {
                    anyhow::bail!("Tally response closed an unexpected element");
                }
                if path.is_empty() {
                    envelope_closed = true;
                }
            }
            Event::Text(text)
                if (path.is_empty()
                    || path_eq(&path, &[b"ENVELOPE"])
                    || path_eq(&path, &[b"ENVELOPE", b"HEADER"])
                    || path_eq(&path, &[b"ENVELOPE", b"BODY"]))
                    && !text.decode()?.trim().is_empty() =>
            {
                anyhow::bail!("Tally response contained unexpected structural text")
            }
            Event::CData(text)
                if (path.is_empty()
                    || path_eq(&path, &[b"ENVELOPE"])
                    || path_eq(&path, &[b"ENVELOPE", b"HEADER"])
                    || path_eq(&path, &[b"ENVELOPE", b"BODY"]))
                    && !text.decode()?.trim().is_empty() =>
            {
                anyhow::bail!("Tally response contained unexpected structural CDATA")
            }
            Event::DocType(_) | Event::PI(_) => {
                anyhow::bail!("Tally export response contained a forbidden XML construct")
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if !saw_envelope || !envelope_closed || !path.is_empty() {
        anyhow::bail!("Tally response ended before ENVELOPE closed");
    }
    if !header_seen {
        anyhow::bail!("Tally export response did not include HEADER");
    }
    if !body_seen {
        anyhow::bail!("Tally export response did not include BODY");
    }
    match version.as_deref() {
        Some("1") => {}
        Some(_) => anyhow::bail!("Tally export response used an unsupported VERSION"),
        // Some observed/custom TDL export responses omit VERSION. Absence is
        // accepted for compatibility, but duplicates and unsupported values
        // are never merged or guessed.
        None => {}
    }
    match status.as_deref() {
        Some("1") => Ok(TallyExportStatus::Success),
        Some("0") => Ok(TallyExportStatus::Failure),
        Some(_) => anyhow::bail!("Tally returned an invalid application STATUS"),
        None => anyhow::bail!("Tally export response did not include HEADER/STATUS"),
    }
}

pub fn export_failure_reason_code(xml: &str) -> &'static str {
    if xml
        .to_ascii_lowercase()
        .contains("could not find company ''")
    {
        "company_not_loaded"
    } else {
        "tally_export_rejected"
    }
}

fn validate_export_response(xml: &str) -> anyhow::Result<()> {
    match export_status(xml)? {
        TallyExportStatus::Success => Ok(()),
        TallyExportStatus::Failure => {
            anyhow::bail!("Tally reported that the export request failed")
        }
    }
}

/// Tally's standard XML messaging response carries `HEADER/STATUS`, but report
/// exports may emit a direct report body beneath `ENVELOPE`. Company discovery
/// is the sole compatibility exception: it accepts that direct form only when
/// it is an exact, complete sequence of unwrapped company rows. Accounting
/// exports deliberately continue to require the standard success header.
fn validate_company_list_response(xml: &str) -> anyhow::Result<()> {
    match export_status(xml) {
        Ok(TallyExportStatus::Success) => Ok(()),
        Ok(TallyExportStatus::Failure) => {
            anyhow::bail!("Tally reported that the export request failed")
        }
        Err(_) => validate_direct_company_list_response(xml),
    }
}

fn validate_direct_company_list_response(xml: &str) -> anyhow::Result<()> {
    let mut reader = configured_reader(xml);
    let mut saw_envelope = false;
    let mut envelope_closed = false;
    let mut company_rows = 0_u64;
    loop {
        match reader.read_event()? {
            Event::Start(element) if !saw_envelope => {
                if !element.name().as_ref().eq_ignore_ascii_case(b"ENVELOPE") {
                    anyhow::bail!("Tally direct company response root must be ENVELOPE");
                }
                validate_only_attributes(&element, &[])?;
                saw_envelope = true;
            }
            Event::Start(element)
                if saw_envelope
                    && !envelope_closed
                    && element.name().as_ref().eq_ignore_ascii_case(b"COMPANYINFO") =>
            {
                validate_only_attributes(&element, &[])?;
                validate_direct_company_info(&mut reader)?;
                company_rows = company_rows.saturating_add(1);
            }
            Event::End(element)
                if saw_envelope
                    && !envelope_closed
                    && element.name().as_ref().eq_ignore_ascii_case(b"ENVELOPE") =>
            {
                envelope_closed = true;
            }
            Event::Start(_) | Event::Empty(_) => {
                anyhow::bail!("Tally direct company response contained an unexpected element")
            }
            Event::End(_) => {
                anyhow::bail!("Tally direct company response closed an unexpected element")
            }
            Event::Text(text) if !text.decode()?.trim().is_empty() => {
                anyhow::bail!("Tally direct company response contained unexpected structural text")
            }
            Event::CData(text) if !text.decode()?.trim().is_empty() => {
                anyhow::bail!("Tally direct company response contained unexpected structural CDATA")
            }
            Event::Comment(_) | Event::DocType(_) | Event::PI(_) => {
                anyhow::bail!("Tally direct company response contained a forbidden XML construct")
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if !saw_envelope || !envelope_closed || company_rows == 0 {
        anyhow::bail!("Tally direct company response was incomplete")
    }
    Ok(())
}

fn validate_direct_company_info(reader: &mut Reader<&[u8]>) -> anyhow::Result<()> {
    let mut name_seen = false;
    let mut guid_seen = false;
    loop {
        match reader.read_event()? {
            Event::Start(element)
                if element
                    .name()
                    .as_ref()
                    .eq_ignore_ascii_case(b"COMPANYNAMEFIELD") =>
            {
                validate_only_attributes(&element, &[])?;
                if std::mem::replace(&mut name_seen, true) {
                    anyhow::bail!("Tally direct company response repeated the company name")
                }
                read_direct_company_identity_text(reader, element.name())?;
            }
            Event::Start(element)
                if element
                    .name()
                    .as_ref()
                    .eq_ignore_ascii_case(b"COMPANYGUIDFIELD") =>
            {
                validate_only_attributes(&element, &[])?;
                if std::mem::replace(&mut guid_seen, true) {
                    anyhow::bail!("Tally direct company response repeated the company identity")
                }
                read_direct_company_identity_text(reader, element.name())?;
            }
            Event::End(element) if element.name().as_ref().eq_ignore_ascii_case(b"COMPANYINFO") => {
                break;
            }
            Event::Start(_) | Event::Empty(_) => {
                anyhow::bail!("Tally direct company record contained an unexpected field")
            }
            Event::Text(text) if !text.decode()?.trim().is_empty() => {
                anyhow::bail!("Tally direct company record contained unexpected text")
            }
            Event::Comment(_) | Event::CData(_) | Event::DocType(_) | Event::PI(_) => {
                anyhow::bail!("Tally direct company record contained a forbidden XML construct")
            }
            Event::Eof => {
                anyhow::bail!("Tally direct company response ended before COMPANYINFO closed")
            }
            _ => {}
        }
    }
    if !name_seen || !guid_seen {
        anyhow::bail!("Tally direct company response omitted a required company identity field")
    }
    Ok(())
}

fn read_direct_company_identity_text(
    reader: &mut Reader<&[u8]>,
    name: QName<'_>,
) -> anyhow::Result<()> {
    let mut saw_text = false;
    loop {
        match reader.read_event()? {
            Event::Text(text) => {
                let decoded = text.decode()?;
                let unescaped = quick_xml::escape::unescape(&decoded)?;
                if saw_text || unescaped.trim().is_empty() {
                    anyhow::bail!(
                        "Tally direct company identity field was not one non-empty text value"
                    )
                }
                saw_text = true;
            }
            Event::End(element) if element.name().as_ref().eq_ignore_ascii_case(name.as_ref()) => {
                if !saw_text {
                    anyhow::bail!("Tally direct company identity field was empty")
                }
                return Ok(());
            }
            Event::Eof => {
                anyhow::bail!("Tally direct company response ended before an identity field closed")
            }
            _ => anyhow::bail!(
                "Tally direct company identity field contained a forbidden XML construct"
            ),
        }
    }
}

fn configured_reader(xml: &str) -> Reader<&[u8]> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    reader
}

fn scan_export_evidence(xml: &str) -> anyhow::Result<ExportEvidence> {
    let mut reader = configured_reader(xml);
    let mut path = Vec::<Vec<u8>>::new();
    let mut company_context = None;
    let mut schema = None;
    let mut object_type = None;
    let mut source_record_count = None;
    let mut identities = HashMap::<String, u64>::new();
    let mut identified_record_count = 0_u64;
    loop {
        match reader.read_event()? {
            Event::Start(element) => {
                if is_supported_export_parent(&path)
                    && element
                        .name()
                        .as_ref()
                        .eq_ignore_ascii_case(b"COMPANYCONTEXT")
                {
                    if company_context.is_some() {
                        anyhow::bail!("Tally response included multiple company contexts");
                    }
                    let parsed = parse_company_context(&mut reader, &element, false)?;
                    company_context = Some(parsed.company);
                    schema = parsed.schema;
                    object_type = parsed.object_type;
                    source_record_count = parsed.source_record_count;
                    continue;
                }
                if is_supported_export_parent(&path)
                    && is_primary_export_row(element.name().as_ref())
                    && record_identities(&reader, &element, &mut identities)?
                {
                    identified_record_count = identified_record_count.saturating_add(1);
                }
                path.push(element.name().as_ref().to_ascii_uppercase());
            }
            Event::Empty(element)
                if is_supported_export_parent(&path)
                    && element
                        .name()
                        .as_ref()
                        .eq_ignore_ascii_case(b"COMPANYCONTEXT") =>
            {
                if company_context.is_some() {
                    anyhow::bail!("Tally response included multiple company contexts");
                }
                let parsed = parse_company_context(&mut reader, &element, true)?;
                company_context = Some(parsed.company);
                schema = parsed.schema;
                object_type = parsed.object_type;
                source_record_count = parsed.source_record_count;
            }
            Event::Empty(element) => {
                if is_supported_export_parent(&path)
                    && is_primary_export_row(element.name().as_ref())
                    && record_identities(&reader, &element, &mut identities)?
                {
                    identified_record_count = identified_record_count.saturating_add(1);
                }
            }
            Event::End(element) => pop_expected_path(&mut path, element.name().as_ref())?,
            Event::Eof => break,
            _ => {}
        }
    }
    let mut duplicate_identities = identities
        .into_iter()
        .filter(|(_, occurrences)| *occurrences > 1)
        .map(|(identity, occurrences)| DuplicateIdentityEvidence {
            identity_sha256: sha256_hex(identity.as_bytes()),
            occurrences,
        })
        .collect::<Vec<_>>();
    duplicate_identities.sort_by(|left, right| left.identity_sha256.cmp(&right.identity_sha256));
    Ok(ExportEvidence {
        company_context,
        schema,
        object_type,
        source_record_count,
        identified_record_count,
        duplicate_identities,
        ..ExportEvidence::default()
    })
}

fn is_supported_export_parent(path: &[Vec<u8>]) -> bool {
    path_eq(path, &[b"ENVELOPE", b"BODY"])
        || path_eq(path, &[b"ENVELOPE", b"BODY", b"DATA"])
        || path_eq(path, &[b"ENVELOPE", b"BODY", b"DATA", b"COLLECTION"])
}

fn is_primary_export_row(name: &[u8]) -> bool {
    [
        b"GROUP".as_slice(),
        b"VOUCHERTYPE".as_slice(),
        b"LEDGER".as_slice(),
        b"VOUCHER".as_slice(),
        b"LEDGERPERIODBALANCE".as_slice(),
    ]
    .into_iter()
    .any(|candidate| name.eq_ignore_ascii_case(candidate))
}

fn pop_expected_path(path: &mut Vec<Vec<u8>>, closing_name: &[u8]) -> anyhow::Result<()> {
    let expected = path
        .pop()
        .ok_or_else(|| anyhow::anyhow!("Tally response closed an unexpected element"))?;
    if !closing_name.eq_ignore_ascii_case(&expected) {
        anyhow::bail!("Tally response closed an unexpected element");
    }
    Ok(())
}

fn record_identities(
    reader: &Reader<&[u8]>,
    element: &quick_xml::events::BytesStart<'_>,
    identities: &mut HashMap<String, u64>,
) -> anyhow::Result<bool> {
    validate_unique_decodable_attributes(reader, element)?;
    let mut observed = false;
    for (identity_kind, key) in [
        ("guid", b"GUID".as_slice()),
        ("remote_id", b"REMOTEID".as_slice()),
        ("master_id", b"MASTERID".as_slice()),
    ] {
        let Some(identity) = attr_value(reader, element, key).filter(|value| !value.is_empty())
        else {
            continue;
        };
        observed = true;
        let object_type = String::from_utf8_lossy(element.name().as_ref()).to_ascii_uppercase();
        let identity = if identity_kind == "guid" {
            identity.to_ascii_lowercase()
        } else {
            identity
        };
        let scoped_identity = format!("{object_type}\0{identity_kind}\0{identity}");
        identities
            .entry(scoped_identity)
            .and_modify(|count| *count = count.saturating_add(1))
            .or_insert(1);
    }
    Ok(observed)
}

#[derive(Default)]
struct ParsedCompanyContext {
    company: CompanyContextEvidence,
    schema: Option<String>,
    object_type: Option<String>,
    source_record_count: Option<u64>,
}

#[derive(Clone, Copy)]
enum ContextField {
    Schema,
    ObjectType,
    Name,
    Guid,
    RecordCount,
    QueryIdentitySetSha256,
    RequestedFrom,
    RequestedTo,
}

fn parse_company_context(
    reader: &mut Reader<&[u8]>,
    element: &quick_xml::events::BytesStart<'_>,
    is_empty: bool,
) -> anyhow::Result<ParsedCompanyContext> {
    let mut context = ParsedCompanyContext::default();
    for attribute in element.attributes().with_checks(true) {
        let attribute = attribute
            .map_err(|_| anyhow::anyhow!("Tally company context contained malformed attributes"))?;
        let Some(field) = context_field(attribute.key.as_ref()) else {
            anyhow::bail!("Tally company context contained an unexpected attribute");
        };
        let value = attribute
            .decoded_and_normalized_value(quick_xml::XmlVersion::Implicit1_0, reader.decoder())
            .map_err(|_| {
                anyhow::anyhow!("Tally company context contained an invalid attribute value")
            })?;
        set_context_field(&mut context, field, value.trim())?;
    }
    if is_empty {
        return Ok(context);
    }
    loop {
        match reader.read_event()? {
            Event::Start(element) => {
                let Some(field) = context_field(element.name().as_ref()) else {
                    anyhow::bail!("Tally company context contained an unexpected child element");
                };
                let value = read_required_text(reader, element.name()).map_err(|_| {
                    anyhow::anyhow!("Tally company context contained an empty or invalid value")
                })?;
                set_context_field(&mut context, field, &value)?;
            }
            Event::Empty(element) if context_field(element.name().as_ref()).is_some() => {
                anyhow::bail!("Tally company context contained an empty metadata value");
            }
            Event::Empty(_) => {
                anyhow::bail!("Tally company context contained an unexpected child element");
            }
            Event::Text(text) if !text.decode()?.trim().is_empty() => {
                anyhow::bail!("Tally company context contained unexpected text");
            }
            Event::End(element)
                if element
                    .name()
                    .as_ref()
                    .eq_ignore_ascii_case(b"COMPANYCONTEXT") =>
            {
                break;
            }
            Event::Eof => anyhow::bail!("Tally response ended before COMPANYCONTEXT closed"),
            _ => {}
        }
    }
    Ok(context)
}

fn context_field(name: &[u8]) -> Option<ContextField> {
    if name.eq_ignore_ascii_case(b"SCHEMA") {
        Some(ContextField::Schema)
    } else if name.eq_ignore_ascii_case(b"OBJECTTYPE") {
        Some(ContextField::ObjectType)
    } else if name.eq_ignore_ascii_case(b"NAME") {
        Some(ContextField::Name)
    } else if name.eq_ignore_ascii_case(b"GUID") {
        Some(ContextField::Guid)
    } else if name.eq_ignore_ascii_case(b"RECORDCOUNT") {
        Some(ContextField::RecordCount)
    } else if name.eq_ignore_ascii_case(b"QUERYIDENTITYSETSHA256") {
        Some(ContextField::QueryIdentitySetSha256)
    } else if name.eq_ignore_ascii_case(b"FROMDATE") {
        Some(ContextField::RequestedFrom)
    } else if name.eq_ignore_ascii_case(b"TODATE") {
        Some(ContextField::RequestedTo)
    } else {
        None
    }
}

fn set_context_field(
    context: &mut ParsedCompanyContext,
    field: ContextField,
    value: &str,
) -> anyhow::Result<()> {
    if value.is_empty() {
        anyhow::bail!("Tally company context contained an empty metadata value");
    }
    match field {
        ContextField::Schema => set_once(&mut context.schema, value.to_owned())?,
        ContextField::ObjectType => set_once(&mut context.object_type, value.to_owned())?,
        ContextField::Name => set_once(&mut context.company.name, value.to_owned())?,
        ContextField::Guid => set_once(&mut context.company.guid, value.to_owned())?,
        ContextField::RecordCount => {
            if !value.bytes().all(|byte| byte.is_ascii_digit()) {
                anyhow::bail!("Tally company context RECORDCOUNT was not a non-negative integer");
            }
            let count = value.parse::<u64>().map_err(|_| {
                anyhow::anyhow!("Tally company context RECORDCOUNT was not a non-negative integer")
            })?;
            set_once(&mut context.source_record_count, count)?;
        }
        ContextField::QueryIdentitySetSha256 => {
            if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                anyhow::bail!("Tally company context contained an invalid query identity digest");
            }
            set_once(
                &mut context.company.query_identity_set_sha256,
                value.to_ascii_lowercase(),
            )?;
        }
        ContextField::RequestedFrom => {
            validate_yyyymmdd_context(value)?;
            set_once(
                &mut context.company.requested_from_yyyymmdd,
                value.to_string(),
            )?;
        }
        ContextField::RequestedTo => {
            validate_yyyymmdd_context(value)?;
            set_once(
                &mut context.company.requested_to_yyyymmdd,
                value.to_string(),
            )?;
        }
    }
    Ok(())
}

fn validate_yyyymmdd_context(value: &str) -> anyhow::Result<()> {
    if value.len() != 8 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        anyhow::bail!("Tally company context contained an invalid date binding");
    }
    Ok(())
}

fn set_once<T>(slot: &mut Option<T>, value: T) -> anyhow::Result<()> {
    if slot.is_some() {
        anyhow::bail!("Tally company context contained duplicate metadata");
    }
    *slot = Some(value);
    Ok(())
}

fn validate_scoped_export(
    evidence: &ExportEvidence,
    expected_schema: &str,
    expected_object_type: &str,
    parsed_record_count: usize,
) -> anyhow::Result<()> {
    let company = evidence
        .company_context
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Tally response omitted company context"))?;
    if company.name.is_none() || company.guid.is_none() {
        anyhow::bail!("Tally company context omitted required company identity");
    }
    match evidence.schema.as_deref() {
        Some(actual) if actual == expected_schema => {}
        Some(_) => anyhow::bail!("Tally response export schema did not match the parser"),
        None => anyhow::bail!("Tally company context omitted export schema"),
    }
    match evidence.object_type.as_deref() {
        Some(actual) if actual == expected_object_type => {}
        Some(_) => anyhow::bail!("Tally response object type did not match the parser"),
        None => anyhow::bail!("Tally company context omitted object type"),
    }
    let reported = evidence
        .source_record_count
        .ok_or_else(|| anyhow::anyhow!("Tally company context omitted source record count"))?;
    let parsed = u64::try_from(parsed_record_count)
        .map_err(|_| anyhow::anyhow!("Tally parsed record count exceeded the supported range"))?;
    if reported != parsed {
        anyhow::bail!("Tally source record count did not match parsed primary rows");
    }
    Ok(())
}

fn parsed_source_identities(
    reader: &Reader<&[u8]>,
    element: &quick_xml::events::BytesStart<'_>,
) -> anyhow::Result<ParsedSourceIdentities> {
    validate_unique_decodable_attributes(reader, element)?;
    Ok(ParsedSourceIdentities {
        guid: validated_optional_identifier(attr_value(reader, element, b"GUID"))?
            .map(|guid| guid.to_ascii_lowercase()),
        remote_id: validated_optional_identifier(attr_value(reader, element, b"REMOTEID"))?,
        master_id: validated_optional_identifier(attr_value(reader, element, b"MASTERID"))?,
    })
}

fn validated_optional_identifier(value: Option<String>) -> anyhow::Result<Option<String>> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_empty() {
        return Ok(None);
    }
    if value.len() > 512 || value.trim() != value || value.chars().any(char::is_control) {
        anyhow::bail!("Tally record contained an invalid source identifier");
    }
    Ok(Some(value))
}

fn preferred_identity(
    identities: &ParsedSourceIdentities,
) -> (Option<String>, Option<ParsedSourceIdentityKind>) {
    if let Some(guid) = &identities.guid {
        (Some(guid.clone()), Some(ParsedSourceIdentityKind::Guid))
    } else if let Some(remote_id) = &identities.remote_id {
        (
            Some(remote_id.clone()),
            Some(ParsedSourceIdentityKind::RemoteId),
        )
    } else if let Some(master_id) = &identities.master_id {
        (
            Some(master_id.clone()),
            Some(ParsedSourceIdentityKind::MasterId),
        )
    } else {
        (None, None)
    }
}

fn source_fragment_sha256(xml: &str, start: usize, end: usize) -> anyhow::Result<String> {
    let fragment = xml
        .as_bytes()
        .get(start..end)
        .ok_or_else(|| anyhow::anyhow!("Tally record fragment boundaries were invalid"))?;
    if fragment.is_empty() {
        anyhow::bail!("Tally record fragment was empty");
    }
    Ok(sha256_hex(fragment))
}

fn source_fragment_sha256_from_sanitized(
    sanitized: &tolerant_xml::SanitizedXml<'_>,
    start: usize,
    end: usize,
) -> anyhow::Result<String> {
    Ok(sha256_hex(sanitized.original_fragment(start, end)?))
}

fn parse_company_info(reader: &mut Reader<&[u8]>) -> anyhow::Result<TallyCompany> {
    let mut company = TallyCompany {
        name: String::new(),
        guid: None,
        company_number: None,
        books_from: None,
    };
    loop {
        match reader.read_event()? {
            Event::Start(element)
                if element
                    .name()
                    .as_ref()
                    .eq_ignore_ascii_case(b"COMPANYNAMEFIELD") =>
            {
                company.name = read_optional_text(reader, element.name())?.unwrap_or_default()
            }
            Event::Start(element)
                if element
                    .name()
                    .as_ref()
                    .eq_ignore_ascii_case(b"COMPANYGUIDFIELD")
                    || element.name().as_ref().eq_ignore_ascii_case(b"GUIDFIELD") =>
            {
                company.guid = read_optional_text(reader, element.name())?
            }
            Event::End(element) if element.name().as_ref().eq_ignore_ascii_case(b"COMPANYINFO") => {
                break;
            }
            Event::Eof => anyhow::bail!("Tally company response ended before COMPANYINFO closed"),
            _ => {}
        }
    }
    Ok(company)
}

fn parse_named_master(
    reader: &mut Reader<&[u8]>,
    element_name: &[u8],
    name: String,
) -> anyhow::Result<TallyNamedMaster> {
    let mut record = TallyNamedMaster {
        name,
        parent: PartyLedgerMasterFieldObservation::NotObserved,
        reserved_name: None,
    };
    let mut parent_seen = false;
    loop {
        match reader.read_event()? {
            Event::Start(element) if element.name().as_ref().eq_ignore_ascii_case(b"PARENT") => {
                validate_only_attributes(&element, &[])?;
                if std::mem::replace(&mut parent_seen, true) {
                    anyhow::bail!("Tally master row repeated PARENT");
                }
                record.parent = PartyLedgerMasterFieldObservation::Returned(
                    read_identifier_text(reader, element.name())?.unwrap_or_default(),
                );
            }
            Event::End(element) if element.name().as_ref().eq_ignore_ascii_case(element_name) => {
                break;
            }
            Event::Start(_) | Event::Empty(_) => {
                anyhow::bail!("Tally master row contained an unexpected field");
            }
            Event::Text(text) if !text.decode()?.trim().is_empty() => {
                anyhow::bail!("Tally master row contained unexpected text");
            }
            Event::Eof => anyhow::bail!("Tally master response ended before its row closed"),
            _ => {}
        }
    }
    Ok(record)
}

fn parse_ledger(reader: &mut Reader<&[u8]>, name: Option<String>) -> anyhow::Result<TallyLedger> {
    let mut ledger = TallyLedger {
        name: name.unwrap_or_default(),
        parent: PartyLedgerMasterFieldObservation::NotObserved,
        party_gstin: PartyLedgerMasterFieldObservation::NotObserved,
        opening_balance: None,
    };
    let mut parent_seen = false;
    let mut gstin_seen = false;
    let mut opening_seen = false;
    loop {
        match reader.read_event()? {
            Event::Start(element) if element.name().as_ref().eq_ignore_ascii_case(b"PARENT") => {
                validate_only_attributes(&element, &[])?;
                if std::mem::replace(&mut parent_seen, true) {
                    anyhow::bail!("Tally ledger row repeated PARENT");
                }
                ledger.parent = PartyLedgerMasterFieldObservation::Returned(
                    read_identifier_text(reader, element.name())?.unwrap_or_default(),
                )
            }
            Event::Start(element)
                if element.name().as_ref().eq_ignore_ascii_case(b"PARTYGSTIN") =>
            {
                validate_only_attributes(&element, &[])?;
                if std::mem::replace(&mut gstin_seen, true) {
                    anyhow::bail!("Tally ledger row repeated PARTYGSTIN");
                }
                ledger.party_gstin = PartyLedgerMasterFieldObservation::Returned(
                    read_optional_text(reader, element.name())?.unwrap_or_default(),
                )
            }
            Event::Start(element)
                if element
                    .name()
                    .as_ref()
                    .eq_ignore_ascii_case(b"OPENINGBALANCE") =>
            {
                validate_only_attributes(&element, &[])?;
                if std::mem::replace(&mut opening_seen, true) {
                    anyhow::bail!("Tally ledger row repeated OPENINGBALANCE");
                }
                ledger.opening_balance = read_optional_text(reader, element.name())?
            }
            Event::Empty(element) if element.name().as_ref().eq_ignore_ascii_case(b"PARENT") => {
                validate_only_attributes(&element, &[])?;
                if std::mem::replace(&mut parent_seen, true) {
                    anyhow::bail!("Tally ledger row repeated PARENT");
                }
                ledger.parent = PartyLedgerMasterFieldObservation::Returned(String::new());
            }
            Event::Empty(element)
                if element.name().as_ref().eq_ignore_ascii_case(b"PARTYGSTIN") =>
            {
                validate_only_attributes(&element, &[])?;
                if std::mem::replace(&mut gstin_seen, true) {
                    anyhow::bail!("Tally ledger row repeated PARTYGSTIN");
                }
                ledger.party_gstin = PartyLedgerMasterFieldObservation::Returned(String::new());
            }
            Event::End(element) if element.name().as_ref().eq_ignore_ascii_case(b"LEDGER") => break,
            Event::Start(_) | Event::Empty(_) => {
                anyhow::bail!("Tally ledger row contained an unexpected field");
            }
            Event::Text(text) if !text.decode()?.trim().is_empty() => {
                anyhow::bail!("Tally ledger row contained unexpected text");
            }
            Event::Eof => anyhow::bail!("Tally ledger response ended before LEDGER closed"),
            _ => {}
        }
    }
    Ok(ledger)
}

fn parse_ledger_write_readback(
    reader: &mut Reader<&[u8]>,
    name: Option<String>,
) -> anyhow::Result<TallyLedger> {
    let mut ledger = TallyLedger {
        name: name.ok_or_else(|| anyhow::anyhow!("Tally write readback omitted ledger NAME"))?,
        parent: PartyLedgerMasterFieldObservation::NotObserved,
        party_gstin: PartyLedgerMasterFieldObservation::NotObserved,
        opening_balance: None,
    };
    let mut parent_seen = false;
    let mut gstin_seen = false;
    let mut opening_seen = false;
    loop {
        match reader.read_event()? {
            Event::Start(element) if element.name().as_ref().eq_ignore_ascii_case(b"PARENT") => {
                validate_only_attributes(&element, &[])?;
                if std::mem::replace(&mut parent_seen, true) {
                    anyhow::bail!("Tally write readback repeated PARENT");
                }
                ledger.parent = PartyLedgerMasterFieldObservation::Returned(
                    read_identifier_text(reader, element.name())?.unwrap_or_default(),
                );
            }
            Event::Start(element)
                if element.name().as_ref().eq_ignore_ascii_case(b"PARTYGSTIN") =>
            {
                validate_only_attributes(&element, &[])?;
                if std::mem::replace(&mut gstin_seen, true) {
                    anyhow::bail!("Tally write readback repeated PARTYGSTIN");
                }
                ledger.party_gstin = PartyLedgerMasterFieldObservation::Returned(
                    read_optional_text(reader, element.name())?.unwrap_or_default(),
                );
            }
            Event::Start(element)
                if element
                    .name()
                    .as_ref()
                    .eq_ignore_ascii_case(b"OPENINGBALANCE") =>
            {
                validate_only_attributes(&element, &[])?;
                if std::mem::replace(&mut opening_seen, true) {
                    anyhow::bail!("Tally write readback repeated OPENINGBALANCE");
                }
                ledger.opening_balance = read_optional_text(reader, element.name())?;
            }
            Event::Empty(element) if element.name().as_ref().eq_ignore_ascii_case(b"PARENT") => {
                validate_only_attributes(&element, &[])?;
                if std::mem::replace(&mut parent_seen, true) {
                    anyhow::bail!("Tally write readback repeated PARENT");
                }
                ledger.parent = PartyLedgerMasterFieldObservation::Returned(String::new());
            }
            Event::Empty(element)
                if element.name().as_ref().eq_ignore_ascii_case(b"PARTYGSTIN") =>
            {
                validate_only_attributes(&element, &[])?;
                if std::mem::replace(&mut gstin_seen, true) {
                    anyhow::bail!("Tally write readback repeated PARTYGSTIN");
                }
                ledger.party_gstin = PartyLedgerMasterFieldObservation::Returned(String::new());
            }
            Event::Empty(element)
                if element
                    .name()
                    .as_ref()
                    .eq_ignore_ascii_case(b"OPENINGBALANCE") =>
            {
                validate_only_attributes(&element, &[])?;
                if std::mem::replace(&mut opening_seen, true) {
                    anyhow::bail!("Tally write readback repeated OPENINGBALANCE");
                }
            }
            Event::Start(_) | Event::Empty(_) => {
                anyhow::bail!("Tally write readback contained an unexpected ledger field");
            }
            Event::Text(text) if !text.decode()?.trim().is_empty() => {
                anyhow::bail!("Tally write readback contained unexpected ledger text");
            }
            Event::End(element) if element.name().as_ref().eq_ignore_ascii_case(b"LEDGER") => break,
            Event::End(_) => anyhow::bail!("Tally write readback closed an unexpected element"),
            Event::Eof => anyhow::bail!("Tally write readback ended before LEDGER closed"),
            _ => {}
        }
    }
    Ok(ledger)
}

fn parse_ledger_period_balance(
    reader: &mut Reader<&[u8]>,
) -> anyhow::Result<TallyLedgerPeriodBalance> {
    let mut opening_balance = None;
    let mut closing_balance = None;
    loop {
        match reader.read_event()? {
            Event::Start(element)
                if element
                    .name()
                    .as_ref()
                    .eq_ignore_ascii_case(b"OPENINGBALANCE") =>
            {
                if opening_balance.is_some() {
                    anyhow::bail!("Tally period-balance row repeated opening amount");
                }
                opening_balance = Some(read_required_text(reader, element.name())?);
            }
            Event::Start(element)
                if element
                    .name()
                    .as_ref()
                    .eq_ignore_ascii_case(b"CLOSINGBALANCE") =>
            {
                if closing_balance.is_some() {
                    anyhow::bail!("Tally period-balance row repeated closing amount");
                }
                closing_balance = Some(read_required_text(reader, element.name())?);
            }
            Event::Empty(element)
                if element
                    .name()
                    .as_ref()
                    .eq_ignore_ascii_case(b"OPENINGBALANCE") =>
            {
                anyhow::bail!("Tally period-balance row contained an empty opening amount");
            }
            Event::Empty(element)
                if element
                    .name()
                    .as_ref()
                    .eq_ignore_ascii_case(b"CLOSINGBALANCE") =>
            {
                anyhow::bail!("Tally period-balance row contained an empty closing amount");
            }
            Event::End(element)
                if element
                    .name()
                    .as_ref()
                    .eq_ignore_ascii_case(b"LEDGERPERIODBALANCE") =>
            {
                break;
            }
            Event::Start(_) | Event::Empty(_) => {
                anyhow::bail!("Tally period-balance row contained an unexpected field");
            }
            Event::Text(text) if !text.decode()?.trim().is_empty() => {
                anyhow::bail!("Tally period-balance row contained unexpected text");
            }
            Event::Eof => anyhow::bail!("Tally period-balance row ended before closing"),
            _ => {}
        }
    }
    Ok(TallyLedgerPeriodBalance {
        opening_balance: opening_balance
            .ok_or_else(|| anyhow::anyhow!("Tally period-balance row omitted opening amount"))?,
        closing_balance: closing_balance
            .ok_or_else(|| anyhow::anyhow!("Tally period-balance row omitted closing amount"))?,
    })
}

fn parse_voucher(
    reader: &mut Reader<&[u8]>,
    id: Option<String>,
    xml: &str,
) -> anyhow::Result<TallyVoucher> {
    let mut voucher = TallyVoucher {
        id,
        date: None,
        voucher_type: None,
        voucher_number: None,
        party_ledger_name: None,
        cancelled: None,
        optional: None,
        ledger_entry_count: None,
        ledger_entries: Vec::new(),
    };
    let mut seen = HashSet::new();
    loop {
        match reader.read_event()? {
            Event::Start(element) if element.name().as_ref().eq_ignore_ascii_case(b"DATE") => {
                validate_only_attributes(&element, &[])?;
                mark_unique_field(&mut seen, "DATE", "voucher")?;
                voucher.date = read_optional_text(reader, element.name())?
            }
            Event::Start(element)
                if element
                    .name()
                    .as_ref()
                    .eq_ignore_ascii_case(b"VOUCHERTYPENAME") =>
            {
                validate_only_attributes(&element, &[])?;
                mark_unique_field(&mut seen, "VOUCHERTYPENAME", "voucher")?;
                voucher.voucher_type = read_identifier_text(reader, element.name())?
            }
            Event::Start(element)
                if element
                    .name()
                    .as_ref()
                    .eq_ignore_ascii_case(b"VOUCHERNUMBER") =>
            {
                validate_only_attributes(&element, &[])?;
                mark_unique_field(&mut seen, "VOUCHERNUMBER", "voucher")?;
                voucher.voucher_number = read_optional_text(reader, element.name())?
            }
            Event::Start(element)
                if element
                    .name()
                    .as_ref()
                    .eq_ignore_ascii_case(b"PARTYLEDGERNAME") =>
            {
                validate_only_attributes(&element, &[])?;
                mark_unique_field(&mut seen, "PARTYLEDGERNAME", "voucher")?;
                voucher.party_ledger_name = read_identifier_text(reader, element.name())?
            }
            Event::Start(element)
                if element.name().as_ref().eq_ignore_ascii_case(b"ISCANCELLED") =>
            {
                validate_only_attributes(&element, &[])?;
                mark_unique_field(&mut seen, "ISCANCELLED", "voucher")?;
                voucher.cancelled = read_optional_text(reader, element.name())?
                    .map(|value| parse_tally_boolean(&value))
                    .transpose()?;
            }
            Event::Start(element)
                if element.name().as_ref().eq_ignore_ascii_case(b"ISOPTIONAL") =>
            {
                validate_only_attributes(&element, &[])?;
                mark_unique_field(&mut seen, "ISOPTIONAL", "voucher")?;
                voucher.optional = read_optional_text(reader, element.name())?
                    .map(|value| parse_tally_boolean(&value))
                    .transpose()?;
            }
            Event::Start(element)
                if element
                    .name()
                    .as_ref()
                    .eq_ignore_ascii_case(b"LEDGERENTRYCOUNT") =>
            {
                validate_only_attributes(&element, &[])?;
                mark_unique_field(&mut seen, "LEDGERENTRYCOUNT", "voucher")?;
                let value = read_optional_text(reader, element.name())?
                    .ok_or_else(|| anyhow::anyhow!("Tally voucher omitted ledger entry count"))?;
                if !value.bytes().all(|byte| byte.is_ascii_digit()) {
                    anyhow::bail!("Tally voucher ledger entry count was invalid");
                }
                voucher.ledger_entry_count = Some(value.parse().map_err(|_| {
                    anyhow::anyhow!("Tally voucher ledger entry count was invalid")
                })?);
            }
            Event::Start(element)
                if element
                    .name()
                    .as_ref()
                    .eq_ignore_ascii_case(b"LEDGERENTRIES") =>
            {
                validate_only_attributes(&element, &[])?;
                mark_unique_field(&mut seen, "LEDGERENTRIES", "voucher")?;
                parse_ledger_entries(reader, xml, &mut voucher.ledger_entries)?;
            }
            Event::End(element) if element.name().as_ref().eq_ignore_ascii_case(b"VOUCHER") => {
                break;
            }
            Event::Start(_) | Event::Empty(_) => {
                anyhow::bail!("Tally voucher row contained an unexpected field");
            }
            Event::Text(text) if !text.decode()?.trim().is_empty() => {
                anyhow::bail!("Tally voucher row contained unexpected text");
            }
            Event::Eof => anyhow::bail!("Tally voucher response ended before VOUCHER closed"),
            _ => {}
        }
    }
    let reported = voucher
        .ledger_entry_count
        .ok_or_else(|| anyhow::anyhow!("Tally voucher omitted ledger entry count"))?;
    if reported != voucher.ledger_entries.len() as u64 {
        anyhow::bail!("Tally voucher ledger entry count did not match parsed rows");
    }
    for (offset, entry) in voucher.ledger_entries.iter().enumerate() {
        if entry.entry_index != (offset + 1) as u64 {
            anyhow::bail!("Tally voucher ledger entry indexes were not contiguous");
        }
    }
    Ok(voucher)
}

fn parse_ledger_entries(
    reader: &mut Reader<&[u8]>,
    xml: &str,
    entries: &mut Vec<TallyLedgerEntry>,
) -> anyhow::Result<()> {
    loop {
        let element_start = reader.buffer_position() as usize;
        match reader.read_event()? {
            Event::Start(element)
                if element.name().as_ref().eq_ignore_ascii_case(b"LEDGERENTRY") =>
            {
                validate_only_attributes(&element, &[])?;
                let mut entry = parse_ledger_entry(reader)?;
                entry.raw_source_sha256 =
                    source_fragment_sha256(xml, element_start, reader.buffer_position() as usize)?;
                entries.push(entry);
            }
            Event::End(element)
                if element
                    .name()
                    .as_ref()
                    .eq_ignore_ascii_case(b"LEDGERENTRIES") =>
            {
                break;
            }
            Event::Start(_) | Event::Empty(_) => {
                anyhow::bail!("Tally ledger-entry collection contained an unexpected field");
            }
            Event::Text(text) if !text.decode()?.trim().is_empty() => {
                anyhow::bail!("Tally ledger-entry collection contained unexpected text");
            }
            Event::Eof => {
                anyhow::bail!("Tally voucher ended before LEDGERENTRIES closed");
            }
            _ => {}
        }
    }
    Ok(())
}

fn parse_ledger_entry(reader: &mut Reader<&[u8]>) -> anyhow::Result<TallyLedgerEntry> {
    let mut entry_index = None;
    let mut ledger_name = None;
    let mut amount = None;
    let mut is_deemed_positive = None;
    let mut seen = HashSet::new();
    loop {
        match reader.read_event()? {
            Event::Start(element)
                if element.name().as_ref().eq_ignore_ascii_case(b"ENTRYINDEX") =>
            {
                validate_only_attributes(&element, &[])?;
                mark_unique_field(&mut seen, "ENTRYINDEX", "ledger entry")?;
                let value = read_optional_text(reader, element.name())?
                    .ok_or_else(|| anyhow::anyhow!("Tally ledger entry omitted its index"))?;
                entry_index = Some(
                    value
                        .parse::<u64>()
                        .map_err(|_| anyhow::anyhow!("Tally ledger entry index was invalid"))?,
                );
            }
            Event::Start(element)
                if element.name().as_ref().eq_ignore_ascii_case(b"LEDGERNAME") =>
            {
                validate_only_attributes(&element, &[])?;
                mark_unique_field(&mut seen, "LEDGERNAME", "ledger entry")?;
                ledger_name = read_identifier_text(reader, element.name())?;
            }
            Event::Start(element) if element.name().as_ref().eq_ignore_ascii_case(b"AMOUNT") => {
                validate_only_attributes(&element, &[])?;
                mark_unique_field(&mut seen, "AMOUNT", "ledger entry")?;
                amount = read_optional_text(reader, element.name())?;
            }
            Event::Start(element)
                if element
                    .name()
                    .as_ref()
                    .eq_ignore_ascii_case(b"ISDEEMEDPOSITIVE") =>
            {
                validate_only_attributes(&element, &[])?;
                mark_unique_field(&mut seen, "ISDEEMEDPOSITIVE", "ledger entry")?;
                is_deemed_positive = read_optional_text(reader, element.name())?
                    .map(|value| parse_tally_boolean(&value))
                    .transpose()?;
            }
            Event::End(element) if element.name().as_ref().eq_ignore_ascii_case(b"LEDGERENTRY") => {
                break;
            }
            Event::Start(_) | Event::Empty(_) => {
                anyhow::bail!("Tally ledger entry contained an unexpected field");
            }
            Event::Text(text) if !text.decode()?.trim().is_empty() => {
                anyhow::bail!("Tally ledger entry contained unexpected text");
            }
            Event::Eof => anyhow::bail!("Tally voucher ended before ledger entry closed"),
            _ => {}
        }
    }
    Ok(TallyLedgerEntry {
        entry_index: entry_index
            .filter(|index| *index > 0)
            .ok_or_else(|| anyhow::anyhow!("Tally ledger entry index was invalid"))?,
        ledger_name: ledger_name
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow::anyhow!("Tally ledger entry omitted ledger name"))?,
        amount: amount
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow::anyhow!("Tally ledger entry omitted amount"))?,
        is_deemed_positive: is_deemed_positive
            .ok_or_else(|| anyhow::anyhow!("Tally ledger entry omitted sign evidence"))?,
        raw_source_sha256: String::new(),
    })
}

fn mark_unique_field(
    seen: &mut HashSet<&'static str>,
    field: &'static str,
    context: &str,
) -> anyhow::Result<()> {
    if !seen.insert(field) {
        anyhow::bail!("Tally {context} repeated {field}");
    }
    Ok(())
}

fn parse_tally_boolean(value: &str) -> anyhow::Result<bool> {
    if value.eq_ignore_ascii_case("yes") || value.eq_ignore_ascii_case("true") || value == "1" {
        Ok(true)
    } else if value.eq_ignore_ascii_case("no")
        || value.eq_ignore_ascii_case("false")
        || value == "0"
    {
        Ok(false)
    } else {
        anyhow::bail!("Tally voucher contained an invalid boolean value")
    }
}

fn attr_value(
    reader: &Reader<&[u8]>,
    element: &quick_xml::events::BytesStart<'_>,
    key: &[u8],
) -> Option<String> {
    element
        .attributes()
        .flatten()
        .find(|attr| attr.key.as_ref().eq_ignore_ascii_case(key))
        .and_then(|attr| {
            attr.decoded_and_normalized_value(quick_xml::XmlVersion::Implicit1_0, reader.decoder())
                .ok()
        })
        .map(|value| value.into_owned())
        .filter(|value| !value.trim().is_empty())
}

fn validate_unique_decodable_attributes(
    reader: &Reader<&[u8]>,
    element: &quick_xml::events::BytesStart<'_>,
) -> anyhow::Result<()> {
    let mut seen = HashSet::new();
    for attribute in element.attributes().with_checks(true) {
        let attribute = attribute
            .map_err(|_| anyhow::anyhow!("Tally record contained malformed attributes"))?;
        if !seen.insert(attribute.key.as_ref().to_ascii_lowercase()) {
            anyhow::bail!("Tally record repeated an attribute");
        }
        attribute
            .decoded_and_normalized_value(quick_xml::XmlVersion::Implicit1_0, reader.decoder())
            .map_err(|_| anyhow::anyhow!("Tally record contained an invalid attribute value"))?;
    }
    Ok(())
}

fn validate_only_attributes(
    element: &quick_xml::events::BytesStart<'_>,
    allowed: &[&[u8]],
) -> anyhow::Result<()> {
    let mut seen = HashSet::new();
    for attribute in element.attributes().with_checks(true) {
        let attribute = attribute
            .map_err(|_| anyhow::anyhow!("Tally period report attributes were malformed"))?;
        if !allowed
            .iter()
            .any(|key| attribute.key.as_ref().eq_ignore_ascii_case(key))
        {
            anyhow::bail!("Tally period report contained an unexpected attribute");
        }
        if !seen.insert(attribute.key.as_ref().to_ascii_lowercase()) {
            anyhow::bail!("Tally response repeated a case-insensitive attribute");
        }
    }
    Ok(())
}

fn read_optional_text(
    reader: &mut Reader<&[u8]>,
    name: QName<'_>,
) -> anyhow::Result<Option<String>> {
    let value = reader.read_text(name)?;
    let decoded = value.decode()?;
    let unescaped = quick_xml::escape::unescape(&decoded)?;
    let trimmed = unescaped.trim();
    Ok((!trimmed.is_empty()).then(|| trimmed.to_owned()))
}

fn read_required_text(reader: &mut Reader<&[u8]>, name: QName<'_>) -> anyhow::Result<String> {
    read_optional_text(reader, name)?
        .ok_or_else(|| anyhow::anyhow!("Tally response contained an empty required value"))
}

/// Reads element text WITHOUT trimming, for foreign identifier references — LEDGERNAME,
/// PARENT, VOUCHERTYPENAME, PARTYLEDGERNAME — that must byte-for-byte match a master NAME
/// attribute. `attr_value` stores that NAME verbatim (untrimmed) per `ForeignText::from_tally`'s
/// no-normalisation contract, and the canonical-window lookup maps are keyed on that verbatim
/// text. Trimming a reference here while the map key stays untrimmed would silently break the
/// lookup for any master whose name carries incidental leading/trailing whitespace. Only the
/// emptiness check trims, matching `attr_value`'s own "all-whitespace counts as absent" rule;
/// the returned value itself is never trimmed.
fn read_identifier_text(
    reader: &mut Reader<&[u8]>,
    name: QName<'_>,
) -> anyhow::Result<Option<String>> {
    let value = reader.read_text(name)?;
    let decoded = value.decode()?;
    let unescaped = quick_xml::escape::unescape(&decoded)?;
    Ok((!unescaped.trim().is_empty()).then(|| unescaped.into_owned()))
}

fn read_required_identifier_text(
    reader: &mut Reader<&[u8]>,
    name: QName<'_>,
) -> anyhow::Result<String> {
    read_identifier_text(reader, name)?
        .ok_or_else(|| anyhow::anyhow!("Tally response contained an empty required value"))
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        write!(&mut encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    encoded
}
