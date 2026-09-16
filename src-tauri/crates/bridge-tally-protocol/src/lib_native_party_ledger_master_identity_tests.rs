use super::*;

const EXPECTED_COMPANY_GUID: &str = "11111111-1111-1111-1111-111111111111";

fn master_response(response_company_guid: Option<&str>) -> String {
    let response_company_guid = response_company_guid
        .map(|guid| format!("<BRIDGECOMPANYGUID>{guid}</BRIDGECOMPANYGUID>"))
        .unwrap_or_default();
    format!(
        "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>\
            <LEDGER NAME=\"Imported selected ledger\"><GUID>{EXPECTED_COMPANY_GUID}-00000001</GUID>\
            <MASTERID>1</MASTERID><ALTERID>1</ALTERID>{response_company_guid}\
            <PARENT>Sundry Debtors</PARENT><OPENINGBALANCE>-100.00</OPENINGBALANCE></LEDGER>\
            </COLLECTION></DATA></BODY></ENVELOPE>"
    )
}

#[test]
fn party_ledger_master_requires_a_response_bound_company_guid() {
    let wrong_company = parse_native_party_ledger_master_records_with_evidence(
        &master_response(Some("22222222-2222-2222-2222-222222222222")),
        EXPECTED_COMPANY_GUID,
    );
    assert!(
        wrong_company.is_err(),
        "an imported selected-prefix ledger cannot prove the responding company"
    );

    let missing_company = parse_native_party_ledger_master_records_with_evidence(
        &master_response(None),
        EXPECTED_COMPANY_GUID,
    );
    assert!(
        missing_company.is_err(),
        "the dedicated master response must carry Tally's computed company GUID"
    );

    assert_eq!(
        parse_native_party_ledger_master_records_with_evidence(
            &master_response(Some(EXPECTED_COMPANY_GUID)),
            EXPECTED_COMPANY_GUID,
        )
        .unwrap()
        .records
        .len(),
        1,
        "a matching response-bound company GUID admits the master response"
    );
}
