//! Observe real preparation records while injecting stdout failures.
use super::*;
use crate::agent::agent_protocol::{finish_response, serve_stdio};
use std::{
    io,
    path::{Path, PathBuf},
    pin::Pin,
    task::{Context, Poll},
};

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

fn records(path: &Path) -> Vec<Value> {
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

struct Writer {
    bytes: Vec<u8>,
    path: PathBuf,
    limit: Option<usize>,
    flush_fails: bool,
    completion_fails: bool,
}

impl Writer {
    fn new(path: PathBuf) -> Self {
        Self {
            bytes: Vec::new(),
            path,
            limit: None,
            flush_fails: false,
            completion_fails: false,
        }
    }
}

impl AsyncWrite for Writer {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        if self.path.is_file() {
            assert_eq!(
                records(&self.path).last().unwrap()["record_type"],
                "response_prepared",
                "preparation must already be durable before writing output"
            );
        }
        let remaining = self
            .limit
            .map_or(bytes.len(), |limit| limit.saturating_sub(self.bytes.len()));
        if remaining == 0 {
            return Poll::Ready(Err(io::Error::other("injected write failure")));
        }
        let written = remaining.min(bytes.len());
        self.bytes.extend_from_slice(&bytes[..written]);
        Poll::Ready(Ok(written))
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.flush_fails {
            return Poll::Ready(Err(io::Error::other("injected flush failure")));
        }
        if self.completion_fails && self.path.is_file() {
            fs::rename(&self.path, self.path.with_extension("prepared")).unwrap();
            fs::create_dir(&self.path).unwrap();
        }
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn delivery_failures_leave_preparation_without_claiming_a_completed_write() {
    for (limit, flush_fails, error) in [
        (Some(0), false, "stdio_write_failed"),
        (Some(7), false, "stdio_write_failed"),
        (None, true, "stdio_flush_failed"),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let server = server(directory.path());
        let path = directory.path().join("agent-egress.jsonl");
        let mut writer = Writer::new(path.clone());
        writer.limit = limit;
        writer.flush_fails = flush_fails;
        let tool = server.call_tool_response("voucher_schema", json!({})).await;
        assert_eq!(
            finish_response(
                &server,
                &mut writer,
                json!(1),
                Ok(tool.value),
                Some(tool.egress),
                None,
                true
            )
            .await,
            Err(error.to_string())
        );
        let events = records(&path);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["record_type"], "response_prepared");
        assert!(events[0]["bytes_written"].is_null());
        assert!(events[0]["bytes_returned"].is_null());
        uuid::Uuid::parse_str(events[0]["receipt_id"].as_str().unwrap()).unwrap();
        if let Some(limit) = limit {
            assert_eq!(writer.bytes.len(), limit);
        } else {
            assert_eq!(
                writer.bytes.len(),
                events[0]["bytes_prepared"].as_u64().unwrap() as usize
            );
        }
    }
}

#[tokio::test]
async fn delivery_success_links_completion_to_the_exact_prepared_response() {
    let directory = tempfile::tempdir().unwrap();
    let server = server(directory.path());
    let path = directory.path().join("agent-egress.jsonl");
    let mut writer = Writer::new(path.clone());
    let tool = server.call_tool_response("voucher_schema", json!({})).await;
    finish_response(
        &server,
        &mut writer,
        json!(1),
        Ok(tool.value),
        Some(tool.egress),
        None,
        true,
    )
    .await
    .unwrap();
    let events = records(&path);
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["record_type"], "response_prepared");
    assert_eq!(events[1]["record_type"], "stdio_write_completed");
    assert_eq!(events[1]["receipt_id"], events[0]["receipt_id"]);
    for event in &events {
        assert_eq!(event["response_sha256"], sha256_hex(&writer.bytes));
    }
    assert_eq!(events[0]["bytes_prepared"], writer.bytes.len());
    assert_eq!(events[1]["bytes_written"], writer.bytes.len());
    assert!(events[1]["fields_prepared"].is_null());
}

#[tokio::test]
async fn delivery_completion_failure_stops_before_another_tool_dispatch() {
    let directory = tempfile::tempdir().unwrap();
    let server = server(directory.path());
    let evidence = server.evidence.clone();
    let path = directory.path().join("agent-egress.jsonl");
    let mut writer = Writer::new(path.clone());
    writer.completion_fails = true;
    let input = [
        json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}),
        json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"voucher_schema"}}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"voucher_schema"}}),
    ].iter().map(|value| format!("{value}\n")).collect::<String>();
    assert_eq!(
        serve_stdio(
            server,
            tokio::io::BufReader::new(input.as_bytes()),
            &mut writer
        )
        .await,
        Err("egress_record_write_failed".into())
    );
    assert_eq!(
        evidence.lock().unwrap().records.len(),
        1,
        "second tool was never dispatched"
    );
    assert_eq!(records(&path.with_extension("prepared")).len(), 1);
    let replies = String::from_utf8(writer.bytes)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(replies.len(), 2);
    assert_eq!(replies[1]["id"], 1);
}

#[tokio::test]
async fn delivery_preparation_failure_releases_no_tool_response() {
    let directory = tempfile::tempdir().unwrap();
    let server = server(directory.path());
    let path = directory.path().join("agent-egress.jsonl");
    fs::create_dir(&path).unwrap();
    let mut writer = Writer::new(path);
    let tool = server.call_tool_response("voucher_schema", json!({})).await;
    assert_eq!(
        finish_response(
            &server,
            &mut writer,
            json!(1),
            Ok(tool.value),
            Some(tool.egress),
            None,
            true
        )
        .await,
        Err("egress_record_write_failed".into())
    );
    assert!(writer.bytes.is_empty());
}
