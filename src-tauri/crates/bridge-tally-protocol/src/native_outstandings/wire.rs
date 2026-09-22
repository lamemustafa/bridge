//! Parsers for the two native response shapes.
//!
//! Both grammars were measured live against TallyPrime (TALLY_PROTOCOL_REFERENCE
//! ground truth captured 2026-08-07) and are documented in this crate's
//! `native_outstandings` module:
//!
//! 1. Bills Receivable/Payable is FLAT: a `<BILLFIXED>` element is followed
//!    by SIBLING `<BILLCL>`, `<BILLDUE>`, `<BILLOVERDUE>` elements directly
//!    under `<ENVELOPE>`, in document order, with no wrapping row element.
//!    Verification is INVERTED: success carries no `<STATUS>` anywhere; a
//!    `<STATUS>` element only ever appears on failure. An empty result is a
//!    bare `<ENVELOPE></ENVELOPE>` and is legitimate zero-row success.
//! 2. The Ledger collection is an ordinary Collection response: it DOES
//!    carry `<STATUS>1</STATUS>` on success, and its rows live only under
//!    `ENVELOPE/BODY/DATA/COLLECTION`. The response also carries a
//!    `CMPINFO` block with bare counter elements sharing row tag names
//!    (`<LEDGER>0</LEDGER>`) — only the `DATA` section may be scanned for
//!    rows, or those counters are misread as ledgers.

use std::collections::HashSet;

use quick_xml::events::{BytesStart, Event};
use quick_xml::name::QName;
use quick_xml::Reader;
use sha2::{Digest, Sha256};

use bridge_tally_primitives::ExactDecimal;

use crate::tolerant_xml::{
    sanitize_invalid_numeric_references, sanitize_invalid_numeric_references_with_provenance,
};
use crate::{PartyLedgerMasterFieldObservation, TallyNamedMaster};

use super::date::{parse_native_display_date, NativeDisplayDateRole};
use super::model::{LedgerSnapshotEntry, NativeBillRow, NativeOutstandingsError};

struct PendingBillRow {
    party: String,
    reference: String,
    bill_date_raw: String,
    closing_balance: Option<ExactDecimal>,
    due_date_raw: Option<String>,
    overdue_seen: bool,
    overdue: Option<i64>,
}

/// Parses the flat Bills Receivable/Payable response into fully resolved
/// rows. The pinned book window resolves their two-digit display dates (see
/// [`super::date::parse_native_display_date`]).
pub fn parse_native_bill_rows(
    xml: &str,
    books_from: &bridge_tally_primitives::TallyDate,
    as_of: &bridge_tally_primitives::TallyDate,
) -> Result<Vec<NativeBillRow>, NativeOutstandingsError> {
    let sanitized = sanitize_invalid_numeric_references(xml);
    let mut reader = Reader::from_str(&sanitized);
    reader.config_mut().trim_text(true);

    let mut root_seen = false;
    let mut envelope_closed = false;
    let mut pending = Vec::<PendingBillRow>::new();

    loop {
        let event = reader
            .read_event()
            .map_err(|_| NativeOutstandingsError::InvalidResponse("bills_xml_malformed"))?;
        match event {
            Event::Start(element) => {
                let name = element.name().as_ref().to_ascii_uppercase();
                if !root_seen {
                    if name != b"ENVELOPE" {
                        return Err(NativeOutstandingsError::InvalidResponse(
                            "bills_root_not_envelope",
                        ));
                    }
                    root_seen = true;
                    continue;
                }
                if envelope_closed {
                    return Err(NativeOutstandingsError::InvalidResponse(
                        "bills_trailing_content",
                    ));
                }
                match name.as_slice() {
                    // The inverted rule: presence of STATUS anywhere in this
                    // report shape means Tally reported failure, regardless
                    // of the value carried.
                    b"STATUS" => return Err(NativeOutstandingsError::TallyReportedFailure),
                    b"BILLFIXED" => {
                        let (party, reference, bill_date_raw) = parse_bill_fixed(&mut reader)?;
                        pending.push(PendingBillRow {
                            party,
                            reference,
                            bill_date_raw,
                            closing_balance: None,
                            due_date_raw: None,
                            overdue_seen: false,
                            overdue: None,
                        });
                    }
                    b"BILLCL" => {
                        let text = read_element_text(&mut reader, element.name())?;
                        let row =
                            pending
                                .last_mut()
                                .ok_or(NativeOutstandingsError::InvalidResponse(
                                    "bills_scalar_before_fixed",
                                ))?;
                        if row.closing_balance.is_some() {
                            return Err(NativeOutstandingsError::InvalidResponse(
                                "bills_duplicate_billcl",
                            ));
                        }
                        row.closing_balance = Some(
                            ExactDecimal::parse(text.trim())
                                .map_err(|_| NativeOutstandingsError::InvalidAmount)?,
                        );
                    }
                    b"BILLDUE" => {
                        let text = read_element_text(&mut reader, element.name())?;
                        let row =
                            pending
                                .last_mut()
                                .ok_or(NativeOutstandingsError::InvalidResponse(
                                    "bills_scalar_before_fixed",
                                ))?;
                        if row.due_date_raw.is_some() {
                            return Err(NativeOutstandingsError::InvalidResponse(
                                "bills_duplicate_billdue",
                            ));
                        }
                        row.due_date_raw = Some(text);
                    }
                    b"BILLOVERDUE" => {
                        let text = read_element_text(&mut reader, element.name())?;
                        let row =
                            pending
                                .last_mut()
                                .ok_or(NativeOutstandingsError::InvalidResponse(
                                    "bills_scalar_before_fixed",
                                ))?;
                        set_bill_overdue(row, &text)?;
                    }
                    _ => {
                        return Err(NativeOutstandingsError::InvalidResponse(
                            "bills_unexpected_element",
                        ))
                    }
                }
            }
            Event::Empty(element) => {
                let name = element.name().as_ref().to_ascii_uppercase();
                if !root_seen {
                    return Err(NativeOutstandingsError::InvalidResponse(
                        "bills_root_not_envelope",
                    ));
                }
                if name.as_slice() == b"STATUS" {
                    return Err(NativeOutstandingsError::TallyReportedFailure);
                }
                if name.as_slice() == b"BILLOVERDUE" {
                    let row =
                        pending
                            .last_mut()
                            .ok_or(NativeOutstandingsError::InvalidResponse(
                                "bills_scalar_before_fixed",
                            ))?;
                    set_bill_overdue(row, "")?;
                    continue;
                }
                return Err(NativeOutstandingsError::InvalidResponse(
                    "bills_unexpected_empty_element",
                ));
            }
            Event::End(element) => {
                if root_seen
                    && !envelope_closed
                    && element.name().as_ref().eq_ignore_ascii_case(b"ENVELOPE")
                {
                    envelope_closed = true;
                    continue;
                }
                return Err(NativeOutstandingsError::InvalidResponse(
                    "bills_unexpected_close",
                ));
            }
            Event::Text(text) => {
                let is_blank = text
                    .decode()
                    .map(|value| value.trim().is_empty())
                    .unwrap_or(false);
                if !is_blank {
                    return Err(NativeOutstandingsError::InvalidResponse(
                        "bills_unexpected_text",
                    ));
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if !envelope_closed {
        return Err(NativeOutstandingsError::InvalidResponse(
            "bills_envelope_unterminated",
        ));
    }

    pending
        .into_iter()
        .map(|row| finalize_bill_row(row, books_from, as_of))
        .collect()
}

fn set_bill_overdue(row: &mut PendingBillRow, text: &str) -> Result<(), NativeOutstandingsError> {
    if row.overdue_seen {
        return Err(NativeOutstandingsError::InvalidResponse(
            "bills_duplicate_billoverdue",
        ));
    }
    row.overdue_seen = true;
    let text = text.trim();
    row.overdue = if text.is_empty() {
        None
    } else {
        Some(
            text.parse::<i64>()
                .map_err(|_| NativeOutstandingsError::InvalidResponse("bills_overdue_invalid"))?,
        )
    };
    Ok(())
}

fn finalize_bill_row(
    row: PendingBillRow,
    books_from: &bridge_tally_primitives::TallyDate,
    as_of: &bridge_tally_primitives::TallyDate,
) -> Result<NativeBillRow, NativeOutstandingsError> {
    let closing_balance = row
        .closing_balance
        .ok_or(NativeOutstandingsError::InvalidResponse(
            "bills_fixed_row_missing_billcl",
        ))?;
    let due_date_raw = row
        .due_date_raw
        .ok_or(NativeOutstandingsError::InvalidResponse(
            "bills_fixed_row_missing_billdue",
        ))?;
    if !row.overdue_seen {
        return Err(NativeOutstandingsError::InvalidResponse(
            "bills_fixed_row_missing_billoverdue",
        ));
    }
    let bill_date = parse_native_display_date(
        &row.bill_date_raw,
        books_from,
        as_of,
        NativeDisplayDateRole::BillDate,
    )?;
    let due_date = parse_native_display_date(
        &due_date_raw,
        books_from,
        as_of,
        NativeDisplayDateRole::DueDate,
    )?;
    Ok(NativeBillRow {
        party: row.party,
        reference: row.reference,
        bill_date,
        due_date,
        closing_balance,
        tally_overdue_days: row.overdue,
    })
}

fn parse_bill_fixed(
    reader: &mut Reader<&[u8]>,
) -> Result<(String, String, String), NativeOutstandingsError> {
    let mut bill_date = None;
    let mut reference = None;
    let mut party = None;
    loop {
        match reader
            .read_event()
            .map_err(|_| NativeOutstandingsError::InvalidResponse("bills_xml_malformed"))?
        {
            Event::Start(child) => {
                let child_name = child.name().as_ref().to_ascii_uppercase();
                let text = read_element_text(reader, child.name())?;
                match child_name.as_slice() {
                    b"BILLDATE" => {
                        set_once(&mut bill_date, text, "bills_fixed_duplicate_billdate")?
                    }
                    b"BILLREF" => set_once(&mut reference, text, "bills_fixed_duplicate_billref")?,
                    b"BILLPARTY" => set_once(&mut party, text, "bills_fixed_duplicate_billparty")?,
                    _ => {
                        return Err(NativeOutstandingsError::InvalidResponse(
                            "bills_fixed_unexpected_field",
                        ))
                    }
                }
            }
            Event::End(end) if end.name().as_ref().eq_ignore_ascii_case(b"BILLFIXED") => break,
            Event::Empty(child) => {
                let code = if child.name().as_ref().eq_ignore_ascii_case(b"BILLPARTY") {
                    "bills_fixed_empty_billparty"
                } else {
                    "bills_fixed_field_empty"
                };
                return Err(NativeOutstandingsError::InvalidResponse(code));
            }
            Event::Eof => {
                return Err(NativeOutstandingsError::InvalidResponse(
                    "bills_fixed_unterminated",
                ))
            }
            _ => {}
        }
    }
    let party = party.ok_or(NativeOutstandingsError::InvalidResponse(
        "bills_fixed_missing_billparty",
    ))?;
    if party.trim().is_empty() {
        return Err(NativeOutstandingsError::InvalidResponse(
            "bills_fixed_empty_billparty",
        ));
    }
    Ok((
        party,
        reference.ok_or(NativeOutstandingsError::InvalidResponse(
            "bills_fixed_missing_billref",
        ))?,
        bill_date.ok_or(NativeOutstandingsError::InvalidResponse(
            "bills_fixed_missing_billdate",
        ))?,
    ))
}

fn set_once(
    slot: &mut Option<String>,
    value: String,
    duplicate_code: &'static str,
) -> Result<(), NativeOutstandingsError> {
    if slot.replace(value).is_some() {
        return Err(NativeOutstandingsError::InvalidResponse(duplicate_code));
    }
    Ok(())
}

/// Parses the `List of Ledgers` collection response, scoping rows strictly
/// to `ENVELOPE/BODY/DATA/COLLECTION` so the `CMPINFO` bare-counter trap
/// (`<LEDGER>0</LEDGER>` inside `DESC/CMPINFO`) cannot be misread as rows.
/// Parses a ledger opening balance, treating an **empty** element as zero.
///
/// The shipped Outstandings path has established this narrow interpretation
/// for opening balances. A closing balance uses the separate parser below so
/// callers can distinguish an empty element from an established numeric zero.
fn parse_ledger_amount(text: &str) -> Result<ExactDecimal, NativeOutstandingsError> {
    if text.is_empty() {
        return Ok(ExactDecimal::zero());
    }
    ExactDecimal::parse(text).map_err(|_| NativeOutstandingsError::InvalidAmount)
}

fn parse_ledger_closing_balance(
    text: &str,
    ledger_name: &str,
) -> Result<Option<ExactDecimal>, NativeOutstandingsError> {
    if text.is_empty() {
        return Ok(None);
    }
    ExactDecimal::parse(text).map(Some).map_err(|_| {
        if is_foreign_currency_balance(text) {
            NativeOutstandingsError::ForeignCurrencyLedgerBalance {
                ledger_name: ledger_name.to_string(),
            }
        } else {
            NativeOutstandingsError::InvalidAmount
        }
    })
}

/// A foreign-currency ledger balance is a display expression, not a decimal:
/// `<qualified amount> @ <qualified rate> = <qualified base amount>`. Keep
/// this structural so the diagnostic does not depend on a particular symbol.
fn is_foreign_currency_balance(text: &str) -> bool {
    let mut parts = text.split('@');
    let Some(foreign_amount) = parts.next() else {
        return false;
    };
    let Some(rate_and_base) = parts.next() else {
        return false;
    };
    if parts.next().is_some() {
        return false;
    }
    let mut rate_parts = rate_and_base.split('=');
    let Some(rate) = rate_parts.next() else {
        return false;
    };
    let Some(base_amount) = rate_parts.next() else {
        return false;
    };
    rate_parts.next().is_none()
        && is_currency_qualified_numeric(foreign_amount)
        && is_currency_qualified_numeric(rate)
        && is_currency_qualified_numeric(base_amount)
}

fn is_currency_qualified_numeric(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty()
        && value.chars().any(|character| character.is_ascii_digit())
        && value.chars().any(|character| {
            !character.is_ascii_digit()
                && !matches!(character, '+' | '-' | '.' | ',' | '/' | ' ' | '\t')
        })
}

pub fn parse_native_ledger_snapshot(
    xml: &str,
) -> Result<Vec<LedgerSnapshotEntry>, NativeOutstandingsError> {
    Ok(parse_native_ledger_snapshot_rows(xml)?
        .into_iter()
        .map(|row| row.entry)
        .collect())
}

/// Parses a native ledger snapshot only when Tally's collection-level compute
/// proves that the response came from the selected company. Row GUIDs identify
/// ledger objects and can legitimately retain an imported company's prefix;
/// they are not evidence of which company answered this request.
pub fn parse_native_ledger_snapshot_for_company(
    xml: &str,
    expected_company_guid: &str,
) -> Result<Vec<LedgerSnapshotEntry>, NativeOutstandingsError> {
    let entries = parse_native_ledger_snapshot_rows(xml)?;
    for row in &entries {
        let response_company_guid = row.response_company_guid.as_deref().ok_or(
            NativeOutstandingsError::InvalidResponse("ledger_response_company_guid_missing"),
        )?;
        if !response_company_guid.eq_ignore_ascii_case(expected_company_guid) {
            return Err(NativeOutstandingsError::InvalidResponse(
                "ledger_response_company_guid_mismatch",
            ));
        }
    }
    if entries.is_empty() {
        return Err(NativeOutstandingsError::InvalidResponse(
            "ledger_response_company_guid_missing",
        ));
    }
    Ok(entries.into_iter().map(|row| row.entry).collect())
}

fn parse_native_ledger_snapshot_rows(
    xml: &str,
) -> Result<Vec<ParsedLedgerSnapshotRow>, NativeOutstandingsError> {
    let sanitized = sanitize_invalid_numeric_references(xml);
    let mut reader = Reader::from_str(&sanitized);
    reader.config_mut().trim_text(true);

    let mut path = Vec::<Vec<u8>>::new();
    let mut status_seen = false;
    let mut collection_seen = false;
    let mut entries = Vec::new();

    loop {
        let event = reader
            .read_event()
            .map_err(|_| NativeOutstandingsError::InvalidResponse("ledger_xml_malformed"))?;
        match event {
            Event::Start(element) => {
                let name = element.name().as_ref().to_ascii_uppercase();
                if path.is_empty() && name != b"ENVELOPE" {
                    return Err(NativeOutstandingsError::InvalidResponse(
                        "ledger_root_not_envelope",
                    ));
                }
                if path_is(&path, &[b"ENVELOPE", b"HEADER"]) && name == b"STATUS" {
                    let text = read_element_text(&mut reader, element.name())?;
                    if text.trim() != "1" {
                        return Err(NativeOutstandingsError::TallyReportedFailure);
                    }
                    status_seen = true;
                    continue;
                }
                if path_is(&path, &[b"ENVELOPE", b"BODY", b"DATA"]) && name == b"COLLECTION" {
                    collection_seen = true;
                }
                if path_is(&path, &[b"ENVELOPE", b"BODY", b"DATA", b"COLLECTION"])
                    && name == b"LEDGER"
                {
                    entries.push(parse_ledger_row(&mut reader, &element)?);
                    continue;
                }
                path.push(name);
            }
            Event::Empty(element) => {
                let name = element.name().as_ref().to_ascii_uppercase();
                if path_is(&path, &[b"ENVELOPE", b"BODY", b"DATA"]) && name == b"COLLECTION" {
                    collection_seen = true;
                    continue;
                }
                if path_is(&path, &[b"ENVELOPE", b"BODY", b"DATA", b"COLLECTION"])
                    && name == b"LEDGER"
                {
                    return Err(NativeOutstandingsError::InvalidResponse("ledger_row_empty"));
                }
            }
            Event::End(element) => {
                let name = element.name().as_ref().to_ascii_uppercase();
                let expected = path.pop().ok_or(NativeOutstandingsError::InvalidResponse(
                    "ledger_unexpected_close",
                ))?;
                if expected != name {
                    return Err(NativeOutstandingsError::InvalidResponse(
                        "ledger_unexpected_close",
                    ));
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if !path.is_empty() {
        return Err(NativeOutstandingsError::InvalidResponse(
            "ledger_envelope_unterminated",
        ));
    }
    if !status_seen {
        return Err(NativeOutstandingsError::TallyReportedFailure);
    }
    if !collection_seen {
        return Err(NativeOutstandingsError::InvalidResponse(
            "ledger_collection_missing",
        ));
    }
    Ok(entries)
}

/// Parses the `List of Groups` collection used to resolve nested party
/// ledgers. As with the ledger collection, only rows under
/// `ENVELOPE/BODY/DATA/COLLECTION` are accepted; `CMPINFO` counters are not
/// group rows. The native family carries no legacy completeness counter: its
/// completeness is established by the caller's paired byte-identical reads.
///
/// The request computes `BRIDGECOMPANYGUID` from `##SVCurrentCompany` and
/// this parser requires every returned row to carry that exact selected
/// company GUID. Tally can silently substitute a different loaded company
/// rather than erroring; neither the ambient GUID-verified extent reads nor
/// a row object's own GUID bind this specific response. Imported masters may
/// retain foreign row GUIDs, so using those row identities for response
/// identity would reject legitimate data and fail to detect substitution.
pub fn parse_native_group_snapshot(
    xml: &str,
    expected_company_guid: &str,
) -> Result<Vec<TallyNamedMaster>, NativeOutstandingsError> {
    Ok(
        parse_native_group_snapshot_with_evidence(xml, expected_company_guid)?
            .into_iter()
            .map(|entry| entry.record)
            .collect(),
    )
}

/// One native Group collection row plus the hash of the exact row bytes
/// consumed by the parser. Native Group rows expose no durable record ID, so
/// callers must keep this evidence distinct from an observed GUID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeGroupSnapshotEntry {
    pub record: TallyNamedMaster,
    pub raw_source_sha256: String,
}

/// Parses the native Group collection while retaining exact row evidence for
/// callers that persist the collection in a canonical snapshot.
///
/// `expected_company_guid` binds every response row through the request's
/// `BRIDGECOMPANYGUID` computed value. Missing or nonmatching values are
/// refused with typed `InvalidResponse` codes; a Group row can therefore not
/// enter a workbook source without response-company identity being considered.
pub fn parse_native_group_snapshot_with_evidence(
    xml: &str,
    expected_company_guid: &str,
) -> Result<Vec<NativeGroupSnapshotEntry>, NativeOutstandingsError> {
    let sanitized = sanitize_invalid_numeric_references_with_provenance(xml);
    let mut reader = Reader::from_str(sanitized.as_str());
    reader.config_mut().trim_text(true);

    let mut path = Vec::<Vec<u8>>::new();
    let mut status_seen = false;
    let mut collection_seen = false;
    let mut entries = Vec::new();
    loop {
        let event_start = reader.buffer_position() as usize;
        let event = reader
            .read_event()
            .map_err(|_| NativeOutstandingsError::InvalidResponse("group_xml_malformed"))?;
        match event {
            Event::Start(element) => {
                let name = element.name().as_ref().to_ascii_uppercase();
                if path.is_empty() && name != b"ENVELOPE" {
                    return Err(NativeOutstandingsError::InvalidResponse(
                        "group_root_not_envelope",
                    ));
                }
                if path_is(&path, &[b"ENVELOPE", b"HEADER"]) && name == b"STATUS" {
                    let text = read_element_text(&mut reader, element.name())?;
                    if text != "1" {
                        return Err(NativeOutstandingsError::TallyReportedFailure);
                    }
                    status_seen = true;
                    continue;
                }
                if path_is(&path, &[b"ENVELOPE", b"BODY", b"DATA"]) && name == b"COLLECTION" {
                    collection_seen = true;
                }
                if path_is(&path, &[b"ENVELOPE", b"BODY", b"DATA", b"COLLECTION"])
                    && name == b"GROUP"
                {
                    let row = parse_group_row(&mut reader, &element)?;
                    let response_company_guid = row.response_company_guid.as_deref().ok_or(
                        NativeOutstandingsError::InvalidResponse(
                            "group_response_company_guid_missing",
                        ),
                    )?;
                    if !response_company_guid.eq_ignore_ascii_case(expected_company_guid) {
                        return Err(NativeOutstandingsError::InvalidResponse(
                            "group_response_company_guid_mismatch",
                        ));
                    }
                    let record_end = reader.buffer_position() as usize;
                    entries.push(NativeGroupSnapshotEntry {
                        record: row.record,
                        raw_source_sha256: sha256_hex(
                            sanitized
                                .original_fragment(event_start, record_end)
                                .map_err(|_| {
                                    NativeOutstandingsError::InvalidResponse(
                                        "group_row_boundaries_invalid",
                                    )
                                })?,
                        ),
                    });
                    continue;
                }
                path.push(name);
            }
            Event::Empty(element) => {
                let name = element.name().as_ref().to_ascii_uppercase();
                if path_is(&path, &[b"ENVELOPE", b"BODY", b"DATA"]) && name == b"COLLECTION" {
                    collection_seen = true;
                } else if path_is(&path, &[b"ENVELOPE", b"BODY", b"DATA", b"COLLECTION"])
                    && name == b"GROUP"
                {
                    return Err(NativeOutstandingsError::InvalidResponse("group_row_empty"));
                }
            }
            Event::End(element) => {
                let name = element.name().as_ref().to_ascii_uppercase();
                let expected = path.pop().ok_or(NativeOutstandingsError::InvalidResponse(
                    "group_unexpected_close",
                ))?;
                if expected != name {
                    return Err(NativeOutstandingsError::InvalidResponse(
                        "group_unexpected_close",
                    ));
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if !path.is_empty() {
        return Err(NativeOutstandingsError::InvalidResponse(
            "group_envelope_unterminated",
        ));
    }
    if !status_seen {
        return Err(NativeOutstandingsError::TallyReportedFailure);
    }
    if !collection_seen {
        return Err(NativeOutstandingsError::InvalidResponse(
            "group_collection_missing",
        ));
    }
    if entries.is_empty() {
        return Err(NativeOutstandingsError::InvalidResponse(
            "group_response_company_guid_missing",
        ));
    }
    Ok(entries)
}

struct ParsedNativeGroupSnapshotRow {
    record: TallyNamedMaster,
    response_company_guid: Option<String>,
}

/// Parses one `GROUP` row and keeps the request-computed responder identity
/// separate from the Group object's own GUID.
fn parse_group_row(
    reader: &mut Reader<&[u8]>,
    element: &BytesStart<'_>,
) -> Result<ParsedNativeGroupSnapshotRow, NativeOutstandingsError> {
    validate_row_attributes(element, "group_row_malformed_attributes")?;
    let name = attribute_value(element, b"NAME").ok_or(
        NativeOutstandingsError::InvalidResponse("group_name_missing"),
    )?;
    // Unlike `attribute_value`, this keeps "present but empty" distinct from
    // "absent entirely" -- RESERVEDNAME's empty string is itself a fact
    // (Tally's own signal that the row is user-created), not the absence of
    // one. See `TallyNamedMaster::reserved_name` and
    // `crate::group_ancestry::GroupIndex::reserved_ancestor`, which climbs
    // through the empty case and refuses the absent one.
    let reserved_name = raw_attribute_value(element, b"RESERVEDNAME");
    let mut parent = None;
    let mut parent_seen = false;
    let mut response_company_guid = None;
    let mut response_company_guid_seen = false;
    loop {
        match reader
            .read_event()
            .map_err(|_| NativeOutstandingsError::InvalidResponse("group_xml_malformed"))?
        {
            Event::Start(child) if child.name().as_ref().eq_ignore_ascii_case(b"PARENT") => {
                // Verbatim: this names another group row, and the hop is
                // matched by exact codepoint. See
                // `read_element_identifier_text`.
                let value = read_element_identifier_text(reader, child.name())?;
                if std::mem::replace(&mut parent_seen, true) {
                    return Err(NativeOutstandingsError::InvalidResponse(
                        "group_duplicate_parent",
                    ));
                }
                parent = (!value.trim().is_empty()).then_some(value);
            }
            Event::Start(child)
                if child
                    .name()
                    .as_ref()
                    .eq_ignore_ascii_case(b"BRIDGECOMPANYGUID") =>
            {
                let value = read_element_text(reader, child.name())?;
                if std::mem::replace(&mut response_company_guid_seen, true) {
                    return Err(NativeOutstandingsError::InvalidResponse(
                        "group_duplicate_response_company_guid",
                    ));
                }
                response_company_guid = (!value.is_empty()).then_some(value);
            }
            Event::Start(_) => skip_subtree(reader)?,
            Event::Empty(child) if child.name().as_ref().eq_ignore_ascii_case(b"PARENT") => {
                if std::mem::replace(&mut parent_seen, true) {
                    return Err(NativeOutstandingsError::InvalidResponse(
                        "group_duplicate_parent",
                    ));
                }
            }
            Event::Empty(child)
                if child
                    .name()
                    .as_ref()
                    .eq_ignore_ascii_case(b"BRIDGECOMPANYGUID") =>
            {
                if std::mem::replace(&mut response_company_guid_seen, true) {
                    return Err(NativeOutstandingsError::InvalidResponse(
                        "group_duplicate_response_company_guid",
                    ));
                }
            }
            Event::Empty(_) => {}
            Event::End(end) if end.name().as_ref().eq_ignore_ascii_case(b"GROUP") => break,
            Event::Eof => {
                return Err(NativeOutstandingsError::InvalidResponse(
                    "group_row_unterminated",
                ))
            }
            _ => {}
        }
    }
    if !parent_seen {
        return Err(NativeOutstandingsError::InvalidResponse(
            "group_parent_missing",
        ));
    }
    Ok(ParsedNativeGroupSnapshotRow {
        record: TallyNamedMaster {
            name,
            parent: PartyLedgerMasterFieldObservation::Returned(parent.unwrap_or_default()),
            reserved_name,
        },
        response_company_guid,
    })
}

fn sha256_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        output.push(HEX[usize::from(byte >> 4)] as char);
        output.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    output
}

fn parse_ledger_row(
    reader: &mut Reader<&[u8]>,
    element: &BytesStart<'_>,
) -> Result<ParsedLedgerSnapshotRow, NativeOutstandingsError> {
    validate_row_attributes(element, "ledger_row_malformed_attributes")?;
    let name = attribute_value(element, b"NAME").ok_or(
        NativeOutstandingsError::InvalidResponse("ledger_name_missing"),
    )?;
    let mut parent = None;
    // The outer option records whether the element was present; the inner one
    // retains Tally's observed empty-element state rather than inventing zero.
    let mut closing_balance = None;
    let mut opening_balance = None;
    let mut bill_wise_on = None;
    let mut response_company_guid = None;
    let mut response_company_guid_seen = false;
    loop {
        match reader
            .read_event()
            .map_err(|_| NativeOutstandingsError::InvalidResponse("ledger_xml_malformed"))?
        {
            Event::Start(child) => {
                let child_name = child.name().as_ref().to_ascii_uppercase();
                match child_name.as_slice() {
                    b"PARENT" => {
                        // Verbatim: this names a group row, and the hop is
                        // matched by exact codepoint. See
                        // `read_element_identifier_text`.
                        let text = read_element_identifier_text(reader, child.name())?;
                        if parent.is_some() {
                            return Err(NativeOutstandingsError::InvalidResponse(
                                "ledger_duplicate_parent",
                            ));
                        }
                        parent = Some((!text.trim().is_empty()).then_some(text));
                    }
                    b"CLOSINGBALANCE" => {
                        let text = read_element_text(reader, child.name())?;
                        if closing_balance.is_some() {
                            return Err(NativeOutstandingsError::InvalidResponse(
                                "ledger_duplicate_closing_balance",
                            ));
                        }
                        closing_balance = Some(parse_ledger_closing_balance(text.trim(), &name)?);
                    }
                    b"OPENINGBALANCE" => {
                        let text = read_element_text(reader, child.name())?;
                        if opening_balance.is_some() {
                            return Err(NativeOutstandingsError::InvalidResponse(
                                "ledger_duplicate_opening_balance",
                            ));
                        }
                        opening_balance = Some(parse_ledger_amount(text.trim())?);
                    }
                    b"ISBILLWISEON" => {
                        let text = read_element_text(reader, child.name())?;
                        if bill_wise_on.is_some() {
                            return Err(NativeOutstandingsError::InvalidResponse(
                                "ledger_duplicate_bill_wise_flag",
                            ));
                        }
                        bill_wise_on = Some(parse_tally_boolean(&text)?);
                    }
                    b"BRIDGECOMPANYGUID" => {
                        let text = read_element_text(reader, child.name())?;
                        if std::mem::replace(&mut response_company_guid_seen, true) {
                            return Err(NativeOutstandingsError::InvalidResponse(
                                "ledger_duplicate_response_company_guid",
                            ));
                        }
                        response_company_guid = (!text.is_empty()).then_some(text);
                    }
                    _ => skip_subtree(reader)?,
                }
            }
            Event::Empty(child)
                if child
                    .name()
                    .as_ref()
                    .eq_ignore_ascii_case(b"BRIDGECOMPANYGUID") =>
            {
                if std::mem::replace(&mut response_company_guid_seen, true) {
                    return Err(NativeOutstandingsError::InvalidResponse(
                        "ledger_duplicate_response_company_guid",
                    ));
                }
            }
            Event::Empty(_) => {}
            Event::End(end) if end.name().as_ref().eq_ignore_ascii_case(b"LEDGER") => break,
            Event::Eof => {
                return Err(NativeOutstandingsError::InvalidResponse(
                    "ledger_row_unterminated",
                ))
            }
            _ => {}
        }
    }
    Ok(ParsedLedgerSnapshotRow {
        entry: LedgerSnapshotEntry {
            name,
            parent: parent.flatten(),
            closing_balance: closing_balance.ok_or(NativeOutstandingsError::InvalidResponse(
                "ledger_closing_balance_missing",
            ))?,
            opening_balance: opening_balance.ok_or(NativeOutstandingsError::InvalidResponse(
                "ledger_opening_balance_missing",
            ))?,
            bill_wise_on: bill_wise_on.ok_or(NativeOutstandingsError::InvalidResponse(
                "ledger_bill_wise_flag_missing",
            ))?,
        },
        response_company_guid,
    })
}

struct ParsedLedgerSnapshotRow {
    entry: LedgerSnapshotEntry,
    response_company_guid: Option<String>,
}

fn skip_subtree(reader: &mut Reader<&[u8]>) -> Result<(), NativeOutstandingsError> {
    let mut depth = 1_u32;
    loop {
        match reader
            .read_event()
            .map_err(|_| NativeOutstandingsError::InvalidResponse("ledger_xml_malformed"))?
        {
            Event::Start(_) => depth += 1,
            Event::End(_) => {
                depth -= 1;
                if depth == 0 {
                    return Ok(());
                }
            }
            Event::Eof => {
                return Err(NativeOutstandingsError::InvalidResponse(
                    "ledger_subtree_unterminated",
                ))
            }
            _ => {}
        }
    }
}

fn parse_tally_boolean(value: &str) -> Result<bool, NativeOutstandingsError> {
    if value.eq_ignore_ascii_case("yes") || value.eq_ignore_ascii_case("true") || value == "1" {
        Ok(true)
    } else if value.eq_ignore_ascii_case("no")
        || value.eq_ignore_ascii_case("false")
        || value == "0"
    {
        Ok(false)
    } else {
        Err(NativeOutstandingsError::InvalidResponse(
            "ledger_bill_wise_flag_invalid",
        ))
    }
}

/// Validates that a row element's attributes are structurally sound before
/// any of them is read: no attribute name repeated (case-insensitively) and
/// every value decodable. `.attributes()` on its own -- as `attribute_value`
/// and `raw_attribute_value` below use it -- silently discards `Err` items
/// via `.flatten()`, and quick-xml 0.41 yields `Err(AttrError::Duplicated)`
/// for a repeated attribute name rather than refusing to iterate, so a row
/// with e.g. two `RESERVEDNAME` attributes would otherwise parse cleanly and
/// quietly keep the first. This deliberately does NOT allowlist which
/// attributes may appear: this parser sits on the shipped outstandings read
/// path, and refusing a response merely because Tally added a new attribute
/// would be an availability regression. Duplicates and undecodable values are
/// a different matter -- they are evidence quick-xml itself already flagged.
fn validate_row_attributes(
    element: &BytesStart<'_>,
    invalid_response_code: &'static str,
) -> Result<(), NativeOutstandingsError> {
    let mut seen = HashSet::new();
    for attribute in element.attributes().with_checks(true) {
        let attribute = attribute
            .map_err(|_| NativeOutstandingsError::InvalidResponse(invalid_response_code))?;
        if !seen.insert(attribute.key.as_ref().to_ascii_lowercase()) {
            return Err(NativeOutstandingsError::InvalidResponse(
                invalid_response_code,
            ));
        }
        attribute
            .normalized_value(quick_xml::XmlVersion::Implicit1_0)
            .map_err(|_| NativeOutstandingsError::InvalidResponse(invalid_response_code))?;
    }
    Ok(())
}

fn attribute_value(element: &BytesStart<'_>, key: &[u8]) -> Option<String> {
    element
        .attributes()
        .flatten()
        .find(|attribute| attribute.key.as_ref().eq_ignore_ascii_case(key))
        .and_then(|attribute| {
            attribute
                .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                .ok()
        })
        .map(|value| value.into_owned())
        .filter(|value| !value.trim().is_empty())
}

/// Like [`attribute_value`], but returns the attribute's raw value verbatim
/// -- including an empty string -- instead of folding "empty" into `None`.
/// `attribute_value` exists for identity attributes (like `NAME`) that must
/// never legitimately be empty, so treating an empty value as absent is
/// correct there. `RESERVEDNAME` is different: an empty value is itself
/// meaningful (Tally's own "this group is user-created" signal), and must
/// stay distinguishable from the attribute never having been sent at all.
fn raw_attribute_value(element: &BytesStart<'_>, key: &[u8]) -> Option<String> {
    element
        .attributes()
        .flatten()
        .find(|attribute| attribute.key.as_ref().eq_ignore_ascii_case(key))
        .and_then(|attribute| {
            attribute
                .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                .ok()
        })
        .map(|value| value.into_owned())
}

fn path_is(path: &[Vec<u8>], expected: &[&[u8]]) -> bool {
    path.len() == expected.len()
        && path
            .iter()
            .zip(expected)
            .all(|(segment, name)| segment.as_slice() == *name)
}

/// Reads an element's text **without normalising it**, for values that are
/// foreign references to a master `NAME` rather than data to be interpreted.
///
/// Tally matches master names by exact codepoint, so a group `PARENT` that
/// differs from the group `NAME` it refers to is an incoherent pair, not a
/// spelling variant. Trimming it resolves that pair against the unpadded group
/// and classifies a ledger on evidence that does not hold — and it does so
/// upstream of the ancestry walk built to refuse exactly that, where the walk
/// cannot see it. Emptiness is still judged on the trimmed view; only the
/// retained value is verbatim.
fn read_element_identifier_text(
    reader: &mut Reader<&[u8]>,
    name: QName<'_>,
) -> Result<String, NativeOutstandingsError> {
    let raw = reader
        .read_text(name)
        .map_err(|_| NativeOutstandingsError::InvalidResponse("native_xml_malformed"))?;
    let decoded = raw
        .decode()
        .map_err(|_| NativeOutstandingsError::InvalidResponse("native_xml_invalid_encoding"))?;
    let unescaped = quick_xml::escape::unescape(&decoded)
        .map_err(|_| NativeOutstandingsError::InvalidResponse("native_xml_invalid_escape"))?;
    Ok(unescaped.into_owned())
}

fn read_element_text(
    reader: &mut Reader<&[u8]>,
    name: QName<'_>,
) -> Result<String, NativeOutstandingsError> {
    Ok(read_element_identifier_text(reader, name)?
        .trim()
        .to_string())
}

use super::model::{CompanyCurrency, CurrencyMaster};

/// Parses the company currency collection.
///
/// Ordinary (non-inverted) `STATUS` applies here -- this is a `Collection`
/// request, not one of the flat `Data` reports. Rows are read only from
/// `<DATA>`, because the same `CMPINFO` counter block that inflates a naive
/// ledger scan also carries a bare `<CURRENCY>0</CURRENCY>`.
pub fn parse_company_currency(xml: &str) -> Result<CompanyCurrency, NativeOutstandingsError> {
    let sanitized = sanitize_invalid_numeric_references(xml);
    let mut reader = Reader::from_str(&sanitized);
    reader.config_mut().trim_text(true);
    let mut path = Vec::<Vec<u8>>::new();
    let mut status_seen = false;
    let mut collection_seen = false;
    let mut rows = Vec::new();
    loop {
        let event = reader
            .read_event()
            .map_err(|_| NativeOutstandingsError::InvalidResponse("currency_xml_malformed"))?;
        match event {
            Event::Start(element) => {
                let name = element.name().as_ref().to_ascii_uppercase();
                if path.is_empty() && name != b"ENVELOPE" {
                    return Err(NativeOutstandingsError::InvalidResponse(
                        "currency_root_not_envelope",
                    ));
                }
                if path_is(&path, &[b"ENVELOPE", b"HEADER"]) && name == b"STATUS" {
                    let text = read_element_text(&mut reader, element.name())?;
                    if text.trim() != "1" {
                        return Err(NativeOutstandingsError::TallyReportedFailure);
                    }
                    status_seen = true;
                    continue;
                }
                if path_is(&path, &[b"ENVELOPE", b"BODY", b"DATA"]) && name == b"COLLECTION" {
                    collection_seen = true;
                }
                if path_is(&path, &[b"ENVELOPE", b"BODY", b"DATA", b"COLLECTION"])
                    && name == b"CURRENCY"
                {
                    rows.push(parse_currency_row(&mut reader, &element)?);
                    continue;
                }
                path.push(name);
            }
            Event::Empty(element) => {
                let name = element.name().as_ref().to_ascii_uppercase();
                if path_is(&path, &[b"ENVELOPE", b"BODY", b"DATA"]) && name == b"COLLECTION" {
                    collection_seen = true;
                } else if path_is(&path, &[b"ENVELOPE", b"BODY", b"DATA", b"COLLECTION"])
                    && name == b"CURRENCY"
                {
                    return Err(NativeOutstandingsError::InvalidResponse(
                        "currency_row_empty",
                    ));
                }
            }
            Event::End(element) => {
                let expected = path.pop().ok_or(NativeOutstandingsError::InvalidResponse(
                    "currency_unexpected_close",
                ))?;
                if expected != element.name().as_ref().to_ascii_uppercase() {
                    return Err(NativeOutstandingsError::InvalidResponse(
                        "currency_unexpected_close",
                    ));
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if !path.is_empty() {
        return Err(NativeOutstandingsError::InvalidResponse(
            "currency_envelope_unterminated",
        ));
    }
    if !status_seen {
        return Err(NativeOutstandingsError::TallyReportedFailure);
    }
    if !collection_seen {
        return Err(NativeOutstandingsError::InvalidResponse(
            "currency_collection_missing",
        ));
    }

    // Only a single defined currency names the BASE currency by itself; with
    // several, the caller identifies it from the company's CURRENCYNAME.
    Ok(CompanyCurrency::from_masters(rows))
}

fn parse_currency_row(
    reader: &mut Reader<&[u8]>,
    element: &BytesStart<'_>,
) -> Result<CurrencyMaster, NativeOutstandingsError> {
    validate_row_attributes(element, "currency_row_malformed_attributes")?;
    let symbol = attribute_value(element, b"NAME").ok_or(
        NativeOutstandingsError::InvalidResponse("currency_name_missing"),
    )?;
    let mut mailing_name = None;
    let mut decimal_places = None;
    let mut original_name = None;
    loop {
        match reader
            .read_event()
            .map_err(|_| NativeOutstandingsError::InvalidResponse("currency_xml_malformed"))?
        {
            Event::Start(child) if child.name().as_ref().eq_ignore_ascii_case(b"ORIGINALNAME") => {
                let text = read_element_text(reader, child.name())?;
                if original_name.replace(text).is_some() {
                    return Err(NativeOutstandingsError::InvalidResponse(
                        "currency_duplicate_original_name",
                    ));
                }
            }
            Event::Start(child) if child.name().as_ref().eq_ignore_ascii_case(b"MAILINGNAME") => {
                let text = read_element_text(reader, child.name())?;
                if mailing_name.replace(text).is_some() {
                    return Err(NativeOutstandingsError::InvalidResponse(
                        "currency_duplicate_mailing_name",
                    ));
                }
            }
            Event::Start(child) if child.name().as_ref().eq_ignore_ascii_case(b"DECIMALPLACES") => {
                let text = read_element_text(reader, child.name())?;
                let parsed = text.parse::<u8>().map_err(|_| {
                    NativeOutstandingsError::InvalidResponse("currency_decimal_places_invalid")
                })?;
                if decimal_places.replace(parsed).is_some() {
                    return Err(NativeOutstandingsError::InvalidResponse(
                        "currency_duplicate_decimal_places",
                    ));
                }
            }
            Event::Start(_) => skip_subtree(reader)?,
            Event::Empty(child) if child.name().as_ref().eq_ignore_ascii_case(b"MAILINGNAME") => {
                if mailing_name.replace(String::new()).is_some() {
                    return Err(NativeOutstandingsError::InvalidResponse(
                        "currency_duplicate_mailing_name",
                    ));
                }
            }
            Event::Empty(child) if child.name().as_ref().eq_ignore_ascii_case(b"DECIMALPLACES") => {
                return Err(NativeOutstandingsError::InvalidResponse(
                    "currency_decimal_places_invalid",
                ));
            }
            Event::End(end) if end.name().as_ref().eq_ignore_ascii_case(b"CURRENCY") => break,
            Event::Eof => {
                return Err(NativeOutstandingsError::InvalidResponse(
                    "currency_row_unterminated",
                ))
            }
            _ => {}
        }
    }
    Ok(CurrencyMaster {
        name: symbol,
        original_name,
        mailing_name: mailing_name.unwrap_or_default(),
        decimal_places: decimal_places.ok_or(NativeOutstandingsError::InvalidResponse(
            "currency_decimal_places_missing",
        ))?,
    })
}

/// The `CURRENCYNAME` of the one company whose `GUID` is `company_guid`, from
/// the `Company` collection [`super::render_company_base_currency_request`]
/// renders. The collection lists every loaded company. Refuses when no row or
/// several rows carry the GUID, or when the chosen row's `CURRENCYNAME` is
/// missing or empty.
pub fn parse_company_currency_name(
    xml: &str,
    company_guid: &str,
) -> Result<String, NativeOutstandingsError> {
    let sanitized = sanitize_invalid_numeric_references(xml);
    let mut reader = Reader::from_str(&sanitized);
    reader.config_mut().trim_text(true);
    let mut path = Vec::<Vec<u8>>::new();
    let mut status_seen = false;
    let mut collection_seen = false;
    let mut chosen: Vec<Option<String>> = Vec::new();
    loop {
        let event = reader.read_event().map_err(|_| {
            NativeOutstandingsError::InvalidResponse("company_currency_xml_malformed")
        })?;
        match event {
            Event::Start(element) => {
                let name = element.name().as_ref().to_ascii_uppercase();
                if path.is_empty() && name != b"ENVELOPE" {
                    return Err(NativeOutstandingsError::InvalidResponse(
                        "company_currency_root_not_envelope",
                    ));
                }
                if path_is(&path, &[b"ENVELOPE", b"HEADER"]) && name == b"STATUS" {
                    let text = read_element_text(&mut reader, element.name())?;
                    if text.trim() != "1" {
                        return Err(NativeOutstandingsError::TallyReportedFailure);
                    }
                    status_seen = true;
                    continue;
                }
                if path_is(&path, &[b"ENVELOPE", b"BODY", b"DATA"]) && name == b"COLLECTION" {
                    collection_seen = true;
                }
                if path_is(&path, &[b"ENVELOPE", b"BODY", b"DATA", b"COLLECTION"])
                    && name == b"COMPANY"
                {
                    let (guid, currency_name) = parse_company_currency_row(&mut reader)?;
                    if guid.eq_ignore_ascii_case(company_guid) {
                        chosen.push(currency_name);
                    }
                    continue;
                }
                path.push(name);
            }
            Event::Empty(element) => {
                let name = element.name().as_ref().to_ascii_uppercase();
                if path_is(&path, &[b"ENVELOPE", b"BODY", b"DATA"]) && name == b"COLLECTION" {
                    collection_seen = true;
                }
            }
            Event::End(element) => {
                let expected = path.pop().ok_or(NativeOutstandingsError::InvalidResponse(
                    "company_currency_unexpected_close",
                ))?;
                if expected != element.name().as_ref().to_ascii_uppercase() {
                    return Err(NativeOutstandingsError::InvalidResponse(
                        "company_currency_unexpected_close",
                    ));
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if !path.is_empty() {
        return Err(NativeOutstandingsError::InvalidResponse(
            "company_currency_envelope_unterminated",
        ));
    }
    if !status_seen {
        return Err(NativeOutstandingsError::TallyReportedFailure);
    }
    if !collection_seen {
        return Err(NativeOutstandingsError::InvalidResponse(
            "company_currency_collection_missing",
        ));
    }
    match chosen.as_slice() {
        [Some(name)] if !name.is_empty() => Ok(name.clone()),
        [_] => Err(NativeOutstandingsError::InvalidResponse(
            "company_currency_name_missing",
        )),
        [] => Err(NativeOutstandingsError::InvalidResponse(
            "company_currency_row_missing",
        )),
        _ => Err(NativeOutstandingsError::InvalidResponse(
            "company_currency_row_ambiguous",
        )),
    }
}

/// One `COMPANY` row's `GUID` and `CURRENCYNAME`; other fields are skipped. A
/// field given twice is refused.
fn parse_company_currency_row(
    reader: &mut Reader<&[u8]>,
) -> Result<(String, Option<String>), NativeOutstandingsError> {
    let mut guid = None;
    let mut currency_name = None;
    loop {
        match reader.read_event().map_err(|_| {
            NativeOutstandingsError::InvalidResponse("company_currency_xml_malformed")
        })? {
            Event::Start(child) if child.name().as_ref().eq_ignore_ascii_case(b"GUID") => {
                let text = read_element_text(reader, child.name())?;
                if guid.replace(text).is_some() {
                    return Err(NativeOutstandingsError::InvalidResponse(
                        "company_currency_duplicate_guid",
                    ));
                }
            }
            Event::Start(child) if child.name().as_ref().eq_ignore_ascii_case(b"CURRENCYNAME") => {
                let text = read_element_text(reader, child.name())?;
                if currency_name.replace(text).is_some() {
                    return Err(NativeOutstandingsError::InvalidResponse(
                        "company_currency_duplicate_name",
                    ));
                }
            }
            Event::Start(_) => skip_subtree(reader)?,
            Event::Empty(child) if child.name().as_ref().eq_ignore_ascii_case(b"CURRENCYNAME") => {
                if currency_name.replace(String::new()).is_some() {
                    return Err(NativeOutstandingsError::InvalidResponse(
                        "company_currency_duplicate_name",
                    ));
                }
            }
            Event::End(end) if end.name().as_ref().eq_ignore_ascii_case(b"COMPANY") => break,
            Event::Eof => {
                return Err(NativeOutstandingsError::InvalidResponse(
                    "company_currency_row_unterminated",
                ))
            }
            _ => {}
        }
    }
    let guid = guid.ok_or(NativeOutstandingsError::InvalidResponse(
        "company_currency_guid_missing",
    ))?;
    Ok((guid, currency_name))
}

#[cfg(test)]
#[path = "wire_currency_tests.rs"]
mod currency_tests;

#[cfg(test)]
#[path = "wire_group_tests.rs"]
mod group_tests;
