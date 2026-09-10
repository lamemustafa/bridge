//! Strict, bounded parsing for a locally chosen historical voucher-import document.
//!
//! This module preserves source text for review. It does not construct a Tally
//! request, derive accounting sides, or make a source document executable.

use std::collections::{BTreeMap, BTreeSet};

use quick_xml::{
    events::{BytesRef, BytesText, Event},
    Reader, XmlVersion,
};
use sha2::{Digest, Sha256};

pub(crate) const MAX_SOURCE_BYTES: usize = 2 * 1024 * 1024;
const MAX_VOUCHERS: usize = 2_000;
const MAX_ENTRIES: usize = 20;
const MAX_TEXT_BYTES: usize = 4_096;
const MAX_DEPTH: usize = 32;
const MAX_TAG_BYTES: usize = 128;
const MAX_SOURCE_NOTICE_KINDS: usize = 64;
const MAX_OMITTED_FIELDS_PER_VOUCHER: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedSource {
    pub(crate) filename: String,
    pub(crate) sha256: String,
    pub(crate) utf8: String,
    pub(crate) vouchers: Vec<SourceVoucher>,
    pub(crate) source_notices: Vec<SourceNotice>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourceNotice {
    pub(crate) kind: String,
    pub(crate) count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourceVoucher {
    pub(crate) position: usize,
    pub(crate) remote_id: String,
    pub(crate) date: String,
    pub(crate) voucher_type: String,
    pub(crate) narration: Option<String>,
    pub(crate) entries: Vec<SourceEntry>,
    pub(crate) omitted_fields: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourceEntry {
    pub(crate) position: usize,
    pub(crate) ledger: String,
    pub(crate) amount: String,
    pub(crate) polarity: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SourceXmlError {
    TooLarge,
    InvalidUtf8,
    DoctypeForbidden,
    InvalidEntity,
    UnsupportedShape,
    InvalidText,
    VoucherLimit,
    EntryLimit,
    SourceNoticeLimit,
    OmittedFieldLimit,
    RequiredFieldMissing,
}

impl SourceXmlError {
    pub(crate) const fn code(self) -> &'static str {
        match self {
            Self::TooLarge => "source_draft_source_too_large",
            Self::InvalidUtf8 => "source_draft_source_not_utf8",
            Self::DoctypeForbidden => "source_draft_doctype_forbidden",
            Self::InvalidEntity => "source_draft_entity_invalid",
            Self::UnsupportedShape => "source_draft_xml_shape_unsupported",
            Self::InvalidText => "source_draft_source_text_invalid",
            Self::VoucherLimit => "source_draft_voucher_limit_exceeded",
            Self::EntryLimit => "source_draft_entry_limit_exceeded",
            Self::SourceNoticeLimit => "source_draft_notice_limit_exceeded",
            Self::OmittedFieldLimit => "source_draft_omitted_field_limit_exceeded",
            Self::RequiredFieldMissing => "source_draft_required_field_missing",
        }
    }
}

pub(crate) fn parse_source_xml(
    bytes: &[u8],
    filename: String,
) -> Result<ParsedSource, SourceXmlError> {
    if bytes.len() > MAX_SOURCE_BYTES {
        return Err(SourceXmlError::TooLarge);
    }
    let xml = std::str::from_utf8(bytes).map_err(|_| SourceXmlError::InvalidUtf8)?;
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut stack = Vec::<String>::new();
    let mut vouchers = Vec::new();
    let mut current: Option<WorkingVoucher> = None;
    let mut entry: Option<WorkingEntry> = None;
    let mut notices = BTreeMap::new();
    let mut saw_root = false;
    loop {
        match reader.read_event() {
            Ok(Event::Decl(_)) if stack.is_empty() && !saw_root => {}
            Ok(Event::Decl(_)) => return Err(SourceXmlError::UnsupportedShape),
            Ok(Event::Start(event)) => {
                start(
                    &mut stack,
                    &event,
                    &reader,
                    &mut current,
                    &mut entry,
                    &mut saw_root,
                    &mut notices,
                )?;
            }
            Ok(Event::Empty(event)) => {
                let tag = tag_name(event.name().as_ref())?;
                start(
                    &mut stack,
                    &event,
                    &reader,
                    &mut current,
                    &mut entry,
                    &mut saw_root,
                    &mut notices,
                )?;
                finish(&mut stack, &tag, &mut vouchers, &mut current, &mut entry)?;
            }
            Ok(Event::Text(text)) => {
                append_text(&stack, &mut current, &mut entry, decode_text(text)?)?
            }
            Ok(Event::CData(text)) => append_text(
                &stack,
                &mut current,
                &mut entry,
                text.decode()
                    .map_err(|_| SourceXmlError::InvalidUtf8)?
                    .into_owned(),
            )?,
            Ok(Event::GeneralRef(reference)) => append_text(
                &stack,
                &mut current,
                &mut entry,
                decode_reference(reference)?,
            )?,
            Ok(Event::End(event)) => {
                let tag = tag_name(event.name().as_ref())?;
                finish(&mut stack, &tag, &mut vouchers, &mut current, &mut entry)?;
            }
            Ok(Event::DocType(_)) => return Err(SourceXmlError::DoctypeForbidden),
            Ok(Event::Comment(_)) | Ok(Event::PI(_)) => {
                return Err(SourceXmlError::UnsupportedShape)
            }
            Ok(Event::Eof) => break,
            Err(_) => return Err(SourceXmlError::UnsupportedShape),
        }
    }
    if !saw_root || !stack.is_empty() || current.is_some() || entry.is_some() || vouchers.is_empty()
    {
        return Err(SourceXmlError::UnsupportedShape);
    }
    if vouchers
        .iter()
        .map(|voucher| voucher.remote_id.as_str())
        .collect::<BTreeSet<_>>()
        .len()
        != vouchers.len()
    {
        return Err(SourceXmlError::UnsupportedShape);
    }
    let sha256 = Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let source_notices = notices
        .into_iter()
        .map(|(kind, count)| SourceNotice { kind, count })
        .collect();
    Ok(ParsedSource {
        filename,
        sha256,
        utf8: xml.to_owned(),
        vouchers,
        source_notices,
    })
}

fn start(
    stack: &mut Vec<String>,
    event: &quick_xml::events::BytesStart<'_>,
    reader: &Reader<&[u8]>,
    current: &mut Option<WorkingVoucher>,
    entry: &mut Option<WorkingEntry>,
    saw_root: &mut bool,
    notices: &mut BTreeMap<String, usize>,
) -> Result<(), SourceXmlError> {
    let tag = tag_name(event.name().as_ref())?;
    validate_attribute_bounds(event, reader)?;
    if stack.len() >= MAX_DEPTH {
        return Err(SourceXmlError::UnsupportedShape);
    }
    let parent = stack.last().map(String::as_str);
    let direct_message = stack.as_slice()
        == [
            "ENVELOPE",
            "BODY",
            "IMPORTDATA",
            "REQUESTDATA",
            "TALLYMESSAGE",
        ];
    if tag == "VOUCHER" && !direct_message {
        return Err(SourceXmlError::UnsupportedShape);
    }
    let allowed = matches!(
        (parent, tag.as_str()),
        (None, "ENVELOPE")
            | (Some("ENVELOPE"), "HEADER" | "BODY")
            | (Some("BODY"), "IMPORTDATA")
            | (Some("IMPORTDATA"), "REQUESTDESC" | "REQUESTDATA")
            | (Some("REQUESTDESC"), _)
            | (Some("STATICVARIABLES"), _)
            | (Some("REQUESTDATA"), "TALLYMESSAGE")
            | (Some("TALLYMESSAGE"), _)
            | (Some("VOUCHER"), "ALLLEDGERENTRIES.LIST")
            | (Some("VOUCHER"), _)
            | (Some("ALLLEDGERENTRIES.LIST"), _)
    );
    let ignored_metadata = matches!(
        parent,
        Some("HEADER") | Some("REQUESTDESC") | Some("STATICVARIABLES")
    ) || (current.is_none()
        && stack.iter().any(|element| element == "TALLYMESSAGE"));
    if (!allowed && !ignored_metadata) || (parent.is_none() && *saw_root) {
        return Err(SourceXmlError::UnsupportedShape);
    }
    if parent == Some("TALLYMESSAGE") && tag != "VOUCHER" {
        record_notice(notices, format!("Ignored TALLYMESSAGE/{tag}"))?;
    } else if parent == Some("ENVELOPE") && tag == "HEADER" {
        record_notice(notices, "Ignored ENVELOPE/HEADER metadata".into())?;
    } else if parent == Some("IMPORTDATA") && tag == "REQUESTDESC" {
        record_notice(notices, "Ignored IMPORTDATA/REQUESTDESC metadata".into())?;
    }
    if tag == "VOUCHER" && direct_message {
        if current.is_some() {
            return Err(SourceXmlError::UnsupportedShape);
        }
        let mut row = WorkingVoucher::default();
        for attr in event.attributes().with_checks(true) {
            let attr = attr.map_err(|_| SourceXmlError::UnsupportedShape)?;
            let key = tag_name(attr.key.as_ref())?;
            let value = attr
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
                .map_err(|_| SourceXmlError::InvalidUtf8)?;
            let value = bounded(value.into_owned())?;
            match key.as_str() {
                "REMOTEID" => claim(&mut row.remote_id, value)?,
                "VCHTYPE" => claim(&mut row.voucher_type, value)?,
                _ => {
                    record_omitted_field(&mut row.omitted, format!("VOUCHER/@{key}"))?;
                }
            }
        }
        *current = Some(row);
    } else if tag == "ALLLEDGERENTRIES.LIST" && current.is_some() && parent == Some("VOUCHER") {
        if entry.is_some() || current.is_none() {
            return Err(SourceXmlError::UnsupportedShape);
        }
        let voucher = current.as_mut().ok_or(SourceXmlError::UnsupportedShape)?;
        for attr in event.attributes().with_checks(true) {
            let attr = attr.map_err(|_| SourceXmlError::UnsupportedShape)?;
            let key = tag_name(attr.key.as_ref())?;
            record_omitted_field(&mut voucher.omitted, format!("ENTRY/@{key}"))?;
        }
        *entry = Some(WorkingEntry::default());
    } else if matches!(parent, Some("VOUCHER") | Some("ALLLEDGERENTRIES.LIST")) {
        let voucher = current.as_mut().ok_or(SourceXmlError::UnsupportedShape)?;
        if parent == Some("VOUCHER") && matches!(tag.as_str(), "DATE" | "NARRATION") {
            let slot = if tag == "DATE" {
                &mut voucher.date
            } else {
                &mut voucher.narration
            };
            if slot.replace(String::new()).is_some() {
                return Err(SourceXmlError::UnsupportedShape);
            }
        } else if parent == Some("VOUCHER") && tag != "ALLLEDGERENTRIES.LIST" {
            record_omitted_field(&mut voucher.omitted, format!("VOUCHER/{tag}"))?;
        }
        if parent == Some("ALLLEDGERENTRIES.LIST")
            && matches!(tag.as_str(), "LEDGERNAME" | "AMOUNT" | "ISDEEMEDPOSITIVE")
        {
            let row = entry.as_mut().ok_or(SourceXmlError::UnsupportedShape)?;
            let slot = match tag.as_str() {
                "LEDGERNAME" => &mut row.ledger,
                "AMOUNT" => &mut row.amount,
                _ => &mut row.polarity,
            };
            if slot.replace(String::new()).is_some() {
                return Err(SourceXmlError::UnsupportedShape);
            }
        } else if parent == Some("ALLLEDGERENTRIES.LIST") {
            record_omitted_field(&mut voucher.omitted, format!("ENTRY/{tag}"))?;
        }
        if event.attributes().next().is_some() {
            return Err(SourceXmlError::UnsupportedShape);
        }
    }
    if tag == "ENVELOPE" {
        *saw_root = true;
    }
    stack.push(tag);
    Ok(())
}

fn record_notice(
    notices: &mut BTreeMap<String, usize>,
    kind: String,
) -> Result<(), SourceXmlError> {
    if !notices.contains_key(&kind) && notices.len() >= MAX_SOURCE_NOTICE_KINDS {
        return Err(SourceXmlError::SourceNoticeLimit);
    }
    *notices.entry(kind).or_insert(0) += 1;
    Ok(())
}

fn record_omitted_field(
    omitted: &mut BTreeSet<String>,
    field: String,
) -> Result<(), SourceXmlError> {
    if !omitted.contains(&field) && omitted.len() >= MAX_OMITTED_FIELDS_PER_VOUCHER {
        return Err(SourceXmlError::OmittedFieldLimit);
    }
    omitted.insert(field);
    Ok(())
}

fn finish(
    stack: &mut Vec<String>,
    tag: &str,
    vouchers: &mut Vec<SourceVoucher>,
    current: &mut Option<WorkingVoucher>,
    entry: &mut Option<WorkingEntry>,
) -> Result<(), SourceXmlError> {
    if stack.pop().as_deref() != Some(tag) {
        return Err(SourceXmlError::UnsupportedShape);
    }
    if tag == "ALLLEDGERENTRIES.LIST" {
        let row = entry.take().ok_or(SourceXmlError::UnsupportedShape)?;
        let voucher = current.as_mut().ok_or(SourceXmlError::UnsupportedShape)?;
        if voucher.entries.len() >= MAX_ENTRIES {
            return Err(SourceXmlError::EntryLimit);
        }
        voucher.entries.push(SourceEntry {
            position: voucher.entries.len() + 1,
            ledger: row.ledger.ok_or(SourceXmlError::RequiredFieldMissing)?,
            amount: row.amount.ok_or(SourceXmlError::RequiredFieldMissing)?,
            polarity: row.polarity,
        });
    }
    if tag == "VOUCHER" {
        let row = current.take().ok_or(SourceXmlError::UnsupportedShape)?;
        if vouchers.len() >= MAX_VOUCHERS {
            return Err(SourceXmlError::VoucherLimit);
        }
        vouchers.push(SourceVoucher {
            position: vouchers.len() + 1,
            remote_id: row.remote_id.ok_or(SourceXmlError::RequiredFieldMissing)?,
            date: row.date.ok_or(SourceXmlError::RequiredFieldMissing)?,
            voucher_type: row
                .voucher_type
                .ok_or(SourceXmlError::RequiredFieldMissing)?,
            narration: row.narration,
            entries: nonempty(row.entries)?,
            omitted_fields: row.omitted.into_iter().collect(),
        });
    }
    Ok(())
}

fn append_text(
    stack: &[String],
    current: &mut Option<WorkingVoucher>,
    entry: &mut Option<WorkingEntry>,
    value: String,
) -> Result<(), SourceXmlError> {
    if value.len() > MAX_TEXT_BYTES {
        return Err(SourceXmlError::InvalidText);
    }
    let (parent, tag) = (
        stack.iter().rev().nth(1).map(String::as_str),
        stack.last().map(String::as_str),
    );
    match (parent, tag, current.as_mut(), entry.as_mut()) {
        (Some("VOUCHER"), Some("DATE"), Some(row), _) => append(&mut row.date, value)?,
        (Some("VOUCHER"), Some("NARRATION"), Some(row), _) => append(&mut row.narration, value)?,
        (Some("ALLLEDGERENTRIES.LIST"), Some("LEDGERNAME"), _, Some(row)) => {
            append(&mut row.ledger, value)?
        }
        (Some("ALLLEDGERENTRIES.LIST"), Some("AMOUNT"), _, Some(row)) => {
            append(&mut row.amount, value)?
        }
        (Some("ALLLEDGERENTRIES.LIST"), Some("ISDEEMEDPOSITIVE"), _, Some(row)) => {
            append(&mut row.polarity, value)?
        }
        (_, _, _, _) if stack.is_empty() && !value.trim().is_empty() => {
            return Err(SourceXmlError::UnsupportedShape)
        }
        (_, _, None, _) | (_, _, _, None) if !stack.iter().any(|tag| tag == "VOUCHER") => {}
        (_, _, _, _) if value.trim().is_empty() => {}
        (Some("VOUCHER") | Some("ALLLEDGERENTRIES.LIST"), _, _, _) => {}
        _ => return Err(SourceXmlError::UnsupportedShape),
    }
    Ok(())
}

fn tag_name(raw: &[u8]) -> Result<String, SourceXmlError> {
    if raw.len() > MAX_TAG_BYTES {
        return Err(SourceXmlError::UnsupportedShape);
    }
    std::str::from_utf8(raw)
        .map(str::to_owned)
        .map_err(|_| SourceXmlError::InvalidUtf8)
}
fn validate_attribute_bounds(
    event: &quick_xml::events::BytesStart<'_>,
    reader: &Reader<&[u8]>,
) -> Result<(), SourceXmlError> {
    for attribute in event.attributes().with_checks(true) {
        let attribute = attribute.map_err(|_| SourceXmlError::UnsupportedShape)?;
        let _ = tag_name(attribute.key.as_ref())?;
        let value = attribute
            .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
            .map_err(|_| SourceXmlError::InvalidUtf8)?;
        bounded(value.into_owned())?;
    }
    Ok(())
}
fn unescape(value: &str) -> Result<String, SourceXmlError> {
    quick_xml::escape::unescape(value)
        .map(|v| v.into_owned())
        .map_err(|_| SourceXmlError::InvalidEntity)
}
fn decode_text(text: BytesText<'_>) -> Result<String, SourceXmlError> {
    unescape(&text.decode().map_err(|_| SourceXmlError::InvalidUtf8)?)
}
fn decode_reference(reference: BytesRef<'_>) -> Result<String, SourceXmlError> {
    unescape(&format!(
        "&{};",
        reference
            .decode()
            .map_err(|_| SourceXmlError::InvalidUtf8)?
    ))
}
fn bounded(value: String) -> Result<String, SourceXmlError> {
    if value.len() > MAX_TEXT_BYTES {
        Err(SourceXmlError::InvalidText)
    } else {
        Ok(value)
    }
}
fn claim(slot: &mut Option<String>, value: String) -> Result<(), SourceXmlError> {
    if slot.replace(value).is_some() {
        Err(SourceXmlError::UnsupportedShape)
    } else {
        Ok(())
    }
}
fn append(slot: &mut Option<String>, value: String) -> Result<(), SourceXmlError> {
    let next = format!("{}{}", slot.as_deref().unwrap_or_default(), value);
    *slot = Some(bounded(next)?);
    Ok(())
}
fn nonempty<T>(value: Vec<T>) -> Result<Vec<T>, SourceXmlError> {
    if value.is_empty() {
        Err(SourceXmlError::RequiredFieldMissing)
    } else {
        Ok(value)
    }
}

#[derive(Default)]
struct WorkingVoucher {
    remote_id: Option<String>,
    date: Option<String>,
    voucher_type: Option<String>,
    narration: Option<String>,
    entries: Vec<SourceEntry>,
    omitted: BTreeSet<String>,
}
#[derive(Default)]
struct WorkingEntry {
    ledger: Option<String>,
    amount: Option<String>,
    polarity: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    const XML: &str = "<ENVELOPE><BODY><IMPORTDATA><REQUESTDATA><TALLYMESSAGE><VOUCHER REMOTEID=\"id&amp;1\" VCHTYPE=\"Receipt\"><DATE>20260901</DATE><NARRATION>Party &amp; Co</NARRATION><VOUCHERNUMBER>1</VOUCHERNUMBER><ALLLEDGERENTRIES.LIST><LEDGERNAME>Cash</LEDGERNAME><AMOUNT>-1.00</AMOUNT></ALLLEDGERENTRIES.LIST></VOUCHER></TALLYMESSAGE></REQUESTDATA></IMPORTDATA></BODY></ENVELOPE>";
    #[test]
    fn captured_tally_collection_export_is_not_a_voucher_import_document() {
        // This real captured response is a different document contract from the
        // user-authored IMPORTDATA candidates supported by local preparation.
        let captured = include_bytes!(
            "../crates/bridge-tally-protocol/tests/fixtures/unit_a_vouchers_wildcard_live.xml"
        );
        assert_eq!(
            parse_source_xml(captured, "captured-collection.xml".into()),
            Err(SourceXmlError::UnsupportedShape)
        );
    }
    #[test]
    fn accepts_the_structure_of_a_real_voucher_import_candidate() {
        // Provenance, stated exactly: this fixture is a structural derivative of
        // a real user-authored Tally voucher-import file. Its envelope nesting,
        // element set and order, attribute set, per-voucher field presence,
        // entry cardinality, balanced +/- pair invariant and value FORMATS are
        // transcribed from that document. Every value is synthetic; no original
        // name, date, amount, narration, transaction id, company or party
        // survives. See the fixture README for the derivation and its limits.
        //
        // What it establishes: the supported shape is not an assumption this
        // suite invented, unlike the hand-authored XML constant above.
        // What it does NOT establish: that Tally accepted an import of this
        // document. The source file's own import outcome is unknown, so this is
        // format evidence, never acceptance evidence.
        let derived = include_bytes!(
            "../crates/bridge-tally-protocol/tests/fixtures/voucher_import_candidate_structure_derived.xml"
        );
        let parsed = parse_source_xml(derived, "voucher-import-candidate.xml".into())
            .expect("the real import candidate shape must parse");

        assert_eq!(parsed.vouchers.len(), 77);
        assert!(parsed
            .vouchers
            .iter()
            .all(|voucher| voucher.entries.len() == 2));

        // Fields this format carries that local preparation does not interpret
        // must be reported as omissions rather than silently dropped.
        for voucher in &parsed.vouchers {
            assert_eq!(
                voucher.omitted_fields,
                [
                    "VOUCHER/@ACTION",
                    "VOUCHER/EFFECTIVEDATE",
                    "VOUCHER/PARTYLEDGERNAME",
                    "VOUCHER/VOUCHERNUMBER",
                ]
            );
        }

        // Entries carry no ISDEEMEDPOSITIVE, so polarity stays unanswered and is
        // never inferred from the amount sign.
        assert!(parsed
            .vouchers
            .iter()
            .flat_map(|voucher| &voucher.entries)
            .all(|entry| entry.polarity.is_none()));

        // Amounts are retained as exact source text, not reparsed into a number.
        let first = &parsed.vouchers[0];
        assert_eq!(first.voucher_type, "Receipt");
        assert_eq!(first.date, "20010101");
        assert_eq!(first.entries[0].amount, "-7.50");
        assert_eq!(first.entries[1].amount, "7.50");

        // The source bytes survive parsing untouched.
        assert_eq!(parsed.utf8.as_bytes(), derived.as_slice());
    }
    #[test]
    fn preserves_raw_source_and_marks_omitted_fields() {
        let parsed = parse_source_xml(XML.as_bytes(), "source.xml".into()).unwrap();
        assert_eq!(parsed.vouchers[0].remote_id, "id&1");
        assert_eq!(parsed.vouchers[0].narration.as_deref(), Some("Party & Co"));
        assert_eq!(parsed.vouchers[0].entries[0].polarity, None);
        assert_eq!(parsed.vouchers[0].omitted_fields, ["VOUCHER/VOUCHERNUMBER"]);

        let with_entry_attribute = XML.replacen(
            "<ALLLEDGERENTRIES.LIST>",
            "<ALLLEDGERENTRIES.LIST OBSERVED=\"metadata\">",
            1,
        );
        let parsed =
            parse_source_xml(with_entry_attribute.as_bytes(), "source.xml".into()).unwrap();
        assert_eq!(parsed.utf8, with_entry_attribute);
        assert_eq!(
            parsed.vouchers[0].omitted_fields,
            ["ENTRY/@OBSERVED", "VOUCHER/VOUCHERNUMBER"]
        );
    }
    #[test]
    fn rejects_doctype_and_nested_unknown_source_fields() {
        assert_eq!(
            parse_source_xml(b"<!DOCTYPE a><ENVELOPE/>", "x".into()),
            Err(SourceXmlError::DoctypeForbidden)
        );
        assert_eq!(
            parse_source_xml(
                XML.replacen(
                    "<VOUCHERNUMBER>1</VOUCHERNUMBER>",
                    "<EXTRA><X>1</X></EXTRA>",
                    1
                )
                .as_bytes(),
                "x".into()
            ),
            Err(SourceXmlError::UnsupportedShape)
        );
    }
    #[test]
    fn duplicate_scalar_refuses_while_present_empty_remains_distinct_from_absent() {
        let duplicate = XML.replacen(
            "<DATE>20260901</DATE>",
            "<DATE>20260901</DATE><DATE>20260902</DATE>",
            1,
        );
        assert_eq!(
            parse_source_xml(duplicate.as_bytes(), "x.xml".into()),
            Err(SourceXmlError::UnsupportedShape)
        );
        let empty = XML.replacen("<NARRATION>Party &amp; Co</NARRATION>", "<NARRATION/>", 1);
        assert_eq!(
            parse_source_xml(empty.as_bytes(), "x.xml".into())
                .unwrap()
                .vouchers[0]
                .narration
                .as_deref(),
            Some("")
        );
        let absent = XML.replacen("<NARRATION>Party &amp; Co</NARRATION>", "", 1);
        assert_eq!(
            parse_source_xml(absent.as_bytes(), "x.xml".into())
                .unwrap()
                .vouchers[0]
                .narration,
            None
        );
    }
    #[test]
    fn hash_changes_with_source_bytes_and_duplicate_remote_id_refuses() {
        let one = parse_source_xml(XML.as_bytes(), "x.xml".into()).unwrap();
        let two = parse_source_xml(format!("{XML}\n").as_bytes(), "x.xml".into()).unwrap();
        assert_ne!(one.sha256, two.sha256);
        let duplicate = XML.replacen(
            "</TALLYMESSAGE>",
            &format!(
                "</TALLYMESSAGE><TALLYMESSAGE>{}</TALLYMESSAGE>",
                XML.split("<TALLYMESSAGE>")
                    .nth(1)
                    .unwrap()
                    .split("</TALLYMESSAGE>")
                    .next()
                    .unwrap()
            ),
            1,
        );
        assert_eq!(
            parse_source_xml(duplicate.as_bytes(), "x.xml".into()),
            Err(SourceXmlError::UnsupportedShape)
        );
    }
    #[test]
    fn distinct_source_notice_categories_are_bounded() {
        let at_limit = (0..MAX_SOURCE_NOTICE_KINDS)
            .map(|index| format!("<NOTICE{index}/>"))
            .collect::<String>();
        let at_limit_xml = XML.replacen("<VOUCHER", &format!("{at_limit}<VOUCHER"), 1);
        assert_eq!(
            parse_source_xml(at_limit_xml.as_bytes(), "x.xml".into())
                .unwrap()
                .source_notices
                .len(),
            MAX_SOURCE_NOTICE_KINDS
        );
        let repeated = parse_source_xml(
            at_limit_xml
                .replacen("<VOUCHER", "<NOTICE0/><VOUCHER", 1)
                .as_bytes(),
            "x.xml".into(),
        )
        .unwrap();
        assert_eq!(repeated.source_notices.len(), MAX_SOURCE_NOTICE_KINDS);
        assert_eq!(
            repeated
                .source_notices
                .iter()
                .find(|notice| notice.kind == "Ignored TALLYMESSAGE/NOTICE0")
                .map(|notice| notice.count),
            Some(2)
        );
        let over_limit = (0..=MAX_SOURCE_NOTICE_KINDS)
            .map(|index| format!("<NOTICE{index}/>"))
            .collect::<String>();
        assert_eq!(
            parse_source_xml(
                XML.replacen("<VOUCHER", &format!("{over_limit}<VOUCHER"), 1)
                    .as_bytes(),
                "x.xml".into()
            ),
            Err(SourceXmlError::SourceNoticeLimit)
        );
    }

    #[test]
    fn omitted_categories_share_one_row_bound_across_all_routes() {
        let attributes = (0..21).map(|i| format!(" A{i}=\"\"")).collect::<String>();
        let voucher_fields = (0..21).map(|i| format!("<F{i}/>")).collect::<String>();
        let entry_fields = (0..22).map(|i| format!("<E{i}/>")).collect::<String>();
        let at_limit = XML
            .replacen("<VOUCHER ", &format!("<VOUCHER{attributes} "), 1)
            .replacen("<VOUCHERNUMBER>1</VOUCHERNUMBER>", &voucher_fields, 1)
            .replacen(
                "</ALLLEDGERENTRIES.LIST>",
                &format!("{entry_fields}</ALLLEDGERENTRIES.LIST>"),
                1,
            );
        let parsed = parse_source_xml(at_limit.as_bytes(), "x.xml".into()).unwrap();
        assert_eq!(
            parsed.vouchers[0].omitted_fields.len(),
            MAX_OMITTED_FIELDS_PER_VOUCHER
        );
        assert_eq!(parsed.utf8, at_limit);
        let repeated =
            at_limit
                .replacen("<F0/>", "<F0/><F0/>", 1)
                .replacen("<E0/>", "<E0/><E0/>", 1);
        assert_eq!(
            parse_source_xml(repeated.as_bytes(), "x.xml".into())
                .unwrap()
                .vouchers[0]
                .omitted_fields
                .len(),
            MAX_OMITTED_FIELDS_PER_VOUCHER
        );
        for over_limit in [
            at_limit.replacen("<VOUCHER ", "<VOUCHER EXTRA=\"\" ", 1),
            at_limit.replacen("</VOUCHER>", "<EXTRA/></VOUCHER>", 1),
            at_limit.replacen(
                "</ALLLEDGERENTRIES.LIST>",
                "<EXTRA/></ALLLEDGERENTRIES.LIST>",
                1,
            ),
            at_limit.replacen(
                "<ALLLEDGERENTRIES.LIST>",
                "<ALLLEDGERENTRIES.LIST EXTRA=\"\">",
                1,
            ),
        ] {
            assert_eq!(
                parse_source_xml(over_limit.as_bytes(), "x.xml".into()),
                Err(SourceXmlError::OmittedFieldLimit)
            );
        }
    }
}
