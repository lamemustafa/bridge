//! Portable decoding and strict application-level parsing for Tally responses.
//!
//! This crate has no HTTP, database, native-library, or Tauri dependency. HTTP
//! success must be checked separately. Every parser that produces durable or
//! qualification evidence requires Tally application `STATUS=1`; interactive
//! company discovery additionally accepts one strict, direct report shape for
//! documented compatibility.

use std::{
    collections::{HashMap, HashSet},
    fmt::Write as _,
};

use quick_xml::{events::Event, name::QName, Reader};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[cfg(feature = "bills-native-outstandings-probe")]
pub mod bills_native_outstandings_probe;
#[cfg(feature = "bills-payments-observation-parser")]
pub mod bills_payments_observation;
#[cfg(feature = "india-tax-observation-parser")]
pub mod india_tax_observation;
#[cfg(feature = "jsonex-parser")]
pub mod jsonex;
#[cfg(feature = "jsonex-request-builder")]
pub mod jsonex_request;
pub mod xml_read_profiles;

pub const BRIDGE_LEDGER_EXPORT_SCHEMA: &str = "bridge.tally.ledgers/1";
pub const BRIDGE_LEDGER_WRITE_READBACK_SCHEMA: &str = "bridge.tally.ledger-write-readback/1";
pub const BRIDGE_GROUP_EXPORT_SCHEMA: &str = "bridge.tally.groups/1";
pub const BRIDGE_VOUCHER_TYPE_EXPORT_SCHEMA: &str = "bridge.tally.voucher-types/1";
pub const BRIDGE_VOUCHER_EXPORT_SCHEMA: &str = "bridge.tally.vouchers/2";
pub const BRIDGE_SELECTED_VOUCHER_EXPORT_SCHEMA: &str = "bridge.tally.vouchers/3";
pub const BRIDGE_LEDGER_PERIOD_BALANCE_SCHEMA: &str = "bridge.tally.ledger-period-balances/1";
pub const MAX_INTERACTIVE_DISCOVERY_COMPANIES: usize = 100;
pub const MAX_STANDARD_LEDGER_IDENTITY_ROWS: usize = 1_000;

#[derive(Debug, Deserialize, Serialize)]
pub struct TallyEnvelope<T> {
    #[serde(rename = "BODY")]
    pub body: Option<T>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct TallyCompany {
    pub name: String,
    pub guid: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandardLedgerIdentityObservation {
    pub company_guid: String,
    pub ledger_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct TallyLedger {
    pub name: String,
    pub parent: Option<String>,
    pub party_gstin: Option<String>,
    pub opening_balance: Option<String>,
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
    pub parent: Option<String>,
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

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct TallyImportResult {
    pub created: u64,
    pub altered: u64,
    pub deleted: u64,
    pub ignored: u64,
    pub errors: u64,
    pub cancelled: u64,
    pub exceptions: u64,
    pub line_error_count: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TallyImportApplicationStatus {
    Success,
    Failure,
    NotReported,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct TallyImportOutcome {
    application_status: TallyImportApplicationStatus,
    counters: TallyImportResult,
    exceptions_were_reported: bool,
}

impl TallyImportOutcome {
    pub fn application_status(&self) -> TallyImportApplicationStatus {
        self.application_status
    }

    pub fn counters(&self) -> &TallyImportResult {
        &self.counters
    }

    /// Distinguishes a source-observed `EXCEPTIONS` counter from the documented
    /// direct profile's Bridge-defaulted zero when that field is absent.
    pub fn exceptions_were_reported(&self) -> bool {
        self.exceptions_were_reported
    }

    pub fn into_counters(self) -> TallyImportResult {
        self.counters
    }
}

/// Redacted, parser-derived evidence for one Tally import response.
///
/// The raw response and raw `LINEERROR` text are deliberately not retained.
/// Callers cannot construct this type, so counter and digest evidence cannot be
/// mixed with a different response.
#[derive(Clone, PartialEq, Eq)]
pub struct ParsedImportEvidence {
    application_status: TallyImportApplicationStatus,
    counters: TallyImportResult,
    exceptions_were_reported: bool,
    response_sha256: String,
    line_error_sha256: Vec<String>,
}

impl std::fmt::Debug for ParsedImportEvidence {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ParsedImportEvidence")
            .field("application_status", &self.application_status)
            .field("counters", &self.counters)
            .field("exceptions_were_reported", &self.exceptions_were_reported)
            .field("response_sha256", &self.response_sha256)
            .field("line_error_count", &self.line_error_sha256.len())
            .finish()
    }
}

impl ParsedImportEvidence {
    pub fn application_status(&self) -> TallyImportApplicationStatus {
        self.application_status
    }

    pub fn counters(&self) -> &TallyImportResult {
        &self.counters
    }

    pub fn exceptions_were_reported(&self) -> bool {
        self.exceptions_were_reported
    }

    pub fn response_sha256(&self) -> &str {
        &self.response_sha256
    }

    pub fn line_error_sha256(&self) -> &[String] {
        &self.line_error_sha256
    }
}

impl TallyImportResult {
    pub fn is_clean_success(&self) -> bool {
        self.ignored == 0
            && self.errors == 0
            && self.cancelled == 0
            && self.exceptions == 0
            && self.line_error_count == 0
    }
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
    pub source_record_count: Option<u64>,
    pub identified_record_count: u64,
    pub duplicate_identities: Vec<DuplicateIdentityEvidence>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TallyTextEncoding {
    Utf8,
    Utf8Bom,
    Utf16LeBom,
    Utf16BeBom,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedTallyText {
    pub text: String,
    pub encoding: TallyTextEncoding,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TallyTextDecodeError {
    TooLarge,
    InvalidUtf8,
    InvalidUtf16Le,
    InvalidUtf16Be,
}

/// Incremental, decoded-size-bounded Tally text decoder.
///
/// The decoder retains the decoded UTF-8 text because the current protocol
/// parsers consume `&str`, but it never retains the complete encoded body. Its
/// boundary state is limited to a possible BOM, an incomplete UTF-8 scalar,
/// or one incomplete UTF-16 code unit/surrogate pair.
pub struct TallyTextStreamDecoder {
    max_decoded_bytes: usize,
    prefix: Vec<u8>,
    mode: Option<TallyTextStreamMode>,
    text: String,
    decoded_sha256: Sha256,
}

enum TallyTextStreamMode {
    Utf8 {
        encoding: TallyTextEncoding,
        pending: Vec<u8>,
    },
    Utf16 {
        encoding: TallyTextEncoding,
        little_endian: bool,
        pending_byte: Option<u8>,
        pending_high_surrogate: Option<u16>,
    },
}

/// Completed incremental decoding evidence. The digest covers the decoded
/// text re-encoded as UTF-8, with any wire BOM removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamDecodedTallyText {
    pub text: String,
    pub encoding: TallyTextEncoding,
    pub decoded_bytes: usize,
    pub decoded_sha256: String,
}

impl TallyTextStreamDecoder {
    pub fn new(max_decoded_bytes: usize) -> Self {
        Self {
            max_decoded_bytes,
            prefix: Vec::with_capacity(3),
            mode: None,
            text: String::new(),
            decoded_sha256: Sha256::new(),
        }
    }

    /// Consumes one encoded response chunk. An error makes the partial decoder
    /// unsuitable for evidence; callers must discard it and must not interpret
    /// the retained prefix as a partial response.
    pub fn push_chunk(&mut self, chunk: &[u8]) -> Result<(), TallyTextDecodeError> {
        let mut offset = 0;
        while self.mode.is_none() && offset < chunk.len() {
            self.prefix.push(chunk[offset]);
            offset += 1;
            match tally_text_prefix_decision(&self.prefix) {
                TallyTextPrefixDecision::NeedMore => {}
                TallyTextPrefixDecision::Utf8Bom => {
                    self.prefix.clear();
                    self.mode = Some(TallyTextStreamMode::Utf8 {
                        encoding: TallyTextEncoding::Utf8Bom,
                        pending: Vec::with_capacity(3),
                    });
                }
                TallyTextPrefixDecision::Utf16LeBom => {
                    self.prefix.clear();
                    self.mode = Some(TallyTextStreamMode::Utf16 {
                        encoding: TallyTextEncoding::Utf16LeBom,
                        little_endian: true,
                        pending_byte: None,
                        pending_high_surrogate: None,
                    });
                }
                TallyTextPrefixDecision::Utf16BeBom => {
                    self.prefix.clear();
                    self.mode = Some(TallyTextStreamMode::Utf16 {
                        encoding: TallyTextEncoding::Utf16BeBom,
                        little_endian: false,
                        pending_byte: None,
                        pending_high_surrogate: None,
                    });
                }
                TallyTextPrefixDecision::Utf8WithoutBom => {
                    self.mode = Some(TallyTextStreamMode::Utf8 {
                        encoding: TallyTextEncoding::Utf8,
                        pending: Vec::with_capacity(3),
                    });
                }
            }
        }

        if self.mode.is_none() {
            return Ok(());
        }
        if !self.prefix.is_empty() {
            let prefix = std::mem::take(&mut self.prefix);
            self.process_selected(&prefix)?;
        }
        self.process_selected(&chunk[offset..])
    }

    pub fn finish(mut self) -> Result<StreamDecodedTallyText, TallyTextDecodeError> {
        if self.mode.is_none() {
            self.mode = Some(TallyTextStreamMode::Utf8 {
                encoding: TallyTextEncoding::Utf8,
                pending: Vec::with_capacity(3),
            });
            let prefix = std::mem::take(&mut self.prefix);
            self.process_selected(&prefix)?;
        }
        let encoding = match self.mode.as_ref().expect("stream mode is selected") {
            TallyTextStreamMode::Utf8 { encoding, pending } => {
                if !pending.is_empty() {
                    return Err(TallyTextDecodeError::InvalidUtf8);
                }
                *encoding
            }
            TallyTextStreamMode::Utf16 {
                encoding,
                pending_byte,
                pending_high_surrogate,
                ..
            } => {
                if pending_byte.is_some() || pending_high_surrogate.is_some() {
                    return Err(invalid_utf16(*encoding));
                }
                *encoding
            }
        };
        let decoded_bytes = self.text.len();
        let decoded_sha256 = encode_sha256(self.decoded_sha256.finalize());
        Ok(StreamDecodedTallyText {
            text: self.text,
            encoding,
            decoded_bytes,
            decoded_sha256,
        })
    }

    fn process_selected(&mut self, bytes: &[u8]) -> Result<(), TallyTextDecodeError> {
        match self.mode.as_mut().expect("stream mode is selected") {
            TallyTextStreamMode::Utf8 { pending, .. } => process_utf8_chunk(
                pending,
                bytes,
                self.max_decoded_bytes,
                &mut self.text,
                &mut self.decoded_sha256,
            ),
            TallyTextStreamMode::Utf16 {
                encoding,
                little_endian,
                pending_byte,
                pending_high_surrogate,
            } => process_utf16_chunk(
                *encoding,
                *little_endian,
                pending_byte,
                pending_high_surrogate,
                bytes,
                self.max_decoded_bytes,
                &mut self.text,
                &mut self.decoded_sha256,
            ),
        }
    }
}

enum TallyTextPrefixDecision {
    NeedMore,
    Utf8Bom,
    Utf16LeBom,
    Utf16BeBom,
    Utf8WithoutBom,
}

fn tally_text_prefix_decision(prefix: &[u8]) -> TallyTextPrefixDecision {
    const UTF8_BOM: &[u8] = &[0xEF, 0xBB, 0xBF];
    const UTF16_LE_BOM: &[u8] = &[0xFF, 0xFE];
    const UTF16_BE_BOM: &[u8] = &[0xFE, 0xFF];
    if prefix == UTF8_BOM {
        TallyTextPrefixDecision::Utf8Bom
    } else if prefix == UTF16_LE_BOM {
        TallyTextPrefixDecision::Utf16LeBom
    } else if prefix == UTF16_BE_BOM {
        TallyTextPrefixDecision::Utf16BeBom
    } else if UTF8_BOM.starts_with(prefix)
        || UTF16_LE_BOM.starts_with(prefix)
        || UTF16_BE_BOM.starts_with(prefix)
    {
        TallyTextPrefixDecision::NeedMore
    } else {
        TallyTextPrefixDecision::Utf8WithoutBom
    }
}

fn process_utf8_chunk(
    pending: &mut Vec<u8>,
    mut bytes: &[u8],
    max_decoded_bytes: usize,
    text: &mut String,
    digest: &mut Sha256,
) -> Result<(), TallyTextDecodeError> {
    while !pending.is_empty() && !bytes.is_empty() {
        pending.push(bytes[0]);
        bytes = &bytes[1..];
        match std::str::from_utf8(pending) {
            Ok(decoded) => {
                append_decoded(decoded, max_decoded_bytes, text, digest)?;
                pending.clear();
            }
            Err(error) if error.error_len().is_some() => {
                return Err(TallyTextDecodeError::InvalidUtf8)
            }
            Err(_) => {}
        }
    }
    if bytes.is_empty() {
        return Ok(());
    }
    match std::str::from_utf8(bytes) {
        Ok(decoded) => append_decoded(decoded, max_decoded_bytes, text, digest),
        Err(error) if error.error_len().is_some() => Err(TallyTextDecodeError::InvalidUtf8),
        Err(error) => {
            let valid = &bytes[..error.valid_up_to()];
            let tail = &bytes[error.valid_up_to()..];
            let decoded =
                std::str::from_utf8(valid).map_err(|_| TallyTextDecodeError::InvalidUtf8)?;
            append_decoded(decoded, max_decoded_bytes, text, digest)?;
            if tail.len() > 3 {
                return Err(TallyTextDecodeError::InvalidUtf8);
            }
            pending.extend_from_slice(tail);
            Ok(())
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn process_utf16_chunk(
    encoding: TallyTextEncoding,
    little_endian: bool,
    pending_byte: &mut Option<u8>,
    pending_high_surrogate: &mut Option<u16>,
    bytes: &[u8],
    max_decoded_bytes: usize,
    text: &mut String,
    digest: &mut Sha256,
) -> Result<(), TallyTextDecodeError> {
    let mut offset = 0;
    if let Some(first) = pending_byte.take() {
        let Some(second) = bytes.first().copied() else {
            *pending_byte = Some(first);
            return Ok(());
        };
        process_utf16_unit(
            encoding,
            if little_endian {
                u16::from_le_bytes([first, second])
            } else {
                u16::from_be_bytes([first, second])
            },
            pending_high_surrogate,
            max_decoded_bytes,
            text,
            digest,
        )?;
        offset = 1;
    }
    while offset + 1 < bytes.len() {
        let pair = [bytes[offset], bytes[offset + 1]];
        let unit = if little_endian {
            u16::from_le_bytes(pair)
        } else {
            u16::from_be_bytes(pair)
        };
        process_utf16_unit(
            encoding,
            unit,
            pending_high_surrogate,
            max_decoded_bytes,
            text,
            digest,
        )?;
        offset += 2;
    }
    if offset < bytes.len() {
        *pending_byte = Some(bytes[offset]);
    }
    Ok(())
}

fn process_utf16_unit(
    encoding: TallyTextEncoding,
    unit: u16,
    pending_high_surrogate: &mut Option<u16>,
    max_decoded_bytes: usize,
    text: &mut String,
    digest: &mut Sha256,
) -> Result<(), TallyTextDecodeError> {
    if (0xD800..=0xDBFF).contains(&unit) {
        if pending_high_surrogate.replace(unit).is_some() {
            return Err(invalid_utf16(encoding));
        }
        return Ok(());
    }
    let scalar = if (0xDC00..=0xDFFF).contains(&unit) {
        let high = pending_high_surrogate
            .take()
            .ok_or_else(|| invalid_utf16(encoding))?;
        0x1_0000 + (((u32::from(high) - 0xD800) << 10) | (u32::from(unit) - 0xDC00))
    } else {
        if pending_high_surrogate.take().is_some() {
            return Err(invalid_utf16(encoding));
        }
        u32::from(unit)
    };
    let character = char::from_u32(scalar).ok_or_else(|| invalid_utf16(encoding))?;
    let mut encoded = [0_u8; 4];
    append_decoded(
        character.encode_utf8(&mut encoded),
        max_decoded_bytes,
        text,
        digest,
    )
}

fn append_decoded(
    decoded: &str,
    max_decoded_bytes: usize,
    text: &mut String,
    digest: &mut Sha256,
) -> Result<(), TallyTextDecodeError> {
    if text.len().saturating_add(decoded.len()) > max_decoded_bytes {
        return Err(TallyTextDecodeError::TooLarge);
    }
    text.push_str(decoded);
    digest.update(decoded.as_bytes());
    Ok(())
}

fn invalid_utf16(encoding: TallyTextEncoding) -> TallyTextDecodeError {
    match encoding {
        TallyTextEncoding::Utf16LeBom => TallyTextDecodeError::InvalidUtf16Le,
        TallyTextEncoding::Utf16BeBom => TallyTextDecodeError::InvalidUtf16Be,
        TallyTextEncoding::Utf8 | TallyTextEncoding::Utf8Bom => TallyTextDecodeError::InvalidUtf8,
    }
}

fn encode_sha256(bytes: impl AsRef<[u8]>) -> String {
    let mut encoded = String::with_capacity(64);
    for byte in bytes.as_ref() {
        write!(&mut encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    encoded
}

pub fn decode_tally_text_bytes_limited(
    bytes: impl AsRef<[u8]>,
    max_bytes: usize,
) -> Result<DecodedTallyText, TallyTextDecodeError> {
    let bytes = bytes.as_ref();
    if bytes.len() > max_bytes {
        return Err(TallyTextDecodeError::TooLarge);
    }
    if let Some(payload) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8(payload.to_vec())
            .map(|text| DecodedTallyText {
                text,
                encoding: TallyTextEncoding::Utf8Bom,
            })
            .map_err(|_| TallyTextDecodeError::InvalidUtf8);
    }
    if let Some(payload) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        if payload.len() % 2 != 0 {
            return Err(TallyTextDecodeError::InvalidUtf16Le);
        }
        let units = payload
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>();
        return String::from_utf16(&units)
            .map(|text| DecodedTallyText {
                text,
                encoding: TallyTextEncoding::Utf16LeBom,
            })
            .map_err(|_| TallyTextDecodeError::InvalidUtf16Le);
    }
    if let Some(payload) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        if payload.len() % 2 != 0 {
            return Err(TallyTextDecodeError::InvalidUtf16Be);
        }
        let units = payload
            .chunks_exact(2)
            .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>();
        return String::from_utf16(&units)
            .map(|text| DecodedTallyText {
                text,
                encoding: TallyTextEncoding::Utf16BeBom,
            })
            .map_err(|_| TallyTextDecodeError::InvalidUtf16Be);
    }
    String::from_utf8(bytes.to_vec())
        .map(|text| DecodedTallyText {
            text,
            encoding: TallyTextEncoding::Utf8,
        })
        .map_err(|_| TallyTextDecodeError::InvalidUtf8)
}

pub fn decode_xml_bytes(bytes: impl AsRef<[u8]>) -> anyhow::Result<String> {
    decode_xml_bytes_limited(bytes, usize::MAX)
}

pub fn decode_xml_bytes_limited(
    bytes: impl AsRef<[u8]>,
    max_bytes: usize,
) -> anyhow::Result<String> {
    match decode_tally_text_bytes_limited(bytes, max_bytes) {
        Ok(decoded) => Ok(decoded.text),
        Err(TallyTextDecodeError::TooLarge) => {
            anyhow::bail!("Tally response exceeded the {max_bytes}-byte limit")
        }
        Err(TallyTextDecodeError::InvalidUtf8) => {
            anyhow::bail!("Tally returned an invalid UTF-8 XML response")
        }
        Err(TallyTextDecodeError::InvalidUtf16Le) => {
            anyhow::bail!("Tally returned an invalid UTF-16LE XML response")
        }
        Err(TallyTextDecodeError::InvalidUtf16Be) => {
            anyhow::bail!("Tally returned an invalid UTF-16BE XML response")
        }
    }
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

/// Validates the fixed, documented `List of Ledgers` collection used only to
/// bootstrap a scoped company identity on responders that reject Bridge's
/// custom report profile. Ledger names, balances, and identities are inspected
/// in memory only and never returned by this parser.
pub fn parse_standard_ledger_identity_observation(
    xml: &str,
    expected_company_name: &str,
) -> anyhow::Result<StandardLedgerIdentityObservation> {
    validate_export_response(xml)?;
    let expected_company_name = normalized_standard_value(expected_company_name, "company name")?;
    let mut reader = configured_reader(xml);
    let mut path = Vec::<Vec<u8>>::new();
    let mut ledger_count = 0_usize;
    let mut company_guid = None::<String>;
    loop {
        match reader.read_event()? {
            Event::Start(element)
                if path_eq(&path, &[b"ENVELOPE", b"BODY", b"DATA", b"COLLECTION"]) =>
            {
                if ledger_count >= MAX_STANDARD_LEDGER_IDENTITY_ROWS {
                    anyhow::bail!(
                        "standard ledger identity collection exceeded the safe row limit"
                    );
                }
                let observed = parse_standard_ledger_identity_row(&mut reader, &element, false)?;
                if observed.company_name != expected_company_name {
                    anyhow::bail!(
                        "standard ledger identity collection did not confirm the requested company"
                    );
                }
                if let Some(previous) = &company_guid {
                    if previous != &observed.company_guid {
                        anyhow::bail!("standard ledger identity collection contained inconsistent company context");
                    }
                } else {
                    company_guid = Some(observed.company_guid);
                }
                ledger_count += 1;
            }
            Event::Start(element) => path.push(element.name().as_ref().to_ascii_uppercase()),
            Event::Empty(_element)
                if path_eq(&path, &[b"ENVELOPE", b"BODY", b"DATA", b"COLLECTION"]) =>
            {
                anyhow::bail!("standard ledger identity collection contained an empty row");
            }
            Event::End(element) => pop_expected_path(&mut path, element.name().as_ref())?,
            Event::Eof => break,
            _ => {}
        }
    }
    if !path.is_empty() {
        anyhow::bail!("standard ledger identity collection ended before its root closed");
    }
    Ok(StandardLedgerIdentityObservation {
        company_guid: company_guid.ok_or_else(|| {
            anyhow::anyhow!(
                "standard ledger identity collection did not return a usable ledger row"
            )
        })?,
        ledger_count: ledger_count as u64,
    })
}

/// Parses the documented `List of Ledgers` collection as a deliberately
/// limited interactive catalog. The source GUIDs prove row uniqueness and
/// company scope in memory only; callers receive no GUIDs or raw XML.
pub fn parse_standard_ledger_catalog(
    xml: &str,
    expected_company_name: &str,
    expected_company_guid: &str,
) -> anyhow::Result<Vec<TallyLedger>> {
    validate_export_response(xml)?;
    let expected_company_name = normalized_standard_value(expected_company_name, "company name")?;
    let expected_company_guid = normalized_standard_value(expected_company_guid, "company GUID")?;
    let mut reader = configured_reader(xml);
    let mut path = Vec::<Vec<u8>>::new();
    let mut rows = Vec::new();
    let mut seen_names = HashSet::new();
    let mut seen_guids = HashSet::new();
    loop {
        match reader.read_event()? {
            Event::Start(element)
                if path_eq(&path, &[b"ENVELOPE", b"BODY", b"DATA", b"COLLECTION"]) =>
            {
                if rows.len() >= MAX_STANDARD_LEDGER_IDENTITY_ROWS {
                    anyhow::bail!("standard ledger catalog exceeded the safe row limit");
                }
                let observed = parse_standard_ledger_identity_row(&mut reader, &element, true)?;
                if observed.company_name != expected_company_name
                    || !observed
                        .company_guid
                        .eq_ignore_ascii_case(&expected_company_guid)
                {
                    anyhow::bail!("standard ledger catalog did not confirm the selected company");
                }
                let ledger_name = observed.ledger_name.ok_or_else(|| {
                    anyhow::anyhow!("standard ledger catalog omitted ledger name")
                })?;
                let ledger_guid = observed.ledger_guid.ok_or_else(|| {
                    anyhow::anyhow!("standard ledger catalog omitted ledger GUID")
                })?;
                if !seen_names.insert(ledger_name.to_lowercase())
                    || !seen_guids.insert(ledger_guid.to_ascii_lowercase())
                {
                    anyhow::bail!("standard ledger catalog contained duplicate ledger identity");
                }
                rows.push(TallyLedger {
                    name: ledger_name,
                    parent: observed.parent,
                    party_gstin: None,
                    opening_balance: None,
                });
            }
            Event::Start(element) => path.push(element.name().as_ref().to_ascii_uppercase()),
            Event::Empty(_) if path_eq(&path, &[b"ENVELOPE", b"BODY", b"DATA", b"COLLECTION"]) => {
                anyhow::bail!("standard ledger catalog contained an empty row");
            }
            Event::End(element) => pop_expected_path(&mut path, element.name().as_ref())?,
            Event::Eof => break,
            _ => {}
        }
    }
    if path.is_empty() && !rows.is_empty() {
        Ok(rows)
    } else {
        anyhow::bail!("standard ledger catalog did not return usable rows")
    }
}

struct StandardLedgerIdentityRow {
    company_name: String,
    company_guid: String,
    ledger_name: Option<String>,
    ledger_guid: Option<String>,
    parent: Option<String>,
}

fn parse_standard_ledger_identity_row(
    reader: &mut Reader<&[u8]>,
    element: &quick_xml::events::BytesStart<'_>,
    include_ledger_name: bool,
) -> anyhow::Result<StandardLedgerIdentityRow> {
    validate_only_attributes(element, &[b"NAME", b"RESERVEDNAME"])?;
    let mut ledger_name = include_ledger_name
        .then(|| attr_value(reader, element, b"NAME"))
        .flatten()
        .map(|value| normalized_standard_ledger_name(&value))
        .transpose()?;
    let row_name = element.name().as_ref().to_ascii_uppercase();
    let mut company_name = None;
    let mut company_guid = None;
    let mut ledger_guid = None;
    let mut parent = None;
    let mut parent_seen = false;
    loop {
        match reader.read_event()? {
            Event::Start(child) => {
                let child_name = child.name().as_ref().to_ascii_uppercase();
                match child_name.as_slice() {
                    b"NAME" if include_ledger_name => {
                        validate_only_attributes(&child, &[b"TYPE"])?;
                        if ledger_name
                            .replace(normalized_standard_ledger_name(&read_required_text(
                                reader,
                                child.name(),
                            )?)?)
                            .is_some()
                        {
                            anyhow::bail!("standard ledger collection repeated ledger name");
                        }
                    }
                    b"NAME" => {
                        validate_only_attributes(&child, &[b"TYPE"])?;
                        skip_standard_ledger_identity_child(
                            reader,
                            child.name().as_ref().to_ascii_uppercase(),
                        )?;
                    }
                    b"BRIDGECOMPANYNAME" => {
                        validate_only_attributes(&child, &[b"TYPE"])?;
                        set_bootstrap_context_once(
                            &mut company_name,
                            normalized_standard_value(
                                &read_required_text(reader, child.name())?,
                                "company name",
                            )?,
                            "company name",
                        )?;
                    }
                    b"BRIDGECOMPANYGUID" => {
                        validate_only_attributes(&child, &[b"TYPE"])?;
                        set_bootstrap_context_once(
                            &mut company_guid,
                            normalized_standard_value(
                                &read_required_text(reader, child.name())?,
                                "company GUID",
                            )?,
                            "company GUID",
                        )?;
                    }
                    b"GUID" if include_ledger_name => {
                        validate_only_attributes(&child, &[b"TYPE"])?;
                        if ledger_guid
                            .replace(normalized_standard_value(
                                &read_required_text(reader, child.name())?,
                                "ledger GUID",
                            )?)
                            .is_some()
                        {
                            anyhow::bail!("standard ledger collection repeated ledger GUID");
                        }
                    }
                    b"GUID" => {
                        validate_only_attributes(&child, &[b"TYPE"])?;
                        skip_standard_ledger_identity_child(
                            reader,
                            child.name().as_ref().to_ascii_uppercase(),
                        )?;
                    }
                    b"PARENT" if include_ledger_name => {
                        validate_only_attributes(&child, &[b"TYPE"])?;
                        if parent_seen {
                            anyhow::bail!("standard ledger collection repeated ledger parent");
                        }
                        parent_seen = true;
                        parent = read_optional_text(reader, child.name())?
                            .and_then(|value| safe_standard_ledger_parent(&value));
                    }
                    b"PARENT" => {
                        validate_only_attributes(&child, &[b"TYPE"])?;
                        skip_standard_ledger_identity_child(
                            reader,
                            child.name().as_ref().to_ascii_uppercase(),
                        )?;
                    }
                    b"LANGUAGENAME.LIST" => skip_standard_ledger_identity_child(
                        reader,
                        child.name().as_ref().to_ascii_uppercase(),
                    )?,
                    _ => anyhow::bail!(
                        "standard ledger identity collection contained an unexpected row field"
                    ),
                }
            }
            Event::End(end) if end.name().as_ref().eq_ignore_ascii_case(&row_name) => break,
            Event::Empty(child)
                if include_ledger_name && child.name().as_ref().eq_ignore_ascii_case(b"PARENT") =>
            {
                validate_only_attributes(&child, &[b"TYPE"])?;
                if parent_seen {
                    anyhow::bail!("standard ledger collection repeated ledger parent");
                }
                parent_seen = true;
            }
            Event::Empty(_) => {
                anyhow::bail!("standard ledger identity collection contained an empty row field")
            }
            Event::Text(text) if !text.decode()?.trim().is_empty() => {
                anyhow::bail!("standard ledger identity collection contained unexpected row text")
            }
            Event::CData(_) | Event::DocType(_) | Event::PI(_) => {
                anyhow::bail!(
                    "standard ledger identity collection contained a forbidden XML construct"
                )
            }
            Event::Eof => {
                anyhow::bail!("standard ledger identity collection row ended before closing")
            }
            _ => {}
        }
    }
    Ok(StandardLedgerIdentityRow {
        company_name: company_name.ok_or_else(|| {
            anyhow::anyhow!("standard ledger identity collection omitted computed company name")
        })?,
        company_guid: company_guid.ok_or_else(|| {
            anyhow::anyhow!("standard ledger identity collection omitted computed company GUID")
        })?,
        ledger_name,
        ledger_guid,
        parent,
    })
}

fn skip_standard_ledger_identity_child(
    reader: &mut Reader<&[u8]>,
    expected_name: Vec<u8>,
) -> anyhow::Result<()> {
    let mut depth = 1_u32;
    loop {
        match reader.read_event()? {
            Event::Start(_) => {
                depth = depth.checked_add(1).ok_or_else(|| {
                    anyhow::anyhow!("standard ledger identity nesting exceeded limits")
                })?
            }
            Event::End(end) => {
                depth = depth.checked_sub(1).ok_or_else(|| {
                    anyhow::anyhow!(
                        "standard ledger identity collection closed an unexpected field"
                    )
                })?;
                if depth == 0 {
                    if !end.name().as_ref().eq_ignore_ascii_case(&expected_name) {
                        anyhow::bail!(
                            "standard ledger identity collection closed an unexpected field"
                        );
                    }
                    return Ok(());
                }
            }
            Event::DocType(_) | Event::PI(_) => {
                anyhow::bail!(
                    "standard ledger identity collection contained a forbidden XML construct"
                )
            }
            Event::Eof => {
                anyhow::bail!("standard ledger identity collection field ended before closing")
            }
            _ => {}
        }
    }
}

fn normalized_standard_value(value: &str, label: &str) -> anyhow::Result<String> {
    let value = value.trim();
    if value.is_empty() || value.len() > 255 || value.chars().any(char::is_control) {
        anyhow::bail!("standard ledger collection contained an invalid {label}");
    }
    Ok(value.to_string())
}

fn normalized_standard_ledger_name(value: &str) -> anyhow::Result<String> {
    let value = value.trim();
    if value.is_empty() || value.len() > 512 || value.chars().any(unsafe_display_character) {
        anyhow::bail!("standard ledger collection contained an invalid ledger name");
    }
    Ok(value.to_string())
}

fn safe_standard_ledger_parent(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || value.len() > 1024 || value.chars().any(unsafe_display_character) {
        return None;
    }
    Some(value.to_string())
}

fn unsafe_display_character(value: char) -> bool {
    value.is_control()
        || matches!(
            value,
            '\u{061C}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}' | '\u{2066}'..='\u{206F}' | '\u{FEFF}'
        )
}

fn set_bootstrap_context_once(
    slot: &mut Option<String>,
    value: String,
    label: &str,
) -> anyhow::Result<()> {
    if slot.replace(value).is_some() {
        anyhow::bail!("standard ledger identity collection repeated computed {label}");
    }
    Ok(())
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
                    raw_source_sha256: source_fragment_sha256(
                        xml,
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
                        parent: None,
                    },
                    source_id,
                    identity_kind,
                    identities,
                    alter_id: attr_value(&reader, &element, b"ALTERID"),
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
                        parent: None,
                        party_gstin: None,
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

pub fn parse_import_outcome(xml: &str) -> anyhow::Result<TallyImportOutcome> {
    let mut reader = configured_reader(xml);
    let mut path = Vec::<Vec<u8>>::new();
    let mut root = None::<Vec<u8>>;
    let mut root_closed = false;
    let mut saw_import_result = false;
    let mut saw_direct_data_result = false;
    let mut envelope_header_seen = false;
    let mut envelope_body_seen = false;
    let mut status = None;
    let mut created = None;
    let mut altered = None;
    let mut deleted = None;
    let mut ignored = None;
    let mut errors = None;
    let mut cancelled = None;
    let mut exceptions = None;
    let mut line_error_count = 0_u64;
    let mut documented_extra_fields = HashSet::new();

    loop {
        match reader.read_event()? {
            Event::Start(element) => {
                let name = element.name().as_ref().to_ascii_uppercase();
                validate_only_attributes(&element, &[]).map_err(|_| {
                    anyhow::anyhow!("Tally import response attributes were invalid")
                })?;
                if path.is_empty() {
                    if root.is_some()
                        || (name.as_slice() != b"RESPONSE" && name.as_slice() != b"ENVELOPE")
                    {
                        anyhow::bail!("Tally import response root must be RESPONSE or ENVELOPE");
                    }
                    root = Some(name.clone());
                }
                if root.as_deref() == Some(b"ENVELOPE") && path_eq(&path, &[b"ENVELOPE"]) {
                    if !envelope_header_seen && name.as_slice() != b"HEADER" {
                        anyhow::bail!("Tally import response expected HEADER before BODY");
                    }
                    if envelope_header_seen && !envelope_body_seen && name.as_slice() != b"BODY" {
                        anyhow::bail!("Tally import response expected BODY after HEADER");
                    }
                    if envelope_body_seen {
                        anyhow::bail!("Tally import response contained an extra ENVELOPE child");
                    }
                }
                if name.as_slice() == b"HEADER" {
                    if !path_eq(&path, &[b"ENVELOPE"]) || envelope_header_seen {
                        anyhow::bail!("Tally import response repeated or misplaced HEADER");
                    }
                    envelope_header_seen = true;
                }
                if name.as_slice() == b"BODY" {
                    if !path_eq(&path, &[b"ENVELOPE"]) || envelope_body_seen {
                        anyhow::bail!("Tally import response repeated or misplaced BODY");
                    }
                    envelope_body_seen = true;
                }
                path.push(name.clone());
                if path_eq(&path, &[b"ENVELOPE", b"BODY", b"DATA", b"IMPORTRESULT"]) {
                    if saw_import_result || saw_direct_data_result {
                        anyhow::bail!("Tally import ENVELOPE repeated IMPORTRESULT");
                    }
                    saw_import_result = true;
                }
                let is_response_field = path.len() == 2 && path[0].as_slice() == b"RESPONSE";
                let is_wrapped_envelope_field = path.len() == 5
                    && path_eq_prefix(&path, &[b"ENVELOPE", b"BODY", b"DATA", b"IMPORTRESULT"]);
                let is_direct_envelope_field = path.len() == 4
                    && path_eq_prefix(&path, &[b"ENVELOPE", b"BODY", b"DATA"])
                    && name.as_slice() != b"IMPORTRESULT";
                let is_counter_field =
                    is_response_field || is_wrapped_envelope_field || is_direct_envelope_field;
                if path_eq(&path, &[b"ENVELOPE", b"HEADER", b"STATUS"]) {
                    let value = read_required_text(&mut reader, element.name())?;
                    set_import_once(&mut status, value, "STATUS")?;
                    path.pop();
                } else if is_counter_field {
                    let consumed = match name.as_slice() {
                        b"CREATED" => {
                            let value = read_counter(&mut reader, element.name(), "CREATED")?;
                            set_import_once(&mut created, value, "CREATED")?;
                            true
                        }
                        b"ALTERED" => {
                            let value = read_counter(&mut reader, element.name(), "ALTERED")?;
                            set_import_once(&mut altered, value, "ALTERED")?;
                            true
                        }
                        b"DELETED" => {
                            let value = read_counter(&mut reader, element.name(), "DELETED")?;
                            set_import_once(&mut deleted, value, "DELETED")?;
                            true
                        }
                        b"IGNORED" => {
                            let value = read_counter(&mut reader, element.name(), "IGNORED")?;
                            set_import_once(&mut ignored, value, "IGNORED")?;
                            true
                        }
                        b"ERRORS" => {
                            let value = read_counter(&mut reader, element.name(), "ERRORS")?;
                            set_import_once(&mut errors, value, "ERRORS")?;
                            true
                        }
                        b"CANCELLED" => {
                            let value = read_counter(&mut reader, element.name(), "CANCELLED")?;
                            set_import_once(&mut cancelled, value, "CANCELLED")?;
                            true
                        }
                        b"EXCEPTIONS" => {
                            let value = read_counter(&mut reader, element.name(), "EXCEPTIONS")?;
                            set_import_once(&mut exceptions, value, "EXCEPTIONS")?;
                            true
                        }
                        b"LINEERROR" => {
                            if read_optional_text(&mut reader, element.name())?.is_some() {
                                line_error_count = line_error_count.saturating_add(1);
                            }
                            true
                        }
                        _ => false,
                    };
                    if consumed {
                        if is_direct_envelope_field {
                            if saw_import_result {
                                anyhow::bail!(
                                    "Tally import ENVELOPE mixed direct and wrapped result profiles"
                                );
                            }
                            saw_direct_data_result = true;
                        }
                        path.pop();
                    } else if is_counter_field
                        && matches!(
                            name.as_slice(),
                            b"LASTVCHID" | b"LASTMID" | b"COMBINED" | b"VCHNUMBER" | b"DESC"
                        )
                    {
                        if !documented_extra_fields.insert(name.clone()) {
                            anyhow::bail!("Tally import response duplicated a documented field");
                        }
                        read_optional_text(&mut reader, element.name())?;
                        if is_direct_envelope_field {
                            if saw_import_result {
                                anyhow::bail!(
                                    "Tally import ENVELOPE mixed direct and wrapped result profiles"
                                );
                            }
                            saw_direct_data_result = true;
                        }
                        path.pop();
                    } else {
                        anyhow::bail!("Tally import response contained an unexpected result field");
                    }
                } else if path.len() >= 2
                    && (path_eq_prefix(&path, &[b"RESPONSE"])
                        || path_eq_prefix(&path, &[b"ENVELOPE", b"HEADER"])
                        || path_eq_prefix(&path, &[b"ENVELOPE", b"BODY"]))
                    && !path_eq(&path, &[b"ENVELOPE", b"HEADER"])
                    && !path_eq(&path, &[b"ENVELOPE", b"BODY"])
                    && !path_eq(&path, &[b"ENVELOPE", b"BODY", b"DATA"])
                    && !path_eq(&path, &[b"ENVELOPE", b"BODY", b"DATA", b"IMPORTRESULT"])
                {
                    anyhow::bail!("Tally import response contained an unexpected element");
                }
            }
            Event::End(element) => {
                let Some(expected) = path.pop() else {
                    anyhow::bail!("Tally import response contained an unexpected closing element");
                };
                if !element.name().as_ref().eq_ignore_ascii_case(&expected) {
                    anyhow::bail!("Tally import response closed an unexpected element");
                }
                if path.is_empty() {
                    root_closed = true;
                }
            }
            Event::Empty(element)
                if path_eq(&path, &[b"ENVELOPE", b"BODY", b"DATA"])
                    && element
                        .name()
                        .as_ref()
                        .eq_ignore_ascii_case(b"IMPORTRESULT") =>
            {
                validate_only_attributes(&element, &[]).map_err(|_| {
                    anyhow::anyhow!("Tally import response attributes were invalid")
                })?;
                if saw_import_result || saw_direct_data_result {
                    anyhow::bail!("Tally import ENVELOPE repeated IMPORTRESULT");
                }
                saw_import_result = true;
            }
            Event::Empty(_) if path_eq(&path, &[b"ENVELOPE"]) => {
                anyhow::bail!(
                    "Tally import response contained an empty or unexpected ENVELOPE child"
                );
            }
            Event::Empty(element)
                if element.name().as_ref().eq_ignore_ascii_case(b"HEADER")
                    || element.name().as_ref().eq_ignore_ascii_case(b"BODY") =>
            {
                anyhow::bail!("Tally import response contained an empty critical container");
            }
            Event::Empty(_) => {
                anyhow::bail!("Tally import response contained an unexpected empty element");
            }
            Event::Text(text) if !text.decode()?.trim().is_empty() => {
                anyhow::bail!("Tally import response contained unexpected mixed text");
            }
            Event::CData(text) if !text.decode()?.trim().is_empty() => {
                anyhow::bail!("Tally import response contained unexpected mixed CDATA");
            }
            Event::DocType(_) | Event::PI(_) => {
                anyhow::bail!("Tally import response contained a forbidden XML construct");
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if !root_closed || !path.is_empty() {
        anyhow::bail!("Tally import response ended before its root element closed");
    }
    let application_status = if root.as_deref() == Some(b"ENVELOPE") {
        if !envelope_header_seen || !envelope_body_seen {
            anyhow::bail!("Tally import ENVELOPE omitted HEADER or BODY");
        }
        if !saw_import_result && !saw_direct_data_result {
            anyhow::bail!("Tally import ENVELOPE did not include a recognized result profile");
        }
        match status.as_deref() {
            Some("1") => TallyImportApplicationStatus::Success,
            Some("0") => TallyImportApplicationStatus::Failure,
            Some(_) => anyhow::bail!("Tally returned an invalid import application STATUS"),
            None => anyhow::bail!("Tally import ENVELOPE did not include HEADER/STATUS"),
        }
    } else {
        TallyImportApplicationStatus::NotReported
    };
    let exceptions_were_reported = exceptions.is_some();
    let counters = TallyImportResult {
        created: created.ok_or_else(|| anyhow::anyhow!("Tally import result omitted CREATED"))?,
        altered: altered.ok_or_else(|| anyhow::anyhow!("Tally import result omitted ALTERED"))?,
        deleted: deleted.unwrap_or(0),
        ignored: ignored.ok_or_else(|| anyhow::anyhow!("Tally import result omitted IGNORED"))?,
        errors: errors.ok_or_else(|| anyhow::anyhow!("Tally import result omitted ERRORS"))?,
        cancelled: cancelled.unwrap_or(0),
        exceptions: exceptions.unwrap_or(0),
        line_error_count,
    };
    Ok(TallyImportOutcome {
        application_status,
        counters,
        exceptions_were_reported,
    })
}

pub fn parse_import_result(xml: &str) -> anyhow::Result<TallyImportResult> {
    let outcome = parse_import_outcome(xml)?;
    if outcome.application_status() == TallyImportApplicationStatus::Failure {
        anyhow::bail!("Tally reported that the import request failed");
    }
    Ok(outcome.into_counters())
}

fn path_eq(path: &[Vec<u8>], expected: &[&[u8]]) -> bool {
    path.len() == expected.len()
        && path
            .iter()
            .zip(expected)
            .all(|(actual, expected)| actual.as_slice() == *expected)
}

fn path_eq_prefix(path: &[Vec<u8>], expected: &[&[u8]]) -> bool {
    path.len() >= expected.len()
        && path
            .iter()
            .zip(expected)
            .all(|(actual, expected)| actual.as_slice() == *expected)
}

fn set_import_once<T>(slot: &mut Option<T>, value: T, label: &str) -> anyhow::Result<()> {
    if slot.replace(value).is_some() {
        anyhow::bail!("Tally import response duplicated {label}");
    }
    Ok(())
}

/// Parses import counters and derives redacted evidence from the same exact
/// response. Digest domains prevent a response commitment from being confused
/// with a payload, intended-state, or readback-state commitment.
pub fn parse_import_evidence(xml: &str) -> anyhow::Result<ParsedImportEvidence> {
    parse_import_evidence_inner(xml)
        .map_err(|_| anyhow::anyhow!("Tally import response evidence was invalid"))
}

fn parse_import_evidence_inner(xml: &str) -> anyhow::Result<ParsedImportEvidence> {
    const MAX_IMPORT_RESPONSE_BYTES: usize = 1024 * 1024;
    const MAX_LINE_ERRORS: usize = 256;

    if xml.len() > MAX_IMPORT_RESPONSE_BYTES {
        anyhow::bail!("Tally import response exceeded the safe byte limit");
    }
    let outcome = parse_import_outcome(xml)?;
    let application_status = outcome.application_status();
    let exceptions_were_reported = outcome.exceptions_were_reported();
    let counters = outcome.into_counters();
    let mut reader = configured_reader(xml);
    let mut line_error_sha256 = Vec::new();
    loop {
        match reader.read_event()? {
            Event::Start(element) if element.name().as_ref().eq_ignore_ascii_case(b"LINEERROR") => {
                if let Some(value) = read_optional_text(&mut reader, element.name())? {
                    if line_error_sha256.len() == MAX_LINE_ERRORS {
                        anyhow::bail!("Tally import response exceeded the line-error limit");
                    }
                    line_error_sha256.push(domain_sha256(
                        b"bridge.tally.import-line-error/1\0",
                        value.as_bytes(),
                    ));
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if counters.line_error_count != line_error_sha256.len() as u64 {
        anyhow::bail!("Tally import line-error evidence was inconsistent");
    }
    Ok(ParsedImportEvidence {
        application_status,
        counters,
        exceptions_were_reported,
        response_sha256: domain_sha256(b"bridge.tally.import-response/1\0", xml.as_bytes()),
        line_error_sha256,
    })
}

fn domain_sha256(domain: &[u8], value: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(domain);
    digest.update(value);
    let mut encoded = String::with_capacity(64);
    for byte in digest.finalize() {
        write!(&mut encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    encoded
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
                if matches!(
                    name.as_slice(),
                    b"ENVELOPE" | b"HEADER" | b"BODY" | b"VERSION" | b"STATUS"
                ) {
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
                if name.as_slice() == b"HEADER" {
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
                if matches!(
                    name.as_slice(),
                    b"ENVELOPE" | b"HEADER" | b"BODY" | b"VERSION" | b"STATUS"
                ) {
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
                } else if matches!(name.as_slice(), b"HEADER" | b"VERSION" | b"STATUS") {
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
                break
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
                break
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

fn parse_company_info(reader: &mut Reader<&[u8]>) -> anyhow::Result<TallyCompany> {
    let mut company = TallyCompany {
        name: String::new(),
        guid: None,
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
                break
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
    let mut record = TallyNamedMaster { name, parent: None };
    let mut parent_seen = false;
    loop {
        match reader.read_event()? {
            Event::Start(element) if element.name().as_ref().eq_ignore_ascii_case(b"PARENT") => {
                validate_only_attributes(&element, &[])?;
                if std::mem::replace(&mut parent_seen, true) {
                    anyhow::bail!("Tally master row repeated PARENT");
                }
                record.parent = read_optional_text(reader, element.name())?;
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
        parent: None,
        party_gstin: None,
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
                ledger.parent = read_optional_text(reader, element.name())?
            }
            Event::Start(element)
                if element.name().as_ref().eq_ignore_ascii_case(b"PARTYGSTIN") =>
            {
                validate_only_attributes(&element, &[])?;
                if std::mem::replace(&mut gstin_seen, true) {
                    anyhow::bail!("Tally ledger row repeated PARTYGSTIN");
                }
                ledger.party_gstin = read_optional_text(reader, element.name())?
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
        parent: None,
        party_gstin: None,
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
                ledger.parent = read_optional_text(reader, element.name())?;
            }
            Event::Start(element)
                if element.name().as_ref().eq_ignore_ascii_case(b"PARTYGSTIN") =>
            {
                validate_only_attributes(&element, &[])?;
                if std::mem::replace(&mut gstin_seen, true) {
                    anyhow::bail!("Tally write readback repeated PARTYGSTIN");
                }
                ledger.party_gstin = read_optional_text(reader, element.name())?;
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
            }
            Event::Empty(element)
                if element.name().as_ref().eq_ignore_ascii_case(b"PARTYGSTIN") =>
            {
                validate_only_attributes(&element, &[])?;
                if std::mem::replace(&mut gstin_seen, true) {
                    anyhow::bail!("Tally write readback repeated PARTYGSTIN");
                }
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
                voucher.voucher_type = read_optional_text(reader, element.name())?
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
                voucher.party_ledger_name = read_optional_text(reader, element.name())?
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
                break
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
                ledger_name = read_optional_text(reader, element.name())?;
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

fn read_counter(reader: &mut Reader<&[u8]>, name: QName<'_>, label: &str) -> anyhow::Result<u64> {
    read_required_text(reader, name)?
        .parse::<u64>()
        .map_err(|_| anyhow::anyhow!("Tally import counter {label} was not a non-negative integer"))
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        write!(&mut encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    encoded
}
