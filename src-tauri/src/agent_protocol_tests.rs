use super::*;
use std::path::Path;

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
                .trim(),
        )
        .unwrap();
        let wire = format!("{}\n", responses[1]);
        assert_eq!(receipt["bytes_returned"], wire.len());
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
            .trim(),
    )
    .unwrap();
    assert_eq!(receipt["rows_returned"], 0);
    assert_eq!(receipt["fields_returned"], json!([]));
    assert_eq!(
        receipt["response_sha256"],
        sha256_hex(format!("{}\n", responses[1]).as_bytes())
    );
}
