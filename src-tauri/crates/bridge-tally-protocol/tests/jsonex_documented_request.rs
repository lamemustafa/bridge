#![cfg(feature = "jsonex-request-builder")]

use bridge_tally_protocol::jsonex_request::{
    build_documented_ledger_request_v1, build_documented_voucher_request_v1,
    JsonExRequestBuildError, JsonExRequestWireEncoding, JsonExResponseEncodingExpectation,
    ValidatedJsonExCompanyName, DOCUMENTED_LEDGER_REQUEST_PROFILE_V1,
    DOCUMENTED_VOUCHER_REQUEST_PROFILE_V1,
};

const LIMIT: usize = 128 * 1024;

fn synthetic_company() -> ValidatedJsonExCompanyName {
    ValidatedJsonExCompanyName::new("BRIDGE SYNTHETIC COMPANY")
        .expect("synthetic company must be valid")
}

#[test]
fn ledger_profile_preserves_exact_docx_keys_values_and_headers() {
    let request = build_documented_ledger_request_v1(
        &synthetic_company(),
        JsonExRequestWireEncoding::PlainAsciiUtf8,
        LIMIT,
    )
    .expect("documented request should build");
    let body: serde_json::Value = serde_json::from_slice(&request.body).expect("valid JSON");

    assert_eq!(request.profile_id, DOCUMENTED_LEDGER_REQUEST_PROFILE_V1);
    assert_eq!(request.headers.content_type, "application/json");
    assert_eq!(request.headers.version, "1");
    assert_eq!(request.headers.tally_request, "Export");
    assert_eq!(request.headers.request_type, "Collection");
    assert_eq!(request.headers.id, "Ledger");
    assert_eq!(body["static_variables"][0]["name"], "svExportFormat");
    assert_eq!(body["static_variables"][0]["value"], "jsonex");
    assert_eq!(body["static_variables"][1]["name"], "svCurrentCompany");
    assert_eq!(
        body["static_variables"][1]["value"],
        "BRIDGE SYNTHETIC COMPANY"
    );
    assert_eq!(
        body["fetch_List"],
        serde_json::json!(["Name", "Parent", "Closing Balance"])
    );
    assert!(body.get("fetchlist").is_none());
    assert!(body.get("fetch_list").is_none());
    assert!(!request.dispatch_eligible());
    assert!(!request.company_identity_verifiable_from_documented_response());
    assert!(!request.date_range_bound());
}

#[test]
fn voucher_profile_preserves_exact_fixed_tdl_definition() {
    let request = build_documented_voucher_request_v1(
        &synthetic_company(),
        JsonExRequestWireEncoding::PlainAsciiUtf8,
        LIMIT,
    )
    .expect("documented request should build");
    let body: serde_json::Value = serde_json::from_slice(&request.body).expect("valid JSON");

    assert_eq!(request.profile_id, DOCUMENTED_VOUCHER_REQUEST_PROFILE_V1);
    assert_eq!(request.headers.id, "TSPLVoucherColl");
    let definition = &body["tdlmessage"][0]["definitions"][0];
    assert_eq!(definition["metadata"]["name"], "TSPLVoucherColl");
    assert_eq!(definition["metadata"]["type"], "Collection");
    assert_eq!(
        definition["attributes"][0],
        serde_json::json!({"Type": "Voucher"})
    );
    assert_eq!(
        definition["attributes"][1],
        serde_json::json!({"Native Method": "VoucherNumber, VoucherTypeName, Date, Amount"})
    );
    assert!(body.get("repeat_variables").is_none());
    assert!(!request.dispatch_eligible());
    assert!(!request.date_range_bound());
}

#[test]
fn bom_profiles_bind_wire_bytes_headers_and_documented_response_expectation() {
    let company = ValidatedJsonExCompanyName::new("ब्रिज सिंथेटिक कंपनी")
        .expect("Unicode company must be preserved");
    let utf8 =
        build_documented_ledger_request_v1(&company, JsonExRequestWireEncoding::Utf8Bom, LIMIT)
            .expect("UTF-8 BOM profile should build");
    assert!(utf8.body.starts_with(&[0xef, 0xbb, 0xbf]));
    assert_eq!(utf8.headers.content_type, "application/json;charset=utf-8");
    assert_eq!(
        utf8.response_encoding_expectation,
        JsonExResponseEncodingExpectation::Utf16Le
    );
    let utf8_body: serde_json::Value =
        serde_json::from_slice(&utf8.body[3..]).expect("BOM body must be UTF-8 JSON");
    assert_eq!(utf8_body["static_variables"][1]["value"], company.as_str());

    let utf16 =
        build_documented_ledger_request_v1(&company, JsonExRequestWireEncoding::Utf16LeBom, LIMIT)
            .expect("UTF-16LE BOM profile should build");
    assert!(utf16.body.starts_with(&[0xff, 0xfe]));
    assert_eq!(
        utf16.headers.content_type,
        "application/json;charset=utf-16"
    );
    assert_eq!(
        utf16.response_encoding_expectation,
        JsonExResponseEncodingExpectation::Utf16Le
    );
    let units = utf16.body[2..]
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect::<Vec<_>>();
    let decoded = String::from_utf16(&units).expect("body must be UTF-16LE");
    let utf16_body: serde_json::Value = serde_json::from_str(&decoded).expect("valid JSON");
    assert_eq!(utf16_body, utf8_body);
}

#[test]
fn plain_profile_rejects_multilingual_company_without_transforming_it() {
    let company = ValidatedJsonExCompanyName::new("BRIDGE कंपनी").expect("valid Unicode name");
    let error = build_documented_ledger_request_v1(
        &company,
        JsonExRequestWireEncoding::PlainAsciiUtf8,
        LIMIT,
    )
    .err();
    assert_eq!(
        error,
        Some(JsonExRequestBuildError::MultilingualCompanyRequiresBomProfile)
    );
}

#[test]
fn company_validation_and_json_serialization_are_injection_safe() {
    for invalid in [
        "",
        "   ",
        "BRIDGE\0COMPANY",
        "BRIDGE\rCOMPANY",
        "BRIDGE\nCOMPANY",
    ] {
        assert!(matches!(
            ValidatedJsonExCompanyName::new(invalid),
            Err(JsonExRequestBuildError::InvalidCompanyName)
        ));
    }
    assert!(matches!(
        ValidatedJsonExCompanyName::new("A".repeat(256)),
        Err(JsonExRequestBuildError::InvalidCompanyName)
    ));

    let exact = "BRIDGE \"SYNTHETIC\" \\ COMPANY";
    let company = ValidatedJsonExCompanyName::new(exact).expect("quotes are JSON-escaped");
    let request = build_documented_ledger_request_v1(
        &company,
        JsonExRequestWireEncoding::PlainAsciiUtf8,
        LIMIT,
    )
    .expect("safe exact name should build");
    let body: serde_json::Value = serde_json::from_slice(&request.body).expect("valid JSON");
    assert_eq!(body["static_variables"][1]["value"], exact);
}

#[test]
fn byte_limits_and_errors_are_fail_closed_and_redacted() {
    let company = ValidatedJsonExCompanyName::new("BRIDGE PRIVATE SENTINEL")
        .expect("sentinel company must be valid");
    assert_eq!(
        build_documented_ledger_request_v1(&company, JsonExRequestWireEncoding::PlainAsciiUtf8, 0,)
            .err(),
        Some(JsonExRequestBuildError::InvalidByteLimit)
    );
    let error =
        build_documented_ledger_request_v1(&company, JsonExRequestWireEncoding::Utf16LeBom, 8)
            .err()
            .expect("small byte limit must fail");
    assert_eq!(error.safe_code(), "jsonex_request_too_large");
    assert!(!error.to_string().contains(company.as_str()));
}
