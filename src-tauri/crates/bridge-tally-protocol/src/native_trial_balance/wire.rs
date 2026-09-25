//! Envelope and ledger-row admission for the native Trial Balance collection.
use super::scalar::*;
use super::{
    CurrencyScopedTrialBalance, NativeTrialBalance, NativeTrialBalanceAmount,
    NativeTrialBalanceError, NativeTrialBalanceRow,
};
use crate::native_outstandings::{classify_ledger_currencies, BaseCurrencyName};
use crate::{
    native_ledger_guid_has_company_prefix, tolerant_xml::sanitize_invalid_numeric_references,
    PartyLedgerMasterFieldObservation,
};
use quick_xml::{
    events::{BytesStart, Event},
    Reader,
};
use std::collections::HashSet;

/// Parses a selected-company Trial Balance response. Each ledger GUID must
/// carry the expected company GUID prefix; a successful empty collection is
/// refused because it provides no source identity to bind to the selection.
pub fn parse_native_trial_balance(
    xml: &str,
    expected_company_guid: &str,
) -> Result<NativeTrialBalance, NativeTrialBalanceError> {
    let rows = parse_envelope(xml, expected_company_guid, false)?
        .into_iter()
        .map(|row| admit_plain_row(row, expected_company_guid))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(NativeTrialBalance { rows })
}

/// Parses the Trial Balance of a book with several Currency masters, read with
/// `CURRENCYNAME` in its `FETCH` (bridge#551). Every row must name its
/// currency. A row kept in another currency is set aside by name, and so is a
/// base-currency row with any value Tally wrote as a currency composite
/// (`<amount> @ <rate> = <base amount>`): on the captured several-currency book
/// a rupee ledger that a dollar invoice touched carries such values, at a
/// derived rate. Neither kind has any value parsed. Only the remaining rows are
/// read, validated and returned, so totals over `report` cover plain
/// base-currency ledgers only and are not expected to balance.
pub fn parse_native_trial_balance_with_currency(
    xml: &str,
    expected_company_guid: &str,
    base: &BaseCurrencyName,
) -> Result<CurrencyScopedTrialBalance, NativeTrialBalanceError> {
    let raw = parse_envelope(xml, expected_company_guid, true)?;
    let mut named = Vec::with_capacity(raw.len());
    for row in &raw {
        let currency = row.currency.as_deref().ok_or(NativeTrialBalanceError::InvalidResponse(
            "trial_balance_currency_missing",
        ))?;
        named.push((row.name.as_str(), Some(currency)));
    }
    let classified = classify_ledger_currencies(base, named)
        .map_err(|refusal| NativeTrialBalanceError::InvalidResponse(refusal.code()))?;
    let foreign = classified
        .foreign
        .iter()
        .map(|ledger| ledger.ledger.as_str())
        .collect::<HashSet<_>>();
    let mut rows = Vec::new();
    let mut mixed = Vec::new();
    for row in raw {
        if foreign.contains(row.name.as_str()) {
            continue;
        }
        if row.amounts().iter().any(|value| is_currency_composite(value)) {
            mixed.push(row.name);
            continue;
        }
        rows.push(admit_plain_row(row, expected_company_guid)?);
    }
    Ok(CurrencyScopedTrialBalance {
        report: NativeTrialBalance { rows },
        foreign_currency_ledgers: classified.foreign,
        mixed_currency_ledgers: mixed,
    })
}

/// The envelope and every ledger row, with each row's amounts still text.
fn parse_envelope(
    xml: &str,
    expected_company_guid: &str,
    with_currency: bool,
) -> Result<Vec<RawRow>, NativeTrialBalanceError> {
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
                if path_is(&path, &[b"ENVELOPE"])
                    && name == b"HEADER"
                    && std::mem::replace(&mut header_seen, true)
                {
                    return Err(NativeTrialBalanceError::InvalidResponse(
                        "trial_balance_duplicate_header",
                    ));
                }
                if path_is(&path, &[b"ENVELOPE"])
                    && name == b"BODY"
                    && std::mem::replace(&mut body_seen, true)
                {
                    return Err(NativeTrialBalanceError::InvalidResponse(
                        "trial_balance_duplicate_body",
                    ));
                }
                if path_is(&path, &[b"ENVELOPE", b"BODY"])
                    && name == b"DATA"
                    && std::mem::replace(&mut data_seen, true)
                {
                    return Err(NativeTrialBalanceError::InvalidResponse(
                        "trial_balance_duplicate_data",
                    ));
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
                    let row = parse_row(&mut reader, &element, with_currency)?;
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
    Ok(rows)
}

/// One ledger row as read: its amounts as text, each element's `TYPE` already
/// checked, and `CURRENCYNAME` when the request fetched it.
struct RawRow {
    name: String,
    guid: String,
    parent: Option<String>,
    opening: String,
    debit: String,
    credit: String,
    closing: String,
    currency: Option<String>,
}

impl RawRow {
    fn amounts(&self) -> [&str; 4] {
        [&self.opening, &self.debit, &self.credit, &self.closing]
    }
}

fn parse_row(
    reader: &mut Reader<&[u8]>,
    element: &BytesStart<'_>,
    with_currency: bool,
) -> Result<RawRow, NativeTrialBalanceError> {
    let name = required_attribute(element, b"NAME", "trial_balance_name_missing")?;
    let mut guid = None;
    let mut parent = None;
    let mut opening = None;
    let mut debit = None;
    let mut credit = None;
    let mut closing = None;
    let mut currency = None;
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
                b"CURRENCYNAME" if with_currency => set_once(
                    &mut currency,
                    read_element_text(reader, child.name())?,
                    "trial_balance_duplicate_currency",
                )?,
                b"TBALOPENING" => set_once(
                    &mut opening,
                    amount_text(&child, read_element_text(reader, child.name())?)?,
                    "trial_balance_duplicate_opening",
                )?,
                b"DEBITTOTALS" => set_once(
                    &mut debit,
                    amount_text(&child, read_element_text(reader, child.name())?)?,
                    "trial_balance_duplicate_debit",
                )?,
                b"CREDITTOTALS" => set_once(
                    &mut credit,
                    amount_text(&child, read_element_text(reader, child.name())?)?,
                    "trial_balance_duplicate_credit",
                )?,
                b"TBALCLOSING" => set_once(
                    &mut closing,
                    amount_text(&child, read_element_text(reader, child.name())?)?,
                    "trial_balance_duplicate_closing",
                )?,
                _ => skip_subtree(reader)?,
            },
            Event::Empty(child) => match child.name().as_ref().to_ascii_uppercase().as_slice() {
                b"PARENT" => {
                    set_once(&mut parent, String::new(), "trial_balance_duplicate_parent")?
                }
                b"CURRENCYNAME" if with_currency => {
                    set_once(&mut currency, String::new(), "trial_balance_duplicate_currency")?
                }
                b"TBALOPENING" => set_once(
                    &mut opening,
                    amount_text(&child, String::new())?,
                    "trial_balance_duplicate_opening",
                )?,
                b"DEBITTOTALS" => set_once(
                    &mut debit,
                    amount_text(&child, String::new())?,
                    "trial_balance_duplicate_debit",
                )?,
                b"CREDITTOTALS" => set_once(
                    &mut credit,
                    amount_text(&child, String::new())?,
                    "trial_balance_duplicate_credit",
                )?,
                b"TBALCLOSING" => set_once(
                    &mut closing,
                    amount_text(&child, String::new())?,
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
    Ok(RawRow {
        name,
        guid: guid.ok_or(NativeTrialBalanceError::InvalidResponse(
            "trial_balance_guid_missing",
        ))?,
        parent,
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
        currency,
    })
}

/// A row's identity and amounts, admitted exactly as the single-currency read
/// always has: company-prefixed GUID, typed amounts, polarity and equation.
fn admit_plain_row(
    row: RawRow,
    expected_company_guid: &str,
) -> Result<NativeTrialBalanceRow, NativeTrialBalanceError> {
    if !native_ledger_guid_has_company_prefix(&row.guid, expected_company_guid) {
        return Err(NativeTrialBalanceError::InvalidResponse(
            "trial_balance_company_guid_mismatch",
        ));
    }
    let row = NativeTrialBalanceRow {
        name: row.name,
        guid: row.guid,
        parent: row
            .parent
            .map(PartyLedgerMasterFieldObservation::Returned)
            .unwrap_or_default(),
        opening: parse_amount_text(row.opening)?,
        debit: parse_amount_text(row.debit)?,
        credit: parse_amount_text(row.credit)?,
        closing: parse_amount_text(row.closing)?,
    };
    validate_guid_suffix(&row.guid, expected_company_guid)?;
    validate_observed_movement_polarity(&row)?;
    validate_observed_row_equation(&row)?;
    Ok(row)
}

/// See TALLY_PROTOCOL_REFERENCE §5.6. Licensed native captures establish
/// signed movements: debit values are negative and credit values are positive.
/// Empty amounts remain unqualified; a nonzero opposite sign is not admitted
/// as a normal Dr/Cr magnitude.
fn validate_observed_movement_polarity(
    row: &NativeTrialBalanceRow,
) -> Result<(), NativeTrialBalanceError> {
    if matches!(&row.debit, NativeTrialBalanceAmount::Present(value) if !value.is_zero() && !value.is_negative())
    {
        return Err(NativeTrialBalanceError::InvalidResponse(
            "trial_balance_debit_polarity_invalid",
        ));
    }
    if matches!(&row.credit, NativeTrialBalanceAmount::Present(value) if value.is_negative()) {
        return Err(NativeTrialBalanceError::InvalidResponse(
            "trial_balance_credit_polarity_invalid",
        ));
    }
    Ok(())
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
