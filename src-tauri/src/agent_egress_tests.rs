use super::*;
use std::fs::OpenOptions;

#[test]
fn failed_partial_write_and_sync_restore_the_original_receipts() {
    for partial_write in [true, false] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("agent-egress.jsonl");
        let original = b"{\"tool\":\"existing\"}\n";
        fs::write(&path, original).unwrap();
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        file.lock().unwrap();
        let result = append_locked(&mut file, r#"{"tool":"failed"}"#, |file, bytes| {
            file.write_all(if partial_write { &bytes[..7] } else { bytes })?;
            Err(std::io::Error::other("injected append or sync failure"))
        });
        file.unlock().unwrap();
        assert_eq!(result, Err("egress_record_write_failed".into()));
        assert_eq!(fs::read(&path).unwrap(), original);
        append_egress_line(&path, r#"{"tool":"next"}"#).unwrap();
        let rows = read_egress_tail(&path, 2).unwrap();
        assert_eq!(
            rows.records,
            [r#"{"tool":"next"}"#, r#"{"tool":"existing"}"#]
        );
    }
}

#[test]
fn failed_rollback_is_distinct_and_incomplete_log_refuses_future_appends() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("agent-egress.jsonl");
    fs::write(&path, b"{}\n").unwrap();
    // A read-only handle makes the real truncation fail on both supported OSes.
    let mut file = File::open(&path).unwrap();
    let result = append_locked(&mut file, "{}", |_, _| {
        OpenOptions::new()
            .append(true)
            .open(&path)?
            .write_all(b"{\"partial")?;
        Err(std::io::Error::other("injected write failure"))
    });
    assert_eq!(result, Err("egress_record_rollback_failed".into()));
    let damaged = fs::read(&path).unwrap();
    assert_eq!(
        append_egress_line(&path, "{}"),
        Err("egress_log_incomplete".into())
    );
    assert_eq!(fs::read(&path).unwrap(), damaged);
    assert_eq!(
        read_egress_tail(&path, 20).unwrap_err(),
        "egress_log_incomplete"
    );
}

#[test]
fn receipt_size_limit_preserves_the_file_and_admitted_tail_is_readable() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("agent-egress.jsonl");
    append_egress_line(&path, "{}").unwrap();
    let oversized = "x".repeat(MAX_EGRESS_TAIL_BYTES - 1);
    assert_eq!(
        append_egress_line(&path, &oversized),
        Err("egress_record_too_large".into())
    );
    assert_eq!(fs::read(&path).unwrap(), b"{}\n");
    let admitted = format!("\"{}\"", "x".repeat(MAX_EGRESS_TAIL_BYTES - 4));
    append_egress_line(&path, &admitted).unwrap();
    assert_eq!(read_egress_tail(&path, 1).unwrap().records, [admitted]);
}

#[test]
fn company_receipts_only_admit_canonical_uuid_identifiers() {
    assert_eq!(
        canonical_company_guid("{12345678-ABCD-4ABC-8ABC-123456789012}"),
        Some("12345678-abcd-4abc-8abc-123456789012".into())
    );
    assert_eq!(canonical_company_guid("unverified selector"), None);
}

#[cfg(unix)]
#[test]
fn receipt_append_creates_and_repairs_private_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("agent-egress.jsonl");
    append_egress_line(&path, "{}").unwrap();
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    append_egress_line(&path, "{}").unwrap();
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn tail_discards_a_partial_unicode_line_before_decoding() {
    let directory = tempfile::tempdir().expect("temporary egress directory");
    let path = directory.path().join("agent-egress.jsonl");
    let body = format!(
        "{{\"tool\":\"{}\"}}\n{{\"tool\":\"new\"}}\n",
        "古".repeat(EGRESS_TAIL_CHUNK_BYTES)
    );
    assert!(!body.is_char_boundary(body.len() - EGRESS_TAIL_CHUNK_BYTES));
    fs::write(&path, body).expect("Unicode receipt followed by recent receipt");
    assert_eq!(
        read_egress_tail(&path, 1)
            .expect("latest complete receipt")
            .records,
        [r#"{"tool":"new"}"#]
    );
}

#[test]
fn locked_append_preserves_existing_receipts_and_adds_a_complete_line() {
    let directory = tempfile::tempdir().expect("temporary egress directory");
    let path = directory.path().join("agent-egress.jsonl");
    fs::write(&path, b"{\"tool\":\"existing\"}\n").expect("existing receipt");
    append_egress_line(&path, r#"{"tool":"new"}"#).expect("locked append");
    assert_eq!(
        fs::read(&path).expect("receipt bytes"),
        b"{\"tool\":\"existing\"}\n{\"tool\":\"new\"}\n"
    );
    assert_eq!(
        read_egress_tail(&path, 2)
            .expect("locked tail read")
            .records,
        [r#"{"tool":"new"}"#, r#"{"tool":"existing"}"#]
    );
}

#[test]
fn egress_tail_distinguishes_complete_logs_from_record_limited_pages() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("agent-egress.jsonl");
    let missing = read_egress_tail(&path, 20).unwrap();
    assert!(missing.records.is_empty());
    assert!(!missing.truncated);
    fs::write(&path, b"").unwrap();
    let empty = read_egress_tail(&path, 20).unwrap();
    assert!(empty.records.is_empty());
    assert!(!empty.truncated);
    append_egress_line(&path, r#"{"index":0}"#).unwrap();
    append_egress_line(&path, r#"{"index":1}"#).unwrap();
    for limit in [2, 20] {
        let complete = read_egress_tail(&path, limit).unwrap();
        assert_eq!(complete.records, [r#"{"index":1}"#, r#"{"index":0}"#]);
        assert!(!complete.truncated);
    }
    let bounded = read_egress_tail(&path, 1).unwrap();
    assert_eq!(bounded.records, [r#"{"index":1}"#]);
    assert!(bounded.truncated);
}

#[tokio::test]
async fn egress_log_reports_byte_bounded_tail_even_below_requested_record_count() {
    use super::super::{Redaction, Server, Settings, TallyEndpointConfig};
    use serde_json::{json, Value};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("agent-egress.jsonl");
    let server = Server::new(Settings {
        endpoint: TallyEndpointConfig {
            host: "127.0.0.1".into(),
            port: 9,
        },
        data_dir: directory.path().to_path_buf(),
        max_rows: 100,
        max_bytes: 1_000_000,
        redaction: Redaction::MaskParties,
        import_enabled: false,
    });
    append_egress_line(&path, r#"{"index":0}"#).unwrap();
    let complete = server.call_tool("egress_log", json!({"limit":20})).await;
    assert_eq!(complete["isError"], false);
    assert_eq!(complete["structuredContent"]["truncated"], false);
    assert_eq!(
        complete["structuredContent"]["result"]["records"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    for index in 1..=6 {
        let row = json!({"index": index, "padding": "古".repeat(20_000)}).to_string();
        append_egress_line(&path, &row).unwrap();
    }
    assert!(fs::metadata(&path).unwrap().len() > MAX_EGRESS_TAIL_BYTES as u64);
    let tail = read_egress_tail(&path, 20).unwrap();
    assert!(tail.truncated);
    assert_eq!(
        tail.records.len(),
        4,
        "only four complete rows fit the scan bound"
    );
    for (offset, row) in tail.records.iter().enumerate() {
        let parsed: Value = serde_json::from_str(row).expect("complete UTF-8 JSON receipt");
        assert_eq!(parsed["index"], 6 - offset);
    }
    let bounded = server.call_tool("egress_log", json!({"limit":20})).await;
    assert_eq!(bounded["isError"], false);
    assert_eq!(bounded["structuredContent"]["truncated"], true);
    assert_eq!(
        bounded["structuredContent"]["result"]["records"],
        json!(tail.records)
    );
}

#[tokio::test]
async fn egress_log_rejects_unterminated_and_malformed_receipts_in_band() {
    use super::super::{Redaction, Server, Settings, TallyEndpointConfig};
    use serde_json::{json, Value};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("agent-egress.jsonl");
    let server = Server::new(Settings {
        endpoint: TallyEndpointConfig {
            host: "127.0.0.1".into(),
            port: 9,
        },
        data_dir: directory.path().to_path_buf(),
        max_rows: 100,
        max_bytes: 1_000_000,
        redaction: Redaction::MaskParties,
        import_enabled: false,
    });
    server
        .append_notification_refusal_egress("voucher_schema", &json!({}))
        .unwrap();
    let valid = fs::read(&path).unwrap();
    let receipt: Value = serde_json::from_slice(&valid).unwrap();
    assert_eq!(receipt["tool"], "voucher_schema");
    assert!(receipt["response_sha256"].is_string());
    assert!(receipt["fields_prepared"].is_array());
    let complete = read_egress_tail(&path, 20).unwrap();
    assert_eq!(complete.records.len(), 1);
    assert!(!complete.truncated);
    let mut prefix_with_newline = valid[..valid.len() / 2].to_vec();
    prefix_with_newline.push(b'\n');
    for damaged in [
        valid[..valid.len() - 1].to_vec(),
        valid[..valid.len() / 2].to_vec(),
        prefix_with_newline,
        [b"{\"tool\":\n".as_slice(), valid.as_slice()].concat(),
    ] {
        fs::write(&path, &damaged).unwrap();
        assert_eq!(
            read_egress_tail(&path, 20).unwrap_err(),
            "egress_log_incomplete"
        );
        let response = server.call_tool("egress_log", json!({"limit":20})).await;
        assert_eq!(response["isError"], true);
        assert_eq!(
            response["structuredContent"]["result"]["error"]["code"],
            "egress_log_incomplete"
        );
        assert_eq!(
            response["structuredContent"]["evidence"]["state"],
            "partial"
        );
        assert!(response["structuredContent"]["result"]["records"].is_null());
        assert_eq!(
            fs::read(&path).unwrap(),
            damaged,
            "reads never repair damaged receipts"
        );
    }
}

#[test]
fn receipt_append_waits_until_all_shared_readers_release_the_file() {
    use std::sync::mpsc;
    use std::time::Duration;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("agent-egress.jsonl");
    append_egress_line(&path, r#"{"tool":"first"}"#).unwrap();
    let reader = File::open(&path).unwrap();
    let other_reader = File::open(&path).unwrap();
    reader.lock_shared().unwrap();
    other_reader.try_lock_shared().unwrap();
    let contender = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    assert!(matches!(
        contender.try_lock(),
        Err(std::fs::TryLockError::WouldBlock)
    ));
    let (started, ready) = mpsc::channel();
    let writer_path = path.clone();
    let writer = std::thread::spawn(move || {
        started.send(()).unwrap();
        append_egress_line(&writer_path, r#"{"tool":"second"}"#)
    });
    ready.recv_timeout(Duration::from_secs(2)).unwrap();
    reader.unlock().unwrap();
    std::thread::sleep(Duration::from_millis(20));
    assert!(
        !writer.is_finished(),
        "remaining reader must exclude the append"
    );
    other_reader.unlock().unwrap();
    writer.join().unwrap().unwrap();
    contender.try_lock().unwrap();
    contender.unlock().unwrap();
    assert_eq!(
        read_egress_tail(&path, 2).unwrap().records,
        [r#"{"tool":"second"}"#, r#"{"tool":"first"}"#]
    );
}
