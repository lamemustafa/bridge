//! Envelope and ledger-row admission for the native Trial Balance collection.
use super::scalar::*;
use super::{
    NativeTrialBalance, NativeTrialBalanceAmount, NativeTrialBalanceError, NativeTrialBalanceRow,
};
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
