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

use bridge_tally_primitives::ExactDecimal;

use crate::tolerant_xml::sanitize_invalid_numeric_references;

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
            Event::Empty(_) => {
                return Err(NativeOutstandingsError::InvalidResponse(
                    "bills_fixed_field_empty",
                ))
            }
            Event::Eof => {
                return Err(NativeOutstandingsError::InvalidResponse(
                    "bills_fixed_unterminated",
                ))
            }
            _ => {}
        }
    }
    Ok((
        party.ok_or(NativeOutstandingsError::InvalidResponse(
            "bills_fixed_missing_billparty",
        ))?,
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
/// Parses a ledger balance, treating an **empty** element as zero.
///
/// Tally emits `<CLOSINGBALANCE></CLOSINGBALANCE>` -- entirely empty, not
/// `"0"` -- for a ledger whose balance is nil. Measured 2026-08-07: 16 of the
/// 88 ledgers on the bulk demo book do this, while the small bill-wise lab
/// book has none, so a parser validated only against the latter rejects every
/// realistic book with `InvalidAmount` and takes the whole read down with it.
///
/// Only a genuinely empty value is accepted as zero. Anything else that fails
/// to parse is still an error: this is a narrow allowance for an observed
/// encoding of zero, not a lenient number parser.
fn parse_ledger_amount(text: &str) -> Result<ExactDecimal, NativeOutstandingsError> {
    if text.is_empty() {
        return Ok(ExactDecimal::zero());
    }
    ExactDecimal::parse(text).map_err(|_| NativeOutstandingsError::InvalidAmount)
}

pub fn parse_native_ledger_snapshot(
    xml: &str,
) -> Result<Vec<LedgerSnapshotEntry>, NativeOutstandingsError> {
    let sanitized = sanitize_invalid_numeric_references(xml);
    let mut reader = Reader::from_str(&sanitized);
    reader.config_mut().trim_text(true);

    let mut path = Vec::<Vec<u8>>::new();
    let mut status_seen = false;
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
    Ok(entries)
}

fn parse_ledger_row(
    reader: &mut Reader<&[u8]>,
    element: &BytesStart<'_>,
) -> Result<LedgerSnapshotEntry, NativeOutstandingsError> {
    let name = attribute_value(element, b"NAME").ok_or(
        NativeOutstandingsError::InvalidResponse("ledger_name_missing"),
    )?;
    let mut parent = None;
    let mut closing_balance = None;
    let mut opening_balance = None;
    let mut bill_wise_on = None;
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
                        closing_balance = Some(parse_ledger_amount(text.trim())?);
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
                    _ => skip_subtree(reader)?,
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
    Ok(LedgerSnapshotEntry {
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
    })
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
    let data = xml
        .find("<DATA>")
        .and_then(|start| {
            xml[start..]
                .find("</DATA>")
                .map(|end| &xml[start + 6..start + end])
        })
        .unwrap_or("");

    let mut rows = Vec::new();
    let mut rest = data;
    while let Some(start) = rest.find("<CURRENCY ") {
        let after = &rest[start..];
        let Some(end) = after.find("</CURRENCY>") else {
            break;
        };
        let block = &after[..end];
        let name = currency_attribute(block, "NAME").unwrap_or_default();
        let mailing = currency_element_text(block, "MAILINGNAME").unwrap_or_default();
        if !name.trim().is_empty() {
            rows.push((name.trim().to_string(), mailing.trim().to_string()));
        }
        rest = &after[end + "</CURRENCY>".len()..];
    }

    let currency_count = rows.len();
    let (symbol, mailing_name) = rows.into_iter().next().unwrap_or_default();
    // Only a single defined currency lets this read name the BASE currency.
    // Tally's Indian symbol is the literal "Rs."; the mailing name is the
    // durable signal, so both are checked.
    let is_inr = currency_count == 1
        && (mailing_name.eq_ignore_ascii_case("Indian Rupees")
            || symbol == "Rs."
            || symbol == "₹"
            || symbol.eq_ignore_ascii_case("INR"));

    Ok(CompanyCurrency {
        symbol,
        mailing_name,
        currency_count,
        is_inr,
    })
}

fn currency_attribute(block: &str, name: &str) -> Option<String> {
    let needle = format!("{name}=\"");
    let start = block.find(&needle)? + needle.len();
    let end = block[start..].find('"')? + start;
    Some(block[start..end].to_string())
}

fn currency_element_text(block: &str, name: &str) -> Option<String> {
    let open = format!("<{name}");
    let start = block.find(&open)?;
    let content_start = block[start..].find('>')? + start + 1;
    let close = format!("</{name}>");
    let end = block[content_start..].find(&close)? + content_start;
    Some(block[content_start..end].to_string())
}

#[cfg(test)]
mod currency_tests {
    use super::*;

    const LIVE: &str = r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DESC><CMPINFO><CURRENCY>0</CURRENCY></CMPINFO></DESC><DATA><COLLECTION><CURRENCY NAME="Rs." RESERVEDNAME=""><MAILINGNAME TYPE="String">Indian Rupees</MAILINGNAME><DECIMALPLACES TYPE="Number"> 2</DECIMALPLACES></CURRENCY></COLLECTION></DATA></BODY></ENVELOPE>"#;

    #[test]
    fn reads_the_live_indian_rupee_shape_and_ignores_the_cmpinfo_counter() {
        let currency = parse_company_currency(LIVE).expect("parses");
        assert_eq!(currency.symbol, "Rs.");
        assert_eq!(currency.mailing_name, "Indian Rupees");
        assert_eq!(
            currency.currency_count, 1,
            "the CMPINFO counter is not a row"
        );
        assert!(currency.is_inr);
    }

    #[test]
    fn several_currencies_cannot_name_the_base_currency() {
        let xml = LIVE.replace(
            "</COLLECTION>",
            r#"<CURRENCY NAME="$" RESERVEDNAME=""><MAILINGNAME TYPE="String">US Dollars</MAILINGNAME></CURRENCY></COLLECTION>"#,
        );
        let currency = parse_company_currency(&xml).expect("parses");
        assert_eq!(currency.currency_count, 2);
        assert!(!currency.is_inr, "must fall back to asking, never guess");
    }

    #[test]
    fn a_non_indian_single_currency_is_not_inr() {
        let xml = LIVE
            .replace("Indian Rupees", "US Dollars")
            .replace(r#"NAME="Rs.""#, r#"NAME="$""#);
        let currency = parse_company_currency(&xml).expect("parses");
        assert!(!currency.is_inr);
    }
}
