//! Local artifact admission tests use only temporary, synthetic file contents.
use super::*;

fn server(path: &Path) -> Server {
    Server::new(crate::agent::Settings {
        endpoint: bridge_tally_transport::TallyEndpointConfig {
            host: "127.0.0.1".into(),
            port: 9,
        },
        data_dir: path.into(),
        max_rows: 10,
        max_bytes: 200_000,
        redaction: crate::agent::Redaction::None,
        import_enabled: true,
    })
}

#[cfg(unix)]
fn symlink_file(target: &Path, link: &Path) {
    std::os::unix::fs::symlink(target, link).unwrap();
}
#[cfg(windows)]
fn symlink_file(target: &Path, link: &Path) {
    std::os::windows::fs::symlink_file(target, link).unwrap();
}

fn assert_alias_refused(symbolic: bool) {
    for leaf in [
        "agent-import-ledger.jsonl",
        "agent-import-admission.lock",
        "staged.xml",
        "agent-egress.jsonl",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("original.txt");
        fs::write(&target, b"original contents\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&target, fs::Permissions::from_mode(0o640)).unwrap();
        }
        let original_permissions = fs::metadata(&target).unwrap().permissions();
        let data = directory.path().join("agent");
        super::super::ensure_private_directory(&data).unwrap();
        let path = data.join(leaf);
        if symbolic {
            symlink_file(&target, &path);
        } else {
            fs::hard_link(&target, &path).unwrap();
        }
        let server = server(&data);
        match leaf {
            "agent-import-ledger.jsonl" => {
                assert_eq!(
                    server.append_import_record_while_admitted(&json!({})),
                    Err("import_ledger_unavailable".into())
                );
                assert_eq!(
                    server.import_snapshots_while_admitted().err(),
                    Some("import_ledger_unavailable".into())
                );
            }
            "agent-import-admission.lock" => {
                assert_eq!(
                    server.lock_import_admission().err(),
                    Some("import_admission_lock_unavailable".into())
                );
                assert_eq!(
                    server.lock_import_admission_shared().err(),
                    Some("import_admission_lock_unavailable".into())
                );
            }
            "staged.xml" => assert_eq!(
                write_private(&path, b"replacement"),
                Err("import_file_write_failed".into())
            ),
            _ => {
                assert_eq!(
                    super::super::egress::append_egress_line(&path, "{}"),
                    Err("egress_record_write_failed".into())
                );
                assert_eq!(
                    super::super::egress::read_egress_tail(&path, 1).err(),
                    Some("egress_log_unreadable".into())
                );
            }
        }
        assert_eq!(fs::read(&target).unwrap(), b"original contents\n");
        assert_eq!(
            fs::metadata(&target).unwrap().permissions(),
            original_permissions
        );
    }
}

#[test]
fn linked_artifact_entries_are_refused_before_mutation() {
    assert_alias_refused(false);
    assert_alias_refused(true);
}

#[test]
fn absent_artifact_target_is_not_created_through_an_alias() {
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("absent");
    let link = directory.path().join("entry");
    symlink_file(&target, &link);
    for writable in [false, true] {
        assert!(super::super::local_file::open_local_file(&link, writable).is_err());
        assert!(!target.exists());
    }
}

#[test]
fn regular_artifacts_keep_read_append_lock_and_replace_behavior() {
    let directory = tempfile::tempdir().unwrap();
    let server = server(directory.path());
    assert!(server.import_snapshots_while_admitted().unwrap().is_empty());
    let guard = server.lock_import_admission().unwrap();
    let journal = directory.path().join("agent-import-ledger.jsonl");
    append_private_import_ledger(&journal, b"first\n", set_private_file).unwrap();
    append_private_import_ledger(&journal, b"second\n", set_private_file).unwrap();
    assert_eq!(fs::read(&journal).unwrap(), b"first\nsecond\n");
    drop(guard);
    drop(server.lock_import_admission_shared().unwrap());
    let stage = directory.path().join("staged.xml");
    write_private(&stage, b"before").unwrap();
    write_private(&stage, b"after").unwrap();
    assert_eq!(fs::read(&stage).unwrap(), b"after");
    let receipts = directory.path().join("agent-egress.jsonl");
    super::super::egress::append_egress_line(&receipts, "{}").unwrap();
    assert_eq!(
        super::super::egress::read_egress_tail(&receipts, 1)
            .unwrap()
            .records,
        ["{}"]
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for path in [
            journal,
            stage,
            receipts,
            directory.path().join("agent-import-admission.lock"),
        ] {
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
}

#[test]
fn directory_artifact_leaf_is_refused() {
    let directory = tempfile::tempdir().unwrap();
    for writable in [false, true] {
        assert!(super::super::local_file::open_local_file(directory.path(), writable).is_err());
    }
}

#[cfg(unix)]
#[test]
fn non_regular_artifact_admission_does_not_wait_for_a_peer() {
    use std::os::unix::ffi::OsStrExt;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("entry");
    let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    // SAFETY: name is a valid NUL-terminated temporary path.
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    for writable in [false, true] {
        assert_eq!(
            super::super::local_file::open_local_file(&path, writable)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::InvalidInput
        );
    }
}
