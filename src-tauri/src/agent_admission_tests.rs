use super::*;

#[test]
fn published_pattern_inventory_preserves_the_admitted_wire_shapes() {
    let accepted_dates = ["20260901", "2026-09-01", "2026-0901", "202609-01"];
    for date in accepted_dates {
        assert!(published_pattern_matches(DATE_WIRE_PATTERN, date), "{date}");
    }
    for rejected in ["2-0-2-6-0-9-0-1", "2026/09/01", "2026090", "202609011"] {
        assert!(
            !published_pattern_matches(DATE_WIRE_PATTERN, rejected),
            "{rejected}"
        );
    }
    assert!(published_pattern_matches(
        NONBLANK_PATTERN,
        "\u{2003}ledger"
    ));
    assert!(!published_pattern_matches(NONBLANK_PATTERN, " \u{2003}\t"));
    assert!(published_pattern_matches(
        BRIDGE_TRANSACTION_ID_PATTERN,
        "batch_20260901-1"
    ));
    assert!(!published_pattern_matches(
        BRIDGE_TRANSACTION_ID_PATTERN,
        "batch 20260901"
    ));
    let sha = "0123456789abcdef".repeat(4);
    assert!(published_pattern_matches(SHA256_HEX_PATTERN, &sha));
    for rejected in [
        &sha[..63],
        &sha.to_uppercase(),
        &format!("{sha}0"),
        &sha.replace('a', "g"),
    ] {
        assert!(
            !published_pattern_matches(SHA256_HEX_PATTERN, rejected),
            "{rejected}"
        );
    }

    fn patterns(value: &Value, found: &mut Vec<String>) {
        match value {
            Value::Object(object) => {
                if let Some(pattern) = object.get("pattern").and_then(Value::as_str) {
                    found.push(pattern.to_string());
                }
                for child in object.values() {
                    patterns(child, found);
                }
            }
            Value::Array(values) => {
                for child in values {
                    patterns(child, found);
                }
            }
            _ => {}
        }
    }

    let definitions = registered_tool_definitions(true, true);
    let schema = definitions
        .as_array()
        .and_then(|tools| tools.iter().find(|tool| tool["name"] == "voucher_presence"))
        .expect("voucher_presence tool");
    let mut found = Vec::new();
    patterns(&schema["inputSchema"], &mut found);
    found.sort();
    found.dedup();
    assert_eq!(
        found,
        vec![
            NONBLANK_PATTERN,
            DATE_WIRE_PATTERN,
            BRIDGE_TRANSACTION_ID_PATTERN
        ]
    );
}

#[test]
fn every_pattern_admission_reads_is_one_it_recognizes() {
    // validate_string_bounds refuses any pattern outside the recognized
    // vocabulary, so a published pattern missing from it refuses every value.
    // amends_batch_id shipped that way: the schema admitted it and every MCP
    // call was refused as argument_invalid before the build could run.
    let definitions = registered_tool_definitions(true, true);
    let mut checked = 0;
    for tool in definitions.as_array().unwrap() {
        let properties = tool["inputSchema"]["properties"].as_object();
        for (key, property) in properties.into_iter().flatten() {
            for fragment in [property, &property["items"]] {
                if fragment["type"] != "string" {
                    continue;
                }
                if let Some(pattern) = fragment["pattern"].as_str() {
                    assert!(
                        published_pattern_matcher(pattern).is_some(),
                        "{}.{key} publishes an unrecognized pattern {pattern}",
                        tool["name"]
                    );
                    checked += 1;
                }
            }
        }
    }
    assert!(checked > 0);
}

#[test]
fn the_batch_id_pattern_admits_exactly_what_the_build_admits() {
    // Two spellings of one rule live in this crate: the build's
    // `valid_batch_id`, which admission now applies, and the byte rule
    // `is_uuid_v4_lowercase` that `proposals_id` uses. They must agree.
    let valid = "bridge-2b1c9f4e-9d3a-4f71-8c2e-5a6b7c8d9e01";
    assert!(published_pattern_matches(BRIDGE_BATCH_ID_PATTERN, valid));
    let refused = [
        "bridge-2B1C9F4E-9D3A-4F71-8C2E-5A6B7C8D9E01",
        "2b1c9f4e-9d3a-4f71-8c2e-5a6b7c8d9e01",
        "bridge-2b1c9f4e-9d3a-1f71-8c2e-5a6b7c8d9e01",
        "bridge-2b1c9f4e-9d3a-4f71-cc2e-5a6b7c8d9e01",
        "bridge-2b1c9f4e9d3a4f718c2e5a6b7c8d9e01",
        "bridge-{2b1c9f4e-9d3a-4f71-8c2e-5a6b7c8d9e01}",
        "bridge-urn:uuid:2b1c9f4e-9d3a-4f71-8c2e-5a6b7c8d9e01",
        "bridge-00000000-0000-0000-0000-000000000000",
        "bridge-2b1c9f4e-9d3a-4f71-8c2e-5a6b7c8d9e01\n",
        "bridge-2b1c9f4e-9d3a-4f71-8c2e-5a6b7c8d9e0\u{e9}",
    ];
    for text in refused {
        assert!(
            !published_pattern_matches(BRIDGE_BATCH_ID_PATTERN, text),
            "{text}"
        );
    }
    for text in std::iter::once(valid.to_string())
        .chain(refused.iter().map(|text| (*text).to_string()))
        .chain((0..256).map(|_| format!("bridge-{}", uuid::Uuid::new_v4())))
    {
        assert_eq!(
            agent_import::valid_batch_id(&text),
            text.strip_prefix("bridge-")
                .is_some_and(is_uuid_v4_lowercase),
            "{text}"
        );
    }
}

#[tokio::test]
async fn voucher_type_selector_is_bounded_before_any_tally_read() {
    let directory = tempfile::tempdir().unwrap();
    let server = Server::new(Settings {
        endpoint: TallyEndpointConfig {
            host: "127.0.0.1".into(),
            port: 9,
        },
        data_dir: directory.path().to_path_buf(),
        max_rows: 500,
        max_bytes: 200_000,
        redaction: Redaction::None,
        import_enabled: false,
        writes_enabled: false,
    });
    let mut args = json!({"company_guid":"00000000-0000-4000-8000-000000000001",
        "from":"20260901","to":"20260902","voucher_type":"名".repeat(1025)});
    let response = server.call_tool_response("vouchers", args.clone()).await;
    assert_eq!(response.value["isError"], true);
    assert_eq!(
        response.value["structuredContent"]["result"]["error"]["code"],
        "argument_invalid:voucher_type"
    );
    assert_eq!(response.value["structuredContent"]["evidence"]["bytes"], 0);
    args["voucher_type"] = json!("名".repeat(1024));
    assert!(validate_tool_arguments("vouchers", &args).is_ok());
}

#[tokio::test]
async fn unqualified_change_feed_is_hidden_and_direct_calls_refuse_before_tally() {
    for import_enabled in [false, true] {
        assert!(!tool_definitions(import_enabled, false)
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["name"] == "changed_since"));
    }
    let directory = tempfile::tempdir().unwrap();
    let server = Server::new(Settings {
        endpoint: TallyEndpointConfig {
            host: "127.0.0.1".into(),
            port: 9,
        },
        data_dir: directory.path().to_path_buf(),
        max_rows: 500,
        max_bytes: 200_000,
        redaction: Redaction::None,
        import_enabled: true,
        writes_enabled: false,
    });
    // A dispatched read against this unavailable endpoint would return a
    // transport/company failure, not the explicit admission refusal.
    for arguments in [
        json!({}),
        json!({"company_guid":"00000000-0000-4000-8000-000000000001"}),
    ] {
        let result = server.call_tool_response("changed_since", arguments).await;
        assert_eq!(result.value["isError"], true);
        assert_eq!(
            result.value["structuredContent"]["result"]["error"]["code"],
            "changed_since_unqualified"
        );
        assert_eq!(result.value["structuredContent"]["evidence"]["bytes"], 0);
    }
}

#[tokio::test]
async fn verification_remains_catalogued_and_admitted_when_import_and_writes_are_disabled() {
    let definitions = tool_definitions(false, false);
    let names = definitions
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect::<Vec<_>>();
    assert!(names.contains(&"verify_import"));
    assert!(!names.contains(&"build_import_xml"));
    assert!(!names.contains(&"post_import"));

    let directory = tempfile::tempdir().unwrap();
    let server = Server::new(Settings {
        endpoint: TallyEndpointConfig {
            host: "127.0.0.1".into(),
            port: 9,
        },
        data_dir: directory.path().to_path_buf(),
        max_rows: 500,
        max_bytes: 200_000,
        redaction: Redaction::None,
        import_enabled: false,
        writes_enabled: false,
    });
    let response = server.call_tool_response("verify_import", json!({})).await;
    assert_eq!(response.value["isError"], true);
    assert_eq!(
        response.value["structuredContent"]["result"]["error"]["code"],
        "company_guid_required"
    );
    assert_eq!(response.value["structuredContent"]["evidence"]["bytes"], 0);
}

#[tokio::test]
async fn master_validation_rejects_unbounded_and_blank_names_before_tally() {
    let directory = tempfile::tempdir().unwrap();
    let server = Server::new(Settings {
        endpoint: TallyEndpointConfig {
            host: "127.0.0.1".into(),
            port: 9,
        },
        data_dir: directory.path().to_path_buf(),
        max_rows: 500,
        max_bytes: 200_000,
        redaction: Redaction::None,
        import_enabled: false,
        writes_enabled: false,
    });
    for ledgers in [
        json!([""]),
        json!([]),
        json!([" \u{2003}\t"]),
        json!(["x".repeat(1025)]),
        json!(vec!["Ledger"; 101]),
    ] {
        let result = server
            .call_tool_response(
                "validate_masters",
                json!({
                    "company_guid":"00000000-0000-4000-8000-000000000001", "ledgers":ledgers,
                }),
            )
            .await;
        assert_eq!(result.value["isError"], true);
        assert_eq!(
            result.value["structuredContent"]["result"]["error"]["code"],
            "argument_invalid:ledgers"
        );
        assert_eq!(result.value["structuredContent"]["evidence"]["bytes"], 0);
    }
    assert!(validate_tool_arguments(
        "validate_masters",
        &json!({
            "company_guid":"00000000-0000-4000-8000-000000000001", "ledgers":vec!["Valid Ledger"; 100],
        })
    )
    .is_ok());
}

#[tokio::test]
async fn ledger_selectors_are_bounded_before_company_or_catalogue_reads() {
    let directory = tempfile::tempdir().unwrap();
    let server = Server::new(Settings {
        endpoint: TallyEndpointConfig {
            host: "127.0.0.1".into(),
            port: 9,
        },
        data_dir: directory.path().to_path_buf(),
        max_rows: 500,
        max_bytes: 200_000,
        redaction: Redaction::None,
        import_enabled: false,
        writes_enabled: false,
    });
    for tool in ["vouchers", "ledger_movement"] {
        for ledger in ["x".repeat(1025), " ".into(), String::new()] {
            let args = json!({"company_guid":"00000000-0000-4000-8000-000000000001",
                "from":"20260901","to":"20260902","ledger":ledger});
            let response = server.call_tool_response(tool, args).await;
            assert_eq!(
                response.value["structuredContent"]["result"]["error"]["code"],
                "argument_invalid:ledger"
            );
            assert_eq!(response.value["structuredContent"]["evidence"]["bytes"], 0);
        }
        assert!(validate_tool_arguments(
            tool,
            &json!({"company_guid":"00000000-0000-4000-8000-000000000001",
            "from":"20260901","to":"20260902","ledger":"名".repeat(1024)})
        )
        .is_ok());
    }
}

#[tokio::test]
async fn oversized_unknown_property_cannot_expand_response_or_retained_evidence() {
    let directory = tempfile::tempdir().unwrap();
    let server = Server::new(Settings {
        endpoint: TallyEndpointConfig {
            host: "127.0.0.1".into(),
            port: 9,
        },
        data_dir: directory.path().to_path_buf(),
        max_rows: 500,
        max_bytes: 5_000_000,
        redaction: Redaction::None,
        import_enabled: false,
        writes_enabled: false,
    });
    for key in ["unknown".to_string(), "x".repeat(1_000_000)] {
        let value = server.call_tool("tally_status", json!({key: null})).await;
        assert!(serde_json::to_vec(&value).unwrap().len() < 2_000);
        assert!(
            value["structuredContent"]["result"]["error"]["code"].as_str()
                == Some("argument_unknown")
        );
        assert_eq!(value["structuredContent"]["evidence"]["bytes"], 0);
    }
    let store = server.evidence.lock().unwrap();
    assert_eq!(store.records.len(), 2);
    assert!(serde_json::to_vec(&store.records).unwrap().len() < 2_000);
    assert!(store
        .records
        .iter()
        .all(|record| record.reason_code.as_deref() == Some("argument_unknown")));
    assert!(server.runtime.snapshots().unwrap().is_empty());
}
