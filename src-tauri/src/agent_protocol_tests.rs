use super::*;
use std::path::Path;

#[path = "agent_protocol_cap_tests.rs"]
mod cap_tests;

fn server(path: &Path) -> Server {
    Server::new(Settings {
        endpoint: TallyEndpointConfig {
            host: "127.0.0.1".into(),
            port: 9,
        },
        data_dir: path.to_path_buf(),
        max_rows: 10,
        max_bytes: 200_000,
        redaction: Redaction::MaskParties,
        import_enabled: false,
    })
}

fn initialize(version: &str) -> Value {
    json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
        "protocolVersion":version,"capabilities":{},"clientInfo":{"name":"regression","version":"1"}
    }})
}

async fn session(server: Server, requests: &[Value]) -> Vec<Value> {
    let input = requests
        .iter()
        .map(|value| format!("{value}\n"))
        .collect::<String>();
    let mut output = Vec::new();
    serve_stdio(server, BufReader::new(input.as_bytes()), &mut output)
        .await
        .expect("stdio session");
    String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[tokio::test]
async fn negotiates_fallback_and_returns_complete_text_to_legacy_clients() {
    for (requested, expected) in [
        ("2024-11-05", "2024-11-05"),
        ("2025-06-18", "2025-06-18"),
        ("2025-11-25", "2025-06-18"),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let responses = session(server(directory.path()), &[
            initialize(requested),
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"voucher_schema","arguments":{}}}),
        ]).await;
        assert_eq!(responses.len(), 2);
        assert_eq!(responses[0]["result"]["protocolVersion"], expected);
        let result = &responses[1]["result"];
        assert_eq!(result["isError"], false);
        let text: Value = serde_json::from_str(result["content"][0]["text"].as_str().unwrap())
            .expect("legacy clients receive JSON");
        assert_eq!(text, result["structuredContent"]);
        assert!(text["result"].is_object());
        // Receipt hashes and counts must cover the duplicated content and newline.
        let receipt: Value = serde_json::from_str(
            fs::read_to_string(directory.path().join("agent-egress.jsonl"))
                .unwrap()
                .lines()
                .next()
                .unwrap(),
        )
        .unwrap();
        let wire = format!("{}\n", responses[1]);
        assert_eq!(receipt["bytes_prepared"], wire.len());
        assert_eq!(receipt["response_sha256"], sha256_hex(wire.as_bytes()));
    }
}

#[tokio::test]
async fn rejects_invalid_envelopes_and_preinit_tools_without_dispatch() {
    let directory = tempfile::tempdir().unwrap();
    let responses = session(server(directory.path()), &[
        json!({"jsonrpc":"2.0","id":0,"method":"tools/call","params":{"name":"voucher_schema"}}),
        json!({"id":1,"method":"tools/call","params":{"name":"voucher_schema"}}),
        json!({"jsonrpc":"2.0","id":null,"method":"tools/call","params":{"name":"voucher_schema"}}),
        json!({"jsonrpc":"2.0","id":true,"method":"tools/call","params":{"name":"voucher_schema"}}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":[]}),
        initialize("2025-06-18"),
        json!({"jsonrpc":"2.0","id":3,"method":"ping"}),
    ]).await;
    assert_eq!(responses[0]["error"]["message"], "initialize_required");
    for response in &responses[1..5] {
        assert_eq!(response["error"]["code"], -32600);
        assert!(response["id"].is_null());
    }
    assert!(responses[5]["result"]["protocolVersion"].is_string());
    assert_eq!(responses[6]["result"], json!({}));
    assert!(!directory.path().join("agent-egress.jsonl").exists());
}

#[tokio::test]
async fn oversized_and_malformed_frames_recover_to_next_request() {
    let directory = tempfile::tempdir().unwrap();
    let input = format!(
        "{}\n{{\n{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}}\n",
        "x".repeat(MAX_REQUEST_BYTES)
    );
    let mut output = Vec::new();
    serve_stdio(
        server(directory.path()),
        BufReader::with_capacity(1024, input.as_bytes()),
        &mut output,
    )
    .await
    .unwrap();
    let responses: Vec<Value> = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(responses.len(), 3);
    assert_eq!(responses[0]["error"]["message"], "request_too_large");
    assert_eq!(responses[1]["error"]["code"], -32700);
    assert!(responses[1]["id"].is_null());
    assert_eq!(responses[2]["result"], json!({}));
}

#[tokio::test]
async fn oversized_request_id_is_refused_before_a_tool_or_receipt() {
    let directory = tempfile::tempdir().unwrap();
    let mut server = server(directory.path());
    server.settings.max_bytes = 256;
    let responses = session(server, &[
        initialize("2025-06-18"),
        json!({"jsonrpc":"2.0","id":"x".repeat(256),"method":"tools/call","params":{"name":"voucher_schema"}}),
    ]).await;
    assert_eq!(responses[1]["error"]["message"], "request_id_too_large");
    assert!(format!("{}\n", responses[1]).len() <= 256);
    assert!(!directory.path().join("agent-egress.jsonl").exists());
}

#[tokio::test]
async fn unknown_tools_and_methods_return_protocol_errors_with_exact_refusal_receipts() {
    let directory = tempfile::tempdir().unwrap();
    let responses = session(server(directory.path()), &[
        initialize("2025-06-18"),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"absent_tool","arguments":{}}}),
        json!({"jsonrpc":"2.0","id":3,"method":"absent/method"}),
        json!({"jsonrpc":"2.0","id":4,"method":"ping"}),
    ]).await;
    assert_eq!(responses[1]["error"]["code"], -32602);
    assert_eq!(responses[1]["error"]["message"], "tool_not_found");
    assert_eq!(responses[2]["error"]["code"], -32601);
    assert_eq!(responses[3]["result"], json!({}));
    let receipt: Value = serde_json::from_str(
        fs::read_to_string(directory.path().join("agent-egress.jsonl"))
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(receipt["rows_prepared"], 0);
    assert_eq!(receipt["tool"], "unknown");
    assert_eq!(receipt["tool_name_sha256"], sha256_hex(b"absent_tool"));
    assert_eq!(receipt["fields_prepared"], json!([]));
    assert_eq!(
        receipt["response_sha256"],
        sha256_hex(format!("{}\n", responses[1]).as_bytes())
    );
}

#[tokio::test]
async fn oversized_untrusted_names_and_selectors_never_expand_receipts() {
    let directory = tempfile::tempdir().unwrap();
    let name = "unrecognized".repeat(100_000);
    let selector = "unverified".repeat(100_000);
    let args = json!({"company_guid":selector});
    let responses = session(server(directory.path()), &[
        initialize("2025-06-18"),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":name,"arguments":args}}),
        json!({"jsonrpc":"2.0","method":"tools/call","params":{"name":name,"arguments":args}}),
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"voucher_schema","arguments":args}}),
        json!({"jsonrpc":"2.0","method":"tools/call","params":{"name":"voucher_schema","arguments":args}}),
        json!({"jsonrpc":"2.0","id":4,"method":"ping"}),
    ]).await;
    assert_eq!(responses[1]["error"]["message"], "tool_not_found");
    assert_eq!(responses.last().unwrap()["result"], json!({}));
    assert_eq!(responses[2]["result"]["isError"], true);
    assert_eq!(
        responses[2]["result"]["structuredContent"]["result"]["error"]["code"],
        "argument_unknown"
    );
    let path = directory.path().join("agent-egress.jsonl");
    let bytes = fs::read(&path).unwrap();
    assert!(bytes.len() < 16_384);
    let receipts: Vec<Value> = std::str::from_utf8(&bytes)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(receipts.len(), 6);
    let preparations = receipts
        .iter()
        .filter(|record| record["record_type"] != "stdio_write_completed")
        .collect::<Vec<_>>();
    assert_eq!(preparations.len(), 4);
    for (index, receipt) in preparations.iter().enumerate() {
        assert_eq!(receipt["args_sha256"], sha256_json(&args));
        assert!(receipt["company_guid"].is_null());
        if index < 2 {
            assert_eq!(receipt["tool"], "unknown");
            assert_eq!(receipt["tool_name_sha256"], sha256_hex(name.as_bytes()));
        } else {
            assert_eq!(receipt["tool"], "voucher_schema");
            assert!(receipt.get("tool_name_sha256").is_none());
        }
    }
    assert_eq!(egress::read_egress_tail(&path, 4).unwrap().records.len(), 4);
}

#[tokio::test]
async fn incomplete_receipt_log_stops_session_but_preserves_durable_batch_recovery() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("agent-egress.jsonl");
    fs::write(&path, b"{\"partial").unwrap();
    let server = server(directory.path());
    let batch_id = "12345678-abcd-4abc-8abc-123456789012";
    let mut output = Vec::new();
    let result = finish_response(
        &server,
        &mut output,
        json!(2),
        Ok(json!({})),
        Some(EgressContext {
            evidence: None,
            tool: "build_import_xml".into(),
            args_sha256: sha256_json(&json!({})),
            company_guid: None,
        }),
        Some(batch_id.into()),
        true,
    )
    .await;
    assert_eq!(result, Err("egress_log_incomplete".into()));
    let response: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(response["error"]["message"], "egress_log_incomplete");
    assert_eq!(response["error"]["data"]["batch_id"], batch_id);
    assert_eq!(fs::read(&path).unwrap(), b"{\"partial");

    // The stdio loop propagates the terminal receipt error before another call.
    let requests = [
        initialize("2025-06-18"),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"voucher_schema"}}),
        json!({"jsonrpc":"2.0","id":3,"method":"ping"}),
    ];
    let input = requests
        .iter()
        .map(|value| format!("{value}\n"))
        .collect::<String>();
    let mut output = Vec::new();
    assert_eq!(
        serve_stdio(server, BufReader::new(input.as_bytes()), &mut output).await,
        Err("egress_log_incomplete".into())
    );
    let responses: Vec<Value> = std::str::from_utf8(&output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0]["id"], 1);
    assert_eq!(fs::read(path).unwrap(), b"{\"partial");
}

#[path = "agent_protocol_evidence_tests.rs"]
mod evidence_tests;
