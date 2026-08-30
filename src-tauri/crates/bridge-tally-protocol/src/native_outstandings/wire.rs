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
        .map(|(entry, _)| entry)
        .collect())
}

/// Parses a native ledger snapshot only when at least one row proves that the
/// response came from the selected company. The request explicitly fetches
/// `GUID`; an ambient extent bracket cannot bind the response it surrounds.
/// Imported masters may retain foreign GUIDs, so a foreign row is retained;
/// only an entire response with no expected-company prefix is refused.
pub fn parse_native_ledger_snapshot_for_company(
    xml: &str,
    expected_company_guid: &str,
) -> Result<Vec<LedgerSnapshotEntry>, NativeOutstandingsError> {
    let entries = parse_native_ledger_snapshot_rows(xml)?;
    if !entries.iter().any(|(_, guid)| {
        guid.as_deref().is_some_and(|guid| {
            crate::native_ledger_guid_has_company_prefix(guid, expected_company_guid)
        })
    }) {
        return Err(NativeOutstandingsError::InvalidResponse(
            "ledger_company_guid_unverified",
        ));
    }
    Ok(entries.into_iter().map(|(entry, _)| entry).collect())
}

fn parse_native_ledger_snapshot_rows(
    xml: &str,
) -> Result<Vec<(LedgerSnapshotEntry, Option<String>)>, NativeOutstandingsError> {
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
/// The collection has no report-envelope company identity either, so -- as
/// with [`crate::parse_native_group_source_records_with_evidence`] on the
/// core-window path -- at least one row's `GUID` must carry the requested
/// company's prefix or the snapshot is rejected outright. Tally is known to
/// silently substitute a different loaded company rather than erroring, and
/// this batch's ambient GUID-verified extent reads (see
/// `fetch_outstandings_native`) bracket the read but do not bind this
/// specific response. A foreign-prefixed row alongside a matching one is
/// still accepted and simply not counted as a match: a book can legitimately
/// hold masters imported with their original GUIDs.
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
/// `expected_company_guid` binds the response the way
/// [`crate::parse_native_group_source_records_with_evidence`] binds the
/// core-window read: at least one row's `GUID` must carry that prefix (see
/// `native_ledger_guid_has_company_prefix`) or the whole snapshot is refused
/// with [`NativeOutstandingsError::InvalidResponse`]`("group_company_guid_unverified")`.
/// Other prefixes remain counted, not rejected -- a book can legitimately
/// hold masters imported with their original GUIDs.
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
    let mut company_guid_prefix_match_count = 0_u64;
    let mut company_guid_prefix_mismatch_count = 0_u64;
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
                    let (record, guid) = parse_group_row(&mut reader, &element)?;
                    // A row with no GUID at all is neither a match nor a
                    // mismatch: it carries no evidence either way. Only a
                    // present GUID is scored against the expected prefix.
                    if let Some(guid) = guid.as_deref() {
                        if crate::native_ledger_guid_has_company_prefix(guid, expected_company_guid)
                        {
                            company_guid_prefix_match_count = company_guid_prefix_match_count
                                .checked_add(1)
                                .ok_or(NativeOutstandingsError::InvalidResponse(
                                    "group_company_guid_count_overflow",
                                ))?;
                        } else {
                            company_guid_prefix_mismatch_count = company_guid_prefix_mismatch_count
                                .checked_add(1)
                                .ok_or(NativeOutstandingsError::InvalidResponse(
                                    "group_company_guid_count_overflow",
                                ))?;
                        }
                    }
                    let record_end = reader.buffer_position() as usize;
                    entries.push(NativeGroupSnapshotEntry {
                        record,
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
    // Tally is known to silently substitute a different loaded company
    // rather than erroring. This batch's ambient GUID-verified extent reads
    // bracket the whole read but do not bind this specific response, so at
    // least one row must carry the expected company's GUID prefix -- the
    // same policy `parse_native_group_source_records_with_evidence` applies
    // on the core-window path. A foreign prefix on some rows is legitimate
    // (imported masters can retain their original GUIDs) and is merely
    // counted above, never individually rejected; only a snapshot with NO
    // matching row at all is refused.
    if company_guid_prefix_match_count == 0 {
        return Err(NativeOutstandingsError::InvalidResponse(
            "group_company_guid_unverified",
        ));
    }
    Ok(entries)
}

/// Parses one `GROUP` row, returning its name/parent plus its `GUID` when the
/// row carries one. The row's GUID is optional here (unlike the core-window
/// reader's mandatory-GUID rows): the caller scores it against the expected
/// company prefix but never rejects a row for omitting it outright.
fn parse_group_row(
    reader: &mut Reader<&[u8]>,
    element: &BytesStart<'_>,
) -> Result<(TallyNamedMaster, Option<String>), NativeOutstandingsError> {
    let name = attribute_value(element, b"NAME").ok_or(
        NativeOutstandingsError::InvalidResponse("group_name_missing"),
    )?;
    // Unlike `attribute_value`, this keeps "present but empty" distinct from
    // "absent entirely" -- RESERVEDNAME's empty string is itself a fact
    // (Tally's own signal that the row is user-created), not the absence of
    // one. See `TallyNamedMaster::reserved_name` and
    // `super::compute::group_identity_key` for how each state is used.
    let reserved_name = raw_attribute_value(element, b"RESERVEDNAME");
    let mut parent = None;
    let mut parent_seen = false;
    let mut guid = None;
    let mut guid_seen = false;
    loop {
        match reader
            .read_event()
            .map_err(|_| NativeOutstandingsError::InvalidResponse("group_xml_malformed"))?
        {
            Event::Start(child) if child.name().as_ref().eq_ignore_ascii_case(b"PARENT") => {
                let value = read_element_text(reader, child.name())?;
                if std::mem::replace(&mut parent_seen, true) {
                    return Err(NativeOutstandingsError::InvalidResponse(
                        "group_duplicate_parent",
                    ));
                }
                parent = (!value.is_empty()).then_some(value);
            }
            Event::Start(child) if child.name().as_ref().eq_ignore_ascii_case(b"GUID") => {
                let value = read_element_text(reader, child.name())?;
                if std::mem::replace(&mut guid_seen, true) {
                    return Err(NativeOutstandingsError::InvalidResponse(
                        "group_duplicate_guid",
                    ));
                }
                guid = (!value.is_empty()).then_some(value);
            }
            Event::Start(_) => skip_subtree(reader)?,
            Event::Empty(child) if child.name().as_ref().eq_ignore_ascii_case(b"PARENT") => {
                if std::mem::replace(&mut parent_seen, true) {
                    return Err(NativeOutstandingsError::InvalidResponse(
                        "group_duplicate_parent",
                    ));
                }
            }
            Event::Empty(child) if child.name().as_ref().eq_ignore_ascii_case(b"GUID") => {
                if std::mem::replace(&mut guid_seen, true) {
                    return Err(NativeOutstandingsError::InvalidResponse(
                        "group_duplicate_guid",
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
    Ok((
        TallyNamedMaster {
            name,
            parent: PartyLedgerMasterFieldObservation::Returned(parent.unwrap_or_default()),
            reserved_name,
        },
        guid,
    ))
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
) -> Result<(LedgerSnapshotEntry, Option<String>), NativeOutstandingsError> {
    let name = attribute_value(element, b"NAME").ok_or(
        NativeOutstandingsError::InvalidResponse("ledger_name_missing"),
    )?;
    let mut parent = None;
    // The outer option records whether the element was present; the inner one
    // retains Tally's observed empty-element state rather than inventing zero.
    let mut closing_balance = None;
    let mut opening_balance = None;
    let mut bill_wise_on = None;
    let mut guid = None;
    let mut guid_seen = false;
    loop {
        match reader
            .read_event()
            .map_err(|_| NativeOutstandingsError::InvalidResponse("ledger_xml_malformed"))?
        {
            Event::Start(child) => {
                let child_name = child.name().as_ref().to_ascii_uppercase();
                match child_name.as_slice() {
                    b"PARENT" => {
                        let text = read_element_text(reader, child.name())?;
                        if parent.is_some() {
                            return Err(NativeOutstandingsError::InvalidResponse(
                                "ledger_duplicate_parent",
                            ));
                        }
                        parent = Some((!text.is_empty()).then_some(text));
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
                    b"GUID" => {
                        let text = read_element_text(reader, child.name())?;
                        if std::mem::replace(&mut guid_seen, true) {
                            return Err(NativeOutstandingsError::InvalidResponse(
                                "ledger_duplicate_guid",
                            ));
                        }
                        guid = (!text.is_empty()).then_some(text);
                    }
                    _ => skip_subtree(reader)?,
                }
            }
            Event::Empty(child) if child.name().as_ref().eq_ignore_ascii_case(b"GUID") => {
                if std::mem::replace(&mut guid_seen, true) {
                    return Err(NativeOutstandingsError::InvalidResponse(
                        "ledger_duplicate_guid",
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
    Ok((
        LedgerSnapshotEntry {
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
        guid,
    ))
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

fn read_element_text(
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
    Ok(unescaped.trim().to_string())
}

use super::model::CompanyCurrency;

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

    let currency_count = rows.len();
    let CurrencyRow {
        symbol,
        mailing_name,
        decimal_places,
    } = rows.into_iter().next().unwrap_or_default();
    // Only a single defined currency lets this read name the BASE currency.
    // "Rs." is shared by several currencies, so only the observed Indian
    // mailing identity is authoritative enough to put ₹ before real money.
    let is_inr = currency_count == 1
        && (mailing_name.eq_ignore_ascii_case("Indian Rupees")
            || mailing_name.eq_ignore_ascii_case("INR"));

    Ok(CompanyCurrency {
        symbol,
        mailing_name,
        currency_count,
        decimal_places,
        is_inr,
    })
}

#[derive(Default)]
struct CurrencyRow {
    symbol: String,
    mailing_name: String,
    decimal_places: u8,
}

fn parse_currency_row(
    reader: &mut Reader<&[u8]>,
    element: &BytesStart<'_>,
) -> Result<CurrencyRow, NativeOutstandingsError> {
    let symbol = attribute_value(element, b"NAME").ok_or(
        NativeOutstandingsError::InvalidResponse("currency_name_missing"),
    )?;
    let mut mailing_name = None;
    let mut decimal_places = None;
    loop {
        match reader
            .read_event()
            .map_err(|_| NativeOutstandingsError::InvalidResponse("currency_xml_malformed"))?
        {
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
    Ok(CurrencyRow {
        symbol,
        mailing_name: mailing_name.unwrap_or_default(),
        decimal_places: decimal_places.ok_or(NativeOutstandingsError::InvalidResponse(
            "currency_decimal_places_missing",
        ))?,
    })
}

#[cfg(test)]
mod currency_tests {
    use super::*;

    const MODERN_LIVE: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/currency_inr_modern_live.utf16le.xml"
    ));
    const LEGACY_LIVE: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/currency_inr_legacy_live.utf16le.xml"
    ));
    const MULTI_LIVE: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/currency_multi_live.utf16le.xml"
    ));
    const FOREX_COMPOSITE_LIVE: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/ledgers_forex_composite_live.utf16le.xml"
    ));

    fn decode_utf16le(bytes: &[u8]) -> String {
        let (units, remainder) = bytes.as_chunks::<2>();
        assert!(
            remainder.is_empty(),
            "captured UTF-16LE must have whole units"
        );
        String::from_utf16(
            &units
                .iter()
                .map(|unit| u16::from_le_bytes(*unit))
                .collect::<Vec<_>>(),
        )
        .expect("captured UTF-16LE must decode")
    }

    #[test]
    fn captured_currency_collections_recognize_both_indian_spellings_without_guessing() {
        for (bytes, sha256, symbol, mailing_name, count, decimal_places, is_inr) in [
            (
                MODERN_LIVE,
                "0dc84aa287cab1e1922db7e99a01f9f2b0bacd0d777fdd0b080adedc6622ed22",
                "I₹",
                "INR",
                1,
                2,
                true,
            ),
            (
                LEGACY_LIVE,
                "dcc3539205080c4272b42d333b693e6c90e1cdd6b9e9e080d4ea6b8ae2abb06e",
                "Rs.",
                "Indian Rupees",
                1,
                2,
                true,
            ),
            (
                MULTI_LIVE,
                "b64c0d5feb528fa02f81de576de5c766a95e1da1000975b1e2932868ae34118b",
                "$",
                "USD",
                2,
                2,
                false,
            ),
        ] {
            assert_eq!(sha256_hex(bytes), sha256, "captured wire bytes changed");
            let currency = parse_company_currency(&decode_utf16le(bytes)).expect("parses");
            assert_eq!(currency.symbol, symbol);
            assert_eq!(currency.mailing_name, mailing_name);
            assert_eq!(currency.currency_count, count, "CMPINFO is not a row");
            assert_eq!(currency.decimal_places, decimal_places);
            assert_eq!(currency.is_inr, is_inr);
        }
    }

    #[test]
    fn several_currencies_cannot_name_the_base_currency() {
        let xml = decode_utf16le(LEGACY_LIVE).replace(
            "</COLLECTION>",
            r#"<CURRENCY NAME="$" RESERVEDNAME=""><MAILINGNAME TYPE="String">US Dollars</MAILINGNAME><DECIMALPLACES TYPE="Number">2</DECIMALPLACES></CURRENCY></COLLECTION>"#,
        );
        let currency = parse_company_currency(&xml).expect("parses");
        assert_eq!(currency.currency_count, 2);
        assert!(!currency.is_inr, "must fall back to asking, never guess");
    }

    #[test]
    fn a_non_indian_single_currency_is_not_inr() {
        // Constructed: no captured company has this single-currency shape.
        let xml = decode_utf16le(LEGACY_LIVE)
            .replace("Indian Rupees", "US Dollars")
            .replace(r#"NAME="Rs.""#, r#"NAME="$""#);
        let currency = parse_company_currency(&xml).expect("parses");
        assert!(!currency.is_inr);
    }

    #[test]
    fn failed_or_structurally_incomplete_currency_collections_fail_closed() {
        for xml in [
            "<ENVELOPE><HEADER><STATUS>0</STATUS></HEADER><BODY><DATA><LINEERROR>failed</LINEERROR></DATA></BODY></ENVELOPE>",
            "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA/></BODY></ENVELOPE>",
            "<not-xml",
        ] {
            assert!(parse_company_currency(xml).is_err(), "{xml}");
        }
    }

    #[test]
    fn currency_precision_is_required_and_typed_at_the_wire_boundary() {
        let missing = decode_utf16le(LEGACY_LIVE)
            .replace(r#"<DECIMALPLACES TYPE="Number"> 2</DECIMALPLACES>"#, "");
        assert_eq!(
            parse_company_currency(&missing),
            Err(NativeOutstandingsError::InvalidResponse(
                "currency_decimal_places_missing"
            ))
        );

        let invalid = decode_utf16le(LEGACY_LIVE).replace(
            r#"<DECIMALPLACES TYPE="Number"> 2</DECIMALPLACES>"#,
            r#"<DECIMALPLACES TYPE="Number">fractional</DECIMALPLACES>"#,
        );
        assert_eq!(
            parse_company_currency(&invalid),
            Err(NativeOutstandingsError::InvalidResponse(
                "currency_decimal_places_invalid"
            ))
        );
    }

    #[test]
    fn common_rs_symbol_does_not_prove_indian_rupees() {
        let xml = decode_utf16le(LEGACY_LIVE).replace("Indian Rupees", "Pakistani Rupees");
        let currency = parse_company_currency(&xml).expect("shaped collection parses");
        assert!(!currency.is_inr);
    }

    #[test]
    fn captured_forex_composite_closing_balance_names_the_ledger_without_parsing_it() {
        assert_eq!(
            sha256_hex(FOREX_COMPOSITE_LIVE),
            "4941f30826ec51da9ab1c834abb1abcd711ffec22464044d5c669b77aaa313f8",
            "captured wire bytes changed"
        );
        let xml = decode_utf16le(FOREX_COMPOSITE_LIVE);
        assert_eq!(xml.matches("<LEDGER NAME=").count(), 8);
        assert_eq!(xml.matches(" @ ").count(), 1);
        assert_eq!(
            parse_native_ledger_snapshot(&xml),
            Err(NativeOutstandingsError::ForeignCurrencyLedgerBalance {
                ledger_name: "FX USD Debtor 02".to_string(),
            })
        );
    }

    #[test]
    fn ledger_snapshot_retains_an_empty_closing_balance_distinct_from_zero() {
        let xml = "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>\
            <LEDGER NAME=\"Empty\"><PARENT>Sundry Debtors</PARENT><CLOSINGBALANCE></CLOSINGBALANCE>\
            <OPENINGBALANCE>0</OPENINGBALANCE><ISBILLWISEON>Yes</ISBILLWISEON></LEDGER>\
            <LEDGER NAME=\"Zero\"><PARENT>Sundry Debtors</PARENT><CLOSINGBALANCE>0</CLOSINGBALANCE>\
            <OPENINGBALANCE>0</OPENINGBALANCE><ISBILLWISEON>Yes</ISBILLWISEON></LEDGER>\
            </COLLECTION></DATA></BODY></ENVELOPE>";
        let rows =
            parse_native_ledger_snapshot(xml).expect("the observed empty element is valid XML");
        assert_eq!(rows[0].closing_balance, None);
        assert_eq!(rows[1].closing_balance, Some(ExactDecimal::zero()));
    }

    #[test]
    fn party_ledger_master_balance_response_with_only_foreign_company_guids_is_withheld() {
        let xml = "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>\
            <LEDGER NAME=\"Same ledger name\"><GUID>22222222-2222-2222-2222-222222222222-00000001</GUID>\
            <PARENT>Sundry Debtors</PARENT><CLOSINGBALANCE>-100.00</CLOSINGBALANCE>\
            <OPENINGBALANCE>-100.00</OPENINGBALANCE><ISBILLWISEON>Yes</ISBILLWISEON></LEDGER>\
            </COLLECTION></DATA></BODY></ENVELOPE>";

        assert_eq!(
            parse_native_ledger_snapshot_for_company(xml, "11111111-1111-1111-1111-111111111111"),
            Err(NativeOutstandingsError::InvalidResponse(
                "ledger_company_guid_unverified"
            ))
        );
        assert_eq!(
            parse_native_ledger_snapshot(xml).unwrap().len(),
            1,
            "ordinary snapshot consumers retain their documented, identity-neutral parser"
        );
        assert_eq!(
            parse_native_ledger_snapshot_for_company(xml, "22222222-2222-2222-2222-222222222222")
                .unwrap()
                .len(),
            1,
            "a matching row GUID binds the export response before it is joined"
        );
    }

    #[test]
    fn constructed_forex_composite_boundaries_remain_fail_closed() {
        let xml = decode_utf16le(FOREX_COMPOSITE_LIVE);
        // Constructed: the capture contains only the measured negative composite balance.
        let positive = xml.replacen(
            "-$ 2000.00 @ I₹ 84/$  = -I₹ 168000.00",
            "$ 2000.00 @ I₹ 84/$  = I₹ 168000.00",
            1,
        );
        assert!(matches!(
            parse_native_ledger_snapshot(&positive),
            Err(NativeOutstandingsError::ForeignCurrencyLedgerBalance { ledger_name })
                if ledger_name == "FX USD Debtor 02"
        ));

        // Constructed: a near-miss without a base-currency tail is invalid, not foreign currency.
        let malformed = xml.replacen(" @ I₹ 84/$  = ", " @ I₹ 84/$ ", 1);
        assert_eq!(
            parse_native_ledger_snapshot(&malformed),
            Err(NativeOutstandingsError::InvalidAmount)
        );
    }
}

#[cfg(test)]
mod group_tests {
    use super::*;

    const COMPANY_GUID: &str = "11111111-1111-1111-1111-111111111111";
    const FOREIGN_GUID: &str = "22222222-2222-2222-2222-222222222222";

    const LIVE_SHAPE: &str = r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DESC><CMPINFO><GROUP>0</GROUP></CMPINFO></DESC><DATA><COLLECTION><GROUP NAME="North Region"><GUID>11111111-1111-1111-1111-111111111111-00000001</GUID><PARENT>Current Assets</PARENT></GROUP><GROUP NAME="Sundry Debtors"><GUID>11111111-1111-1111-1111-111111111111-00000002</GUID><PARENT>&#4; Primary</PARENT></GROUP></COLLECTION></DATA></BODY></ENVELOPE>"#;

    #[test]
    fn reads_group_rows_only_from_the_native_collection() {
        let groups = parse_native_group_snapshot(LIVE_SHAPE, COMPANY_GUID)
            .expect("native group snapshot parses");
        assert_eq!(groups.len(), 2, "CMPINFO group counter is not a row");
        assert_eq!(groups[0].name, "North Region");
        assert_eq!(groups[0].parent.returned_text(), Some("Current Assets"));
        assert_eq!(
            groups[1].parent.returned_text(),
            Some("\u{fffd}#4; Primary")
        );

        let evidence = parse_native_group_snapshot_with_evidence(LIVE_SHAPE, COMPANY_GUID)
            .expect("native group snapshot evidence parses");
        assert_eq!(evidence.len(), 2);
        assert_eq!(evidence[0].record, groups[0]);
        assert_eq!(evidence[0].raw_source_sha256.len(), 64);
        assert_ne!(evidence[0].raw_source_sha256, evidence[1].raw_source_sha256);
    }

    #[test]
    fn missing_group_parent_fails_closed() {
        let xml = LIVE_SHAPE.replace("<PARENT>Current Assets</PARENT>", "");
        assert_eq!(
            parse_native_group_snapshot(&xml, COMPANY_GUID),
            Err(NativeOutstandingsError::InvalidResponse(
                "group_parent_missing"
            ))
        );
    }

    #[test]
    fn group_evidence_hashes_the_unsanitised_wire_fragment() {
        let decimal = parse_native_group_snapshot_with_evidence(LIVE_SHAPE, COMPANY_GUID)
            .expect("decimal illegal reference remains parseable");
        let hexadecimal_xml = LIVE_SHAPE.replace("&#4;", "&#x4;");
        let hexadecimal = parse_native_group_snapshot_with_evidence(&hexadecimal_xml, COMPANY_GUID)
            .expect("hexadecimal illegal reference remains parseable");

        let decimal_fragment = b"<GROUP NAME=\"Sundry Debtors\"><GUID>11111111-1111-1111-1111-111111111111-00000002</GUID><PARENT>&#4; Primary</PARENT></GROUP>";
        let hexadecimal_fragment = b"<GROUP NAME=\"Sundry Debtors\"><GUID>11111111-1111-1111-1111-111111111111-00000002</GUID><PARENT>&#x4; Primary</PARENT></GROUP>";
        assert_eq!(decimal[1].raw_source_sha256, sha256_hex(decimal_fragment));
        assert_eq!(
            hexadecimal[1].raw_source_sha256,
            sha256_hex(hexadecimal_fragment)
        );
        assert_ne!(
            decimal[1].raw_source_sha256,
            hexadecimal[1].raw_source_sha256
        );
    }

    /// CONFIRMED P1 (PR #158 code review): the group parser used to accept
    /// any response regardless of which company it actually came from. A
    /// response whose rows all carry a different company's GUID prefix must
    /// now be rejected outright, not silently trusted.
    #[test]
    fn group_response_carrying_only_a_foreign_company_guid_is_rejected() {
        let xml = LIVE_SHAPE
            .replace(
                "11111111-1111-1111-1111-111111111111-00000001",
                "22222222-2222-2222-2222-222222222222-00000001",
            )
            .replace(
                "11111111-1111-1111-1111-111111111111-00000002",
                "22222222-2222-2222-2222-222222222222-00000002",
            );
        assert!(xml.contains(FOREIGN_GUID), "sanity: replacement took hold");
        assert_eq!(
            parse_native_group_snapshot(&xml, COMPANY_GUID),
            Err(NativeOutstandingsError::InvalidResponse(
                "group_company_guid_unverified"
            ))
        );
    }

    /// A response with the correct prefix continues to parse names and
    /// parents exactly as before the fix.
    #[test]
    fn group_response_with_the_correct_prefix_is_accepted_and_still_parses() {
        let groups = parse_native_group_snapshot(LIVE_SHAPE, COMPANY_GUID)
            .expect("a correctly-prefixed response is accepted");
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].name, "North Region");
        assert_eq!(groups[0].parent.returned_text(), Some("Current Assets"));
        assert_eq!(groups[1].name, "Sundry Debtors");
    }

    /// A book can legitimately hold masters imported with their original
    /// GUIDs. A mostly-correct response with one foreign-prefixed row must
    /// still be accepted, with that row retained rather than dropped.
    #[test]
    fn mixed_prefix_response_is_accepted_with_the_foreign_row_retained_and_counted() {
        let xml = LIVE_SHAPE.replace(
            "11111111-1111-1111-1111-111111111111-00000002",
            "22222222-2222-2222-2222-222222222222-00000002",
        );
        let groups = parse_native_group_snapshot(&xml, COMPANY_GUID)
            .expect("one correctly-prefixed row is enough to accept the whole snapshot");
        assert_eq!(
            groups.len(),
            2,
            "the foreign-prefix row is retained, not rejected"
        );
        assert_eq!(groups[1].name, "Sundry Debtors");
    }

    /// A row that omits GUID entirely (the pre-fix wire shape, before the
    /// request asked for GUID/MASTERID/ALTERID) is simply not evidence
    /// either way -- it is neither a match nor a mismatch. The snapshot is
    /// still accepted as long as some other row does carry the expected
    /// prefix.
    #[test]
    fn row_omitting_guid_entirely_is_not_scored_but_does_not_block_acceptance() {
        let xml = LIVE_SHAPE.replace(
            "<GUID>11111111-1111-1111-1111-111111111111-00000001</GUID>",
            "",
        );
        let groups = parse_native_group_snapshot(&xml, COMPANY_GUID)
            .expect("the second row's correct prefix is enough to bind the response");
        assert_eq!(groups.len(), 2);
    }

    /// If every row omits GUID entirely, there is no evidence at all binding
    /// the response to the expected company, so it is rejected -- the same
    /// outcome as a response carrying only foreign prefixes.
    ///
    /// This mutation-based test is kept alongside
    /// `a_real_pre_widening_capture_with_no_guid_anywhere_is_rejected` below:
    /// it shares `LIVE_SHAPE`/`COMPANY_GUID` with the single-row omission
    /// and mismatch tests in this module, forming a matched family that
    /// isolates exactly one row-count/GUID variable at a time in a way a
    /// fixed real capture cannot. The real capture below proves the same
    /// rejection against actual TallyPrime bytes, not just a hand-mutated
    /// shape.
    #[test]
    fn a_response_where_every_row_omits_guid_is_rejected() {
        let xml = LIVE_SHAPE
            .replace(
                "<GUID>11111111-1111-1111-1111-111111111111-00000001</GUID>",
                "",
            )
            .replace(
                "<GUID>11111111-1111-1111-1111-111111111111-00000002</GUID>",
                "",
            );
        assert_eq!(
            parse_native_group_snapshot(&xml, COMPANY_GUID),
            Err(NativeOutstandingsError::InvalidResponse(
                "group_company_guid_unverified"
            ))
        );
    }

    /// `group_snapshot_aarav.xml` is a real TallyPrime response, captured
    /// live before the native Group request was widened to fetch
    /// `GUID, MASTERID, ALTERID` (see `render_native_group_snapshot_request`).
    /// All 28 of its rows genuinely omit `GUID` -- not by mutation, but
    /// because the request never asked for it -- which makes this capture
    /// the real-bytes instance of exactly the case
    /// `a_response_where_every_row_omits_guid_is_rejected` constructs
    /// synthetically above: a group snapshot with no row identity anywhere
    /// cannot bind to any company and must be rejected outright. Real
    /// captured bytes are worth more than constructed XML.
    #[test]
    fn a_real_pre_widening_capture_with_no_guid_anywhere_is_rejected() {
        let xml = include_str!("../../tests/fixtures/native/group_snapshot_aarav.xml");
        assert_eq!(
            parse_native_group_snapshot(xml, "bb8ad19e-6aef-4239-a917-87fec0c6215e"),
            Err(NativeOutstandingsError::InvalidResponse(
                "group_company_guid_unverified"
            ))
        );
    }

    /// `RESERVEDNAME` parsing must keep three states distinct: a real
    /// (non-empty) predefined identity, Tally's own explicit empty-string
    /// "this is user-created" signal, and the attribute being absent
    /// entirely (an older capture, or a build that omits it). Folding the
    /// empty-string case into "absent" would let a custom group merely named
    /// like a predefined one pass as the identity fallback -- see
    /// `native_outstandings::compute::group_identity_key` for how the
    /// distinction is used.
    #[test]
    fn reserved_name_attribute_parsing_distinguishes_present_empty_and_absent() {
        let xml = r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><GROUP NAME="WR5 Renamed Suspense" RESERVEDNAME="Sundry Debtors"><GUID>11111111-1111-1111-1111-111111111111-00000001</GUID><PARENT>Primary</PARENT></GROUP><GROUP NAME="Sundry Debtors" RESERVEDNAME=""><GUID>11111111-1111-1111-1111-111111111111-00000002</GUID><PARENT>Primary</PARENT></GROUP><GROUP NAME="Old Capture Group"><GUID>11111111-1111-1111-1111-111111111111-00000003</GUID><PARENT>Primary</PARENT></GROUP></COLLECTION></DATA></BODY></ENVELOPE>"#;
        let groups = parse_native_group_snapshot(xml, COMPANY_GUID).expect("parses");
        assert_eq!(
            groups[0].reserved_name.as_deref(),
            Some("Sundry Debtors"),
            "a renamed predefined group keeps its immutable RESERVEDNAME identity"
        );
        assert_eq!(
            groups[1].reserved_name.as_deref(),
            Some(""),
            "an empty RESERVEDNAME is Tally's own signal, not a missing value"
        );
        assert_eq!(
            groups[2].reserved_name, None,
            "a row that never carried the attribute at all must stay None, not empty-string"
        );
    }
}
