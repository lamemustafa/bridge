use super::*;

fn batch() -> ImportLedgerLine {
    ImportLedgerLine {
        batch_id: "bridge-00000000-0000-4000-8000-000000000001".into(),
        company_guid: GUID.into(),
        company: None,
        txn_ids: vec!["txn-001".into()],
        date_from: "20260901".into(),
        date_to: "20260901".into(),
        sha256: "a".repeat(64),
        built_at: now(),
        status: "built".into(),
        pre_import_mark: PreImportMark {
            kind: "company_high_water".into(),
            value: Some(10),
            master_value: Some(10),
        },
        vouchers: vec![payload().vouchers.remove(0)],
    }
}

fn server(path: &Path) -> Server {
    Server::new(crate::agent::Settings {
        endpoint: TallyEndpointConfig {
            host: "127.0.0.1".into(),
            port: 9,
        },
        data_dir: path.to_path_buf(),
        max_rows: 10,
        max_bytes: 200_000,
        redaction: crate::agent::Redaction::None,
        import_enabled: true,
    })
}

#[test]
fn repeated_verification_appends_only_compact_status_and_preserves_batch_bytes() {
    let directory = tempfile::tempdir().unwrap();
    let server = server(directory.path());
    let mut batch = batch();
    batch.vouchers = (0..1000)
        .map(|index| {
            let mut voucher = batch.vouchers[0].clone();
            voucher.bridge_txn_id = format!("txn-{index}");
            voucher.narration = Some("synthetic narration ".repeat(50));
            voucher
        })
        .collect();
    batch.txn_ids = batch
        .vouchers
        .iter()
        .map(|voucher| voucher.bridge_txn_id.clone())
        .collect();
    server.append_import_ledger(&batch).unwrap();
    let path = directory.path().join("agent-import-ledger.jsonl");
    let original = fs::read(&path).unwrap();
    assert!(original.len() > 1_000_000);
    for index in 0..25 {
        batch.status = if index % 2 == 0 {
            "posted_verified"
        } else {
            "verification_incomplete"
        }
        .into();
        let proof = json!({"batch_id":batch.batch_id,"company":{"name":"Synthetic Book"}});
        server.persist_import_verification(&proof, &batch).unwrap();
    }
    let bytes = fs::read(&path).unwrap();
    assert!(bytes.starts_with(&original));
    let added = std::str::from_utf8(&bytes[original.len()..])
        .unwrap()
        .lines()
        .collect::<Vec<_>>();
    assert_eq!(added.len(), 25);
    for line in added {
        assert!(
            line.len() < 300,
            "verification status must not scale with payload"
        );
        let value: Value = serde_json::from_str(line).unwrap();
        assert_eq!(value["record_type"], "verification_status");
        assert_eq!(value["batch_sha256"], batch.sha256);
        assert!(value.get("vouchers").is_none());
        assert!(value.get("txn_ids").is_none());
    }
    let loaded = server.latest_import_line(&batch.batch_id).unwrap().unwrap();
    assert_eq!(loaded.status, "posted_verified");
    assert_eq!(
        sha256_json(&serde_json::to_value(&loaded.vouchers).unwrap()),
        sha256_json(&serde_json::to_value(&batch.vouchers).unwrap())
    );
    assert_eq!(loaded.txn_ids, batch.txn_ids);
    assert_eq!(server.import_ledger().unwrap().len(), 1);
}

#[test]
fn compact_status_hydrates_legacy_full_records_and_rejects_unknown_builds() {
    let directory = tempfile::tempdir().unwrap();
    let server = server(directory.path());
    let original = batch();
    server.append_import_ledger(&original).unwrap();
    let mut legacy_update = original.clone();
    legacy_update.status = "posted_verified".into();
    server.append_import_ledger(&legacy_update).unwrap();
    let mut current = original.clone();
    current.status = "verification_incomplete".into();
    server
        .persist_import_verification(&json!({}), &current)
        .unwrap();
    let loaded = server
        .latest_import_line(&original.batch_id)
        .unwrap()
        .unwrap();
    assert_eq!(loaded.status, "verification_incomplete");
    assert_eq!(loaded.txn_ids, original.txn_ids);
    assert_eq!(loaded.vouchers.len(), 1);
    assert_eq!(server.import_ledger().unwrap().len(), 2);
    let existing = fs::read_to_string(directory.path().join("agent-import-ledger.jsonl")).unwrap();
    for change in [
        json!({"batch_id":"missing"}),
        json!({"batch_sha256":"mismatch"}),
    ] {
        let mut value = serde_json::to_value(ledger::StatusRecord::from(&current)).unwrap();
        for (key, new_value) in change.as_object().unwrap() {
            value[key] = new_value.clone();
        }
        assert_eq!(
            ledger::parse_records(&format!("{existing}{value}\n")).err(),
            Some("import_ledger_invalid".into())
        );
    }
    let mut unknown_record = serde_json::to_value(&original).unwrap();
    unknown_record["record_type"] = json!("future_record");
    assert_eq!(
        ledger::parse_records(&unknown_record.to_string()).err(),
        Some("import_ledger_invalid".into())
    );
}
