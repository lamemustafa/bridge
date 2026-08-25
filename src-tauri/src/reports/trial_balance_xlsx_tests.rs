use std::io::{Cursor, Read};

use bridge_tally_core::ExactDecimal;
use bridge_tally_protocol::trial_balance::parse_trial_balance;

use super::trial_balance::{TrialBalanceWorkbookSource, TrialBalanceXlsxError};
use super::trial_balance_xlsx::render_trial_balance_xlsx;

const COMPANY_GUID: &str = "ec4454ae-5c4c-4bfa-b3b0-68182a749689";
const RESPONSE: &str = include_str!(
    "../../crates/bridge-tally-protocol/tests/fixtures/native/trial_balance_probe_b.xml"
);

fn captured_source() -> TrialBalanceWorkbookSource {
    TrialBalanceWorkbookSource {
        company: "BRIDGE PROBE B SANDBOX".to_string(),
        from_yyyymmdd: "20250401".to_string(),
        to_yyyymmdd: "20260814".to_string(),
        source_bytes: RESPONSE.len(),
        trial_balance: parse_trial_balance(RESPONSE, COMPANY_GUID).unwrap(),
    }
}

fn workbook_xml(bytes: Vec<u8>) -> String {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut text = String::new();
    for name in ["xl/worksheets/sheet1.xml", "xl/sharedStrings.xml"] {
        archive
            .by_name(name)
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
    }
    text
}

#[test]
fn captured_balances_render_as_a_real_numeric_workbook() {
    let bytes = render_trial_balance_xlsx(&captured_source()).unwrap();
    assert!(bytes.len() > 200);
    assert_eq!(&bytes[0..2], b"PK");
    let xml = workbook_xml(bytes);
    assert!(xml.contains("Net change is closing minus opening"));
    assert!(xml.contains("Bridge does not assert a currency"));
    assert!(xml.contains("63232399.1"));
}

#[test]
fn opening_difference_is_explicit_and_never_absorbed_into_totals() {
    let mut source = captured_source();
    source.trial_balance.rows[0].opening = ExactDecimal::parse("-100.00").unwrap();
    source.trial_balance.rows[0].closing = source.trial_balance.rows[0]
        .closing
        .checked_subtract(&ExactDecimal::parse("100.00").unwrap())
        .unwrap();
    source.trial_balance.opening_difference = ExactDecimal::parse("100.00").unwrap();

    let xml = workbook_xml(render_trial_balance_xlsx(&source).unwrap());
    assert!(xml.contains("Difference in opening balances"));
}

#[test]
fn stale_or_missing_opening_difference_fails_closed() {
    let mut source = captured_source();
    source.trial_balance.rows[0].opening = ExactDecimal::parse("-100.00").unwrap();
    source.trial_balance.rows[0].closing = source.trial_balance.rows[0]
        .closing
        .checked_subtract(&ExactDecimal::parse("100.00").unwrap())
        .unwrap();

    assert!(matches!(
        render_trial_balance_xlsx(&source),
        Err(TrialBalanceXlsxError::ControlMismatch)
    ));
}
