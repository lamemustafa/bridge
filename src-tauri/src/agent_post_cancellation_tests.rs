//! MCP input lifecycle tests; no simulated Tally protocol is needed.
use super::*;
use std::{
    path::Path,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, AsyncWriteExt, ReadBuf};

fn server(path: &Path) -> Server {
    Server::new(Settings {
        endpoint: TallyEndpointConfig {
            host: "127.0.0.1".into(),
            port: 9,
        },
        data_dir: path.to_path_buf(),
        max_rows: 10,
        max_bytes: 200_000,
        redaction: Redaction::None,
        import_enabled: true,
        writes_enabled: true,
    })
}

#[tokio::test]
async fn cancellation_drops_pending_post_before_its_side_effect() {
    let (mut client, source) = tokio::io::duplex(1024);
    let mut reader = BufReader::new(source);
    let mut framer = Framer::default();
    let mut pending = std::collections::VecDeque::new();
    let directory = tempfile::tempdir().unwrap();
    let server = server(directory.path());
    let mut output = Vec::new();
    client.write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/cancelled\",\"params\":{\"requestId\":7}}\n").await.unwrap();
    let future = async {
        std::future::pending::<()>().await;
        panic!("must not dispatch");
    };
    assert!(await_post(
        future,
        &json!(7),
        &server,
        &mut reader,
        &mut framer,
        &mut pending,
        &mut output,
    )
    .await
    .unwrap()
    .is_none());
}

#[tokio::test]
async fn disconnect_drops_pending_post() {
    let mut reader = BufReader::new(&b""[..]);
    let directory = tempfile::tempdir().unwrap();
    let server = server(directory.path());
    let result = await_post(
        std::future::pending(),
        &json!(7),
        &server,
        &mut reader,
        &mut Framer::default(),
        &mut std::collections::VecDeque::new(),
        &mut Vec::new(),
    )
    .await;
    assert_eq!(result.err().as_deref(), Some("stdio_client_disconnected"));
}

#[tokio::test]
async fn interrupted_partial_frame_is_preserved() {
    let (mut client, source) = tokio::io::duplex(1024);
    let mut reader = BufReader::new(source);
    let mut framer = Framer::default();
    client.write_all(b"{\"jsonrpc\":\"2.0\",").await.unwrap();
    tokio::select! {
        biased;
        _ = framer.read(&mut reader, 1024) => panic!("not a complete frame"),
        _ = tokio::task::yield_now() => {}
    }
    client
        .write_all(b"\"id\":8,\"method\":\"ping\"}\n")
        .await
        .unwrap();
    let request = parse_request(
        framer
            .read(&mut reader, 1024)
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(request["id"], 8);
}

#[tokio::test]
async fn queue_overflow_is_refused_in_band_and_waits_for_cancellation() {
    let input = format!(
        "{}{{\"jsonrpc\":\"2.0\",\"method\":\"notifications/cancelled\",\"params\":{{\"requestId\":7}}}}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":8,\"method\":\"ping\"}\n".repeat(9),
    );
    let mut reader = BufReader::new(input.as_bytes());
    let mut pending = std::collections::VecDeque::new();
    let directory = tempfile::tempdir().unwrap();
    let server = server(directory.path());
    let mut output = Vec::new();
    let result = await_post(
        std::future::pending(),
        &json!(7),
        &server,
        &mut reader,
        &mut Framer::default(),
        &mut pending,
        &mut output,
    )
    .await;
    assert!(result.unwrap().is_none());
    assert_eq!(pending.len(), 8);
    let refusal: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(refusal["id"], 8);
    assert_eq!(refusal["error"]["code"], -32000);
    assert_eq!(
        refusal["error"]["message"],
        "stdio_pending_requests_exceeded"
    );
}

#[tokio::test]
async fn queue_overflow_refuses_an_oversized_id_without_ending_the_post_wait() {
    let oversized = "é\"".repeat(100);
    let input = format!(
        "{}{{\"jsonrpc\":\"2.0\",\"id\":{},\"method\":\"ping\"}}\n{{\"jsonrpc\":\"2.0\",\"method\":\"notifications/cancelled\",\"params\":{{\"requestId\":7}}}}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":8,\"method\":\"ping\"}\n".repeat(8),
        serde_json::to_string(&oversized).unwrap(),
    );
    let mut reader = BufReader::new(input.as_bytes());
    let mut pending = std::collections::VecDeque::new();
    let directory = tempfile::tempdir().unwrap();
    let mut server = server(directory.path());
    server.settings.max_bytes = 256;
    let mut output = Vec::new();
    let result = await_post(
        std::future::pending(),
        &json!(7),
        &server,
        &mut reader,
        &mut Framer::default(),
        &mut pending,
        &mut output,
    )
    .await;
    assert!(result.unwrap().is_none());
    assert_eq!(pending.len(), 8);
    assert!(output.len() <= 256);
    let refusal: Value = serde_json::from_slice(&output).unwrap();
    assert!(refusal["id"].is_null());
    assert_eq!(refusal["error"]["message"], "request_id_too_large");
}

#[tokio::test]
async fn queue_overflow_tool_request_has_a_prepared_and_completed_refusal_receipt() {
    let input = format!(
        "{}{{\"jsonrpc\":\"2.0\",\"id\":9,\"method\":\"tools/call\",\"params\":{{\"name\":\"voucher_schema\",\"arguments\":{{}}}}}}\n{{\"jsonrpc\":\"2.0\",\"method\":\"notifications/cancelled\",\"params\":{{\"requestId\":7}}}}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":8,\"method\":\"ping\"}\n".repeat(8),
    );
    let mut reader = BufReader::new(input.as_bytes());
    let mut pending = std::collections::VecDeque::new();
    let directory = tempfile::tempdir().unwrap();
    let server = server(directory.path());
    let mut output = Vec::new();
    assert!(await_post(
        std::future::pending(),
        &json!(7),
        &server,
        &mut reader,
        &mut Framer::default(),
        &mut pending,
        &mut output,
    )
    .await
    .unwrap()
    .is_none());
    let records = fs::read_to_string(directory.path().join("agent-egress.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(records[0]["record_type"], "response_prepared");
    assert_eq!(records[0]["tool"], "voucher_schema");
    assert_eq!(records[1]["record_type"], "stdio_write_completed");
}

struct AlwaysReadable {
    frame: Vec<u8>,
}

impl AsyncRead for AlwaysReadable {
    fn poll_read(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        read: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        read.put_slice(&self.frame);
        Poll::Ready(Ok(()))
    }
}

impl AsyncBufRead for AlwaysReadable {
    fn poll_fill_buf(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<&[u8]>> {
        Poll::Ready(Ok(&self.get_mut().frame))
    }

    fn consume(self: Pin<&mut Self>, _: usize) {}
}

#[tokio::test]
async fn readable_queue_traffic_cannot_starve_the_pending_post() {
    let directory = tempfile::tempdir().unwrap();
    let server = server(directory.path());
    let mut reader = AlwaysReadable {
        frame: b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n".to_vec(),
    };
    let mut pending = std::collections::VecDeque::new();
    let mut output = Vec::new();
    let completed = tokio::time::timeout(
        std::time::Duration::from_millis(100),
        await_post(
            async {
                tokio::task::yield_now().await;
                ToolResponse {
                    value: json!({}),
                    egress: EgressContext {
                        evidence: None,
                        tool: "post_import".into(),
                        args_sha256: sha256_hex(b"post"),
                        company_guid: None,
                    },
                    recovery_batch_id: None,
                }
            },
            &json!(7),
            &server,
            &mut reader,
            &mut Framer::default(),
            &mut pending,
            &mut output,
        ),
    )
    .await;
    assert!(completed.unwrap().unwrap().is_some());
}
