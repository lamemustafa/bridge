//! Closed, read-only native `List of Ledgers` trial-balance protocol.
//!
//! `TBAL*` fields are the wire representation used by Tally's Trial Balance;
//! `CLOSINGBALANCE` has a different Profit & Loss presentation and is not a
//! substitute. This module deliberately only renders and parses that measured
//! collection shape. Dispatch, pairing, currency observation, and totals are
//! runtime responsibilities.

use std::{collections::HashSet, fmt};

use bridge_tally_primitives::ExactDecimal;
use quick_xml::{
    events::{BytesStart, Event},
    name::QName,
    Reader,
};
use serde::Serialize;

use crate::{
    native_ledger_guid_has_company_prefix, native_outstandings::NativeLedgerSnapshotPeriod,
    tolerant_xml::sanitize_invalid_numeric_references, PartyLedgerMasterFieldObservation,
};

/// One amount exactly as the native collection exposed it.
///
/// An empty `TYPE="Amount"` element is source evidence and is never coerced
/// to numeric zero. A missing element is a malformed Trial Balance row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", content = "value", rename_all = "snake_case")]
pub enum NativeTrialBalanceAmount {
    Present(ExactDecimal),
    PresentEmpty,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NativeTrialBalanceRow {
    pub name: String,
    pub guid: String,
    pub parent: PartyLedgerMasterFieldObservation,
    pub opening: NativeTrialBalanceAmount,
    pub debit: NativeTrialBalanceAmount,
    pub credit: NativeTrialBalanceAmount,
    pub closing: NativeTrialBalanceAmount,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NativeTrialBalance {
    pub rows: Vec<NativeTrialBalanceRow>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeTrialBalanceError {
    InvalidAmount,
    InvalidResponse(&'static str),
    TallyReportedFailure,
}

impl fmt::Display for NativeTrialBalanceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidAmount => {
                formatter.write_str("Tally returned an invalid Trial Balance amount")
            }
            Self::InvalidResponse(code) => {
                write!(formatter, "native Trial Balance response invalid ({code})")
            }
            Self::TallyReportedFailure => {
                formatter.write_str("Tally reported failure for the native Trial Balance request")
            }
        }
    }
}

impl std::error::Error for NativeTrialBalanceError {}

/// Renders the only supported native Trial Balance read. Both dates come from
/// the already-admitted snapshot period: `TBALOPENING` and `TBALCLOSING` are
/// meaningful only for the same validated window.
pub fn render_native_trial_balance_request(
    company: &str,
    period: &NativeLedgerSnapshotPeriod,
) -> String {
    format!(
        r#"<ENVELOPE><HEADER><VERSION>1</VERSION><TALLYREQUEST>Export</TALLYREQUEST><TYPE>Collection</TYPE><ID>List of Ledgers</ID></HEADER><BODY><DESC><STATICVARIABLES><SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT><SVCURRENTCOMPANY>{company}</SVCURRENTCOMPANY><SVFROMDATE TYPE="Date">{from}</SVFROMDATE><SVTODATE TYPE="Date">{to}</SVTODATE></STATICVARIABLES><TDL><TDLMESSAGE><COLLECTION NAME="List of Ledgers" ISMODIFY="Yes"><FETCH>NAME, GUID, PARENT, TBALOPENING, DEBITTOTALS, CREDITTOTALS, TBALCLOSING</FETCH></COLLECTION></TDLMESSAGE></TDL></DESC></BODY></ENVELOPE>"#,
        company = xml_escape(company),
        from = period.from().as_str(),
        to = period.to().as_str(),
    )
}

/// Parses a selected-company Trial Balance response. Each ledger GUID must
/// carry the expected company GUID prefix; a successful empty collection is
/// refused because it provides no source identity to bind to the selection.
pub fn parse_native_trial_balance(
    xml: &str,
    expected_company_guid: &str,
) -> Result<NativeTrialBalance, NativeTrialBalanceError> {
    let sanitized = sanitize_invalid_numeric_references(xml);
    let mut reader = Reader::from_str(&sanitized);
    reader.config_mut().trim_text(false);
    let mut path = Vec::<Vec<u8>>::new();
    let mut root_seen = false;
    let mut envelope_closed = false;
    let mut header_seen = false;
    let mut body_seen = false;
    let mut data_seen = false;
    let mut status_seen = false;
    let mut collection_seen = false;
    let mut rows = Vec::new();
    let mut identities = HashSet::new();
    let mut names = HashSet::new();

    loop {
        match reader
            .read_event()
            .map_err(|_| NativeTrialBalanceError::InvalidResponse("trial_balance_xml_malformed"))?
        {
            Event::Start(element) => {
                let name = element.name().as_ref().to_ascii_uppercase();
                if envelope_closed || (path.is_empty() && (name != b"ENVELOPE" || root_seen)) {
                    return Err(NativeTrialBalanceError::InvalidResponse(
                        "trial_balance_root_not_envelope",
                    ));
                }
                if path.is_empty() {
                    root_seen = true;
                }
                if path_is(&path, &[b"ENVELOPE"]) && name == b"HEADER" {
                    if std::mem::replace(&mut header_seen, true) {
                        return Err(NativeTrialBalanceError::InvalidResponse(
                            "trial_balance_duplicate_header",
                        ));
                    }
                }
                if path_is(&path, &[b"ENVELOPE"]) && name == b"BODY" {
                    if std::mem::replace(&mut body_seen, true) {
                        return Err(NativeTrialBalanceError::InvalidResponse(
                            "trial_balance_duplicate_body",
                        ));
                    }
                }
                if path_is(&path, &[b"ENVELOPE", b"BODY"]) && name == b"DATA" {
                    if std::mem::replace(&mut data_seen, true) {
                        return Err(NativeTrialBalanceError::InvalidResponse(
                            "trial_balance_duplicate_data",
                        ));
                    }
                }
                if name == b"LINEERROR" || name == b"ERROR" || name == b"DOCTYPE" {
                    return Err(NativeTrialBalanceError::TallyReportedFailure);
                }
                if path_is(&path, &[b"ENVELOPE", b"HEADER"]) && name == b"STATUS" {
                    if status_seen {
                        return Err(NativeTrialBalanceError::InvalidResponse(
                            "trial_balance_duplicate_status",
                        ));
                    }
                    if read_element_text(&mut reader, element.name())?.trim() != "1" {
                        return Err(NativeTrialBalanceError::TallyReportedFailure);
                    }
                    status_seen = true;
                    continue;
                }
                if path_is(&path, &[b"ENVELOPE", b"BODY", b"DATA"]) && name == b"COLLECTION" {
                    if collection_seen {
                        return Err(NativeTrialBalanceError::InvalidResponse(
                            "trial_balance_duplicate_collection",
                        ));
                    }
                    collection_seen = true;
                }
                if path_is(&path, &[b"ENVELOPE", b"BODY", b"DATA", b"COLLECTION"])
                    && name == b"LEDGER"
                {
                    let row = parse_row(&mut reader, &element, expected_company_guid)?;
                    if !identities.insert(row.guid.to_ascii_lowercase()) {
                        return Err(NativeTrialBalanceError::InvalidResponse(
                            "trial_balance_duplicate_guid",
                        ));
                    }
                    if !names.insert(row.name.to_lowercase()) {
                        return Err(NativeTrialBalanceError::InvalidResponse(
                            "trial_balance_duplicate_name",
                        ));
                    }
                    rows.push(row);
                    continue;
                }
                path.push(name);
            }
            Event::Empty(element) => {
                let name = element.name().as_ref().to_ascii_uppercase();
                if name == b"LINEERROR" || name == b"ERROR" || name == b"DOCTYPE" {
                    return Err(NativeTrialBalanceError::TallyReportedFailure);
                }
                if path_is(&path, &[b"ENVELOPE", b"BODY", b"DATA"]) && name == b"COLLECTION" {
                    if collection_seen {
                        return Err(NativeTrialBalanceError::InvalidResponse(
                            "trial_balance_duplicate_collection",
                        ));
                    }
                    collection_seen = true;
                } else if path_is(&path, &[b"ENVELOPE", b"BODY", b"DATA", b"COLLECTION"])
                    && name == b"LEDGER"
                {
                    return Err(NativeTrialBalanceError::InvalidResponse(
                        "trial_balance_row_empty",
                    ));
                }
            }
            Event::End(element) => {
                let expected = path.pop().ok_or(NativeTrialBalanceError::InvalidResponse(
                    "trial_balance_unexpected_close",
                ))?;
                if expected != element.name().as_ref().to_ascii_uppercase() {
                    return Err(NativeTrialBalanceError::InvalidResponse(
                        "trial_balance_unexpected_close",
                    ));
                }
                if expected == b"ENVELOPE" {
                    envelope_closed = true;
                }
            }
            Event::DocType(_) => {
                return Err(NativeTrialBalanceError::InvalidResponse(
                    "trial_balance_doctype_forbidden",
                ))
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if !path.is_empty() {
        return Err(NativeTrialBalanceError::InvalidResponse(
            "trial_balance_envelope_unterminated",
        ));
    }
    if !root_seen || !envelope_closed || !header_seen || !body_seen || !data_seen {
        return Err(NativeTrialBalanceError::InvalidResponse(
            "trial_balance_envelope_incomplete",
        ));
    }
    if !status_seen {
        return Err(NativeTrialBalanceError::TallyReportedFailure);
    }
    if !collection_seen {
        return Err(NativeTrialBalanceError::InvalidResponse(
            "trial_balance_collection_missing",
        ));
    }
    if rows.is_empty() {
        return Err(NativeTrialBalanceError::InvalidResponse(
            "trial_balance_source_identity_missing",
        ));
    }
    Ok(NativeTrialBalance { rows })
}

fn parse_row(
    reader: &mut Reader<&[u8]>,
    element: &BytesStart<'_>,
    expected_company_guid: &str,
) -> Result<NativeTrialBalanceRow, NativeTrialBalanceError> {
    let name = required_attribute(element, b"NAME", "trial_balance_name_missing")?;
    let mut guid = None;
    let mut parent = None;
    let mut opening = None;
    let mut debit = None;
    let mut credit = None;
    let mut closing = None;
    loop {
        match reader
            .read_event()
            .map_err(|_| NativeTrialBalanceError::InvalidResponse("trial_balance_xml_malformed"))?
        {
            Event::Start(child) => match child.name().as_ref().to_ascii_uppercase().as_slice() {
                b"GUID" => set_once(
                    &mut guid,
                    read_element_text(reader, child.name())?,
                    "trial_balance_duplicate_guid",
                )?,
                b"PARENT" => set_once(
                    &mut parent,
                    read_element_text(reader, child.name())?,
                    "trial_balance_duplicate_parent",
                )?,
                b"TBALOPENING" => set_once(
                    &mut opening,
                    parse_amount(&child, read_element_text(reader, child.name())?)?,
                    "trial_balance_duplicate_opening",
                )?,
                b"DEBITTOTALS" => set_once(
                    &mut debit,
                    parse_amount(&child, read_element_text(reader, child.name())?)?,
                    "trial_balance_duplicate_debit",
                )?,
                b"CREDITTOTALS" => set_once(
                    &mut credit,
                    parse_amount(&child, read_element_text(reader, child.name())?)?,
                    "trial_balance_duplicate_credit",
                )?,
                b"TBALCLOSING" => set_once(
                    &mut closing,
                    parse_amount(&child, read_element_text(reader, child.name())?)?,
                    "trial_balance_duplicate_closing",
                )?,
                _ => skip_subtree(reader)?,
            },
            Event::Empty(child) => match child.name().as_ref().to_ascii_uppercase().as_slice() {
                b"PARENT" => {
                    set_once(&mut parent, String::new(), "trial_balance_duplicate_parent")?
                }
                b"TBALOPENING" => set_once(
                    &mut opening,
                    parse_amount(&child, String::new())?,
                    "trial_balance_duplicate_opening",
                )?,
                b"DEBITTOTALS" => set_once(
                    &mut debit,
                    parse_amount(&child, String::new())?,
                    "trial_balance_duplicate_debit",
                )?,
                b"CREDITTOTALS" => set_once(
                    &mut credit,
                    parse_amount(&child, String::new())?,
                    "trial_balance_duplicate_credit",
                )?,
                b"TBALCLOSING" => set_once(
                    &mut closing,
                    parse_amount(&child, String::new())?,
                    "trial_balance_duplicate_closing",
                )?,
                _ => {}
            },
            Event::End(end) if end.name().as_ref().eq_ignore_ascii_case(b"LEDGER") => break,
            Event::Eof => {
                return Err(NativeTrialBalanceError::InvalidResponse(
                    "trial_balance_row_unterminated",
                ))
            }
            _ => {}
        }
    }
    let guid = guid.ok_or(NativeTrialBalanceError::InvalidResponse(
        "trial_balance_guid_missing",
    ))?;
    if !native_ledger_guid_has_company_prefix(&guid, expected_company_guid) {
        return Err(NativeTrialBalanceError::InvalidResponse(
            "trial_balance_company_guid_mismatch",
        ));
    }
    let row = NativeTrialBalanceRow {
        name,
        guid,
        parent: parent
            .map(PartyLedgerMasterFieldObservation::Returned)
            .unwrap_or_default(),
        opening: opening.ok_or(NativeTrialBalanceError::InvalidResponse(
            "trial_balance_opening_missing",
        ))?,
        debit: debit.ok_or(NativeTrialBalanceError::InvalidResponse(
            "trial_balance_debit_missing",
        ))?,
        credit: credit.ok_or(NativeTrialBalanceError::InvalidResponse(
            "trial_balance_credit_missing",
        ))?,
        closing: closing.ok_or(NativeTrialBalanceError::InvalidResponse(
            "trial_balance_closing_missing",
        ))?,
    };
    validate_guid_suffix(&row.guid, expected_company_guid)?;
    validate_observed_row_equation(&row)?;
    Ok(row)
}

fn validate_guid_suffix(
    guid: &str,
    expected_company_guid: &str,
) -> Result<(), NativeTrialBalanceError> {
    let suffix = guid
        .get(expected_company_guid.len().saturating_add(1)..)
        .ok_or(NativeTrialBalanceError::InvalidResponse(
            "trial_balance_guid_suffix_invalid",
        ))?;
    if suffix.len() != 8 || !suffix.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(NativeTrialBalanceError::InvalidResponse(
            "trial_balance_guid_suffix_invalid",
        ));
    }
    Ok(())
}

/// A fully observed native row must satisfy Tally's signed movement identity.
/// Present-empty fields deliberately bypass this check: their numeric value is
/// not established and must never be substituted with zero.
fn validate_observed_row_equation(
    row: &NativeTrialBalanceRow,
) -> Result<(), NativeTrialBalanceError> {
    let (
        NativeTrialBalanceAmount::Present(opening),
        NativeTrialBalanceAmount::Present(debit),
        NativeTrialBalanceAmount::Present(credit),
        NativeTrialBalanceAmount::Present(closing),
    ) = (&row.opening, &row.debit, &row.credit, &row.closing)
    else {
        return Ok(());
    };
    let movement = opening
        .checked_add(debit)
        .and_then(|sum| sum.checked_add(credit))
        .map_err(|_| NativeTrialBalanceError::InvalidResponse("trial_balance_amount_overflow"))?;
    if !movement.numeric_eq(closing) {
        return Err(NativeTrialBalanceError::InvalidResponse(
            "trial_balance_row_equation_mismatch",
        ));
    }
    Ok(())
}

fn parse_amount(
    element: &BytesStart<'_>,
    value: String,
) -> Result<NativeTrialBalanceAmount, NativeTrialBalanceError> {
    if required_attribute(element, b"TYPE", "trial_balance_amount_type_missing")? != "Amount" {
        return Err(NativeTrialBalanceError::InvalidResponse(
            "trial_balance_amount_type_invalid",
        ));
    }
    if value.is_empty() {
        Ok(NativeTrialBalanceAmount::PresentEmpty)
    } else {
        ExactDecimal::parse(value)
            .map(NativeTrialBalanceAmount::Present)
            .map_err(|_| NativeTrialBalanceError::InvalidAmount)
    }
}

fn set_once<T>(
    slot: &mut Option<T>,
    value: T,
    duplicate: &'static str,
) -> Result<(), NativeTrialBalanceError> {
    if slot.replace(value).is_some() {
        Err(NativeTrialBalanceError::InvalidResponse(duplicate))
    } else {
        Ok(())
    }
}

fn required_attribute(
    element: &BytesStart<'_>,
    key: &[u8],
    missing: &'static str,
) -> Result<String, NativeTrialBalanceError> {
    let mut found = None;
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|_| {
            NativeTrialBalanceError::InvalidResponse("trial_balance_attribute_malformed")
        })?;
        if !attribute.key.as_ref().eq_ignore_ascii_case(key) {
            continue;
        }
        if found.is_some() {
            return Err(NativeTrialBalanceError::InvalidResponse(
                "trial_balance_attribute_duplicate",
            ));
        }
        let value = attribute
            .normalized_value(quick_xml::XmlVersion::Implicit1_0)
            .map_err(|_| {
                NativeTrialBalanceError::InvalidResponse("trial_balance_attribute_malformed")
            })?
            .into_owned();
        if value.is_empty() {
            return Err(NativeTrialBalanceError::InvalidResponse(missing));
        }
        found = Some(value);
    }
    found.ok_or(NativeTrialBalanceError::InvalidResponse(missing))
}

fn read_element_text(
    reader: &mut Reader<&[u8]>,
    name: QName<'_>,
) -> Result<String, NativeTrialBalanceError> {
    let mut value = String::new();
    loop {
        match reader
            .read_event()
            .map_err(|_| NativeTrialBalanceError::InvalidResponse("trial_balance_xml_malformed"))?
        {
            Event::Text(text) => {
                let decoded = text.decode().map_err(|_| {
                    NativeTrialBalanceError::InvalidResponse("trial_balance_xml_invalid_encoding")
                })?;
                let unescaped = quick_xml::escape::unescape(&decoded).map_err(|_| {
                    NativeTrialBalanceError::InvalidResponse("trial_balance_xml_invalid_escape")
                })?;
                value.push_str(&unescaped);
            }
            Event::GeneralRef(reference) => {
                let decoded = reference.decode().map_err(|_| {
                    NativeTrialBalanceError::InvalidResponse("trial_balance_xml_invalid_encoding")
                })?;
                match decoded.as_ref() {
                    "amp" => value.push('&'),
                    "lt" => value.push('<'),
                    "gt" => value.push('>'),
                    "quot" => value.push('"'),
                    "apos" => value.push('\''),
                    _ => {
                        return Err(NativeTrialBalanceError::InvalidResponse(
                            "trial_balance_scalar_general_reference_invalid",
                        ))
                    }
                }
            }
            Event::End(end) if end.name() == name => return Ok(value),
            Event::Start(_) | Event::Empty(_) | Event::CData(_) | Event::Comment(_) => {
                return Err(NativeTrialBalanceError::InvalidResponse(
                    "trial_balance_scalar_not_text_only",
                ))
            }
            Event::Eof => {
                return Err(NativeTrialBalanceError::InvalidResponse(
                    "trial_balance_scalar_unterminated",
                ))
            }
            _ => {
                return Err(NativeTrialBalanceError::InvalidResponse(
                    "trial_balance_scalar_not_text_only",
                ))
            }
        }
    }
}

fn skip_subtree(reader: &mut Reader<&[u8]>) -> Result<(), NativeTrialBalanceError> {
    let mut depth = 1_u32;
    loop {
        match reader
            .read_event()
            .map_err(|_| NativeTrialBalanceError::InvalidResponse("trial_balance_xml_malformed"))?
        {
            Event::Start(_) => depth += 1,
            Event::End(_) => {
                depth -= 1;
                if depth == 0 {
                    return Ok(());
                }
            }
            Event::Eof => {
                return Err(NativeTrialBalanceError::InvalidResponse(
                    "trial_balance_subtree_unterminated",
                ))
            }
            _ => {}
        }
    }
}

fn path_is(path: &[Vec<u8>], expected: &[&[u8]]) -> bool {
    path.len() == expected.len()
        && path
            .iter()
            .zip(expected)
            .all(|(part, expected)| part.as_slice() == *expected)
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
