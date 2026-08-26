use bridge_tally_protocol::parse_company_gateway_capability_observation;

// Malformed-response regressions only, not live mode qualification fixtures.
#[test]
fn incomplete_rows_and_in_band_errors_cannot_authorize_licensed_periods() {
    let row = "<COMPANY NAME=\"Synthetic Company\"><GUID>synthetic-guid</GUID><PRODUCTNAME>TallyPrime</PRODUCTNAME><EDUMODE>No</EDUMODE><SILVER>Yes</SILVER><GOLD>No</GOLD></COMPANY>";
    let envelope = |rows: &str, suffix: &str| {
        format!(
        "<ENVELOPE><HEADER><VERSION>1</VERSION><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>{rows}</COLLECTION>{suffix}</DATA></BODY></ENVELOPE>"
    )
    };
    assert!(parse_company_gateway_capability_observation(&envelope(row, "")).is_ok());
    for invalid in [
        envelope(&format!("{row}<COMPANY NAME=\"Incomplete\"/>"), ""),
        envelope(row, "<LINEERROR>synthetic error</LINEERROR>"),
        envelope(row, "<LINEERROR/>"),
        envelope(row, "<RESPONSE>synthetic error</RESPONSE>"),
        envelope(
            &row.replace(
                "</COMPANY>",
                "<LINEERROR>synthetic error</LINEERROR></COMPANY>",
            ),
            "",
        ),
    ] {
        assert!(parse_company_gateway_capability_observation(&invalid).is_err());
    }
}
