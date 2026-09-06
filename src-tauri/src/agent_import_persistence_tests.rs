use super::*;

fn line() -> ImportLedgerLine {
    serde_json::from_value(json!({
        "batch_id":"batch-proof", "company_guid":"00000000-0000-4000-8000-000000000001",
        "txn_ids":[], "date_from":"20260901", "date_to":"20260901", "sha256":"hash",
        "built_at":"2026-09-06T00:00:00Z", "status":"verified", "vouchers":[],
        "pre_import_mark":{"kind":"company_high_water", "value":10, "master_value":10}
    }))
    .unwrap()
}

#[tokio::test]
async fn uncertain_build_retains_xml_journal_and_exact_recovery_error() {
    let directory = tempfile::tempdir().unwrap();
    let imports = directory.path().join("imports");
    fs::create_dir(&imports).unwrap();
    let ledger = directory.path().join("agent-import-ledger.jsonl");
    fs::write(&ledger, b"previous\n").unwrap();
    // The actual partial append survives when rollback cannot truncate its handle.
    struct CannotRollback {
        reader: fs::File,
        writer: fs::File,
    }
    impl ImportLedgerWriter for CannotRollback {
        fn length(&mut self) -> std::io::Result<u64> {
            self.reader.metadata().map(|m| m.len())
        }
        fn append(&mut self, bytes: &[u8]) -> std::io::Result<()> {
            self.writer.write_all(&bytes[..3])?;
            Err(std::io::Error::other("injected partial write"))
        }
        fn sync(&mut self) -> std::io::Result<()> {
            self.writer.sync_data()
        }
        fn truncate(&mut self, length: u64) -> std::io::Result<()> {
            self.reader.set_len(length)
        }
    }
    let mut writer = CannotRollback {
        reader: fs::File::open(&ledger).unwrap(),
        writer: OpenOptions::new().append(true).open(&ledger).unwrap(),
    };
    let update = line();
    let code = persist_build(&imports, &update, b"<retained-batch/>", || {
        append_import_ledger_bytes(&mut writer, b"next status\n")
    })
    .unwrap()
    .unwrap();
    assert_eq!(code, "import_ledger_rollback_failed");
    assert_eq!(
        fs::read(imports.join("batch-proof.xml")).unwrap(),
        b"<retained-batch/>"
    );
    let journal: Value = serde_json::from_slice(
        &fs::read(imports.join(BUILD_TRANSACTION).join("update.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(journal["batch_id"], update.batch_id);
    assert_eq!(
        require_settled(&imports),
        Err("import_publication_recovery_required".into())
    );
    let server = Server::new(crate::agent::Settings {
        endpoint: bridge_tally_transport::TallyEndpointConfig {
            host: "127.0.0.1".into(),
            port: 9,
        },
        data_dir: directory.path().to_path_buf(),
        max_rows: 10,
        max_bytes: 256,
        redaction: crate::agent::Redaction::None,
        import_enabled: true,
    });
    let result = json!({"isError":true,"content":[],"structuredContent":{"result":{
        "batch_id":update.batch_id,"error":{"code":code}}}});
    assert_eq!(
        server.lock_import_admission().err(),
        Some("import_publication_recovery_required".into())
    );
    assert_eq!(
        server.lock_import_admission_shared().err(),
        Some("import_publication_recovery_required".into())
    );
    for result in [
        result,
        crate::agent::response_too_large("build_import_xml", &code),
    ] {
        let mut wire = Vec::new();
        crate::agent::agent_protocol::finish_response(
            &server,
            &mut wire,
            json!(1),
            Ok(result),
            None,
            Some(update.batch_id.clone()),
            true,
        )
        .await
        .unwrap();
        let response: Value = serde_json::from_slice(&wire).unwrap();
        assert_eq!(
            response["error"]["message"],
            "import_ledger_rollback_failed"
        );
        assert_eq!(response["error"]["data"]["batch_id"], update.batch_id);
        assert!(wire.len() <= 256);
    }
}

#[test]
fn confirmed_build_rollback_removes_only_the_uncommitted_batch() {
    let directory = tempfile::tempdir().unwrap();
    let result = persist_build(directory.path(), &line(), b"<uncommitted/>", || {
        Err("import_file_permissions_failed".into())
    });
    assert_eq!(result, Err("import_file_permissions_failed".into()));
    assert!(!directory.path().join("batch-proof.xml").exists());
    assert_eq!(require_settled(directory.path()), Ok(()));
}

#[test]
fn publication_failures_restore_prior_proofs_and_leave_status_unchanged() {
    for existing in [false, true] {
        for fail_at in [
            PublicationStep::StageJson,
            PublicationStep::StageMarkdown,
            PublicationStep::BackupJson,
            PublicationStep::BackupMarkdown,
            PublicationStep::PublishJson,
            PublicationStep::PublishMarkdown,
            PublicationStep::AppendStatus,
        ] {
            let directory = tempfile::tempdir().unwrap();
            let imports = directory.path();
            let json_path = imports.join("batch-proof.proof.json");
            let md_path = imports.join("batch-proof.proof.md");
            if existing {
                fs::write(&json_path, b"previous JSON").unwrap();
                fs::write(&md_path, b"previous Markdown").unwrap();
            }
            let ledger = imports.join("ledger.jsonl");
            fs::write(&ledger, b"previous status\n").unwrap();
            let result = publish_proofs(
                imports,
                &line(),
                b"next JSON",
                b"next Markdown",
                || append_private_import_ledger(&ledger, b"next status\n", set_private_file),
                |step| {
                    if step == fail_at {
                        Err("injected_publication_failure".into())
                    } else {
                        Ok(())
                    }
                },
            );
            assert_eq!(result, Err("injected_publication_failure".into()));
            if existing {
                assert_eq!(fs::read(&json_path).unwrap(), b"previous JSON");
                assert_eq!(fs::read(&md_path).unwrap(), b"previous Markdown");
            } else {
                assert!(!json_path.exists());
                assert!(!md_path.exists());
            }
            assert_eq!(fs::read(&ledger).unwrap(), b"previous status\n");
            assert_eq!(require_settled(imports), Ok(()));
        }
    }
}

#[test]
fn markdown_stage_io_failure_never_replaces_the_existing_proof() {
    let directory = tempfile::tempdir().unwrap();
    let imports = directory.path();
    fs::write(imports.join("batch-proof.proof.json"), b"previous").unwrap();
    let result = publish_proofs(
        imports,
        &line(),
        b"next",
        b"next",
        || panic!("status must not append"),
        |step| {
            if step == PublicationStep::StageMarkdown {
                fs::create_dir(imports.join(TRANSACTION).join("next.md")).unwrap();
            }
            Ok(())
        },
    );
    assert_eq!(result, Err("import_file_write_failed".into()));
    assert_eq!(
        fs::read(imports.join("batch-proof.proof.json")).unwrap(),
        b"previous"
    );
    assert_eq!(require_settled(imports), Ok(()));
}

#[test]
fn failed_status_sync_restores_real_ledger_bytes_and_both_proofs() {
    struct FailOnce {
        file: fs::File,
        failed: bool,
    }
    impl ImportLedgerWriter for FailOnce {
        fn length(&mut self) -> std::io::Result<u64> {
            self.file.metadata().map(|m| m.len())
        }
        fn append(&mut self, bytes: &[u8]) -> std::io::Result<()> {
            self.file.write_all(bytes)
        }
        fn sync(&mut self) -> std::io::Result<()> {
            if !self.failed {
                self.failed = true;
                Err(std::io::Error::other("injected sync failure"))
            } else {
                self.file.sync_data()
            }
        }
        fn truncate(&mut self, length: u64) -> std::io::Result<()> {
            self.file.set_len(length)
        }
    }
    let directory = tempfile::tempdir().unwrap();
    let imports = directory.path();
    let ledger = imports.join("ledger.jsonl");
    fs::write(&ledger, b"previous status\n").unwrap();
    fs::write(imports.join("batch-proof.proof.json"), b"previous JSON").unwrap();
    fs::write(imports.join("batch-proof.proof.md"), b"previous Markdown").unwrap();
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&ledger)
        .unwrap();
    file.seek(SeekFrom::End(0)).unwrap();
    let result = publish_proofs(
        imports,
        &line(),
        b"next JSON",
        b"next Markdown",
        || {
            append_import_ledger_bytes(
                &mut FailOnce {
                    file,
                    failed: false,
                },
                b"next status\n",
            )
        },
        |_| Ok(()),
    );
    assert_eq!(result, Err("import_ledger_unavailable".into()));
    assert_eq!(fs::read(&ledger).unwrap(), b"previous status\n");
    assert_eq!(
        fs::read(imports.join("batch-proof.proof.json")).unwrap(),
        b"previous JSON"
    );
    assert_eq!(
        fs::read(imports.join("batch-proof.proof.md")).unwrap(),
        b"previous Markdown"
    );
    assert_eq!(require_settled(imports), Ok(()));
}

#[test]
fn rollback_failure_retains_recovery_material_and_blocks_import_admission() {
    let directory = tempfile::tempdir().unwrap();
    let imports = directory.path().join("imports");
    fs::create_dir(&imports).unwrap();
    let json_path = imports.join("batch-proof.proof.json");
    fs::write(&json_path, b"previous JSON").unwrap();
    let result = publish_proofs(
        &imports,
        &line(),
        b"next JSON",
        b"next Markdown",
        || Ok(()),
        |step| {
            if step == PublicationStep::PublishMarkdown {
                fs::remove_file(&json_path).unwrap();
                fs::create_dir(&json_path).unwrap();
                return Err("injected_publication_failure".into());
            }
            Ok(())
        },
    );
    assert_eq!(result, Err("proof_publication_rollback_failed".into()));
    assert_eq!(
        fs::read(imports.join(TRANSACTION).join("previous.json")).unwrap(),
        b"previous JSON"
    );
    let server = Server::new(crate::agent::Settings {
        endpoint: bridge_tally_transport::TallyEndpointConfig {
            host: "127.0.0.1".into(),
            port: 9,
        },
        data_dir: directory.path().to_path_buf(),
        max_rows: 10,
        max_bytes: 200_000,
        redaction: crate::agent::Redaction::None,
        import_enabled: true,
    });
    assert_eq!(
        server.lock_import_admission().err(),
        Some("proof_publication_recovery_required".into())
    );
    assert_eq!(
        server.lock_import_admission_shared().err(),
        Some("proof_publication_recovery_required".into())
    );
}

#[test]
fn ledger_permissions_failure_precedes_all_appends() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("ledger.jsonl");
    fs::write(&path, b"previous\n").unwrap();
    let result = append_private_import_ledger(&path, b"next\n", |_| {
        Err("import_file_permissions_failed".into())
    });
    assert_eq!(result, Err("import_file_permissions_failed".into()));
    assert_eq!(fs::read(&path).unwrap(), b"previous\n");
    append_private_import_ledger(&path, b"next\n", set_private_file).unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"previous\nnext\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
