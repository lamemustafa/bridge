//! Tally import responses: the application status and create/alter/delete/error
//! counters of an import `RESPONSE`, and the hashed evidence retained for them.

use std::{collections::HashSet, fmt::Write as _};

use quick_xml::{events::Event, name::QName, Reader};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    configured_reader, path_eq, read_optional_text, read_required_text, validate_only_attributes,
};

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
    /// Whether every result counter was present in the source response.  Missing
    /// fields in older saved response records deserialize as not observed, so
    /// they cannot retrospectively prove a clean import.
    #[serde(default)]
    pub counter_presence: TallyImportCounterPresence,
}

/// Source presence for Tally import counters.
///
/// Counts alone cannot distinguish a reported zero from a parser default. This
/// is persisted beside the counts so restarted processes retain that boundary.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
pub struct TallyImportCounterPresence {
    pub created: bool,
    pub altered: bool,
    pub deleted: bool,
    pub ignored: bool,
    pub errors: bool,
    pub cancelled: bool,
    pub exceptions: bool,
}

impl TallyImportCounterPresence {
    pub fn all_reported(&self) -> bool {
        self.created
            && self.altered
            && self.deleted
            && self.ignored
            && self.errors
            && self.cancelled
            && self.exceptions
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TallyImportApplicationStatus {
    Success,
    Failure,
    NotReported,
}

/// At most this many `LINEERROR`s keep their text; `line_error_count` keeps
/// the full count.
pub const MAX_TALLY_LINE_ERRORS: usize = 64;
/// At most this many characters of one `LINEERROR`'s text are kept.
pub const MAX_TALLY_LINE_ERROR_CHARS: usize = 512;
/// At most this many bytes of kept text, as JSON escapes it, in one
/// outcome. This only keeps the text small: what stops it ever changing a
/// refusal is that the agent drops it first when a result is over its cap.
pub const MAX_TALLY_LINE_ERROR_BYTES: usize = 4_096;

/// Tally's own text from one `LINEERROR`, for a person to read. It is trimmed
/// of surrounding whitespace, every character that could hide, reorder or
/// break what a person reads (Unicode Cc, Cf, Zl, Zp, Co, Cn and
/// Default_Ignorable_Code_Point) is replaced by U+FFFD, and it is
/// clipped to `MAX_TALLY_LINE_ERROR_CHARS` on a character boundary, with
/// `truncated` marking a clip. The text is untrusted, names no voucher and is
/// unreliable as a cause (IMPLEMENTATION_GUIDE, import success), so Bridge
/// never decides on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TallyLineError {
    text: String,
    truncated: bool,
}

impl TallyLineError {
    fn bounded(text: &str, already_truncated: bool) -> Self {
        use icu_properties::{
            props::{DefaultIgnorableCodePoint, GeneralCategory},
            CodePointMapData, CodePointSetData,
        };
        let ignorable = CodePointSetData::new::<DefaultIgnorableCodePoint>();
        let category = CodePointMapData::<GeneralCategory>::new();
        let mut characters = text.chars().map(|character| {
            if character.is_control()
                || ignorable.contains(character)
                || matches!(
                    category.get(character),
                    GeneralCategory::Format
                        | GeneralCategory::LineSeparator
                        | GeneralCategory::ParagraphSeparator
                        | GeneralCategory::PrivateUse
                        | GeneralCategory::Unassigned
                )
            {
                char::REPLACEMENT_CHARACTER
            } else {
                character
            }
        });
        let text = characters
            .by_ref()
            .take(MAX_TALLY_LINE_ERROR_CHARS)
            .collect();
        let truncated = already_truncated || characters.next().is_some();
        Self { text, truncated }
    }

    /// The bytes this text takes once JSON escapes it. Controls are already
    /// replaced, so only a quote or a backslash grows.
    fn escaped_len(&self) -> usize {
        self.text.len()
            + self
                .text
                .chars()
                .filter(|character| matches!(character, '"' | '\\'))
                .count()
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn truncated(&self) -> bool {
        self.truncated
    }
}

/// The texts kept, in document order, and how many `LINEERROR`s kept none:
/// those past `line_error_count`, `MAX_TALLY_LINE_ERRORS` or
/// `MAX_TALLY_LINE_ERROR_BYTES`.
fn bounded_line_errors(
    line_errors: impl IntoIterator<Item = TallyLineError>,
    line_error_count: u64,
) -> (Vec<TallyLineError>, u64) {
    let limit = usize::try_from(line_error_count)
        .unwrap_or(usize::MAX)
        .min(MAX_TALLY_LINE_ERRORS);
    let mut kept = Vec::new();
    let mut bytes = 0_usize;
    for line_error in line_errors.into_iter().take(limit) {
        bytes = bytes.saturating_add(line_error.escaped_len());
        if bytes > MAX_TALLY_LINE_ERROR_BYTES {
            break;
        }
        kept.push(line_error);
    }
    let omitted = line_error_count.saturating_sub(kept.len() as u64);
    (kept, omitted)
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(from = "StoredTallyImportOutcome")]
pub struct TallyImportOutcome {
    application_status: TallyImportApplicationStatus,
    counters: TallyImportResult,
    exceptions_were_reported: bool,
    /// The `LINEERROR` texts kept, in document order. Skipped when empty, so
    /// a response without a `LINEERROR` records exactly as before.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tally_line_errors: Vec<TallyLineError>,
    /// How many `LINEERROR`s kept no text, so a short list is explicit.
    #[serde(skip_serializing_if = "is_zero")]
    tally_line_errors_omitted: u64,
}

/// A saved outcome as read back. Its text is bounded again on the way in, so
/// a record can never show more than a fresh parse would keep, and display
/// text never fails a record: a malformed list reads as none kept.
#[derive(Deserialize)]
struct StoredTallyImportOutcome {
    application_status: TallyImportApplicationStatus,
    counters: TallyImportResult,
    exceptions_were_reported: bool,
    #[serde(default, deserialize_with = "stored_line_errors")]
    tally_line_errors: Vec<StoredTallyLineError>,
}

#[derive(Deserialize)]
struct StoredTallyLineError {
    text: String,
    #[serde(default)]
    truncated: bool,
}

fn stored_line_errors<'de, D>(deserializer: D) -> Result<Vec<StoredTallyLineError>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Stored {
        Readable(Vec<StoredTallyLineError>),
        Unreadable(serde::de::IgnoredAny),
    }
    Ok(match Stored::deserialize(deserializer)? {
        Stored::Readable(line_errors) => line_errors,
        Stored::Unreadable(_) => Vec::new(),
    })
}

impl From<StoredTallyImportOutcome> for TallyImportOutcome {
    fn from(stored: StoredTallyImportOutcome) -> Self {
        let (tally_line_errors, tally_line_errors_omitted) = bounded_line_errors(
            stored
                .tally_line_errors
                .into_iter()
                .map(|line_error| TallyLineError::bounded(&line_error.text, line_error.truncated)),
            stored.counters.line_error_count,
        );
        Self {
            application_status: stored.application_status,
            counters: stored.counters,
            exceptions_were_reported: stored.exceptions_were_reported,
            tally_line_errors,
            tally_line_errors_omitted,
        }
    }
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

    /// Tally's `LINEERROR` text, for a person to read. Never a verdict input.
    pub fn tally_line_errors(&self) -> &[TallyLineError] {
        &self.tally_line_errors
    }

    /// How many `LINEERROR`s kept no text.
    pub fn tally_line_errors_omitted(&self) -> u64 {
        self.tally_line_errors_omitted
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
    /// Accepts only an exact, non-zero intended import mutation with no
    /// negative result counters or reported line errors.
    pub fn is_clean_success_for(
        &self,
        expected_created: u64,
        expected_altered: u64,
        expected_deleted: u64,
    ) -> bool {
        (expected_created > 0 || expected_altered > 0 || expected_deleted > 0)
            && self.counter_presence.all_reported()
            && self.created == expected_created
            && self.altered == expected_altered
            && self.deleted == expected_deleted
            && self.ignored == 0
            && self.errors == 0
            && self.cancelled == 0
            && self.exceptions == 0
            && self.line_error_count == 0
    }
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
    let mut tally_line_errors = Vec::new();
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
                            let text = read_optional_text(&mut reader, element.name())?;
                            line_error_count = line_error_count.saturating_add(1);
                            if tally_line_errors.len() < MAX_TALLY_LINE_ERRORS {
                                tally_line_errors.push(TallyLineError::bounded(
                                    text.as_deref().unwrap_or(""),
                                    false,
                                ));
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
                        if name.as_slice() == b"LASTVCHID" {
                            // Tally documents LASTVCHID as a numeric import-result field. We do
                            // not retain it yet, but accepting arbitrary text here would make the
                            // parser evidence unusable for a future identifier cross-check.
                            read_counter(&mut reader, element.name(), "LASTVCHID")?;
                        } else {
                            read_optional_text(&mut reader, element.name())?;
                        }
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
        counter_presence: TallyImportCounterPresence {
            created: created.is_some(),
            altered: altered.is_some(),
            deleted: deleted.is_some(),
            ignored: ignored.is_some(),
            errors: errors.is_some(),
            cancelled: cancelled.is_some(),
            exceptions: exceptions.is_some(),
        },
    };
    let (tally_line_errors, tally_line_errors_omitted) =
        bounded_line_errors(tally_line_errors, counters.line_error_count);
    Ok(TallyImportOutcome {
        application_status,
        counters,
        exceptions_were_reported,
        tally_line_errors,
        tally_line_errors_omitted,
    })
}

pub fn parse_import_result(xml: &str) -> anyhow::Result<TallyImportResult> {
    let outcome = parse_import_outcome(xml)?;
    if outcome.application_status() == TallyImportApplicationStatus::Failure {
        anyhow::bail!("Tally reported that the import request failed");
    }
    Ok(outcome.into_counters())
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
                let value = read_optional_text(&mut reader, element.name())?.unwrap_or_default();
                if line_error_sha256.len() == MAX_LINE_ERRORS {
                    anyhow::bail!("Tally import response exceeded the line-error limit");
                }
                line_error_sha256.push(domain_sha256(
                    b"bridge.tally.import-line-error/1\0",
                    value.as_bytes(),
                ));
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

fn read_counter(reader: &mut Reader<&[u8]>, name: QName<'_>, label: &str) -> anyhow::Result<u64> {
    read_required_text(reader, name)?
        .parse::<u64>()
        .map_err(|_| anyhow::anyhow!("Tally import counter {label} was not a non-negative integer"))
}
