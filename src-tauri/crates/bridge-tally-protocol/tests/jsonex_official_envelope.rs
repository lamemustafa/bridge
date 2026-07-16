#![cfg(feature = "jsonex-parser")]

use bridge_tally_protocol::{
    jsonex::{
        parse_documented_ledger_collection_v1, parse_documented_voucher_collection_v1,
        JsonExExpectedEncoding, JsonExLimits, JsonExProtocolError, JsonExTextPresence,
        DOCUMENTED_LEDGER_COLLECTION_PROFILE_V1, DOCUMENTED_VOUCHER_COLLECTION_PROFILE_V1,
    },
    TallyTextEncoding,
};

const LEDGER: &str = include_str!("fixtures/jsonex/ledger_collection_sanitized.json");
const VOUCHER: &str = include_str!("fixtures/jsonex/voucher_collection_nested_sanitized.json");

fn parse_ledger(
    bytes: &[u8],
) -> Result<bridge_tally_protocol::jsonex::UnboundJsonExLedgerCollection, JsonExProtocolError> {
    parse_documented_ledger_collection_v1(
        bytes,
        "application/json; charset=utf-8",
        JsonExExpectedEncoding::Utf8,
        JsonExLimits::default(),
    )
}

fn parse_ledger_text(
    text: &str,
) -> Result<bridge_tally_protocol::jsonex::UnboundJsonExLedgerCollection, JsonExProtocolError> {
    parse_ledger(text.as_bytes())
}

fn utf16_le_bom(text: &str) -> Vec<u8> {
    let mut bytes = vec![0xff, 0xfe];
    for unit in text.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    bytes
}

fn utf16_be_bom(text: &str) -> Vec<u8> {
    let mut bytes = vec![0xfe, 0xff];
    for unit in text.encode_utf16() {
        bytes.extend_from_slice(&unit.to_be_bytes());
    }
    bytes
}

#[test]
fn ledger_fixture_preserves_absent_empty_zero_and_multilingual_values() {
    let parsed = parse_ledger(LEDGER.as_bytes()).expect("documented ledger shape should parse");

    assert_eq!(
        parsed.evidence.profile_id,
        DOCUMENTED_LEDGER_COLLECTION_PROFILE_V1
    );
    assert_eq!(parsed.evidence.encoding, TallyTextEncoding::Utf8);
    assert_eq!(parsed.evidence.record_count, 2);
    assert_eq!(parsed.records.len(), 2);
    assert_eq!(parsed.records[0].name, "BRIDGE SYNTHETIC LEDGER A");
    assert!(matches!(
        &parsed.records[0].parent,
        JsonExTextPresence::Value(value) if value == "BRIDGE SYNTHETIC GROUP"
    ));
    assert!(matches!(
        &parsed.records[0].closing_balance,
        JsonExTextPresence::Value(value) if value == "-243900.00"
    ));
    assert!(matches!(
        &parsed.records[0].opening_balance,
        JsonExTextPresence::Value(value) if value == "0.00"
    ));
    assert_eq!(
        parsed.records[0].language_names[0][1],
        "\u{092c}\u{094d}\u{0930}\u{093f}\u{091c} \u{0938}\u{093f}\u{0902}\u{0925}\u{0947}\u{091f}\u{093f}\u{0915} \u{0916}\u{093e}\u{0924}\u{093e}"
    );
    assert!(matches!(
        parsed.records[1].parent,
        JsonExTextPresence::Absent
    ));
    assert!(matches!(
        parsed.records[1].closing_balance,
        JsonExTextPresence::Empty
    ));
    assert!(matches!(
        parsed.records[1].opening_balance,
        JsonExTextPresence::Absent
    ));
}

#[test]
fn voucher_fixture_validates_documented_nested_allocation_shapes() {
    let parsed = parse_documented_voucher_collection_v1(
        VOUCHER.as_bytes(),
        "application/json",
        JsonExExpectedEncoding::Utf8,
        JsonExLimits::default(),
    )
    .expect("documented voucher shape should parse");

    assert_eq!(
        parsed.evidence.profile_id,
        DOCUMENTED_VOUCHER_COLLECTION_PROFILE_V1
    );
    assert_eq!(parsed.evidence.record_count, 1);
    let voucher = &parsed.records[0];
    assert_eq!(voucher.date_yyyymmdd, "20250401");
    assert_eq!(voucher.voucher_type, "Sales");
    assert_eq!(
        voucher.voucher_number.as_deref(),
        Some("BRIDGE-SYNTHETIC-1")
    );
    assert!(matches!(
        &voucher.amount,
        JsonExTextPresence::Value(value) if value == "-180000.00"
    ));
    assert_eq!(voucher.all_ledger_entry_count, 1);
    assert_eq!(voucher.inventory_entry_count, 1);
    assert_eq!(voucher.invoice_ledger_entry_count, 1);
    assert_eq!(voucher.batch_allocation_count, 1);
    assert_eq!(voucher.accounting_allocation_count, 1);
    assert_eq!(voucher.bill_allocation_count, 1);
}

#[test]
fn documented_collection_metadata_values_are_profile_exact() {
    for changed in [
        LEDGER.replacen(
            r#""is_mst_dep_type": true"#,
            r#""is_mst_dep_type": false"#,
            1,
        ),
        LEDGER.replacen(r#""mst_dep_type": "8""#, r#""mst_dep_type": "9""#, 1),
    ] {
        assert_eq!(
            parse_ledger_text(&changed).err(),
            Some(JsonExProtocolError::ProfileMismatch)
        );
    }

    for changed in [
        VOUCHER.replacen(
            r#""is_cmp_dep_type": true"#,
            r#""is_cmp_dep_type": false"#,
            1,
        ),
        VOUCHER.replacen(r#""cmp_locus": 4"#, r#""cmp_locus": 5"#, 1),
        VOUCHER.replacen(r#""cmp_dep_type": 64"#, r#""cmp_dep_type": 65"#, 1),
    ] {
        assert_eq!(
            parse_documented_voucher_collection_v1(
                changed.as_bytes(),
                "application/json",
                JsonExExpectedEncoding::Utf8,
                JsonExLimits::default(),
            )
            .err(),
            Some(JsonExProtocolError::ProfileMismatch)
        );
    }
}

#[test]
fn accepts_utf8_bom_and_utf16_le_only_when_the_contract_matches() {
    let mut utf8_bom = vec![0xef, 0xbb, 0xbf];
    utf8_bom.extend_from_slice(LEDGER.as_bytes());
    let parsed = parse_ledger(&utf8_bom).expect("UTF-8 BOM is documented");
    assert_eq!(parsed.evidence.encoding, TallyTextEncoding::Utf8Bom);

    let utf16 = utf16_le_bom(LEDGER);
    let parsed = parse_documented_ledger_collection_v1(
        &utf16,
        "application/json; charset=utf-16",
        JsonExExpectedEncoding::Utf16Le,
        JsonExLimits::default(),
    )
    .expect("UTF-16LE BOM is documented");
    assert_eq!(parsed.evidence.encoding, TallyTextEncoding::Utf16LeBom);

    assert_eq!(
        parse_documented_ledger_collection_v1(
            LEDGER.as_bytes(),
            "application/json",
            JsonExExpectedEncoding::Utf16Le,
            JsonExLimits::default(),
        )
        .err(),
        Some(JsonExProtocolError::EncodingMismatch)
    );
    assert_eq!(
        parse_documented_ledger_collection_v1(
            &utf16,
            "application/json; charset=utf-8",
            JsonExExpectedEncoding::Utf16Le,
            JsonExLimits::default(),
        )
        .err(),
        Some(JsonExProtocolError::EncodingMismatch)
    );
}

#[test]
fn rejects_unsupported_or_invalid_encodings_and_content_types() {
    assert_eq!(
        parse_documented_ledger_collection_v1(
            &utf16_be_bom(LEDGER),
            "application/json; charset=utf-16",
            JsonExExpectedEncoding::Utf16Le,
            JsonExLimits::default(),
        )
        .err(),
        Some(JsonExProtocolError::UnsupportedEncoding)
    );
    assert_eq!(
        parse_documented_ledger_collection_v1(
            &[0xff, 0xfe, b'{'],
            "application/json; charset=utf-16",
            JsonExExpectedEncoding::Utf16Le,
            JsonExLimits::default(),
        )
        .err(),
        Some(JsonExProtocolError::InvalidEncoding)
    );
    assert_eq!(
        parse_documented_ledger_collection_v1(
            LEDGER.as_bytes(),
            "text/json",
            JsonExExpectedEncoding::Utf8,
            JsonExLimits::default(),
        )
        .err(),
        Some(JsonExProtocolError::UnsupportedContentType)
    );
    for malformed in [
        "application/json; charset=\"utf-8",
        "application/json; charset=utf-8\"",
        "application/json; charset=\"utf\\-8\"",
        "application/json\r\n; charset=utf-8",
        "application/json; charset=\rutf-8",
        "application/json; \tcharset=utf-8",
    ] {
        assert_eq!(
            parse_documented_ledger_collection_v1(
                LEDGER.as_bytes(),
                malformed,
                JsonExExpectedEncoding::Utf8,
                JsonExLimits::default(),
            )
            .err(),
            Some(JsonExProtocolError::UnsupportedContentType)
        );
    }
}

#[test]
fn application_failure_is_classified_before_success_payload_shape() {
    for body in [
        r#"{"status":"0"}"#,
        r#"{"status":"0","result":{"unexpected":"BRIDGE SECRET SENTINEL"}}"#,
        r#"{"status":"0","detail":1.5}"#,
        r#"{"status":"0","detail":1e400}"#,
    ] {
        assert_eq!(
            parse_ledger_text(body).err(),
            Some(JsonExProtocolError::ApplicationRejected)
        );
    }

    assert_eq!(
        parse_ledger_text(r#"{"data":{}}"#).err(),
        Some(JsonExProtocolError::StatusMissing)
    );
    assert_eq!(
        parse_ledger_text(r#"{"status":0}"#).err(),
        Some(JsonExProtocolError::StatusInvalid)
    );
    assert_eq!(
        parse_ledger_text(r#"{"status":"2"}"#).err(),
        Some(JsonExProtocolError::StatusInvalid)
    );
    assert_eq!(
        parse_ledger_text(r#"{"status":"1","tallymessage":[]}"#).err(),
        Some(JsonExProtocolError::WrongContainer)
    );
}

#[test]
fn duplicate_keys_are_rejected_recursively() {
    let duplicate_root = LEDGER.replacen(r#""status": "1""#, r#""status": "1", "status": "1""#, 1);
    assert_eq!(
        parse_ledger_text(&duplicate_root).err(),
        Some(JsonExProtocolError::DuplicateField)
    );

    let duplicate_nested = LEDGER.replacen(
        r#""type": "Amount",
          "value": "-243900.00""#,
        r#""type": "Amount", "type": "Amount",
          "value": "-243900.00""#,
        1,
    );
    assert_eq!(
        parse_ledger_text(&duplicate_nested).err(),
        Some(JsonExProtocolError::DuplicateField)
    );
}

#[test]
fn malformed_trailing_and_wrong_container_documents_are_rejected() {
    assert_eq!(
        parse_ledger_text("{").err(),
        Some(JsonExProtocolError::MalformedJson)
    );
    assert_eq!(
        parse_ledger_text(&format!("{LEDGER} true")).err(),
        Some(JsonExProtocolError::MalformedJson)
    );
    let mut wrong_container: serde_json::Value =
        serde_json::from_str(LEDGER).expect("fixture must be JSON");
    wrong_container["data"]["collection"] = serde_json::json!({});
    assert_eq!(
        parse_ledger_text(&wrong_container.to_string()).err(),
        Some(JsonExProtocolError::WrongContainer)
    );
}

#[test]
fn typed_wrappers_require_exact_labels_and_value_kinds() {
    let wrong_label = LEDGER.replacen(r#""type": "Amount""#, r#""type": "String""#, 1);
    assert_eq!(
        parse_ledger_text(&wrong_label).err(),
        Some(JsonExProtocolError::TypedValueMismatch)
    );

    let null_value = LEDGER.replacen(r#""value": "-243900.00""#, r#""value": null"#, 1);
    assert_eq!(
        parse_ledger_text(&null_value).err(),
        Some(JsonExProtocolError::TypedValueMismatch)
    );

    let imprecise = LEDGER.replacen(r#""value": "-243900.00""#, r#""value": -243900.00"#, 1);
    assert_eq!(
        parse_ledger_text(&imprecise).err(),
        Some(JsonExProtocolError::TypedValueMismatch)
    );

    let mut singleton: serde_json::Value =
        serde_json::from_str(VOUCHER).expect("fixture must be JSON");
    let first_entry = singleton["data"]["collection"][0]["allledgerentries"][0].clone();
    singleton["data"]["collection"][0]["allledgerentries"] = first_entry;
    assert_eq!(
        parse_documented_voucher_collection_v1(
            singleton.to_string().as_bytes(),
            "application/json",
            JsonExExpectedEncoding::Utf8,
            JsonExLimits::default(),
        )
        .err(),
        Some(JsonExProtocolError::ProfileMismatch)
    );

    let mut empty_required_amount: serde_json::Value =
        serde_json::from_str(VOUCHER).expect("fixture must be JSON");
    empty_required_amount["data"]["collection"][0]["allledgerentries"][0]["amount"]["value"] =
        serde_json::json!("");
    assert_eq!(
        parse_documented_voucher_collection_v1(
            empty_required_amount.to_string().as_bytes(),
            "application/json",
            JsonExExpectedEncoding::Utf8,
            JsonExLimits::default(),
        )
        .err(),
        Some(JsonExProtocolError::TypedValueMismatch)
    );
}

#[test]
fn exact_decimal_date_and_integer_rules_reject_ambiguous_values() {
    let exponent = LEDGER.replacen(r#""value": "-243900.00""#, r#""value": "-2.439e5""#, 1);
    assert_eq!(
        parse_ledger_text(&exponent).err(),
        Some(JsonExProtocolError::TypedValueMismatch)
    );

    let bad_date = VOUCHER.replacen(r#""value": "20250401""#, r#""value": "20250229""#, 1);
    assert_eq!(
        parse_documented_voucher_collection_v1(
            bad_date.as_bytes(),
            "application/json",
            JsonExExpectedEncoding::Utf8,
            JsonExLimits::default(),
        )
        .err(),
        Some(JsonExProtocolError::TypedValueMismatch)
    );

    let signed_number = VOUCHER.replacen(r#""value": " 50""#, r#""value": " -50""#, 1);
    assert_eq!(
        parse_documented_voucher_collection_v1(
            signed_number.as_bytes(),
            "application/json",
            JsonExExpectedEncoding::Utf8,
            JsonExLimits::default(),
        )
        .err(),
        Some(JsonExProtocolError::TypedValueMismatch)
    );
}

#[test]
fn every_resource_limit_is_fail_closed() {
    let cases = [
        (
            JsonExLimits {
                max_encoded_bytes: LEDGER.len() - 1,
                ..JsonExLimits::default()
            },
            JsonExProtocolError::ResponseTooLarge,
        ),
        (
            JsonExLimits {
                max_decoded_bytes: LEDGER.len() - 1,
                ..JsonExLimits::default()
            },
            JsonExProtocolError::DecodedResponseTooLarge,
        ),
        (
            JsonExLimits {
                max_depth: 2,
                ..JsonExLimits::default()
            },
            JsonExProtocolError::ResourceLimitExceeded,
        ),
        (
            JsonExLimits {
                max_nodes: 5,
                ..JsonExLimits::default()
            },
            JsonExProtocolError::ResourceLimitExceeded,
        ),
        (
            JsonExLimits {
                max_object_members: 1,
                ..JsonExLimits::default()
            },
            JsonExProtocolError::ResourceLimitExceeded,
        ),
        (
            JsonExLimits {
                max_array_items: 1,
                ..JsonExLimits::default()
            },
            JsonExProtocolError::ResourceLimitExceeded,
        ),
        (
            JsonExLimits {
                max_string_bytes: 5,
                ..JsonExLimits::default()
            },
            JsonExProtocolError::ResourceLimitExceeded,
        ),
        (
            JsonExLimits {
                max_records: 1,
                ..JsonExLimits::default()
            },
            JsonExProtocolError::RecordLimitExceeded,
        ),
        (
            JsonExLimits {
                max_record_decoded_bytes: 100,
                ..JsonExLimits::default()
            },
            JsonExProtocolError::RecordTooLarge,
        ),
    ];

    for (limits, expected) in cases {
        let result = parse_documented_ledger_collection_v1(
            LEDGER.as_bytes(),
            "application/json",
            JsonExExpectedEncoding::Utf8,
            limits,
        );
        assert_eq!(result.err(), Some(expected));
    }

    let zero_limit = JsonExLimits {
        max_nodes: 0,
        ..JsonExLimits::default()
    };
    assert_eq!(
        parse_documented_ledger_collection_v1(
            LEDGER.as_bytes(),
            "application/json",
            JsonExExpectedEncoding::Utf8,
            zero_limit,
        )
        .err(),
        Some(JsonExProtocolError::ResourceLimitExceeded)
    );
}

#[test]
fn array_limit_rejects_before_deserializing_the_over_limit_value() {
    let mut value: serde_json::Value = serde_json::from_str(LEDGER).expect("fixture must be JSON");
    value["data"]["collection"]
        .as_array_mut()
        .expect("collection must be an array")
        .push(serde_json::json!(1.5));
    let limits = JsonExLimits {
        max_array_items: 2,
        ..JsonExLimits::default()
    };

    assert_eq!(
        parse_documented_ledger_collection_v1(
            value.to_string().as_bytes(),
            "application/json",
            JsonExExpectedEncoding::Utf8,
            limits,
        )
        .err(),
        Some(JsonExProtocolError::ResourceLimitExceeded)
    );
}

#[test]
fn parser_errors_expose_only_stable_safe_codes() {
    let secret = "BRIDGE-DO-NOT-ECHO-SENTINEL";
    let body = format!(r#"{{"status":"0","detail":"{secret}"}}"#);
    let error = match parse_ledger_text(&body) {
        Err(error) => error,
        Ok(_) => panic!("status zero must fail"),
    };
    assert_eq!(error.to_string(), "jsonex_application_rejected");
    assert_eq!(error.safe_code(), "jsonex_application_rejected");
    assert!(!error.to_string().contains(secret));
}

#[test]
fn checked_in_corpus_contains_only_synthetic_identity_markers() {
    let corpus = format!("{LEDGER}\n{VOUCHER}");
    let normalized_paths = corpus.replace("\\\\", "\\");
    for forbidden in [
        "C:\\Users\\",
        "C:/Users/",
        "@gmail.com",
        "@outlook.com",
        "GSTIN",
        "PAN",
        "BANK ACCOUNT",
        "MOBILE NO",
    ] {
        let forbidden = forbidden.to_ascii_uppercase();
        assert!(!corpus.to_ascii_uppercase().contains(&forbidden));
        assert!(!normalized_paths.to_ascii_uppercase().contains(&forbidden));
    }
    assert!(corpus.contains("BRIDGE SYNTHETIC"));
}
