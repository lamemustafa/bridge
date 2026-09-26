//! Tally's own Balance Sheet and Profit and Loss, requested by report name.
//!
//! These are built-in reports (protocol reference §12a.1): a success carries no
//! `HEADER` or `STATUS`, an unknown report name returns a bare `RESPONSE`, and
//! nothing in the response identifies the company, so a caller must bracket
//! the read with GUID-verified identity and extent reads. Their observed shape
//! on licensed TallyPrime 7.1 is recorded in §12a.11.
//!
//! The response is a flat sequence of line pairs: a name, then its amounts in
//! two columns (a sub-amount and a main amount). Amounts are plain signed
//! decimals, a debit negative; an empty element is kept as empty, never read
//! as zero. The parser is closed: anything other than well-formed pairs of
//! the expected shape is refused.
use bridge_tally_primitives::ExactDecimal;
use quick_xml::{
    events::{BytesStart, Event},
    name::QName,
    Reader,
};
use serde::Serialize;
use std::{collections::HashSet, fmt};

use crate::{native_outstandings::NativeLedgerSnapshotPeriod, tolerant_xml::sanitize_invalid_numeric_references};

/// Which built-in statement a request names and a response is parsed as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeStatementKind {
    BalanceSheet,
    ProfitAndLoss,
}

impl NativeStatementKind {
    /// The report name Tally's gateway accepts (§12a.11).
    pub fn report_id(self) -> &'static str {
        match self {
            Self::BalanceSheet => "Balance Sheet",
            Self::ProfitAndLoss => "Profit and Loss",
        }
    }

    fn shape(self) -> Shape {
        match self {
            // <BSNAME><DSPACCNAME><DSPDISPNAME>…</DSPDISPNAME></DSPACCNAME></BSNAME>
            // <BSAMT><BSSUBAMT>…</BSSUBAMT><BSMAINAMT>…</BSMAINAMT></BSAMT>
            Self::BalanceSheet => Shape {
                name: b"BSNAME",
                name_wrapped: true,
                amounts: b"BSAMT",
                sub: b"BSSUBAMT",
            },
            // <DSPACCNAME><DSPDISPNAME>…</DSPDISPNAME></DSPACCNAME>
            // <PLAMT><PLSUBAMT>…</PLSUBAMT><BSMAINAMT>…</BSMAINAMT></PLAMT>
            Self::ProfitAndLoss => Shape {
                name: b"DSPACCNAME",
                name_wrapped: false,
                amounts: b"PLAMT",
                sub: b"PLSUBAMT",
            },
        }
    }
}

#[derive(Clone, Copy)]
struct Shape {
    name: &'static [u8],
    /// The Balance Sheet wraps `DSPACCNAME` in `BSNAME`; the P&L does not.
    name_wrapped: bool,
    amounts: &'static [u8],
    sub: &'static [u8],
}

const MAIN: &[u8] = b"BSMAINAMT";

/// One amount exactly as the report returned it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", content = "value", rename_all = "snake_case")]
pub enum NativeStatementAmount {
    Present(ExactDecimal),
    /// The element was present and empty: not zero.
    Empty,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NativeStatementLine {
    /// The line's display name, as Tally shows it. Report lines carry no GUID.
    pub name: String,
    pub sub: NativeStatementAmount,
    pub main: NativeStatementAmount,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NativeStatement {
    pub kind: NativeStatementKind,
    pub lines: Vec<NativeStatementLine>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeStatementError {
    /// A `STATUS` or `LINEERROR` in the response: Tally reported failure.
    TallyReportedFailure,
    /// A bare `RESPONSE`: Tally did not recognise the report name (§12a.1).
    UnknownReport,
    /// An amount that is not a plain signed decimal (e.g. digit-grouped).
    InvalidAmount,
    InvalidResponse(&'static str),
}

impl NativeStatementError {
    pub fn code(self) -> &'static str {
        match self {
            Self::TallyReportedFailure => "statement_tally_reported_failure",
            Self::UnknownReport => "statement_report_unknown",
            Self::InvalidAmount => "statement_amount_invalid",
            Self::InvalidResponse(code) => code,
        }
    }
}

impl fmt::Display for NativeStatementError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "native statement response refused ({})", self.code())
    }
}

impl std::error::Error for NativeStatementError {}

/// Renders the only supported request: a built-in report by name, over the
/// already-admitted period, for the named company (§12a.1's shape). No TDL.
pub fn render_native_statement_request(
    kind: NativeStatementKind,
    company: &str,
    period: &NativeLedgerSnapshotPeriod,
) -> String {
    format!(
        r#"<ENVELOPE><HEADER><VERSION>1</VERSION><TALLYREQUEST>Export</TALLYREQUEST><TYPE>Data</TYPE><ID>{id}</ID></HEADER><BODY><DESC><STATICVARIABLES><SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT><SVCURRENTCOMPANY>{company}</SVCURRENTCOMPANY><SVFROMDATE TYPE="Date">{from}</SVFROMDATE><SVTODATE TYPE="Date">{to}</SVTODATE></STATICVARIABLES></DESC></BODY></ENVELOPE>"#,
        id = kind.report_id(),
        company = xml_escape(company),
        from = period.from().as_str(),
        to = period.to().as_str(),
    )
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Parses a built-in Balance Sheet or Profit and Loss response. Refuses a
/// failure signal, an unknown report, an empty report, any element outside
/// the expected pairs, a name without its amounts (or amounts without a
/// name), a repeated line name, and any amount that is not a plain signed
/// decimal.
pub fn parse_native_statement(
    kind: NativeStatementKind,
    xml: &str,
) -> Result<NativeStatement, NativeStatementError> {
    let shape = kind.shape();
    let sanitized = sanitize_invalid_numeric_references(xml);
    let mut reader = Reader::from_str(&sanitized);
    // Not trimmed: an entity splits a name into several text events, and
    // trimming each would drop the spaces around it ("Profit & Loss A/c").
    reader.config_mut().trim_text(false);
    let mut root_seen = false;
    let mut envelope_closed = false;
    let mut pending_name: Option<String> = None;
    let mut lines = Vec::new();
    let mut names = HashSet::new();

    loop {
        match reader.read_event().map_err(|_| malformed())? {
            Event::Start(element) => {
                let name = element.name().as_ref().to_ascii_uppercase();
                if !root_seen {
                    match name.as_slice() {
                        b"ENVELOPE" => root_seen = true,
                        b"RESPONSE" => return Err(NativeStatementError::UnknownReport),
                        _ => return Err(invalid("statement_root_not_envelope")),
                    }
                    continue;
                }
                if envelope_closed {
                    return Err(invalid("statement_trailing_content"));
                }
                reject_failure_signal(&name)?;
                if name.as_slice() == shape.name {
                    if pending_name.is_some() {
                        return Err(invalid("statement_name_without_amounts"));
                    }
                    let text = if shape.name_wrapped {
                        read_wrapped_display_name(&mut reader)?
                    } else {
                        read_display_name(&mut reader, element.name())?
                    };
                    if text.trim().is_empty() {
                        return Err(invalid("statement_line_name_empty"));
                    }
                    pending_name = Some(text);
                } else if name.as_slice() == shape.amounts {
                    let line_name =
                        pending_name.take().ok_or(invalid("statement_amounts_without_name"))?;
                    let (sub, main) = read_amounts(&mut reader, &element, shape)?;
                    if !names.insert(line_name.to_lowercase()) {
                        return Err(invalid("statement_duplicate_line"));
                    }
                    lines.push(NativeStatementLine {
                        name: line_name,
                        sub,
                        main,
                    });
                } else {
                    return Err(invalid("statement_unexpected_element"));
                }
            }
            Event::Empty(element) => {
                let name = element.name().as_ref().to_ascii_uppercase();
                if !root_seen {
                    return Err(invalid("statement_root_not_envelope"));
                }
                reject_failure_signal(&name)?;
                return Err(invalid("statement_unexpected_empty_element"));
            }
            Event::End(element) => {
                if element.name().as_ref().eq_ignore_ascii_case(b"ENVELOPE") && !envelope_closed {
                    envelope_closed = true;
                } else {
                    return Err(malformed());
                }
            }
            Event::Text(text) => {
                if !text.as_ref().iter().all(u8::is_ascii_whitespace) {
                    return Err(invalid("statement_stray_text"));
                }
            }
            Event::Decl(_) | Event::Comment(_) => {}
            Event::Eof => break,
            _ => return Err(invalid("statement_unexpected_content")),
        }
    }
    if !root_seen || !envelope_closed {
        return Err(invalid("statement_envelope_unterminated"));
    }
    if pending_name.is_some() {
        return Err(invalid("statement_name_without_amounts"));
    }
    if lines.is_empty() {
        return Err(invalid("statement_empty"));
    }
    Ok(NativeStatement { kind, lines })
}

fn invalid(code: &'static str) -> NativeStatementError {
    NativeStatementError::InvalidResponse(code)
}

fn malformed() -> NativeStatementError {
    invalid("statement_xml_malformed")
}

fn reject_failure_signal(name: &[u8]) -> Result<(), NativeStatementError> {
    match name {
        b"STATUS" | b"LINEERROR" | b"ERROR" => Err(NativeStatementError::TallyReportedFailure),
        _ => Ok(()),
    }
}

/// `<BSNAME>` wrapping exactly one `<DSPACCNAME>`.
fn read_wrapped_display_name(reader: &mut Reader<&[u8]>) -> Result<String, NativeStatementError> {
    let mut found = None;
    loop {
        match reader.read_event().map_err(|_| malformed())? {
            Event::Start(element)
                if element.name().as_ref().eq_ignore_ascii_case(b"DSPACCNAME") =>
            {
                if found.is_some() {
                    return Err(invalid("statement_duplicate_name_element"));
                }
                found = Some(read_display_name(reader, element.name())?);
            }
            Event::End(element) if element.name().as_ref().eq_ignore_ascii_case(b"BSNAME") => {
                return found.ok_or(invalid("statement_name_missing"));
            }
            Event::Text(text) if text.as_ref().iter().all(u8::is_ascii_whitespace) => {}
            _ => return Err(invalid("statement_name_shape")),
        }
    }
}

/// `<DSPACCNAME>` holding exactly one `<DSPDISPNAME>` text element.
fn read_display_name(
    reader: &mut Reader<&[u8]>,
    outer: QName<'_>,
) -> Result<String, NativeStatementError> {
    let outer = outer.as_ref().to_vec();
    let mut found = None;
    loop {
        match reader.read_event().map_err(|_| malformed())? {
            Event::Start(element)
                if element.name().as_ref().eq_ignore_ascii_case(b"DSPDISPNAME") =>
            {
                if found.is_some() {
                    return Err(invalid("statement_duplicate_name_element"));
                }
                found = Some(read_text(reader, element.name())?);
            }
            Event::End(element) if element.name().as_ref() == outer.as_slice() => {
                return found.ok_or(invalid("statement_name_missing"));
            }
            Event::Text(text) if text.as_ref().iter().all(u8::is_ascii_whitespace) => {}
            _ => return Err(invalid("statement_name_shape")),
        }
    }
}

/// The amounts block: exactly one sub-amount and one main amount, each text
/// or empty, and nothing else.
fn read_amounts(
    reader: &mut Reader<&[u8]>,
    block: &BytesStart<'_>,
    shape: Shape,
) -> Result<(NativeStatementAmount, NativeStatementAmount), NativeStatementError> {
    let block = block.name().as_ref().to_vec();
    let mut sub = None;
    let mut main = None;
    loop {
        match reader.read_event().map_err(|_| malformed())? {
            Event::Start(element) => {
                let name = element.name().as_ref().to_ascii_uppercase();
                let slot = amount_slot(&name, shape, &mut sub, &mut main)?;
                *slot = Some(amount(&read_text(reader, element.name())?)?);
            }
            Event::Empty(element) => {
                let name = element.name().as_ref().to_ascii_uppercase();
                let slot = amount_slot(&name, shape, &mut sub, &mut main)?;
                *slot = Some(NativeStatementAmount::Empty);
            }
            Event::End(element) if element.name().as_ref() == block.as_slice() => {
                return match (sub, main) {
                    (Some(sub), Some(main)) => Ok((sub, main)),
                    _ => Err(invalid("statement_amount_missing")),
                };
            }
            Event::Text(text) if text.as_ref().iter().all(u8::is_ascii_whitespace) => {}
            _ => return Err(invalid("statement_amounts_shape")),
        }
    }
}

fn amount_slot<'a>(
    name: &[u8],
    shape: Shape,
    sub: &'a mut Option<NativeStatementAmount>,
    main: &'a mut Option<NativeStatementAmount>,
) -> Result<&'a mut Option<NativeStatementAmount>, NativeStatementError> {
    reject_failure_signal(name)?;
    let slot = if name == shape.sub {
        sub
    } else if name == MAIN {
        main
    } else {
        return Err(invalid("statement_amounts_shape"));
    };
    if slot.is_some() {
        return Err(invalid("statement_duplicate_amount"));
    }
    Ok(slot)
}

fn amount(text: &str) -> Result<NativeStatementAmount, NativeStatementError> {
    if text.is_empty() {
        return Ok(NativeStatementAmount::Empty);
    }
    // ExactDecimal admits only an optional minus, digits and an optional
    // fraction: grouping, symbols, a Dr/Cr suffix or surrounding space refuse.
    ExactDecimal::parse(text)
        .map(NativeStatementAmount::Present)
        .map_err(|_| NativeStatementError::InvalidAmount)
}

/// Text-only element content, with the five predefined entities decoded.
fn read_text(reader: &mut Reader<&[u8]>, name: QName<'_>) -> Result<String, NativeStatementError> {
    let end = name.as_ref().to_vec();
    let mut value = String::new();
    loop {
        match reader.read_event().map_err(|_| malformed())? {
            Event::Text(text) => {
                let decoded = text.decode().map_err(|_| invalid("statement_xml_invalid_encoding"))?;
                let unescaped = quick_xml::escape::unescape(&decoded)
                    .map_err(|_| invalid("statement_xml_invalid_escape"))?;
                value.push_str(&unescaped);
            }
            Event::GeneralRef(reference) => {
                let decoded = reference
                    .decode()
                    .map_err(|_| invalid("statement_xml_invalid_encoding"))?;
                match decoded.as_ref() {
                    "amp" => value.push('&'),
                    "lt" => value.push('<'),
                    "gt" => value.push('>'),
                    "quot" => value.push('"'),
                    "apos" => value.push('\''),
                    _ => return Err(invalid("statement_general_reference_invalid")),
                }
            }
            Event::End(element) if element.name().as_ref() == end.as_slice() => return Ok(value),
            _ => return Err(invalid("statement_scalar_not_text_only")),
        }
    }
}

#[cfg(test)]
#[path = "native_statement_reports_tests.rs"]
mod tests;
