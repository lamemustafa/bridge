use super::*;
use crate::text_encoding::{
    decode_tally_xml_response_bytes_limited, decode_xml_bytes, ExpectedTallyTextEncoding,
};
use bridge_tally_primitives::TallyDate;

use crate::outstandings_shared::DateBoundaryProfile;

const BS_FY: &[u8] = include_bytes!("../tests/fixtures/statement_balance_sheet_fy_live.utf16le.xml");
const PL_FY: &[u8] = include_bytes!("../tests/fixtures/statement_profit_and_loss_fy_live.utf16le.xml");
const BS_EMPTY_MONTH: &[u8] =
    include_bytes!("../tests/fixtures/statement_balance_sheet_empty_month_live.utf16le.xml");
const BS_DENSE_MONTH: &[u8] =
    include_bytes!("../tests/fixtures/statement_balance_sheet_dense_month_live.utf16le.xml");
const BS_FY_REQUEST: &[u8] =
    include_bytes!("../tests/fixtures/statement_balance_sheet_fy_request.utf16le.xml");
const PL_FY_REQUEST: &[u8] =
    include_bytes!("../tests/fixtures/statement_profit_and_loss_fy_request.utf16le.xml");

/// A captured request: UTF-16LE with a BOM.
fn text(bytes: &[u8]) -> String {
    decode_xml_bytes(bytes).unwrap()
}

/// A captured response, decoded as production decodes it: BOM-less UTF-16LE
/// under the `charset=utf-16` content type Tally sent.
fn response(bytes: &[u8]) -> String {
    decode_tally_xml_response_bytes_limited(
        bytes,
        "text/xml; charset=utf-16",
        ExpectedTallyTextEncoding::Utf16Le,
        bytes.len(),
    )
    .unwrap()
    .text
}

fn present(value: &str) -> NativeStatementAmount {
    NativeStatementAmount::Present(ExactDecimal::parse(value).unwrap())
}

fn line(name: &str, sub: NativeStatementAmount, main: NativeStatementAmount) -> NativeStatementLine {
    NativeStatementLine {
        name: name.to_string(),
        sub,
        main,
    }
}

use NativeStatementAmount::Empty;

#[test]
fn a_captured_full_year_balance_sheet_parses_to_its_five_lines() {
    let parsed = parse_native_statement(NativeStatementKind::BalanceSheet, &response(BS_FY)).unwrap();
    assert_eq!(
        parsed.lines,
        vec![
            line("Capital Account", Empty, Empty),
            line("Loans (Liability)", Empty, Empty),
            line("Current Liabilities", Empty, present("4250.00")),
            line("Profit & Loss A/c", Empty, present("-4250.00")),
            line("Current Assets", Empty, Empty),
        ]
    );
}

#[test]
fn a_captured_full_year_profit_and_loss_keeps_both_columns() {
    let parsed = parse_native_statement(NativeStatementKind::ProfitAndLoss, &response(PL_FY)).unwrap();
    assert_eq!(
        parsed.lines,
        vec![
            line("Cost of Sales :", Empty, present("-4250.00")),
            line("Purchase Accounts", present("-4250.00"), Empty),
        ]
    );
}

#[test]
fn an_empty_month_keeps_every_amount_empty_not_zero() {
    let parsed =
        parse_native_statement(NativeStatementKind::BalanceSheet, &response(BS_EMPTY_MONTH)).unwrap();
    assert_eq!(parsed.lines.len(), 5);
    assert!(parsed.lines.iter().all(|l| l.sub == Empty && l.main == Empty));
}

#[test]
fn a_heavy_book_month_keeps_large_signed_amounts_exact() {
    let parsed =
        parse_native_statement(NativeStatementKind::BalanceSheet, &response(BS_DENSE_MONTH)).unwrap();
    let mains: Vec<_> = parsed.lines.iter().map(|l| l.main.clone()).collect();
    assert!(mains.contains(&present("222962422.38")));
    assert!(mains.contains(&present("-222962422.38")));
}

#[test]
fn the_renderer_emits_exactly_the_captured_requests() {
    let period = NativeLedgerSnapshotPeriod::new(
        DateBoundaryProfile::ModeAgnostic,
        TallyDate::parse("20250401").unwrap(),
        TallyDate::parse("20260331").unwrap(),
    )
    .unwrap();
    for (kind, captured) in [
        (NativeStatementKind::BalanceSheet, BS_FY_REQUEST),
        (NativeStatementKind::ProfitAndLoss, PL_FY_REQUEST),
    ] {
        assert_eq!(
            render_native_statement_request(kind, "BRIDGE READS LAB", &period),
            text(captured)
        );
    }
}

#[test]
fn a_shape_parsed_as_the_other_statement_is_refused() {
    assert_eq!(
        parse_native_statement(NativeStatementKind::ProfitAndLoss, &response(BS_FY)).unwrap_err(),
        NativeStatementError::InvalidResponse("statement_unexpected_element")
    );
    assert_eq!(
        parse_native_statement(NativeStatementKind::BalanceSheet, &response(PL_FY)).unwrap_err(),
        NativeStatementError::InvalidResponse("statement_unexpected_element")
    );
}

/// One well-formed Balance Sheet line with `amounts` as its main amount.
fn bs(main: &str) -> String {
    format!(
        "<ENVELOPE><BSNAME><DSPACCNAME><DSPDISPNAME>Capital Account</DSPDISPNAME></DSPACCNAME></BSNAME><BSAMT><BSSUBAMT></BSSUBAMT><BSMAINAMT>{main}</BSMAINAMT></BSAMT></ENVELOPE>"
    )
}

#[test]
fn the_control_line_parses() {
    assert_eq!(
        parse_native_statement(NativeStatementKind::BalanceSheet, &bs("-1.00"))
            .unwrap()
            .lines,
        vec![line("Capital Account", Empty, present("-1.00"))]
    );
}

#[test]
fn display_formatted_amounts_are_refused() {
    for formatted in ["4,250.00", "1500.00 Dr", "(-)10.00", "₹ 10.00", " 10.00", "10.00 "] {
        assert_eq!(
            parse_native_statement(NativeStatementKind::BalanceSheet, &bs(formatted)).unwrap_err(),
            NativeStatementError::InvalidAmount,
            "{formatted:?}"
        );
    }
}

#[test]
fn failure_signals_and_unknown_reports_are_refused() {
    let cases = [
        ("<RESPONSE>Unknown Request, cannot be processed</RESPONSE>", NativeStatementError::UnknownReport),
        ("<ENVELOPE><HEADER><STATUS>0</STATUS></HEADER></ENVELOPE>", NativeStatementError::InvalidResponse("statement_unexpected_element")),
        ("<ENVELOPE><STATUS>1</STATUS></ENVELOPE>", NativeStatementError::TallyReportedFailure),
        ("<ENVELOPE><LINEERROR>Could not set company</LINEERROR></ENVELOPE>", NativeStatementError::TallyReportedFailure),
    ];
    for (xml, expected) in cases {
        assert_eq!(
            parse_native_statement(NativeStatementKind::BalanceSheet, xml).unwrap_err(),
            expected,
            "{xml}"
        );
    }
}

#[test]
fn structural_breaks_are_refused_with_their_own_codes() {
    let name = "<BSNAME><DSPACCNAME><DSPDISPNAME>Capital Account</DSPDISPNAME></DSPACCNAME></BSNAME>";
    let amounts = "<BSAMT><BSSUBAMT></BSSUBAMT><BSMAINAMT>1.00</BSMAINAMT></BSAMT>";
    let cases = [
        ("<ENVELOPE></ENVELOPE>".to_string(), "statement_empty"),
        (format!("<ENVELOPE>{name}</ENVELOPE>"), "statement_name_without_amounts"),
        (format!("<ENVELOPE>{amounts}</ENVELOPE>"), "statement_amounts_without_name"),
        (format!("<ENVELOPE>{name}{name}{amounts}</ENVELOPE>"), "statement_name_without_amounts"),
        (format!("<ENVELOPE>{name}{amounts}{name}{amounts}</ENVELOPE>"), "statement_duplicate_line"),
        (format!("<ENVELOPE>{name}<BSAMT><BSMAINAMT>1.00</BSMAINAMT></BSAMT></ENVELOPE>"), "statement_amount_missing"),
        (format!("<ENVELOPE>{name}<BSAMT><BSSUBAMT/><BSMAINAMT>1</BSMAINAMT><BSMAINAMT>1</BSMAINAMT></BSAMT></ENVELOPE>"), "statement_duplicate_amount"),
        (format!("<ENVELOPE>{name}<BSAMT><BSSUBAMT/><BSMAINAMT/><OTHER/></BSAMT></ENVELOPE>"), "statement_amounts_shape"),
        (format!("<ENVELOPE>{name}{amounts}<EXTRA>x</EXTRA></ENVELOPE>"), "statement_unexpected_element"),
        (format!("<ENVELOPE>{name}{amounts}</ENVELOPE><ENVELOPE></ENVELOPE>"), "statement_trailing_content"),
        ("<ENVELOPE><BSNAME><DSPACCNAME><DSPDISPNAME> </DSPDISPNAME></DSPACCNAME></BSNAME><BSAMT><BSSUBAMT/><BSMAINAMT/></BSAMT></ENVELOPE>".to_string(), "statement_line_name_empty"),
        ("<OTHER/>".to_string(), "statement_root_not_envelope"),
        (format!("<ENVELOPE>{name}{amounts}"), "statement_envelope_unterminated"),
    ];
    for (xml, code) in cases {
        assert_eq!(
            parse_native_statement(NativeStatementKind::BalanceSheet, &xml).unwrap_err(),
            NativeStatementError::InvalidResponse(code),
            "{xml}"
        );
    }
}

#[test]
fn an_empty_element_amount_is_empty_like_an_open_close_pair() {
    let xml = "<ENVELOPE><BSNAME><DSPACCNAME><DSPDISPNAME>Capital Account</DSPDISPNAME></DSPACCNAME></BSNAME><BSAMT><BSSUBAMT/><BSMAINAMT/></BSAMT></ENVELOPE>";
    assert_eq!(
        parse_native_statement(NativeStatementKind::BalanceSheet, xml).unwrap().lines,
        vec![line("Capital Account", Empty, Empty)]
    );
}
