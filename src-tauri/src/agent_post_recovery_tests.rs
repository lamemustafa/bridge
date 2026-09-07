//! Local durable-state and MCP cancellation tests; no Tally responses are invented.
use super::*;
use std::io::Write;

const BATCH: &str = "bridge-00000000-0000-4000-8000-000000000001";
const COMPANY: &str = "00000000-0000-4000-8000-000000000002";

fn local_batch(root: &std::path::Path) -> Server {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(root, fs::Permissions::from_mode(0o700)).unwrap();
    }
    ensure_private_directory(root).unwrap();
    let batch = json!({
        "batch_id":BATCH,"identity_scheme":"batch_v1","company_guid":COMPANY,
        "company":{"name":"Synthetic Accounts","guid":COMPANY,"company_number":"100001","books_from":"20260401"},
        "txn_ids":["journal-test"],"date_from":"20260901","date_to":"20260901",
        "sha256":"a".repeat(64),"built_at":"2026-09-07T00:00:00Z","status":"built",
        "pre_import_mark":{"kind":"company_high_water","value":1,"master_value":1},
        "vouchers":[{"bridge_txn_id":"journal-test","date":"20260901","voucher_type":"Journal",
            "entries":[{"ledger":"Expense","amount":"12.50","side":"Dr"},{"ledger":"Cash","amount":"12.50","side":"Cr"}]}]
    });
    fs::write(root.join("agent-import-ledger.jsonl"), format!("{batch}\n")).unwrap();
    Server::new(Settings {
        endpoint: TallyEndpointConfig {
            host: "127.0.0.1".into(),
            port: 9,
        },
        data_dir: root.to_owned(),
        max_rows: 10,
        max_bytes: 200_000,
        redaction: Redaction::None,
        import_enabled: true,
        writes_enabled: true,
    })
}

fn args() -> Value {
    json!({"batch_id":BATCH,"company_guid":COMPANY})
}

async fn cancellation_wire(server: &Server) -> Value {
    let response = server.finish_tool_response(
        "post_import",
        &args(),
        Utc::now(),
        server.cancelled_import(&args()),
    );
    let mut output = Vec::new();
    finish_response(
        server,
        &mut output,
        json!(7),
        Ok(response.value),
        Some(response.egress),
        response.recovery_batch_id,
        true,
    )
    .await
    .unwrap();
    serde_json::from_slice(&output).unwrap()
}

#[tokio::test]
async fn cancellation_after_durable_intent_returns_framed_recovery_state() {
    let directory = tempfile::tempdir().unwrap();
    let server = local_batch(directory.path());
    let (mut client, source) = tokio::io::duplex(1024);
    let future = async {
        let record = json!({"record_type":"dispatch_intent","batch_id":BATCH,"batch_sha256":"a".repeat(64),"status":"dispatch_started"});
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(directory.path().join("agent-import-ledger.jsonl"))
            .unwrap();
        writeln!(file, "{record}").unwrap();
        file.sync_all().unwrap();
        client.write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/cancelled\",\"params\":{\"requestId\":7}}\n").await.unwrap();
        std::future::pending::<ToolResponse>().await
    };
    assert!(await_post(
        future,
        &json!(7),
        &server,
        &mut BufReader::new(source),
        &mut Framer::default(),
        &mut std::collections::VecDeque::new(),
        &mut Vec::new()
    )
    .await
    .unwrap()
    .is_none());
    let response = cancellation_wire(&server).await;
    assert_eq!(response["result"]["isError"], true);
    let content = &response["result"]["structuredContent"];
    assert_eq!(content["result"]["batch_id"], BATCH);
    assert_eq!(content["result"]["attempt_recorded"], true);
    assert_eq!(
        content["result"]["dispatch"]["state"],
        "reconciliation_required"
    );
    assert_eq!(content["result"]["error"]["code"], "request_cancelled");
    let text: Value =
        serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(&text, content);
    assert_eq!(
        fs::read_to_string(directory.path().join("agent-import-ledger.jsonl"))
            .unwrap()
            .lines()
            .count(),
        2
    );
    assert_eq!(
        fs::read_to_string(directory.path().join("agent-egress.jsonl"))
            .unwrap()
            .lines()
            .count(),
        2
    );
}

#[tokio::test]
async fn cancellation_distinguishes_no_intent_from_unreadable_history() {
    let directory = tempfile::tempdir().unwrap();
    let server = local_batch(directory.path());
    let response = cancellation_wire(&server).await;
    assert_eq!(
        response["result"]["structuredContent"]["result"]["attempt_recorded"],
        false
    );
    assert_eq!(
        response["result"]["structuredContent"]["result"]["dispatch"]["state"],
        "not_dispatched"
    );
    fs::write(
        directory.path().join("agent-import-ledger.jsonl"),
        "invalid history\n",
    )
    .unwrap();
    let response = cancellation_wire(&server).await;
    assert!(response["result"]["structuredContent"]["result"]["attempt_recorded"].is_null());
    assert_eq!(
        response["result"]["structuredContent"]["result"]["dispatch"]["state"],
        "reconciliation_required"
    );
}

#[tokio::test]
async fn buffered_post_cancellation_removes_call_before_it_can_start() {
    let directory = tempfile::tempdir().unwrap();
    let server = local_batch(directory.path());
    let queued = json!({"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"post_import","arguments":args()}});
    let cancel =
        |id| json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":id}});
    let input = format!("{queued}\n{}\n{}\n", cancel(8), cancel(7));
    let mut pending = std::collections::VecDeque::new();
    let mut output = Vec::new();
    assert!(await_post(
        std::future::pending(),
        &json!(7),
        &server,
        &mut BufReader::new(input.as_bytes()),
        &mut Framer::default(),
        &mut pending,
        &mut output
    )
    .await
    .unwrap()
    .is_none());
    assert!(
        pending.is_empty(),
        "cancelled queued call must never reach the dispatch loop"
    );
    let response: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(response["id"], 8);
    assert_eq!(
        response["result"]["structuredContent"]["result"]["error"]["code"],
        "request_cancelled"
    );
    assert_eq!(
        response["result"]["structuredContent"]["result"]["attempt_recorded"],
        false
    );
    assert_eq!(
        fs::read_to_string(directory.path().join("agent-import-ledger.jsonl"))
            .unwrap()
            .lines()
            .count(),
        1
    );
}
