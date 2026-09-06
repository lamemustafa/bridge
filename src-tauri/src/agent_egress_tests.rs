use super::*;

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
        file.lock_exclusive().unwrap();
        let result = append_locked(&mut file, r#"{"tool":"failed"}"#, |file, bytes| {
            file.write_all(if partial_write { &bytes[..7] } else { bytes })?;
            Err(std::io::Error::other("injected append or sync failure"))
        });
        file.unlock().unwrap();
        assert_eq!(result, Err("egress_record_write_failed".into()));
        assert_eq!(fs::read(&path).unwrap(), original);
        append_egress_line(&path, r#"{"tool":"next"}"#).unwrap();
        let rows = read_egress_tail(&path, 2).unwrap();
        assert_eq!(rows, [r#"{"tool":"next"}"#, r#"{"tool":"existing"}"#]);
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
    assert_eq!(read_egress_tail(&path, 1).unwrap(), [admitted]);
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
        read_egress_tail(&path, 1).expect("latest complete receipt"),
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
        read_egress_tail(&path, 2).expect("locked tail read"),
        [r#"{"tool":"new"}"#, r#"{"tool":"existing"}"#]
    );
}
