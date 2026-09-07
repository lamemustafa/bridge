//! Local durable-state and MCP cancellation tests; no Tally responses are invented.
use super::*;
use std::io::Write;
use std::{
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::AsyncWrite;

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

fn append_dispatch_intent(root: &std::path::Path) {
    let record = json!({
        "record_type":"dispatch_intent",
        "batch_id":BATCH,
        "batch_sha256":"a".repeat(64),
        "status":"dispatch_started",
        "native_request_sha256":"b".repeat(64),
    });
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(root.join("agent-import-ledger.jsonl"))
        .unwrap();
    writeln!(file, "{record}").unwrap();
    file.sync_all().unwrap();
}

fn completed_response() -> ToolResponse {
    ToolResponse {
        value: json!({"completed":true}),
        egress: EgressContext {
            evidence: None,
            tool: "post_import".into(),
            args_sha256: sha256_hex(b"post"),
            company_guid: Some(COMPANY.into()),
        },
        recovery_batch_id: Some(BATCH.into()),
    }
}

struct FailingWriter;

impl AsyncWrite for FailingWriter {
    fn poll_write(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        _: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Poll::Ready(Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe)))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe)))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
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
async fn cancellation_after_durable_intent_finishes_the_original_response_once() {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    let directory = tempfile::tempdir().unwrap();
    let server = local_batch(directory.path());
    append_dispatch_intent(directory.path());

    let (mut client, source) = tokio::io::duplex(1024);
    client
        .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/cancelled\",\"params\":{\"requestId\":7}}\n")
        .await
        .unwrap();
    let (finish, wait_for_finish) = tokio::sync::oneshot::channel();
    let calls = Arc::new(AtomicUsize::new(0));
    let observed_calls = calls.clone();
    let future = async move {
        wait_for_finish.await.expect("test completion");
        observed_calls.fetch_add(1, Ordering::AcqRel);
        completed_response()
    };
    let mut reader = BufReader::new(source);
    let mut output = Vec::new();
    let id = json!(7);
    let request_args = args();
    let mut framer = Framer::default();
    let mut pending = std::collections::VecDeque::new();
    let response = {
        let result = await_post(
            future,
            PostRequest {
                id: &id,
                args: &request_args,
            },
            &server,
            &mut reader,
            &mut framer,
            &mut pending,
            &mut output,
        );
        tokio::pin!(result);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut result)
                .await
                .is_err(),
            "durable intent must drain rather than cancel the future"
        );
        finish.send(()).unwrap();
        result.await.unwrap().expect("completed original response")
    };
    assert_eq!(response.value, json!({"completed":true}));
    assert_eq!(
        calls.load(Ordering::Acquire),
        1,
        "no replacement post is started"
    );
    finish_response(
        &server,
        &mut output,
        json!(7),
        Ok(response.value),
        Some(response.egress),
        response.recovery_batch_id,
        true,
    )
    .await
    .unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&output).unwrap()["result"]["completed"],
        true
    );
    assert_eq!(
        fs::read_to_string(directory.path().join("agent-import-ledger.jsonl"))
            .unwrap()
            .lines()
            .count(),
        2,
    );
}

#[tokio::test]
async fn cancellation_before_durable_intent_drops_the_controlled_future() {
    let directory = tempfile::tempdir().unwrap();
    let server = local_batch(directory.path());
    let (mut client, source) = tokio::io::duplex(1024);
    client
        .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/cancelled\",\"params\":{\"requestId\":7}}\n")
        .await
        .unwrap();
    let result = await_post(
        std::future::pending::<ToolResponse>(),
        PostRequest {
            id: &json!(7),
            args: &args(),
        },
        &server,
        &mut BufReader::new(source),
        &mut Framer::default(),
        &mut std::collections::VecDeque::new(),
        &mut Vec::new(),
    )
    .await
    .unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn eof_after_durable_intent_drains_the_original_future() {
    let directory = tempfile::tempdir().unwrap();
    let server = local_batch(directory.path());
    append_dispatch_intent(directory.path());
    let (finish, wait_for_finish) = tokio::sync::oneshot::channel();
    let id = json!(7);
    let request_args = args();
    let mut reader = BufReader::new(&b""[..]);
    let mut framer = Framer::default();
    let mut pending = std::collections::VecDeque::new();
    let mut output = Vec::new();
    let response = {
        let result = await_post(
            async move {
                wait_for_finish.await.expect("test completion");
                completed_response()
            },
            PostRequest {
                id: &id,
                args: &request_args,
            },
            &server,
            &mut reader,
            &mut framer,
            &mut pending,
            &mut output,
        );
        tokio::pin!(result);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut result)
                .await
                .is_err(),
            "EOF must drain a durable post before returning"
        );
        finish.send(()).unwrap();
        result.await.unwrap().expect("completed original response")
    };
    assert_eq!(response.value, json!({"completed":true}),);
}

#[tokio::test]
async fn output_error_after_durable_intent_drains_the_original_future() {
    let directory = tempfile::tempdir().unwrap();
    let server = local_batch(directory.path());
    append_dispatch_intent(directory.path());
    let (finish, wait_for_finish) = tokio::sync::oneshot::channel();
    let id = json!(7);
    let request_args = args();
    let mut reader = BufReader::new(&b"{\"jsonrpc\":\"2.0\",\"id\":8,\"method\":\"ping\"}\n"[..]);
    let mut framer = Framer::default();
    let mut pending = std::collections::VecDeque::new();
    let mut output = FailingWriter;
    let response = {
        let result = await_post(
            async move {
                wait_for_finish.await.expect("test completion");
                completed_response()
            },
            PostRequest {
                id: &id,
                args: &request_args,
            },
            &server,
            &mut reader,
            &mut framer,
            &mut pending,
            &mut output,
        );
        tokio::pin!(result);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut result)
                .await
                .is_err(),
            "stdio write failure must drain a durable post before returning"
        );
        finish.send(()).unwrap();
        result.await.unwrap().expect("completed original response")
    };
    assert_eq!(response.value, json!({"completed":true}),);
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
        PostRequest {
            id: &json!(7),
            args: &args(),
        },
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
