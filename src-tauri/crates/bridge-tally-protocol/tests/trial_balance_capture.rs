use bridge_tally_primitives::TallyDate;
use bridge_tally_protocol::{
    native_outstandings::NativeLedgerSnapshotPeriod,
    outstandings_shared::DateBoundaryProfile,
    trial_balance::{parse_trial_balance, render_trial_balance_request, TrialBalanceError},
};
use sha2::{Digest, Sha256};

const COMPANY: &str = "BRIDGE PROBE B SANDBOX";
const COMPANY_GUID: &str = "ec4454ae-5c4c-4bfa-b3b0-68182a749689";
const RESPONSE: &str = include_str!("fixtures/native/trial_balance_probe_b.xml");
const REQUEST: &str = include_str!("fixtures/native/trial_balance_probe_b_request.xml");
const RESPONSE_BYTES: &[u8] = include_bytes!("fixtures/native/trial_balance_probe_b.xml");
const REQUEST_BYTES: &[u8] = include_bytes!("fixtures/native/trial_balance_probe_b_request.xml");

fn period() -> NativeLedgerSnapshotPeriod {
    NativeLedgerSnapshotPeriod::new(
        DateBoundaryProfile::ModeAgnostic,
        TallyDate::parse("20250401").unwrap(),
        TallyDate::parse("20260814").unwrap(),
    )
    .unwrap()
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[test]
fn production_request_is_the_exact_captured_shape() {
    assert_eq!(REQUEST_BYTES.len(), 602);
    assert_eq!(
        sha256_hex(REQUEST_BYTES),
        "04e42cd8d847f91b679a417c3d50fd691158ec0d9eaf33e653f38aa287150f20"
    );
    assert_eq!(
        render_trial_balance_request(COMPANY, &period()),
        REQUEST.trim_end()
    );
    let escaped = render_trial_balance_request("Synthetic & <Books> 'One'", &period());
    assert!(escaped.contains(
        "<SVCURRENTCOMPANY>Synthetic &amp; &lt;Books&gt; &apos;One&apos;</SVCURRENTCOMPANY>"
    ));
}

#[test]
fn captured_trial_balance_is_identity_bound_and_exactly_balanced() {
    assert_eq!(RESPONSE_BYTES.len(), 18_196);
    assert_eq!(
        sha256_hex(RESPONSE_BYTES),
        "04d9b9784d846bf46ccc695e5aceac54d3bdbdd78d7980b165023be548549592"
    );
    let trial_balance = parse_trial_balance(RESPONSE, COMPANY_GUID).unwrap();
    assert_eq!(trial_balance.rows.len(), 24);
    assert!(trial_balance
        .rows
        .iter()
        .any(|row| row.name == "Profit & Loss A/c"));
    assert_eq!(
        trial_balance
            .rows
            .iter()
            .find(|row| row.name == "PROBE Sales")
            .unwrap()
            .closing
            .as_str(),
        "63232399.10"
    );
}

#[test]
fn collection_identity_and_balanced_movement_fail_closed() {
    assert_eq!(
        parse_trial_balance(RESPONSE, "00000000-0000-0000-0000-000000000000").unwrap_err(),
        TrialBalanceError::InvalidResponse("company_guid_unverified")
    );

    let unbalanced = RESPONSE.replacen(
        "<TBALCLOSING TYPE=\"Amount\">-5500.00</TBALCLOSING>",
        "<TBALCLOSING TYPE=\"Amount\">-5500.01</TBALCLOSING>",
        1,
    );
    assert_eq!(
        parse_trial_balance(&unbalanced, COMPANY_GUID).unwrap_err(),
        TrialBalanceError::InvalidResponse("movement_does_not_balance")
    );

    let opening_difference = RESPONSE
        .replacen(
            "<TBALOPENING TYPE=\"Amount\">0.00</TBALOPENING>",
            "<TBALOPENING TYPE=\"Amount\">-100.00</TBALOPENING>",
            1,
        )
        .replacen(
            "<TBALCLOSING TYPE=\"Amount\">-5500.00</TBALCLOSING>",
            "<TBALCLOSING TYPE=\"Amount\">-5600.00</TBALCLOSING>",
            1,
        );
    assert_eq!(
        parse_trial_balance(&opening_difference, COMPANY_GUID).unwrap_err(),
        TrialBalanceError::InvalidResponse("opening_difference_unverified")
    );
}

#[test]
fn balance_sheet_closing_is_not_substituted_for_tbalclosing() {
    let altered_presentation = RESPONSE.replacen(
        "<CLOSINGBALANCE TYPE=\"Amount\">-5500.00</CLOSINGBALANCE>",
        "<CLOSINGBALANCE TYPE=\"Amount\">999999.00</CLOSINGBALANCE>",
        1,
    );
    let trial_balance = parse_trial_balance(&altered_presentation, COMPANY_GUID).unwrap();
    assert_eq!(
        trial_balance
            .rows
            .iter()
            .find(|row| row.name == "Cash")
            .unwrap()
            .closing
            .as_str(),
        "-5500.00"
    );
}

#[test]
fn malformed_controls_and_oversize_text_fail_closed() {
    let duplicate_status = RESPONSE.replacen(
        "<STATUS>1</STATUS>",
        "<STATUS>1</STATUS><STATUS>1</STATUS>",
        1,
    );
    assert_eq!(
        parse_trial_balance(&duplicate_status, COMPANY_GUID).unwrap_err(),
        TrialBalanceError::InvalidResponse("status_duplicate")
    );

    let empty_amount = RESPONSE.replacen(
        "<TBALOPENING TYPE=\"Amount\">0.00</TBALOPENING>",
        "<TBALOPENING TYPE=\"Amount\"/>",
        1,
    );
    assert_eq!(
        parse_trial_balance(&empty_amount, COMPANY_GUID).unwrap_err(),
        TrialBalanceError::InvalidResponse("amount_invalid")
    );

    let oversize_name = RESPONSE.replacen(
        "NAME=\"Cash\"",
        &format!("NAME=\"{}\"", "x".repeat(4_097)),
        1,
    );
    assert_eq!(
        parse_trial_balance(&oversize_name, COMPANY_GUID).unwrap_err(),
        TrialBalanceError::InvalidResponse("ledger_text_limit")
    );

    let second_root = format!("{RESPONSE}<ENVELOPE/>");
    assert_eq!(
        parse_trial_balance(&second_root, COMPANY_GUID).unwrap_err(),
        TrialBalanceError::InvalidResponse("root_not_envelope")
    );

    let line_error = RESPONSE.replacen(
        "<DATA>",
        "<LINEERROR>synthetic failure</LINEERROR><DATA>",
        1,
    );
    assert_eq!(
        parse_trial_balance(&line_error, COMPANY_GUID).unwrap_err(),
        TrialBalanceError::TallyReportedFailure
    );

    let nested_line_error = RESPONSE.replacen(
        "<CLOSINGBALANCE TYPE=\"Amount\">-5500.00</CLOSINGBALANCE>",
        "<IGNORED><LINEERROR>synthetic nested failure</LINEERROR></IGNORED><CLOSINGBALANCE TYPE=\"Amount\">-5500.00</CLOSINGBALANCE>",
        1,
    );
    assert_eq!(
        parse_trial_balance(&nested_line_error, COMPANY_GUID).unwrap_err(),
        TrialBalanceError::TallyReportedFailure
    );

    let duplicate_name = RESPONSE.replacen(
        "<LEDGER NAME=\"Cash\" RESERVEDNAME=\"\">",
        "<LEDGER NAME=\"Cash\" NAME=\"Synthetic duplicate\" RESERVEDNAME=\"\">",
        1,
    );
    assert!(matches!(
        parse_trial_balance(&duplicate_name, COMPANY_GUID),
        Err(TrialBalanceError::InvalidResponse(
            "ledger_attribute_invalid" | "ledger_attribute_duplicate"
        ))
    ));
}
