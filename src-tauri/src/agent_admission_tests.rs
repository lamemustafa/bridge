use super::*;

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
