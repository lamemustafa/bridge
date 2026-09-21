//! Admission of the tally-read v1 `company` part: the response to
//! `ReadOnlyProfile::AuditCompanyObjectV1`.
//!
//! The request names the company by `ID TYPE="Name"`, and Tally resolves that
//! name however it likes (protocol reference §9.11), so the response is bound
//! to the verified identity by what it carries, never by what was asked for.
//! Both consumers read the first `COMPANY` element that has a `GUID`, anywhere
//! in the document. A captured object export also carries a `CMPINFO/COMPANY`
//! object counter (value `0`) with no GUID. Admission reads only
//! `DATA/TALLYMESSAGE/COMPANY`, requires exactly one, and refuses any other
//! `COMPANY` that carries a GUID, so the element admitted is the element both
//! consumers will choose.

use quick_xml::events::Event;

use crate::tolerant_xml::sanitize_invalid_numeric_references;

/// What an admitted company part says. The GUID is ASCII-lowercased to match
/// the verified identity's canonical form; every other value is as received.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditCompanyPart {
    pub name: String,
    pub guid: String,
    pub books_from_yyyymmdd: String,
    /// `None` when the company carries no `ISINTEGRATED`, or an empty one, as
    /// the Python loader reads both. The stock test then
    /// reports integration as "unknown" (Lane B, 2026-09-21), which is visible,
    /// so absence is admitted; a repeated tag is not.
    pub is_integrated: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditCompanyPartError {
    Malformed,
    StatusNotSuccess,
    NotExactlyOneCompany,
    FieldMissingOrRepeated,
    GuidMismatch,
    BooksFromMismatch,
}

impl AuditCompanyPartError {
    pub fn code(self) -> &'static str {
        match self {
            Self::Malformed => "audit_company_part_malformed",
            Self::StatusNotSuccess => "audit_company_part_status_not_success",
            Self::NotExactlyOneCompany => "audit_company_part_not_exactly_one_company",
            Self::FieldMissingOrRepeated => "audit_company_part_field_missing_or_repeated",
            Self::GuidMismatch => "audit_company_part_guid_mismatch",
            Self::BooksFromMismatch => "audit_company_part_books_from_mismatch",
        }
    }
}

impl std::fmt::Display for AuditCompanyPartError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::error::Error for AuditCompanyPartError {}

const COMPANY_PATH: [&[u8]; 5] = [b"ENVELOPE", b"BODY", b"DATA", b"TALLYMESSAGE", b"COMPANY"];
const TALLYMESSAGE_PATH: [&[u8]; 4] = [b"ENVELOPE", b"BODY", b"DATA", b"TALLYMESSAGE"];
const STATUS_PATH: [&[u8]; 3] = [b"ENVELOPE", b"HEADER", b"STATUS"];
/// The consumer fields of the company part, directly under the company: the
/// first two must occur exactly once, the last at most once. `NAME` is the
/// element's attribute, not one of these.
const FIELDS: [&[u8]; 3] = [b"GUID", b"BOOKSFROM", b"ISINTEGRATED"];

/// Admits `body` as the company part of a read of the company whose verified
/// GUID and books-from date are given. A split sibling shares its parent's
/// GUID (protocol reference §9.11b), so the date is part of the match.
pub fn admit_audit_company_part(
    body: &str,
    expected_guid: &str,
    expected_books_from_yyyymmdd: &str,
) -> Result<AuditCompanyPart, AuditCompanyPartError> {
    let text = sanitize_invalid_numeric_references(body);
    let mut reader = quick_xml::Reader::from_str(&text);
    let mut path = Vec::<Vec<u8>>::new();
    let mut status = Vec::<String>::new();
    let mut messages = 0usize;
    let mut message_children = 0usize;
    let mut companies = 0usize;
    let mut name = Vec::<String>::new();
    let mut fields: [Vec<String>; 3] = Default::default();
    let mut current_text: Option<(usize, String)> = None;
    let mut status_text: Option<String> = None;
    let mut roots = 0usize;
    let at = |path: &[Vec<u8>], expected: &[&[u8]]| {
        path.len() == expected.len()
            && path
                .iter()
                .zip(expected)
                .all(|(have, want)| have.as_slice() == *want)
    };
    loop {
        let event = reader
            .read_event()
            .map_err(|_| AuditCompanyPartError::Malformed)?;
        match event {
            Event::Start(ref start) | Event::Empty(ref start) => {
                let is_empty = matches!(event, Event::Empty(_));
                let element = start.name().as_ref().to_vec();
                if path.is_empty() {
                    roots += 1;
                    if roots > 1 {
                        return Err(AuditCompanyPartError::Malformed);
                    }
                }
                // The engines read a field's text up to its first child, so a
                // field with a child would be read differently here and there.
                if current_text.is_some() || status_text.is_some() {
                    return Err(AuditCompanyPartError::Malformed);
                }
                // The engines take the first `COMPANY` with a GUID anywhere in
                // the document, and the Rust engine its `BOOKSFROM` from the
                // first `COMPANY` that has one; any other company carrying
                // either would be read in place of this one.
                if matches!(element.as_slice(), b"GUID" | b"BOOKSFROM")
                    && path.last().map(Vec::as_slice) == Some(b"COMPANY".as_slice())
                    && !at(&path, &COMPANY_PATH)
                {
                    return Err(AuditCompanyPartError::NotExactlyOneCompany);
                }
                if at(&path, &TALLYMESSAGE_PATH[..3]) && element == b"TALLYMESSAGE" {
                    messages += 1;
                }
                if at(&path, &TALLYMESSAGE_PATH) {
                    message_children += 1;
                }
                path.push(element);
                if at(&path, &COMPANY_PATH) {
                    companies += 1;
                    for attribute in start.attributes() {
                        let attribute = attribute.map_err(|_| AuditCompanyPartError::Malformed)?;
                        if attribute.key.as_ref() == b"NAME" {
                            name.push(
                                attribute
                                    .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                                    .map_err(|_| AuditCompanyPartError::Malformed)?
                                    .into_owned(),
                            );
                        }
                    }
                }
                if path.len() == COMPANY_PATH.len() + 1 && at(&path[..5], &COMPANY_PATH) {
                    if let Some(index) = FIELDS.iter().position(|field| path[5] == *field) {
                        if is_empty {
                            fields[index].push(String::new());
                        } else {
                            current_text = Some((index, String::new()));
                        }
                    }
                }
                if at(&path, &STATUS_PATH) && !is_empty {
                    status_text = Some(String::new());
                }
                if is_empty {
                    path.pop();
                }
            }
            Event::Text(text) => {
                let decoded = text
                    .decode()
                    .map_err(|_| AuditCompanyPartError::Malformed)?;
                if path.is_empty()
                    && !decoded
                        .trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}')
                        .is_empty()
                {
                    return Err(AuditCompanyPartError::Malformed);
                }
                if let Some((_, value)) = current_text.as_mut() {
                    value.push_str(&decoded);
                }
                if let Some(value) = status_text.as_mut() {
                    value.push_str(&decoded);
                }
            }
            // Only a reference inside a field admission reads is resolved; the
            // rest of the company definition (addresses carry `&#13;&#10;`) is
            // stored as received and never interpreted here.
            Event::GeneralRef(reference) if current_text.is_some() || status_text.is_some() => {
                let resolved = match reference
                    .resolve_char_ref()
                    .map_err(|_| AuditCompanyPartError::Malformed)?
                {
                    Some(character) => character.to_string(),
                    None => {
                        let entity = reference
                            .decode()
                            .map_err(|_| AuditCompanyPartError::Malformed)?;
                        quick_xml::escape::resolve_predefined_entity(&entity)
                            .ok_or(AuditCompanyPartError::Malformed)?
                            .to_string()
                    }
                };
                if let Some((_, value)) = current_text.as_mut() {
                    value.push_str(&resolved);
                }
                if let Some(value) = status_text.as_mut() {
                    value.push_str(&resolved);
                }
            }
            Event::End(end) => {
                if path.last().map(Vec::as_slice) != Some(end.name().as_ref()) {
                    return Err(AuditCompanyPartError::Malformed);
                }
                if path.len() == COMPANY_PATH.len() + 1 {
                    if let Some((index, value)) = current_text.take() {
                        fields[index].push(value);
                    }
                }
                if at(&path, &STATUS_PATH) {
                    if let Some(value) = status_text.take() {
                        status.push(value);
                    }
                }
                path.pop();
            }
            Event::DocType(_) => return Err(AuditCompanyPartError::Malformed),
            Event::Eof => break,
            _ => {}
        }
    }
    if !path.is_empty() {
        return Err(AuditCompanyPartError::Malformed);
    }
    if status.len() != 1 || status[0].trim() != "1" {
        return Err(AuditCompanyPartError::StatusNotSuccess);
    }
    if messages != 1 || message_children != 1 || companies != 1 {
        return Err(AuditCompanyPartError::NotExactlyOneCompany);
    }
    let [guid, books_from, is_integrated] = fields;
    if name.len() != 1 || guid.len() != 1 || books_from.len() != 1 || is_integrated.len() > 1 {
        return Err(AuditCompanyPartError::FieldMissingOrRepeated);
    }
    let [guid, books_from] =
        [guid, books_from].map(|mut values| values.pop().expect("exactly one value checked above"));
    let guid = guid.trim().to_ascii_lowercase();
    if guid.is_empty() || guid != expected_guid.trim().to_ascii_lowercase() {
        return Err(AuditCompanyPartError::GuidMismatch);
    }
    if books_from.trim() != expected_books_from_yyyymmdd {
        return Err(AuditCompanyPartError::BooksFromMismatch);
    }
    Ok(AuditCompanyPart {
        name: name.pop().expect("exactly one name checked above"),
        guid,
        books_from_yyyymmdd: books_from.trim().to_string(),
        is_integrated: is_integrated
            .into_iter()
            .next()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty()),
    })
}

#[cfg(test)]
#[path = "audit_company_part_tests.rs"]
mod tests;
