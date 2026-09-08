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
    native_outstandings::NativeLedgerSnapshotPeriod,
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
    reader.config_mut().trim_text(true);
    let mut path = Vec::<Vec<u8>>::new();
    let mut status_seen = false;
    let mut collection_seen = false;
    let mut rows = Vec::new();
    let mut identities = HashSet::new();

    loop {
        match reader
            .read_event()
            .map_err(|_| NativeTrialBalanceError::InvalidResponse("trial_balance_xml_malformed"))?
        {
            Event::Start(element) => {
                let name = element.name().as_ref().to_ascii_uppercase();
                if path.is_empty() && name != b"ENVELOPE" {
                    return Err(NativeTrialBalanceError::InvalidResponse(
                        "trial_balance_root_not_envelope",
                    ));
                }
                if path_is(&path, &[b"ENVELOPE", b"HEADER"]) && name == b"STATUS" {
                    if read_element_text(&mut reader, element.name())? != "1" {
                        return Err(NativeTrialBalanceError::TallyReportedFailure);
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
                    let row = parse_row(&mut reader, &element, expected_company_guid)?;
                    if !identities.insert(row.guid.clone()) {
                        return Err(NativeTrialBalanceError::InvalidResponse(
                            "trial_balance_duplicate_guid",
                        ));
                    }
                    rows.push(row);
                    continue;
                }
                path.push(name);
            }
            Event::Empty(element) => {
                let name = element.name().as_ref().to_ascii_uppercase();
                if path_is(&path, &[b"ENVELOPE", b"BODY", b"DATA"]) && name == b"COLLECTION" {
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
                    parse_amount(read_element_text(reader, child.name())?)?,
                    "trial_balance_duplicate_opening",
                )?,
                b"DEBITTOTALS" => set_once(
                    &mut debit,
                    parse_amount(read_element_text(reader, child.name())?)?,
                    "trial_balance_duplicate_debit",
                )?,
                b"CREDITTOTALS" => set_once(
                    &mut credit,
                    parse_amount(read_element_text(reader, child.name())?)?,
                    "trial_balance_duplicate_credit",
                )?,
                b"TBALCLOSING" => set_once(
                    &mut closing,
                    parse_amount(read_element_text(reader, child.name())?)?,
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
                    NativeTrialBalanceAmount::PresentEmpty,
                    "trial_balance_duplicate_opening",
                )?,
                b"DEBITTOTALS" => set_once(
                    &mut debit,
                    NativeTrialBalanceAmount::PresentEmpty,
                    "trial_balance_duplicate_debit",
                )?,
                b"CREDITTOTALS" => set_once(
                    &mut credit,
                    NativeTrialBalanceAmount::PresentEmpty,
                    "trial_balance_duplicate_credit",
                )?,
                b"TBALCLOSING" => set_once(
                    &mut closing,
                    NativeTrialBalanceAmount::PresentEmpty,
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
    if !guid
        .strip_prefix(expected_company_guid)
        .is_some_and(|suffix| suffix.starts_with('-'))
    {
        return Err(NativeTrialBalanceError::InvalidResponse(
            "trial_balance_company_guid_mismatch",
        ));
    }
    Ok(NativeTrialBalanceRow {
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
    })
}

fn parse_amount(value: String) -> Result<NativeTrialBalanceAmount, NativeTrialBalanceError> {
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
        .ok_or(NativeTrialBalanceError::InvalidResponse(missing))
}

fn read_element_text(
    reader: &mut Reader<&[u8]>,
    name: QName<'_>,
) -> Result<String, NativeTrialBalanceError> {
    let raw = reader
        .read_text(name)
        .map_err(|_| NativeTrialBalanceError::InvalidResponse("trial_balance_xml_malformed"))?;
    let decoded = raw.decode().map_err(|_| {
        NativeTrialBalanceError::InvalidResponse("trial_balance_xml_invalid_encoding")
    })?;
    quick_xml::escape::unescape(&decoded)
        .map(|value| value.trim().to_string())
        .map_err(|_| NativeTrialBalanceError::InvalidResponse("trial_balance_xml_invalid_escape"))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outstandings_shared::DateBoundaryProfile;
    use bridge_tally_primitives::TallyDate;

    const COMPANY: &str = "eebb9a9f-1679-4468-9e8f-814c729674cb";
    const KNOWN_LAB: &str = include_str!("../tests/fixtures/native/trial_balance_known_lab.xml");
    const OPENING_YEAR: &str =
        include_str!("../tests/fixtures/native/trial_balance_opening_year.xml");

    #[test]
    fn request_binds_both_admitted_snapshot_boundaries_and_escapes_company() {
        let period = NativeLedgerSnapshotPeriod::new(
            DateBoundaryProfile::ModeAgnostic,
            TallyDate::parse("20260601").unwrap(),
            TallyDate::parse("20260731").unwrap(),
        )
        .unwrap();
        let request = render_native_trial_balance_request("A & B <Co>", &period);
        assert!(request.contains("<SVCURRENTCOMPANY>A &amp; B &lt;Co&gt;</SVCURRENTCOMPANY>"));
        assert!(request.contains("<SVFROMDATE TYPE=\"Date\">20260601</SVFROMDATE>"));
        assert!(request.contains("<SVTODATE TYPE=\"Date\">20260731</SVTODATE>"));
        assert!(request.contains("TBALOPENING, DEBITTOTALS, CREDITTOTALS, TBALCLOSING"));
        assert!(!request.contains("CLOSINGBALANCE"));
    }

    #[test]
    fn captured_trial_balance_preserves_empty_and_nonzero_openings() {
        let report = parse_native_trial_balance(KNOWN_LAB, COMPANY).unwrap();
        assert_eq!(report.rows.len(), 6);
        assert_eq!(
            report.rows[0].opening,
            NativeTrialBalanceAmount::Present(ExactDecimal::parse("0.00").unwrap())
        );
        assert_eq!(report.rows[0].debit, NativeTrialBalanceAmount::PresentEmpty);
        assert_eq!(
            report.rows[0].credit,
            NativeTrialBalanceAmount::PresentEmpty
        );
        assert_eq!(
            report.rows[0].closing,
            NativeTrialBalanceAmount::PresentEmpty
        );
        assert_eq!(
            report.rows[0].parent.returned_text(),
            Some("Sundry Debtors")
        );

        let opening =
            parse_native_trial_balance(OPENING_YEAR, "915d42f8-42ae-4b03-8291-55f596e3a2ea")
                .unwrap();
        assert!(opening.rows.iter().any(|row| row.opening
            == NativeTrialBalanceAmount::Present(ExactDecimal::parse("125000.00").unwrap())));
    }

    #[test]
    fn captured_trial_balance_mutations_fail_closed() {
        for (mutation, expected) in [
            (
                KNOWN_LAB.replacen("<TBALCLOSING TYPE=\"Amount\"></TBALCLOSING>", "", 1),
                "trial_balance_closing_missing",
            ),
            (
                KNOWN_LAB.replacen(
                    "<DEBITTOTALS TYPE=\"Amount\"></DEBITTOTALS>",
                    "<DEBITTOTALS TYPE=\"Amount\">not-money</DEBITTOTALS>",
                    1,
                ),
                "invalid_amount",
            ),
            (
                KNOWN_LAB.replacen(
                    "eebb9a9f-1679-4468-9e8f-814c729674cb-000000d1",
                    "wrong-company-000000d1",
                    1,
                ),
                "trial_balance_company_guid_mismatch",
            ),
            (
                KNOWN_LAB.replacen(
                    "eebb9a9f-1679-4468-9e8f-814c729674cb-000000d1",
                    "eebb9a9f-1679-4468-9e8f-814c729674cb-000000ce",
                    1,
                ),
                "trial_balance_duplicate_guid",
            ),
        ] {
            let error = parse_native_trial_balance(&mutation, COMPANY).unwrap_err();
            match (error, expected) {
                (NativeTrialBalanceError::InvalidAmount, "invalid_amount")
                | (
                    NativeTrialBalanceError::InvalidResponse("trial_balance_closing_missing"),
                    "trial_balance_closing_missing",
                )
                | (
                    NativeTrialBalanceError::InvalidResponse("trial_balance_company_guid_mismatch"),
                    "trial_balance_company_guid_mismatch",
                )
                | (
                    NativeTrialBalanceError::InvalidResponse("trial_balance_duplicate_guid"),
                    "trial_balance_duplicate_guid",
                ) => {}
                other => panic!("unexpected result: {other:?}"),
            }
        }

        let collection_start = KNOWN_LAB.find("<COLLECTION ").unwrap();
        let rows_start = KNOWN_LAB[collection_start..].find('>').unwrap() + collection_start + 1;
        let rows_end = KNOWN_LAB.find("</COLLECTION>").unwrap();
        let mut empty = KNOWN_LAB.to_owned();
        empty.replace_range(rows_start..rows_end, "");
        assert_eq!(
            parse_native_trial_balance(&empty, COMPANY),
            Err(NativeTrialBalanceError::InvalidResponse(
                "trial_balance_source_identity_missing"
            ))
        );
    }
}
