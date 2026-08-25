//! Strict native Trial Balance request, parser, and exact accounting controls.
//!
//! Tally's ledger-wise trial-balance presentation uses `TBALOPENING` and
//! `TBALCLOSING`. `CLOSINGBALANCE` is a different balance-sheet presentation
//! and must not be substituted for the closing column. See Tally protocol
//! reference §7.1.

use std::collections::HashSet;

use bridge_tally_primitives::ExactDecimal;
use quick_xml::{events::Event, Reader};

use crate::tolerant_xml::sanitize_invalid_numeric_references;

pub use crate::trial_balance_model::{TrialBalance, TrialBalanceError, TrialBalanceLedger};
pub use crate::trial_balance_request::render_trial_balance_request;

const MAX_TRIAL_BALANCE_ROWS: usize = 200_000;
const MAX_ROW_TEXT_BYTES: usize = 4_096;
const MAX_TOTAL_TEXT_BYTES: usize = 64 * 1024 * 1024;

pub fn parse_trial_balance(
    xml: &str,
    expected_company_guid: &str,
) -> Result<TrialBalance, TrialBalanceError> {
    let sanitized = sanitize_invalid_numeric_references(xml);
    let mut reader = Reader::from_str(&sanitized);
    reader.config_mut().trim_text(true);
    let mut path = Vec::<Vec<u8>>::new();
    let mut root_seen = false;
    let mut root_closed = false;
    let mut status_seen = false;
    let mut collection_seen = false;
    let mut company_guid_match = false;
    let mut rows = Vec::new();
    let mut total_text_bytes = 0_usize;

    loop {
        match reader
            .read_event()
            .map_err(|_| TrialBalanceError::InvalidResponse("xml_malformed"))?
        {
            Event::Start(element) => {
                let name = element.name().as_ref().to_ascii_uppercase();
                if path.is_empty() {
                    if root_seen || name != b"ENVELOPE" {
                        return Err(TrialBalanceError::InvalidResponse("root_not_envelope"));
                    }
                    root_seen = true;
                }
                if matches!(name.as_slice(), b"RESPONSE" | b"LINEERROR") {
                    return Err(TrialBalanceError::TallyReportedFailure);
                }
                if path_is(&path, &[b"ENVELOPE", b"HEADER"]) && name == b"STATUS" {
                    if status_seen {
                        return Err(TrialBalanceError::InvalidResponse("status_duplicate"));
                    }
                    if read_text(&mut reader, element.name().as_ref())? != "1" {
                        return Err(TrialBalanceError::TallyReportedFailure);
                    }
                    status_seen = true;
                    continue;
                }
                if path_is(&path, &[b"ENVELOPE", b"BODY", b"DATA"]) && name == b"COLLECTION" {
                    if collection_seen {
                        return Err(TrialBalanceError::InvalidResponse("collection_duplicate"));
                    }
                    collection_seen = true;
                }
                if path_is(&path, &[b"ENVELOPE", b"BODY", b"DATA", b"COLLECTION"])
                    && name == b"LEDGER"
                {
                    let row = parse_ledger(&mut reader, &element)?;
                    company_guid_match |= crate::native_ledger_guid_has_company_prefix(
                        &row.guid,
                        expected_company_guid,
                    );
                    if rows.len() >= MAX_TRIAL_BALANCE_ROWS {
                        return Err(TrialBalanceError::InvalidResponse("ledger_row_limit"));
                    }
                    let row_text_bytes = row
                        .name
                        .len()
                        .checked_add(row.parent.as_deref().map_or(0, str::len))
                        .and_then(|value| value.checked_add(row.guid.len()))
                        .ok_or(TrialBalanceError::InvalidResponse("ledger_text_limit"))?;
                    if row.name.len() > MAX_ROW_TEXT_BYTES
                        || row
                            .parent
                            .as_deref()
                            .is_some_and(|value| value.len() > MAX_ROW_TEXT_BYTES)
                        || row.guid.len() > MAX_ROW_TEXT_BYTES
                    {
                        return Err(TrialBalanceError::InvalidResponse("ledger_text_limit"));
                    }
                    total_text_bytes = total_text_bytes
                        .checked_add(row_text_bytes)
                        .filter(|value| *value <= MAX_TOTAL_TEXT_BYTES)
                        .ok_or(TrialBalanceError::InvalidResponse("ledger_text_limit"))?;
                    rows.push(row);
                    continue;
                }
                path.push(name);
            }
            Event::Empty(element) => {
                let name = element.name().as_ref().to_ascii_uppercase();
                if path.is_empty() {
                    return Err(TrialBalanceError::InvalidResponse("root_not_envelope"));
                } else if matches!(name.as_slice(), b"RESPONSE" | b"LINEERROR") {
                    return Err(TrialBalanceError::TallyReportedFailure);
                } else if path_is(&path, &[b"ENVELOPE", b"BODY", b"DATA"]) && name == b"COLLECTION"
                {
                    if collection_seen {
                        return Err(TrialBalanceError::InvalidResponse("collection_duplicate"));
                    }
                    collection_seen = true;
                } else if path_is(&path, &[b"ENVELOPE", b"BODY", b"DATA", b"COLLECTION"])
                    && name == b"LEDGER"
                {
                    return Err(TrialBalanceError::InvalidResponse("ledger_row_empty"));
                }
            }
            Event::End(element) => {
                let expected = path
                    .pop()
                    .ok_or(TrialBalanceError::InvalidResponse("unexpected_close"))?;
                if expected != element.name().as_ref().to_ascii_uppercase() {
                    return Err(TrialBalanceError::InvalidResponse("unexpected_close"));
                }
                if path.is_empty() {
                    root_closed = true;
                }
            }
            Event::Text(text) if path.is_empty() && !text_is_empty(&text)? => {
                return Err(TrialBalanceError::InvalidResponse("unexpected_text"));
            }
            Event::CData(_) | Event::DocType(_) | Event::PI(_) => {
                return Err(TrialBalanceError::InvalidResponse("active_xml_construct"));
            }
            Event::Eof => break,
            _ => {}
        }
    }

    if !path.is_empty() || !root_seen || !root_closed || !status_seen || !collection_seen {
        return Err(TrialBalanceError::InvalidResponse("envelope_incomplete"));
    }
    if rows.is_empty() {
        return Err(TrialBalanceError::InvalidResponse("ledger_rows_missing"));
    }
    if !company_guid_match {
        return Err(TrialBalanceError::InvalidResponse(
            "company_guid_unverified",
        ));
    }
    let mut names = HashSet::with_capacity(rows.len());
    if rows
        .iter()
        .any(|row| !names.insert(row.name.to_lowercase()))
    {
        return Err(TrialBalanceError::InvalidResponse("ledger_name_duplicate"));
    }

    let opening_total = sum(rows.iter().map(|row| &row.opening))?;
    let closing_total = sum(rows.iter().map(|row| &row.closing))?;
    let movement_total = closing_total
        .checked_subtract(&opening_total)
        .map_err(|_| TrialBalanceError::Arithmetic)?;
    if !movement_total.is_zero() {
        return Err(TrialBalanceError::InvalidResponse(
            "movement_does_not_balance",
        ));
    }
    if !opening_total.is_zero() {
        return Err(TrialBalanceError::InvalidResponse(
            "opening_difference_unverified",
        ));
    }
    Ok(TrialBalance { rows })
}

fn parse_ledger(
    reader: &mut Reader<&[u8]>,
    element: &quick_xml::events::BytesStart<'_>,
) -> Result<TrialBalanceLedger, TrialBalanceError> {
    let name = attribute(element, b"NAME")?
        .filter(|value| !value.is_empty())
        .ok_or(TrialBalanceError::InvalidResponse("ledger_name_missing"))?;
    let mut parent = None;
    let mut guid = None;
    let mut master_id = None;
    let mut alter_id = None;
    let mut opening = None;
    let mut closing = None;
    loop {
        match reader
            .read_event()
            .map_err(|_| TrialBalanceError::InvalidResponse("xml_malformed"))?
        {
            Event::Start(child) => {
                let tag = child.name().as_ref().to_ascii_uppercase();
                let value = match tag.as_slice() {
                    b"PARENT" | b"GUID" | b"MASTERID" | b"ALTERID" | b"TBALOPENING"
                    | b"TBALCLOSING" => Some(read_text(reader, child.name().as_ref())?),
                    _ => {
                        skip_subtree(reader)?;
                        None
                    }
                };
                match (tag.as_slice(), value) {
                    (b"PARENT", Some(value)) => set_once(&mut parent, value, "parent_duplicate")?,
                    (b"GUID", Some(value)) => set_once(&mut guid, value, "guid_duplicate")?,
                    (b"MASTERID", Some(value)) => {
                        set_once(&mut master_id, parse_u64(&value)?, "master_id_duplicate")?
                    }
                    (b"ALTERID", Some(value)) => {
                        set_once(&mut alter_id, parse_u64(&value)?, "alter_id_duplicate")?
                    }
                    (b"TBALOPENING", Some(value)) => {
                        set_once(&mut opening, parse_amount(&value)?, "opening_duplicate")?
                    }
                    (b"TBALCLOSING", Some(value)) => {
                        set_once(&mut closing, parse_amount(&value)?, "closing_duplicate")?
                    }
                    _ => {}
                }
            }
            Event::Empty(child) => {
                let tag = child.name().as_ref().to_ascii_uppercase();
                match tag.as_slice() {
                    b"PARENT" => set_once(&mut parent, String::new(), "parent_duplicate")?,
                    b"GUID" => return Err(TrialBalanceError::InvalidResponse("guid_missing")),
                    b"MASTERID" | b"ALTERID" => {
                        return Err(TrialBalanceError::InvalidResponse(
                            "identity_number_invalid",
                        ))
                    }
                    b"TBALOPENING" | b"TBALCLOSING" => {
                        return Err(TrialBalanceError::InvalidResponse("amount_invalid"))
                    }
                    _ => {}
                }
            }
            Event::End(end) if end.name().as_ref().eq_ignore_ascii_case(b"LEDGER") => break,
            Event::Text(text) if !text_is_empty(&text)? => {
                return Err(TrialBalanceError::InvalidResponse("ledger_unexpected_text"));
            }
            Event::CData(_) | Event::DocType(_) | Event::PI(_) => {
                return Err(TrialBalanceError::InvalidResponse("active_xml_construct"));
            }
            Event::Eof => return Err(TrialBalanceError::InvalidResponse("ledger_unterminated")),
            _ => {}
        }
    }
    let guid = guid
        .filter(|value| !value.is_empty())
        .ok_or(TrialBalanceError::InvalidResponse("guid_missing"))?;
    Ok(TrialBalanceLedger {
        name,
        parent: parent.filter(|value| !value.is_empty()),
        guid,
        master_id: master_id.ok_or(TrialBalanceError::InvalidResponse("master_id_missing"))?,
        alter_id: alter_id.ok_or(TrialBalanceError::InvalidResponse("alter_id_missing"))?,
        opening: opening.ok_or(TrialBalanceError::InvalidResponse("opening_missing"))?,
        closing: closing.ok_or(TrialBalanceError::InvalidResponse("closing_missing"))?,
    })
}

fn parse_amount(value: &str) -> Result<ExactDecimal, TrialBalanceError> {
    ExactDecimal::parse(value.trim().to_string())
        .map_err(|_| TrialBalanceError::InvalidResponse("amount_invalid"))
}

fn parse_u64(value: &str) -> Result<u64, TrialBalanceError> {
    value
        .trim()
        .parse()
        .map_err(|_| TrialBalanceError::InvalidResponse("identity_number_invalid"))
}

fn sum<'a>(
    mut values: impl Iterator<Item = &'a ExactDecimal>,
) -> Result<ExactDecimal, TrialBalanceError> {
    values.try_fold(ExactDecimal::zero(), |total, value| {
        total
            .checked_add(value)
            .map_err(|_| TrialBalanceError::Arithmetic)
    })
}

fn set_once<T>(
    slot: &mut Option<T>,
    value: T,
    code: &'static str,
) -> Result<(), TrialBalanceError> {
    if slot.replace(value).is_some() {
        return Err(TrialBalanceError::InvalidResponse(code));
    }
    Ok(())
}

fn attribute(
    element: &quick_xml::events::BytesStart<'_>,
    name: &[u8],
) -> Result<Option<String>, TrialBalanceError> {
    let mut found = None;
    for attribute in element.attributes() {
        let attribute = attribute
            .map_err(|_| TrialBalanceError::InvalidResponse("ledger_attribute_invalid"))?;
        if attribute.key.as_ref().eq_ignore_ascii_case(name) {
            if found.is_some() {
                return Err(TrialBalanceError::InvalidResponse(
                    "ledger_attribute_duplicate",
                ));
            }
            found = Some(
                attribute
                    .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                    .map_err(|_| TrialBalanceError::InvalidResponse("ledger_attribute_invalid"))?
                    .into_owned(),
            );
        }
    }
    Ok(found)
}

fn read_text(reader: &mut Reader<&[u8]>, name: &[u8]) -> Result<String, TrialBalanceError> {
    let raw = reader
        .read_text(quick_xml::name::QName(name))
        .map_err(|_| TrialBalanceError::InvalidResponse("element_text_invalid"))?;
    let decoded = raw
        .decode()
        .map_err(|_| TrialBalanceError::InvalidResponse("element_text_invalid"))?;
    let unescaped = quick_xml::escape::unescape(&decoded)
        .map_err(|_| TrialBalanceError::InvalidResponse("element_text_invalid"))?;
    Ok(unescaped.trim().to_string())
}

fn skip_subtree(reader: &mut Reader<&[u8]>) -> Result<(), TrialBalanceError> {
    let mut depth = 1_u32;
    loop {
        match reader
            .read_event()
            .map_err(|_| TrialBalanceError::InvalidResponse("xml_malformed"))?
        {
            Event::Start(element) => {
                if matches!(
                    element.name().as_ref().to_ascii_uppercase().as_slice(),
                    b"RESPONSE" | b"LINEERROR"
                ) {
                    return Err(TrialBalanceError::TallyReportedFailure);
                }
                depth = depth
                    .checked_add(1)
                    .ok_or(TrialBalanceError::InvalidResponse("subtree_depth_limit"))?;
            }
            Event::Empty(element)
                if matches!(
                    element.name().as_ref().to_ascii_uppercase().as_slice(),
                    b"RESPONSE" | b"LINEERROR"
                ) =>
            {
                return Err(TrialBalanceError::TallyReportedFailure);
            }
            Event::End(_) => {
                depth -= 1;
                if depth == 0 {
                    return Ok(());
                }
            }
            Event::CData(_) | Event::DocType(_) | Event::PI(_) => {
                return Err(TrialBalanceError::InvalidResponse("active_xml_construct"));
            }
            Event::Eof => return Err(TrialBalanceError::InvalidResponse("subtree_unterminated")),
            _ => {}
        }
    }
}

fn text_is_empty(text: &quick_xml::events::BytesText<'_>) -> Result<bool, TrialBalanceError> {
    Ok(text
        .decode()
        .map_err(|_| TrialBalanceError::InvalidResponse("element_text_invalid"))?
        .trim()
        .is_empty())
}

fn path_is(path: &[Vec<u8>], expected: &[&[u8]]) -> bool {
    path.len() == expected.len()
        && path
            .iter()
            .zip(expected)
            .all(|(actual, expected)| actual.as_slice() == *expected)
}
