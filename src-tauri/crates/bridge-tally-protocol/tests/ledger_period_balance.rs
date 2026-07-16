use bridge_tally_protocol::{
    parse_ledger_period_balance_report, BRIDGE_LEDGER_PERIOD_BALANCE_SCHEMA,
};

fn report(rows: &str, count: u64) -> String {
    format!(
        r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><COMPANYCONTEXT><SCHEMA>{}</SCHEMA><OBJECTTYPE>LEDGERPERIODBALANCE</OBJECTTYPE><GUID>company-guid</GUID><FROMDATE>20260701</FROMDATE><TODATE>20260731</TODATE><ORDINARYBOOKSREQUESTED>Yes</ORDINARYBOOKSREQUESTED><RECORDCOUNT>{}</RECORDCOUNT></COMPANYCONTEXT>{}</BODY></ENVELOPE>"#,
        BRIDGE_LEDGER_PERIOD_BALANCE_SCHEMA, count, rows
    )
}

#[test]
fn parses_strict_identity_and_exact_period_amounts() {
    let parsed = parse_ledger_period_balance_report(&report(
        r#"<LEDGERPERIODBALANCE GUID="ledger-guid" REMOTEID="remote" MASTERID="7" ALTERID="9"><OPENINGBALANCE>-100.000</OPENINGBALANCE><CLOSINGBALANCE>99.999</CLOSINGBALANCE></LEDGERPERIODBALANCE>"#,
        1,
    ))
    .unwrap();
    assert_eq!(parsed.context.company_guid, "company-guid");
    assert_eq!(parsed.context.from_yyyymmdd, "20260701");
    assert_eq!(parsed.context.to_yyyymmdd, "20260731");
    assert!(parsed.context.ordinary_books_requested);
    assert_eq!(parsed.records[0].source_id.as_deref(), Some("ledger-guid"));
    assert_eq!(parsed.records[0].record.opening_balance, "-100.000");
    assert_eq!(parsed.records[0].record.closing_balance, "99.999");
}

#[test]
fn rejects_count_schema_status_and_required_field_failures() {
    let row = r#"<LEDGERPERIODBALANCE GUID="ledger-guid"><OPENINGBALANCE>0</OPENINGBALANCE><CLOSINGBALANCE>0</CLOSINGBALANCE></LEDGERPERIODBALANCE>"#;
    assert!(parse_ledger_period_balance_report(&report(row, 2)).is_err());

    let wrong_schema = report(row, 1).replace(
        BRIDGE_LEDGER_PERIOD_BALANCE_SCHEMA,
        "bridge.tally.ledger-period-balances/999",
    );
    assert!(parse_ledger_period_balance_report(&wrong_schema).is_err());

    let failure = report(row, 1).replace("<STATUS>1</STATUS>", "<STATUS>0</STATUS>");
    assert!(parse_ledger_period_balance_report(&failure).is_err());

    let missing_closing = report(
        r#"<LEDGERPERIODBALANCE GUID="ledger-guid"><OPENINGBALANCE>0</OPENINGBALANCE></LEDGERPERIODBALANCE>"#,
        1,
    );
    assert!(parse_ledger_period_balance_report(&missing_closing).is_err());
}

#[test]
fn rejects_duplicate_context_and_invalid_boolean() {
    let base = report("", 0);
    let context = r#"<COMPANYCONTEXT><SCHEMA>bridge.tally.ledger-period-balances/1</SCHEMA><OBJECTTYPE>LEDGERPERIODBALANCE</OBJECTTYPE><GUID>company-guid</GUID><FROMDATE>20260701</FROMDATE><TODATE>20260731</TODATE><ORDINARYBOOKSREQUESTED>Yes</ORDINARYBOOKSREQUESTED><RECORDCOUNT>0</RECORDCOUNT></COMPANYCONTEXT>"#;
    assert!(parse_ledger_period_balance_report(
        &base.replace("</BODY>", &format!("{context}</BODY>"))
    )
    .is_err());
    assert!(parse_ledger_period_balance_report(&base.replace(
        "<ORDINARYBOOKSREQUESTED>Yes</ORDINARYBOOKSREQUESTED>",
        "<ORDINARYBOOKSREQUESTED>Maybe</ORDINARYBOOKSREQUESTED>"
    ))
    .is_err());
}

#[test]
fn rejects_empty_and_duplicate_amounts() {
    let empty = report(
        r#"<LEDGERPERIODBALANCE GUID="ledger-guid"><OPENINGBALANCE/><CLOSINGBALANCE>0</CLOSINGBALANCE></LEDGERPERIODBALANCE>"#,
        1,
    );
    assert!(parse_ledger_period_balance_report(&empty).is_err());

    let duplicate = report(
        r#"<LEDGERPERIODBALANCE GUID="ledger-guid"><OPENINGBALANCE>0</OPENINGBALANCE><OPENINGBALANCE>1</OPENINGBALANCE><CLOSINGBALANCE>0</CLOSINGBALANCE></LEDGERPERIODBALANCE>"#,
        1,
    );
    assert!(parse_ledger_period_balance_report(&duplicate).is_err());
}
