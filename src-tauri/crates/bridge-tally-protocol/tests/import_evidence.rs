use bridge_tally_protocol::{
    parse_import_evidence, parse_import_outcome, parse_import_result,
    parse_ledger_write_readback_with_evidence, TallyImportApplicationStatus,
};

#[test]
fn evidence_binds_counters_response_and_redacted_line_errors() {
    let xml = "<RESPONSE><CREATED>0</CREATED><ALTERED>0</ALTERED><IGNORED>1</IGNORED><ERRORS>1</ERRORS><EXCEPTIONS>0</EXCEPTIONS><LINEERROR>PRIVATE-SYNTHETIC-SENTINEL</LINEERROR></RESPONSE>";
    let evidence = parse_import_evidence(xml).unwrap();

    assert_eq!(evidence.counters().line_error_count, 1);
    assert_eq!(evidence.line_error_sha256().len(), 1);
    assert_eq!(evidence.response_sha256().len(), 64);
    let debug = format!("{evidence:?}");
    assert!(!debug.contains("PRIVATE-SYNTHETIC-SENTINEL"));
}

#[test]
fn response_and_line_error_commitments_change_with_exact_input() {
    let make = |message: &str| {
        parse_import_evidence(&format!(
            "<RESPONSE><CREATED>0</CREATED><ALTERED>0</ALTERED><IGNORED>1</IGNORED><ERRORS>1</ERRORS><EXCEPTIONS>0</EXCEPTIONS><LINEERROR>{message}</LINEERROR></RESPONSE>"
        ))
        .unwrap()
    };
    let first = make("synthetic-a");
    let second = make("synthetic-b");
    assert_ne!(first.response_sha256(), second.response_sha256());
    assert_ne!(first.line_error_sha256(), second.line_error_sha256());
}

#[test]
fn duplicate_or_wrongly_nested_status_and_counters_are_rejected() {
    let duplicate_counter = "<RESPONSE><CREATED>0</CREATED><CREATED>1</CREATED><ALTERED>0</ALTERED><IGNORED>0</IGNORED><ERRORS>0</ERRORS><EXCEPTIONS>0</EXCEPTIONS></RESPONSE>";
    assert!(parse_import_evidence(duplicate_counter).is_err());

    let nested_counter = "<RESPONSE><UNRELATED><CREATED>1</CREATED></UNRELATED><ALTERED>0</ALTERED><IGNORED>0</IGNORED><ERRORS>0</ERRORS><EXCEPTIONS>0</EXCEPTIONS></RESPONSE>";
    assert!(parse_import_evidence(nested_counter).is_err());

    let duplicate_status = "<ENVELOPE><HEADER><STATUS>0</STATUS><STATUS>1</STATUS></HEADER><BODY><DATA><IMPORTRESULT><CREATED>1</CREATED><ALTERED>0</ALTERED><IGNORED>0</IGNORED><ERRORS>0</ERRORS><EXCEPTIONS>0</EXCEPTIONS></IMPORTRESULT></DATA></BODY></ENVELOPE>";
    assert!(parse_import_evidence(duplicate_status).is_err());

    let duplicate_container = "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><IMPORTRESULT><CREATED>1</CREATED><ALTERED>0</ALTERED><IGNORED>0</IGNORED><ERRORS>0</ERRORS><EXCEPTIONS>0</EXCEPTIONS></IMPORTRESULT><IMPORTRESULT/></DATA></BODY></ENVELOPE>";
    assert!(parse_import_evidence(duplicate_container).is_err());

    let duplicate_header = "<ENVELOPE><HEADER/><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><CREATED>1</CREATED><ALTERED>0</ALTERED><IGNORED>0</IGNORED><ERRORS>0</ERRORS><EXCEPTIONS>0</EXCEPTIONS></DATA></BODY></ENVELOPE>";
    assert!(parse_import_evidence(duplicate_header).is_err());

    let doctype = "<!DOCTYPE ENVELOPE><ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><CREATED>1</CREATED><ALTERED>0</ALTERED><IGNORED>0</IGNORED><ERRORS>0</ERRORS><EXCEPTIONS>0</EXCEPTIONS></DATA></BODY></ENVELOPE>";
    assert!(parse_import_evidence(doctype).is_err());

    let duplicate_body = "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><CREATED>1</CREATED><ALTERED>0</ALTERED><IGNORED>0</IGNORED><ERRORS>0</ERRORS></DATA></BODY><BODY/></ENVELOPE>";
    assert!(parse_import_evidence(duplicate_body).is_err());

    let body_before_header = "<ENVELOPE><BODY/><HEADER><STATUS>1</STATUS></HEADER></ENVELOPE>";
    assert!(parse_import_evidence(body_before_header).is_err());

    let attributed_status = "<ENVELOPE><HEADER><STATUS unsafe=\"1\">1</STATUS></HEADER><BODY><DATA><CREATED>1</CREATED><ALTERED>0</ALTERED><IGNORED>0</IGNORED><ERRORS>0</ERRORS></DATA></BODY></ENVELOPE>";
    assert!(parse_import_evidence(attributed_status).is_err());

    let mixed_text = "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY>mixed<DATA><CREATED>1</CREATED><ALTERED>0</ALTERED><IGNORED>0</IGNORED><ERRORS>0</ERRORS></DATA></BODY></ENVELOPE>";
    assert!(parse_import_evidence(mixed_text).is_err());

    let unknown_wrapper = "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><UNEXPECTED><CREATED>1</CREATED></UNEXPECTED><ALTERED>0</ALTERED><IGNORED>0</IGNORED><ERRORS>0</ERRORS></DATA></BODY></ENVELOPE>";
    assert!(parse_import_evidence(unknown_wrapper).is_err());
}

#[test]
fn exact_envelope_import_result_path_is_accepted() {
    let xml = "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><IMPORTRESULT><CREATED>1</CREATED><ALTERED>0</ALTERED><DELETED>0</DELETED><IGNORED>0</IGNORED><ERRORS>0</ERRORS><CANCELLED>0</CANCELLED><EXCEPTIONS>0</EXCEPTIONS></IMPORTRESULT></DATA></BODY></ENVELOPE>";
    let evidence = parse_import_evidence(xml).unwrap();
    assert_eq!(evidence.counters().created, 1);
}

#[test]
fn documented_direct_data_import_result_path_is_accepted_without_profile_mixing() {
    let xml = "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><CREATED>2</CREATED><ALTERED>1</ALTERED><DELETED>0</DELETED><IGNORED>0</IGNORED><ERRORS>1</ERRORS><CANCELLED>0</CANCELLED><EXCEPTIONS>1</EXCEPTIONS><LINEERROR>BRIDGE SYNTHETIC ERROR</LINEERROR></DATA></BODY></ENVELOPE>";
    let evidence = parse_import_evidence(xml).expect("documented direct DATA profile");
    assert_eq!(evidence.counters().created, 2);
    assert_eq!(evidence.counters().altered, 1);
    assert_eq!(evidence.counters().errors, 1);
    assert_eq!(evidence.counters().exceptions, 1);
    assert_eq!(evidence.counters().line_error_count, 1);

    let mixed = "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><CREATED>1</CREATED><IMPORTRESULT><ALTERED>0</ALTERED><IGNORED>0</IGNORED><ERRORS>0</ERRORS><EXCEPTIONS>0</EXCEPTIONS></IMPORTRESULT></DATA></BODY></ENVELOPE>";
    assert!(parse_import_evidence(mixed).is_err());

    let direct_extra_then_wrapped = "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><LASTVCHID>1</LASTVCHID><IMPORTRESULT><CREATED>1</CREATED><ALTERED>0</ALTERED><IGNORED>0</IGNORED><ERRORS>0</ERRORS></IMPORTRESULT></DATA></BODY></ENVELOPE>";
    assert!(parse_import_evidence(direct_extra_then_wrapped).is_err());

    let wrapped_then_direct_extra = "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><IMPORTRESULT><CREATED>1</CREATED><ALTERED>0</ALTERED><IGNORED>0</IGNORED><ERRORS>0</ERRORS></IMPORTRESULT><LASTVCHID>1</LASTVCHID></DATA></BODY></ENVELOPE>";
    assert!(parse_import_evidence(wrapped_then_direct_extra).is_err());
}

#[test]
fn official_legacy_and_wrapped_success_profiles_accept_auxiliary_fields() {
    let legacy = "<RESPONSE><CREATED>2</CREATED><ALTERED>0</ALTERED><LASTVCHID>0</LASTVCHID><LASTMID>0</LASTMID><COMBINED>0</COMBINED><IGNORED>0</IGNORED><ERRORS>0</ERRORS></RESPONSE>";
    let legacy_evidence = parse_import_evidence(legacy).expect("official legacy RESPONSE profile");
    assert_eq!(
        legacy_evidence.application_status(),
        TallyImportApplicationStatus::NotReported
    );
    assert_eq!(legacy_evidence.counters().created, 2);
    assert!(!legacy_evidence.exceptions_were_reported());

    let wrapped = "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><IMPORTRESULT><CREATED>2</CREATED><ALTERED>0</ALTERED><LASTVCHID>119</LASTVCHID><LASTMID>0</LASTMID><COMBINED>0</COMBINED><IGNORED>0</IGNORED><ERRORS>0</ERRORS></IMPORTRESULT></DATA></BODY></ENVELOPE>";
    let wrapped_evidence =
        parse_import_evidence(wrapped).expect("official wrapped IMPORTRESULT profile");
    assert_eq!(
        wrapped_evidence.application_status(),
        TallyImportApplicationStatus::Success
    );
    assert_eq!(wrapped_evidence.counters().created, 2);
    assert!(!wrapped_evidence.exceptions_were_reported());
}

#[test]
fn documented_direct_failure_shape_retains_counters_without_becoming_success() {
    let xml = "<ENVELOPE><HEADER><STATUS>0</STATUS></HEADER><BODY><DATA><CREATED>0</CREATED><ALTERED>0</ALTERED><DELETED>0</DELETED><LASTVCHID>0</LASTVCHID><LASTMID>0</LASTMID><COMBINED>0</COMBINED><IGNORED>0</IGNORED><ERRORS>1</ERRORS><CANCELLED>0</CANCELLED><LINEERROR>BRIDGE SYNTHETIC FAILURE</LINEERROR><VCHNUMBER>BRIDGE-SYNTHETIC-1</VCHNUMBER><DESC>BRIDGE SYNTHETIC FAILURE</DESC></DATA></BODY></ENVELOPE>";
    let outcome = parse_import_outcome(xml).expect("parse documented failure counters");
    assert_eq!(
        outcome.application_status(),
        TallyImportApplicationStatus::Failure
    );
    assert_eq!(outcome.counters().created, 0);
    assert_eq!(outcome.counters().errors, 1);
    assert_eq!(outcome.counters().exceptions, 0);
    assert!(!outcome.exceptions_were_reported());
    let evidence = parse_import_evidence(xml).expect("retain documented failure evidence");
    assert_eq!(
        evidence.application_status(),
        TallyImportApplicationStatus::Failure
    );
    assert_eq!(evidence.counters().errors, 1);
    assert_eq!(evidence.line_error_sha256().len(), 1);
    assert!(!evidence.exceptions_were_reported());
    assert!(
        parse_import_result(xml).is_err(),
        "legacy success-oriented API must remain fail-closed"
    );
}

#[test]
fn malformed_line_error_entities_return_only_a_safe_error() {
    let xml = "<RESPONSE><CREATED>0</CREATED><ALTERED>0</ALTERED><IGNORED>1</IGNORED><ERRORS>1</ERRORS><EXCEPTIONS>0</EXCEPTIONS><LINEERROR>&PRIVATE_SYNTHETIC_SENTINEL;</LINEERROR></RESPONSE>";
    let error = parse_import_evidence(xml).unwrap_err().to_string();
    assert_eq!(error, "Tally import response evidence was invalid");
    assert!(!error.contains("PRIVATE"));
    assert!(!error.contains("SENTINEL"));
}

#[test]
fn write_readback_rejects_wrong_nesting_duplicate_fields_and_attributes() {
    const HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let wrap = |ledger: &str, count: usize| {
        format!(
            r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><COMPANYCONTEXT SCHEMA="bridge.tally.ledger-write-readback/1" OBJECTTYPE="LEDGER" NAME="BRIDGE SYNTHETIC BOOK" GUID="company-guid" RECORDCOUNT="{count}" QUERYIDENTITYSETSHA256="{HASH}"/>{ledger}</BODY></ENVELOPE>"#
        )
    };
    let valid = r#"<LEDGER NAME="BRIDGE LEDGER" REMOTEID="remote-1"><PARENT>BRIDGE GROUP</PARENT><OPENINGBALANCE>0</OPENINGBALANCE></LEDGER>"#;
    let valid_result = parse_ledger_write_readback_with_evidence(&wrap(valid, 1));
    assert!(valid_result.is_ok(), "{valid_result:?}");

    let nested = format!("<COLLECTION>{valid}</COLLECTION>");
    assert!(parse_ledger_write_readback_with_evidence(&wrap(&nested, 1)).is_err());
    let duplicate_field = valid.replace("</LEDGER>", "<OPENINGBALANCE>0</OPENINGBALANCE></LEDGER>");
    assert!(parse_ledger_write_readback_with_evidence(&wrap(&duplicate_field, 1)).is_err());
    let duplicate_attribute = valid.replace(
        "REMOTEID=\"remote-1\"",
        "REMOTEID=\"remote-1\" REMOTEID=\"remote-1\"",
    );
    assert!(parse_ledger_write_readback_with_evidence(&wrap(&duplicate_attribute, 1)).is_err());
    let unexpected_attribute = valid.replace(
        "REMOTEID=\"remote-1\"",
        "REMOTEID=\"remote-1\" UNSAFE=\"1\"",
    );
    assert!(parse_ledger_write_readback_with_evidence(&wrap(&unexpected_attribute, 1)).is_err());

    let case_variant_duplicate = valid.replace(
        "REMOTEID=\"remote-1\"",
        "REMOTEID=\"remote-1\" remoteid=\"other\"",
    );
    assert!(parse_ledger_write_readback_with_evidence(&wrap(&case_variant_duplicate, 1)).is_err());

    let nested_context = wrap("", 0)
        .replace("<COMPANYCONTEXT", "<WRAPPER><COMPANYCONTEXT")
        .replace("/>", "/></WRAPPER>");
    assert!(parse_ledger_write_readback_with_evidence(&nested_context).is_err());

    let duplicate_status =
        wrap("", 0).replace("<STATUS>1</STATUS>", "<STATUS>0</STATUS><STATUS>1</STATUS>");
    assert!(parse_ledger_write_readback_with_evidence(&duplicate_status).is_err());
}
