use super::*;
use bridge_tally_transport::TallyEndpointConfig;
use tally_protocol_simulator::{
    Fixture, ResponseFraming, ScenarioPlan, SequenceSimulator, WireEncoding,
};

const GUID: &str = "00000000-0000-4000-8000-000000000001";
const CAPTURED_GUID: &str = "61c6de69-1748-461c-ad3f-162cb949df9f";

fn captured_catalogue_payload() -> ImportPayload {
    let mut input = payload();
    input.company_guid = CAPTURED_GUID.into();
    for voucher in &mut input.vouchers {
        voucher.voucher_type = VoucherType::Journal;
    }
    for entry in input
        .vouchers
        .iter_mut()
        .flat_map(|voucher| &mut voucher.entries)
    {
        entry.ledger = match entry.ledger.as_str() {
            "Expense" => "Bridge Nested Debtor WR4",
            "Bank" => "Cash",
            "Income" => "WR2 Sales",
            _ => unreachable!("known simulator payload ledger"),
        }
        .into();
    }
    input
}

fn payload() -> ImportPayload {
    serde_json::from_value(json!({"company_guid":GUID,"vouchers":[
        {"bridge_txn_id":"txn-001","date":"2026-09-01","voucher_type":"Payment","narration":"Paid & settled","reference":"REF-1","entries":[{"ledger":"Expense","amount":"12.50","side":"Dr"},{"ledger":"Bank","amount":"12.50","side":"Cr"}]},
        {"bridge_txn_id":"txn-002","date":"2026-09-02","voucher_type":"Receipt","entries":[{"ledger":"Bank","amount":"7.50","side":"Dr"},{"ledger":"Income","amount":"7.50","side":"Cr"}]}
    ]})).expect("sample payload")
}

#[tokio::test]
async fn batch_total_overflow_is_refused_before_dispatch_or_persistence() {
    let directory = tempfile::tempdir().unwrap();
    let server = Server::new(super::super::Settings {
        endpoint: TallyEndpointConfig {
            host: "127.0.0.1".into(),
            port: 9,
        },
        data_dir: directory.path().to_path_buf(),
        max_rows: 10,
        max_bytes: 200_000,
        redaction: super::super::Redaction::None,
        import_enabled: true,
    });
    let mut input = payload();
    for entry in input
        .vouchers
        .iter_mut()
        .flat_map(|voucher| &mut voucher.entries)
    {
        entry.amount = format!("{}.99", "9".repeat(253));
    }
    validate_payload(&input).expect("each voucher individually fits exact decimal bounds");
    let result = server
        .build_import_xml(&serde_json::to_value(input).unwrap())
        .await;
    let failure = result.err().unwrap();
    assert_eq!(failure.code, "voucher_amount_overflow");
    assert!(failure.evidence.is_none());
    assert!(!directory.path().join("imports").exists());
    assert!(!directory.path().join("agent-import-ledger.jsonl").exists());
}

#[test]
fn narration_and_reference_reject_reserved_markers_after_entity_decoding() {
    for text in ["[bridge:forged]", "&#91;BrIdGe:forged]"] {
        let mut input = payload();
        input.vouchers[0].narration = Some(text.to_string());
        assert_eq!(
            validate_payload(&input),
            Err("narration_reserved_marker".to_string())
        );

        let mut input = payload();
        input.vouchers[0].reference = Some(text.to_string());
        assert_eq!(
            validate_payload(&input),
            Err("narration_reserved_marker".to_string())
        );
    }
}

#[test]
fn import_ledger_append_rolls_back_a_partial_failing_write() {
    struct FailingWriter {
        bytes: Vec<u8>,
    }

    impl ImportLedgerWriter for FailingWriter {
        fn length(&mut self) -> std::io::Result<u64> {
            Ok(self.bytes.len() as u64)
        }

        fn append(&mut self, bytes: &[u8]) -> std::io::Result<()> {
            self.bytes.extend_from_slice(&bytes[..1]);
            Err(std::io::Error::other("synthetic write failure"))
        }

        fn sync(&mut self) -> std::io::Result<()> {
            Ok(())
        }

        fn truncate(&mut self, length: u64) -> std::io::Result<()> {
            self.bytes.truncate(length as usize);
            Ok(())
        }
    }

    let mut writer = FailingWriter {
        bytes: b"before\n".to_vec(),
    };
    assert_eq!(
        append_import_ledger_bytes(&mut writer, b"after\n"),
        Err("import_ledger_unavailable".to_string())
    );
    assert_eq!(writer.bytes, b"before\n");
}

#[test]
fn external_import_ledger_read_waits_for_the_append_admission_lock() {
    let directory = tempfile::tempdir().expect("temporary agent directory");
    let settings = super::super::Settings {
        endpoint: TallyEndpointConfig {
            host: "127.0.0.1".to_string(),
            port: 9,
        },
        data_dir: directory.path().to_path_buf(),
        max_rows: 10,
        max_bytes: 200_000,
        redaction: super::super::Redaction::None,
        import_enabled: true,
    };
    let server = Server::new(settings.clone());
    let append_admission = server
        .lock_import_admission()
        .expect("append admission lock");
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        started_tx.send(()).expect("reader started");
        done_tx
            .send(Server::new(settings).import_ledger())
            .expect("reader completed");
    });
    started_rx.recv().expect("reader started");
    assert!(
        done_rx
            .recv_timeout(std::time::Duration::from_millis(100))
            .is_err(),
        "the external reader must wait while an append is admitted"
    );
    drop(append_admission);
    assert!(done_rx
        .recv()
        .expect("reader result")
        .expect("ledger read after append admission releases")
        .is_empty());
    reader.join().expect("reader thread");
}

#[test]
fn concurrent_verifications_replace_both_proofs_and_status_under_one_admission() {
    let directory = tempfile::tempdir().expect("temporary agent directory");
    let settings = super::super::Settings {
        endpoint: TallyEndpointConfig {
            host: "127.0.0.1".into(),
            port: 9,
        },
        data_dir: directory.path().to_path_buf(),
        max_rows: 10,
        max_bytes: 200_000,
        redaction: super::super::Redaction::None,
        import_enabled: true,
    };
    let server = Server::new(settings.clone());
    let initial = ImportLedgerLine {
        batch_id: "batch-proof".into(),
        company_guid: GUID.into(),
        company: None,
        txn_ids: vec![],
        date_from: "20260901".into(),
        date_to: "20260901".into(),
        sha256: "hash".into(),
        built_at: now(),
        status: "built".into(),
        pre_import_mark: PreImportMark {
            kind: "company_high_water".into(),
            value: Some(10),
            master_value: Some(10),
        },
        vouchers: vec![],
    };
    server.append_import_ledger(&initial).unwrap();
    let admission = server
        .lock_import_admission()
        .expect("hold publication admission");
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let writers = ["first", "second"].map(|state| {
        let mut update = initial.clone();
        update.status = state.into();
        let settings = settings.clone();
        let started = started_tx.clone();
        let done = done_tx.clone();
        std::thread::spawn(move || {
            let proof = json!({"batch_id":"batch-proof", "company":{"name":state}, "writer":state});
            started.send(()).expect("writer started");
            let result = Server::new(settings).persist_import_verification(&proof, &update);
            done.send(result).expect("writer result");
        })
    });
    for _ in 0..2 {
        started_rx.recv().expect("both writers started");
    }
    assert!(done_rx
        .recv_timeout(std::time::Duration::from_millis(100))
        .is_err());
    let json_path = directory.path().join("imports/batch-proof.proof.json");
    let md_path = directory.path().join("imports/batch-proof.proof.md");
    let published_before_admission = json_path.exists() || md_path.exists();
    drop(admission);
    for writer in writers {
        writer.join().expect("publication writer");
    }
    for _ in 0..2 {
        done_rx
            .recv()
            .expect("publication result")
            .expect("publication succeeds");
    }
    assert!(
        !published_before_admission,
        "proof files must wait for publication admission"
    );
    let proof: Value =
        serde_json::from_slice(&fs::read(json_path).expect("JSON proof")).expect("parseable proof");
    let markdown = fs::read_to_string(md_path).expect("Markdown proof");
    let latest = server
        .latest_import_line("batch-proof")
        .expect("ledger read")
        .expect("published status");
    assert_eq!(proof["writer"], latest.status);
    assert!(markdown.contains(&format!("- Company: `{}`", latest.status)));
}

#[test]
fn schema_balance_matcher_rendering_and_ledger_append_are_fail_closed() {
    let input = payload();
    validate_payload(&input).expect("valid payload");
    let mut unbalanced = input.clone();
    unbalanced.vouchers[0].entries[1].amount = "12.49".to_string();
    assert_eq!(
        validate_payload(&unbalanced),
        Err("voucher_not_balanced".to_string())
    );
    assert_eq!(
        master_match("bank ", &["Bank".to_string()])["match_state"],
        "near_miss"
    );
    assert_eq!(
        master_match("bank", &["Bank".to_string()])["match_state"],
        "near_miss"
    );
    assert_eq!(
        master_match("A\u{a0}B", &["A B".to_string()])["match_state"],
        "near_miss"
    );
    assert_eq!(
        master_match("Fees-Admin", &["Fees–Admin".to_string()])["match_state"],
        "near_miss"
    );
    assert_eq!(
        master_match("Bob's", &["Bob’s".to_string()])["match_state"],
        "near_miss"
    );
    assert_eq!(
        master_match("Bank", &["Bank Charges".to_string()])["match_state"],
        "near_miss"
    );
    let xml = render_import_xml("Book & Co", &input.vouchers);
    assert!(xml.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
    assert!(xml.contains("<NARRATION>Paid &amp; settled [BRIDGE:txn-001]</NARRATION>"));
    assert!(xml.contains("<NARRATION>[BRIDGE:txn-002]</NARRATION>"));
    assert_eq!(
        voucher_input_schema()["properties"]["vouchers"]["items"]["properties"]["entries"]["items"]
            ["properties"]["side"]["enum"],
        json!(["Dr", "Cr"])
    );
    let directory = tempfile::tempdir().expect("temporary data directory");
    let server = Server::new(super::super::Settings {
        endpoint: TallyEndpointConfig {
            host: "127.0.0.1".to_string(),
            port: 9,
        },
        data_dir: directory.path().to_path_buf(),
        max_rows: 10,
        max_bytes: 200_000,
        redaction: super::super::Redaction::None,
        import_enabled: true,
    });
    let line = ImportLedgerLine {
        batch_id: "batch-a".to_string(),
        company_guid: GUID.to_string(),
        company: None,
        txn_ids: vec!["txn-001".to_string()],
        date_from: "2026-09-01".to_string(),
        date_to: "2026-09-01".to_string(),
        sha256: "hash".to_string(),
        built_at: now(),
        status: "built".to_string(),
        pre_import_mark: PreImportMark {
            kind: "latest_voucher_alterid_seen".to_string(),
            value: Some(4),
            master_value: Some(4),
        },
        vouchers: vec![input.vouchers[0].clone()],
    };
    server.append_import_ledger(&line).expect("append");
    assert_eq!(server.import_ledger().expect("read").len(), 1);

    let settings = server.settings.clone();
    let first_settings = settings.clone();
    let first_line = line.clone();
    let second_settings = settings;
    let second_line = line.clone();
    let first =
        std::thread::spawn(move || Server::new(first_settings).append_import_ledger(&first_line));
    let second =
        std::thread::spawn(move || Server::new(second_settings).append_import_ledger(&second_line));
    first.join().expect("first writer").expect("first append");
    second
        .join()
        .expect("second writer")
        .expect("second append");
    let lines = server.import_ledger().expect("parseable ledger lines");
    assert_eq!(lines.len(), 3);
}

#[test]
fn duplicate_detection_uses_stable_voucher_identity_independently_of_remote_id() {
    for use_guid in [true, false] {
        for remote_id in [None, Some("shared-remote")] {
            let row = |id: &str| ReadVoucher {
                remote_id: remote_id.map(str::to_string),
                guid: use_guid.then(|| id.to_string()),
                master_id: (!use_guid).then(|| id.to_string()),
                alter_id: Some(11),
                date: Some("20260901".into()),
                voucher_type: Some("Payment".into()),
                narration: None,
                voucher_number: None,
                cancelled: Some(false),
                optional: Some(false),
                entries: vec![ReadEntry {
                    ledger: "Expense".into(),
                    amount: "-12.50".into(),
                    is_deemed_positive: "Yes".into(),
                }],
            };
            let first = row("1");
            let second = row("2");
            let found =
                test_duplicates(&[first.clone(), second.clone()]).expect("identified vouchers");
            assert!(found
                .iter()
                .any(|item| item["kind"] == "accounting_fingerprint"));
            assert_eq!(
                found.iter().any(|item| item["kind"] == "remote_id"),
                remote_id.is_some()
            );
            assert!(
                test_duplicates(&[first.clone(), first])
                    .expect("same identity")
                    .is_empty(),
                "repeated wire rows do not establish separate posted vouchers"
            );
            let mut different = second;
            different.entries[0].amount = "-13.00".into();
            let found = test_duplicates(&[row("1"), different]).expect("different contents");
            assert!(!found
                .iter()
                .any(|item| item["kind"] == "accounting_fingerprint"));
            assert_eq!(
                found.iter().any(|item| item["kind"] == "remote_id"),
                remote_id.is_some()
            );
        }
    }
}

#[test]
fn verification_masks_entry_diffs_and_duplicate_fingerprints_before_release() {
    let input = payload();
    let line = ImportLedgerLine {
        batch_id: "synthetic-redaction-batch".into(),
        company_guid: GUID.into(),
        company: None,
        txn_ids: vec!["txn-001".into()],
        date_from: "20260901".into(),
        date_to: "20260901".into(),
        sha256: "synthetic-hash".into(),
        built_at: now(),
        status: "built".into(),
        pre_import_mark: PreImportMark {
            kind: "company_high_water".into(),
            value: Some(10),
            master_value: Some(10),
        },
        vouchers: vec![input.vouchers[0].clone()],
    };
    let voucher = ReadVoucher {
        remote_id: None,
        guid: Some("synthetic-guid-1".into()),
        master_id: None,
        alter_id: Some(11),
        date: Some("20260901".into()),
        voucher_type: Some("Payment".into()),
        narration: Some("[BRIDGE:txn-001]".into()),
        voucher_number: None,
        cancelled: Some(false),
        optional: Some(false),
        entries: vec![ReadEntry {
            ledger: "Private Synthetic Party".into(),
            amount: "-12.50".into(),
            is_deemed_positive: "Yes".into(),
        }],
    };
    let mut duplicate = voucher.clone();
    duplicate.guid = Some("synthetic-guid-2".into());
    duplicate.narration = None;
    let mut unrelated = duplicate.clone();
    unrelated.guid = Some("synthetic-guid-3".into());
    unrelated.entries[0].ledger = "Unrelated Synthetic Party".into();
    let mut unrelated_duplicate = unrelated.clone();
    unrelated_duplicate.guid = Some("synthetic-guid-4".into());
    let result =
        verify_observed_batch(&line, &[voucher, duplicate, unrelated, unrelated_duplicate])
            .unwrap();
    assert_eq!(result["counts"]["posted_divergent"], 1);
    assert_eq!(result["duplicates"].as_array().unwrap().len(), 1);
    assert_eq!(
        result["unrelated_duplicates_in_window"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let masked = super::super::redact_value(result.clone(), super::super::Redaction::MaskParties);
    for name in [
        "Expense",
        "Bank",
        "Private Synthetic Party",
        "Unrelated Synthetic Party",
    ] {
        assert!(
            !masked.to_string().contains(name),
            "unmasked ledger in verification output: {name}"
        );
    }
    let clear = super::super::redact_value(result, super::super::Redaction::None);
    assert_eq!(
        clear["vouchers"][0]["diffs"][0]["entries"]["observed"][0]["ledger"],
        "Private Synthetic Party"
    );
    for key in ["duplicates", "unrelated_duplicates_in_window"] {
        assert!(masked[key][0].get("fingerprint").is_none());
        assert_eq!(
            masked[key][0]["fingerprint_sha256"].as_str().unwrap().len(),
            64
        );
    }
}

#[test]
fn verification_reports_absence_divergence_and_duplicate_fingerprints() {
    let input = payload();
    let line = ImportLedgerLine {
        batch_id: "batch-b".to_string(),
        company_guid: GUID.to_string(),
        company: None,
        txn_ids: input
            .vouchers
            .iter()
            .map(|voucher| voucher.bridge_txn_id.clone())
            .collect(),
        date_from: "2026-09-01".to_string(),
        date_to: "2026-09-02".to_string(),
        sha256: "hash".to_string(),
        built_at: now(),
        status: "built".to_string(),
        pre_import_mark: PreImportMark {
            kind: "latest_voucher_alterid_seen".to_string(),
            value: Some(10),
            master_value: Some(10),
        },
        vouchers: input.vouchers,
    };
    let observed = vec![
        ReadVoucher {
            remote_id: Some("txn-001".to_string()),
            guid: Some("g-1".to_string()),
            alter_id: Some(12),
            date: Some("20260901".to_string()),
            voucher_type: Some("Payment".to_string()),
            narration: Some("[BRIDGE:txn-001]".to_string()),
            voucher_number: None,
            master_id: None,
            cancelled: Some(false),
            optional: Some(false),
            entries: vec![
                ReadEntry {
                    ledger: "Expense".to_string(),
                    amount: "-12.51".to_string(),
                    is_deemed_positive: "Yes".to_string(),
                },
                ReadEntry {
                    ledger: "Bank".to_string(),
                    amount: "12.50".to_string(),
                    is_deemed_positive: "No".to_string(),
                },
            ],
        },
        ReadVoucher {
            remote_id: Some("other-id".to_string()),
            guid: Some("g-2".to_string()),
            alter_id: Some(13),
            date: Some("20260901".to_string()),
            voucher_type: Some("Payment".to_string()),
            narration: None,
            voucher_number: None,
            master_id: None,
            cancelled: Some(false),
            optional: Some(false),
            entries: vec![
                ReadEntry {
                    ledger: "Expense".to_string(),
                    amount: "-12.51".to_string(),
                    is_deemed_positive: "Yes".to_string(),
                },
                ReadEntry {
                    ledger: "Bank".to_string(),
                    amount: "12.50".to_string(),
                    is_deemed_positive: "No".to_string(),
                },
            ],
        },
    ];
    let result = verify_observed_batch(&line, &observed).expect("verification result");
    assert_eq!(result["counts"]["posted_divergent"], 1);
    assert_eq!(result["counts"]["not_found"], 1);
    assert_eq!(result["duplicates"].as_array().map(Vec::len), Some(1));
    assert_eq!(
        parse_import_vouchers(""),
        Err("import_verification_protocol_invalid".to_string())
    );
    assert_eq!(
        parse_import_vouchers("<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><RESPONSE>error</RESPONSE></BODY></ENVELOPE>"),
        Err("import_verification_protocol_invalid".to_string())
    );
}

#[test]
fn verification_window_corroboration_rejects_each_unsafe_branch() {
    let voucher = |guid: &str, alter_id, date: &str| ReadVoucher {
        remote_id: None,
        guid: Some(guid.to_string()),
        alter_id: Some(alter_id),
        date: Some(date.to_string()),
        voucher_type: None,
        narration: None,
        voucher_number: None,
        master_id: None,
        cancelled: Some(false),
        optional: Some(false),
        entries: vec![],
    };
    let inside = voucher("guid-1", 3, "20260901");
    assert_eq!(
        corroborate_observed_window(
            &[voucher("guid-1", 3, "20260903")],
            std::slice::from_ref(&inside),
            "20260901",
            "20260902",
        ),
        Err("window_not_honoured".to_string())
    );
    assert_eq!(
        corroborate_observed_window(
            std::slice::from_ref(&inside),
            &[voucher("guid-2", 3, "20260901")],
            "20260901",
            "20260902",
        ),
        Err("verification_incomplete:window_not_corroborated".to_string())
    );
    assert_eq!(
        corroborate_observed_window(
            std::slice::from_ref(&inside),
            std::slice::from_ref(&inside),
            "20260901",
            "20260902",
        ),
        Ok(())
    );
}

#[test]
fn verification_narration_with_truncated_text_is_not_a_completeness_marker() {
    let xml = "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><VOUCHER><ISCANCELLED>No</ISCANCELLED><ISOPTIONAL>No</ISOPTIONAL><DATE>20260901</DATE><VOUCHERTYPENAME>Payment</VOUCHERTYPENAME><GUID>guid-1</GUID><ALTERID>3</ALTERID><NARRATION>truncated payment</NARRATION></VOUCHER></COLLECTION></DATA></BODY></ENVELOPE>";
    let observed = parse_import_vouchers(xml).expect("verification response");
    assert_eq!(
        corroborate_verification_window(&observed, &observed, "20260901", "20260902"),
        Ok(())
    );
}

#[test]
fn batch_guid_is_canonicalized_and_compared_case_insensitively() {
    let stored = canonical_batch_guid("A1B2-C3D4");
    assert_eq!(stored, "a1b2-c3d4");
    assert!(batch_guid_matches(&stored, "A1B2-C3D4"));
}

#[test]
fn unwritable_ledger_path_removes_the_written_import_file() {
    let directory = tempfile::tempdir().expect("temporary directory");
    fs::create_dir(directory.path().join("agent-import-ledger.jsonl"))
        .expect("directory makes the ledger path unwritable");
    let server = Server::new(super::super::Settings {
        endpoint: TallyEndpointConfig {
            host: "127.0.0.1".to_string(),
            port: 9,
        },
        data_dir: directory.path().to_path_buf(),
        max_rows: 10,
        max_bytes: 200_000,
        redaction: super::super::Redaction::None,
        import_enabled: true,
    });
    let input = payload();
    let line = ImportLedgerLine {
        batch_id: "batch-unwritable".to_string(),
        company_guid: GUID.to_string(),
        company: None,
        txn_ids: vec!["txn-001".to_string()],
        date_from: "20260901".to_string(),
        date_to: "20260901".to_string(),
        sha256: "hash".to_string(),
        built_at: now(),
        status: "built".to_string(),
        pre_import_mark: PreImportMark {
            kind: "company_high_water".to_string(),
            value: Some(10),
            master_value: Some(7),
        },
        vouchers: vec![input.vouchers[0].clone()],
    };
    let imports = server.imports_dir().unwrap();
    let _admission = server.lock_import_admission().unwrap();
    let result = persistence::persist_build(&imports, &line, b"xml", || {
        server.append_import_ledger_while_admitted(&line)
    });
    assert_eq!(result, Err("import_ledger_unavailable".into()));
    assert!(!imports.join("batch-unwritable.xml").exists());
    assert_eq!(persistence::require_settled(&imports), Ok(()));
}

#[test]
fn unrelated_window_duplicates_do_not_block_a_verified_batch() {
    let input = payload();
    let line = ImportLedgerLine {
        batch_id: "batch-unrelated".to_string(),
        company_guid: GUID.to_string(),
        company: None,
        txn_ids: vec!["txn-001".to_string()],
        date_from: "20260901".to_string(),
        date_to: "20260901".to_string(),
        sha256: "hash".to_string(),
        built_at: now(),
        status: "built".to_string(),
        pre_import_mark: PreImportMark {
            kind: "company_high_water".to_string(),
            value: Some(10),
            master_value: Some(7),
        },
        vouchers: vec![input.vouchers[0].clone()],
    };
    let expected = &line.vouchers[0];
    let posted = ReadVoucher {
        remote_id: Some("posted-1".to_string()),
        guid: Some("posted-guid".to_string()),
        alter_id: Some(11),
        date: Some("20260901".to_string()),
        voucher_type: Some("Payment".to_string()),
        narration: Some("[BRIDGE:txn-001]".to_string()),
        voucher_number: None,
        master_id: None,
        cancelled: Some(false),
        optional: Some(false),
        entries: vec![
            ReadEntry {
                ledger: "Expense".to_string(),
                amount: "-12.50".to_string(),
                is_deemed_positive: "Yes".to_string(),
            },
            ReadEntry {
                ledger: "Bank".to_string(),
                amount: "12.50".to_string(),
                is_deemed_positive: "No".to_string(),
            },
        ],
    };
    assert_eq!(expected.bridge_txn_id, "txn-001");
    let unrelated = |guid: &str| ReadVoucher {
        remote_id: Some("unrelated-duplicate".to_string()),
        guid: Some(guid.to_string()),
        alter_id: Some(3),
        date: Some("20260901".to_string()),
        voucher_type: Some("Journal".to_string()),
        narration: None,
        voucher_number: None,
        master_id: None,
        cancelled: Some(false),
        optional: Some(false),
        entries: vec![ReadEntry {
            ledger: "Unrelated".to_string(),
            amount: "1.00".to_string(),
            is_deemed_positive: "No".to_string(),
        }],
    };
    let result = verify_observed_batch(&line, &[posted, unrelated("u-1"), unrelated("u-2")])
        .expect("verification result");
    assert_eq!(result["counts"]["posted_verified"], 1);
    assert!(result["duplicates"].as_array().is_some_and(Vec::is_empty));
    assert_eq!(
        result["unrelated_duplicates_in_window"]
            .as_array()
            .map(Vec::len),
        Some(2)
    );
    assert_eq!(
        verification_status(&result, line.vouchers.len()),
        "posted_verified"
    );
}

#[test]
fn fingerprint_only_verification_requires_a_post_mark_voucher() {
    let input = payload();
    let line = ImportLedgerLine {
        batch_id: "batch-mark".to_string(),
        company_guid: GUID.to_string(),
        company: None,
        txn_ids: vec!["txn-001".to_string()],
        date_from: "20260901".to_string(),
        date_to: "20260901".to_string(),
        sha256: "hash".to_string(),
        built_at: now(),
        status: "built".to_string(),
        pre_import_mark: PreImportMark {
            kind: "company_high_water".to_string(),
            value: Some(10),
            master_value: Some(10),
        },
        vouchers: vec![input.vouchers[0].clone()],
    };
    let observed = |alter_id| ReadVoucher {
        remote_id: None,
        guid: Some("fingerprint-observed-guid".to_string()),
        alter_id: Some(alter_id),
        date: Some("20260901".to_string()),
        voucher_type: Some("Payment".to_string()),
        narration: None,
        voucher_number: None,
        master_id: None,
        cancelled: Some(false),
        optional: Some(false),
        entries: vec![
            ReadEntry {
                ledger: "Expense".to_string(),
                amount: "-12.50".to_string(),
                is_deemed_positive: "Yes".to_string(),
            },
            ReadEntry {
                ledger: "Bank".to_string(),
                amount: "12.50".to_string(),
                is_deemed_positive: "No".to_string(),
            },
        ],
    };
    assert_eq!(
        verify_observed_batch(&line, &[observed(10)]).expect("verification result")["vouchers"][0]
            ["status"],
        "not_attributable"
    );
    // A concurrent manual voucher can have the same accounting contents
    // after the build mark without ever importing this batch's file.
    let after = verify_observed_batch(&line, &[observed(11)]).expect("verification result");
    assert_eq!(after["vouchers"][0]["status"], "matching_content_observed");
    assert_eq!(after["vouchers"][0]["attribution"], "not_established");
    assert_eq!(after["counts"]["posted_verified"], 0);
    assert_eq!(verification_status(&after, 1), "verification_incomplete");
}

#[test]
fn fingerprint_fallback_consumes_an_observed_voucher_once_per_batch() {
    let input = payload();
    let mut duplicate = input.vouchers[0].clone();
    duplicate.bridge_txn_id = "txn-duplicate".to_string();
    let line = ImportLedgerLine {
        batch_id: "batch-fingerprint-once".to_string(),
        company_guid: GUID.to_string(),
        company: None,
        txn_ids: vec!["txn-001".to_string(), "txn-duplicate".to_string()],
        date_from: "20260901".to_string(),
        date_to: "20260901".to_string(),
        sha256: "hash".to_string(),
        built_at: now(),
        status: "built".to_string(),
        pre_import_mark: PreImportMark {
            kind: "company_high_water".to_string(),
            value: Some(10),
            master_value: Some(10),
        },
        vouchers: vec![input.vouchers[0].clone(), duplicate],
    };
    let observed = ReadVoucher {
        remote_id: None,
        guid: Some("posted-guid".to_string()),
        alter_id: Some(11),
        date: Some("20260901".to_string()),
        voucher_type: Some("Payment".to_string()),
        narration: None,
        voucher_number: None,
        master_id: None,
        cancelled: Some(false),
        optional: Some(false),
        entries: vec![
            ReadEntry {
                ledger: "Expense".to_string(),
                amount: "-12.50".to_string(),
                is_deemed_positive: "Yes".to_string(),
            },
            ReadEntry {
                ledger: "Bank".to_string(),
                amount: "12.50".to_string(),
                is_deemed_positive: "No".to_string(),
            },
        ],
    };
    let result = verify_observed_batch(&line, &[observed]).expect("verification result");
    assert_eq!(result["counts"]["matching_content_observed"], 1);
    assert_eq!(result["counts"]["posted_verified"], 0);
    assert_eq!(result["counts"]["not_found"], 1);
    assert_eq!(result["vouchers"][0]["ambiguous_within_batch"], true);
    assert_eq!(result["vouchers"][1]["status"], "not_found");
    assert_eq!(
        result["ambiguous_within_batch"],
        json!(["txn-001", "txn-duplicate"])
    );
}

#[test]
fn tagged_matches_are_reserved_and_consumed_independently_of_batch_order() {
    let input = payload();
    let mut duplicate = input.vouchers[0].clone();
    duplicate.bridge_txn_id = "txn-duplicate".to_string();
    let mut line = ImportLedgerLine {
        batch_id: "batch-fingerprint-once".to_string(),
        company_guid: GUID.to_string(),
        company: None,
        txn_ids: vec!["txn-001".to_string(), "txn-duplicate".to_string()],
        date_from: "20260901".to_string(),
        date_to: "20260901".to_string(),
        sha256: "hash".to_string(),
        built_at: now(),
        status: "built".to_string(),
        pre_import_mark: PreImportMark {
            kind: "company_high_water".to_string(),
            value: Some(10),
            master_value: Some(10),
        },
        vouchers: vec![input.vouchers[0].clone(), duplicate],
    };
    let observed = ReadVoucher {
        remote_id: None,
        guid: Some("posted-guid".to_string()),
        alter_id: Some(11),
        date: Some("20260901".to_string()),
        voucher_type: Some("Payment".to_string()),
        narration: Some("[BRIDGE:txn-001]".to_string()),
        voucher_number: None,
        master_id: None,
        cancelled: Some(false),
        optional: Some(false),
        entries: vec![
            ReadEntry {
                ledger: "Expense".to_string(),
                amount: "-12.50".to_string(),
                is_deemed_positive: "Yes".to_string(),
            },
            ReadEntry {
                ledger: "Bank".to_string(),
                amount: "12.50".to_string(),
                is_deemed_positive: "No".to_string(),
            },
        ],
    };
    for reverse in [false, true] {
        if reverse {
            line.vouchers.reverse();
        }
        let result = verify_observed_batch(&line, std::slice::from_ref(&observed))
            .expect("verification result");
        assert_eq!(result["counts"]["posted_verified"], 1);
        assert_eq!(verification_status(&result, 2), "verification_incomplete");
        for row in result["vouchers"].as_array().expect("voucher results") {
            assert_eq!(
                row["status"],
                if row["bridge_txn_id"] == "txn-001" {
                    "posted_verified"
                } else {
                    "not_found"
                }
            );
        }
    }
}

#[test]
fn narration_tag_verification_requires_a_post_mark_voucher() {
    let input = payload();
    let line = ImportLedgerLine {
        batch_id: "batch-tag-mark".to_string(),
        company_guid: GUID.to_string(),
        company: None,
        txn_ids: vec!["txn-001".to_string()],
        date_from: "20260901".to_string(),
        date_to: "20260901".to_string(),
        sha256: "hash".to_string(),
        built_at: now(),
        status: "built".to_string(),
        pre_import_mark: PreImportMark {
            kind: "company_high_water".to_string(),
            value: Some(10),
            master_value: Some(7),
        },
        vouchers: vec![input.vouchers[0].clone()],
    };
    let observed = |alter_id| ReadVoucher {
        remote_id: Some("posted-1".to_string()),
        guid: Some("posted-guid".to_string()),
        alter_id: Some(alter_id),
        date: Some("20260901".to_string()),
        voucher_type: Some("Payment".to_string()),
        narration: Some("[BRIDGE:txn-001]".to_string()),
        voucher_number: None,
        master_id: None,
        cancelled: Some(false),
        optional: Some(false),
        entries: vec![
            ReadEntry {
                ledger: "Expense".to_string(),
                amount: "-12.50".to_string(),
                is_deemed_positive: "Yes".to_string(),
            },
            ReadEntry {
                ledger: "Bank".to_string(),
                amount: "12.50".to_string(),
                is_deemed_positive: "No".to_string(),
            },
        ],
    };
    let before = verify_observed_batch(&line, &[observed(10)]).expect("pre-mark tag");
    assert_eq!(before["vouchers"][0]["status"], "not_attributable");
    assert_eq!(
        before["vouchers"][0]["reason"],
        "tag_precedes_pre_import_voucher_mark"
    );
    assert_eq!(
        verify_observed_batch(&line, &[observed(11)]).expect("post-mark tag")["vouchers"][0]
            ["status"],
        "posted_verified"
    );
}

#[test]
fn verification_compares_amounts_numerically_and_preserves_real_divergence() {
    let mut input = payload();
    for entry in &mut input.vouchers[0].entries {
        entry.amount = "0012.50".to_string();
    }
    validate_payload(&input).expect("leading zeros satisfy the input contract");
    let line = ImportLedgerLine {
        batch_id: "batch-tag-mark".to_string(),
        company_guid: GUID.to_string(),
        company: None,
        txn_ids: vec!["txn-001".to_string()],
        date_from: "20260901".to_string(),
        date_to: "20260901".to_string(),
        sha256: "hash".to_string(),
        built_at: now(),
        status: "built".to_string(),
        pre_import_mark: PreImportMark {
            kind: "company_high_water".to_string(),
            value: Some(10),
            master_value: Some(7),
        },
        vouchers: vec![input.vouchers[0].clone()],
    };
    let observed = |alter_id| ReadVoucher {
        remote_id: Some("posted-1".to_string()),
        guid: Some("posted-guid".to_string()),
        alter_id: Some(alter_id),
        date: Some("20260901".to_string()),
        voucher_type: Some("Payment".to_string()),
        narration: Some("[BRIDGE:txn-001]".to_string()),
        voucher_number: None,
        master_id: None,
        cancelled: Some(false),
        optional: Some(false),
        entries: vec![
            ReadEntry {
                ledger: "Expense".to_string(),
                amount: "-12.500".to_string(),
                is_deemed_positive: "Yes".to_string(),
            },
            ReadEntry {
                ledger: "Bank".to_string(),
                amount: "12.50".to_string(),
                is_deemed_positive: "No".to_string(),
            },
        ],
    };
    let matching = observed(11);
    let result = verify_observed_batch(&line, std::slice::from_ref(&matching))
        .expect("numeric verification");
    assert_eq!(result["vouchers"][0]["status"], "posted_verified");
    let mut divergent = matching;
    divergent.entries[0].amount = "-12.51".to_string();
    let result = verify_observed_batch(&line, &[divergent]).expect("numeric divergence");
    assert_eq!(result["vouchers"][0]["status"], "posted_divergent");
}

#[test]
fn verified_import_vouchers_require_observed_effective_accounting_flags() {
    let input = payload();
    let line = ImportLedgerLine {
        batch_id: "batch-accounting-state".to_string(),
        company_guid: GUID.to_string(),
        company: None,
        txn_ids: vec!["txn-001".to_string()],
        date_from: "20260901".to_string(),
        date_to: "20260901".to_string(),
        sha256: "hash".to_string(),
        built_at: now(),
        status: "built".to_string(),
        pre_import_mark: PreImportMark {
            kind: "company_high_water".to_string(),
            value: Some(10),
            master_value: Some(7),
        },
        vouchers: vec![input.vouchers[0].clone()],
    };
    let observed = ReadVoucher {
        remote_id: Some("posted-1".to_string()),
        guid: Some("posted-guid".to_string()),
        alter_id: Some(11),
        date: Some("20260901".to_string()),
        voucher_type: Some("Payment".to_string()),
        narration: Some("[BRIDGE:txn-001]".to_string()),
        voucher_number: None,
        master_id: None,
        cancelled: Some(false),
        optional: Some(false),
        entries: vec![
            ReadEntry {
                ledger: "Expense".to_string(),
                amount: "-12.50".to_string(),
                is_deemed_positive: "Yes".to_string(),
            },
            ReadEntry {
                ledger: "Bank".to_string(),
                amount: "12.50".to_string(),
                is_deemed_positive: "No".to_string(),
            },
        ],
    };
    assert_eq!(
        verify_observed_batch(&line, std::slice::from_ref(&observed)).expect("effective voucher")
            ["vouchers"][0]["status"],
        "posted_verified"
    );
    for (cancelled, optional) in [(Some(true), Some(false)), (Some(false), Some(true))] {
        let mut ineffective = observed.clone();
        ineffective.cancelled = cancelled;
        ineffective.optional = optional;
        let result =
            verify_observed_batch(&line, &[ineffective]).expect("ineffective voucher result");
        assert_eq!(result["vouchers"][0]["status"], "posted_not_effective");
        assert_eq!(result["counts"]["posted_not_effective"], 1);
    }
    let mut missing = observed;
    missing.optional = None;
    assert_eq!(
        verify_observed_batch(&line, &[missing]),
        Err("voucher_accounting_state_not_observed".to_string())
    );
}

#[test]
fn company_high_water_mark_refuses_voucher_scan_shapes_and_preserves_attribution_boundary() {
    assert_eq!(
        company_high_water_mark(&json!({"vouchers":[{"alter_id":999}]})),
        Err("pre_import_mark_unobserved".to_string())
    );
    let mark =
        company_high_water_mark(&json!({"altvchid":10,"altmstid":7})).expect("company high water");
    assert_eq!(mark.kind, "company_high_water");
    assert_eq!(mark.value, Some(10));
    assert_eq!(mark.master_value, Some(7));
}

#[test]
fn verification_unescapes_every_record_text_node_before_fingerprinting() {
    let xml = "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><VOUCHER><GUID>guid-escape</GUID><ISCANCELLED>No</ISCANCELLED><ISOPTIONAL>No</ISOPTIONAL><DATE>20260901</DATE><VOUCHERTYPENAME>Payment</VOUCHERTYPENAME><NARRATION>Party &amp; Co &lt;quoted&gt; &quot;name&quot; &#x26;</NARRATION><ALLLEDGERENTRIES.LIST><LEDGERNAME>R&amp;D &lt;Lab&gt; &quot;A&quot; &#38;</LEDGERNAME><ISDEEMEDPOSITIVE>Yes</ISDEEMEDPOSITIVE><AMOUNT>-12.50</AMOUNT></ALLLEDGERENTRIES.LIST></VOUCHER></COLLECTION></DATA></BODY></ENVELOPE>";
    let observed = parse_import_vouchers(xml).expect("escaped export parses");
    assert_eq!(
        observed.rows[0].narration.as_deref(),
        Some("Party & Co <quoted> \"name\" &")
    );
    assert_eq!(observed.rows[0].entries[0].ledger, "R&D <Lab> \"A\" &");
}

#[test]
fn verification_rejects_unknown_entities_in_ledger_and_narration_fragments() {
    for xml in [
        "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><VOUCHER><ALLLEDGERENTRIES.LIST><LEDGERNAME>A&bogus;B</LEDGERNAME></ALLLEDGERENTRIES.LIST></VOUCHER></COLLECTION></DATA></BODY></ENVELOPE>",
        "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><VOUCHER><NARRATION>A&bogus;B</NARRATION></VOUCHER></COLLECTION></DATA></BODY></ENVELOPE>",
    ] {
        assert_eq!(
            parse_import_vouchers(xml),
            Err("import_verification_export_invalid".to_string())
        );
    }
}

#[test]
fn verification_rejects_incomplete_ledger_entries() {
    for entry in [
        "<AMOUNT>-12.50</AMOUNT><ISDEEMEDPOSITIVE>Yes</ISDEEMEDPOSITIVE>",
        "<LEDGERNAME>Expense</LEDGERNAME><ISDEEMEDPOSITIVE>Yes</ISDEEMEDPOSITIVE>",
        "<LEDGERNAME>Expense</LEDGERNAME><AMOUNT>-12.50</AMOUNT>",
    ] {
        let xml = format!(
            "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><VOUCHER><ALLLEDGERENTRIES.LIST>{entry}</ALLLEDGERENTRIES.LIST></VOUCHER></COLLECTION></DATA></BODY></ENVELOPE>"
        );
        assert_eq!(
            parse_import_vouchers(&xml),
            Err("import_verification_export_invalid".to_string())
        );
    }
}

#[test]
fn optional_voucher_number_is_rendered_only_when_valid_and_supplied() {
    let mut input = payload();
    input.vouchers[0].voucher_number = Some("PV-0001".to_string());
    let rendered = render_import_xml("Book", &input.vouchers);
    assert!(rendered.contains("<VOUCHERNUMBER>PV-0001</VOUCHERNUMBER>"));
    assert_eq!(rendered.matches("<VOUCHERNUMBER>").count(), 1);
    input.vouchers[0].voucher_number = Some("bad$number".to_string());
    assert_eq!(
        validate_payload(&input),
        Err("voucher_number_invalid".to_string())
    );
}

#[test]
fn batch_company_tuple_rejects_a_same_guid_different_book() {
    let company = |books_from: &str| bridge_tally_protocol::TallyCompany {
        name: "Bridge Book".to_string(),
        guid: Some(GUID.to_string()),
        company_number: Some("1".to_string()),
        books_from: Some(books_from.to_string()),
    };
    let original = import_company_tuple(&company("20260401")).expect("complete tuple");
    let different_book = import_company_tuple(&company("20270401")).expect("complete tuple");
    assert_ne!(original, different_book);
}

#[tokio::test]
async fn simulator_verification_is_independent_of_the_output_row_limit() {
    for max_rows in [1, 2, 10] {
        let simulator =
            SequenceSimulator::spawn(qualified_import_cycle_plans()).expect("simulator");
        let directory = tempfile::tempdir().expect("temporary data directory");
        let server = Server::new(super::super::Settings {
            endpoint: TallyEndpointConfig {
                host: "127.0.0.1".to_string(),
                port: simulator.address().port(),
            },
            data_dir: directory.path().to_path_buf(),
            max_rows,
            max_bytes: 200_000,
            redaction: super::super::Redaction::None,
            import_enabled: true,
        });
        let built = server
            .build_import_xml(&serde_json::to_value(captured_catalogue_payload()).expect("json"))
            .await
            .expect("build");
        let batch_id = built.payload["result"]["batch_id"]
            .as_str()
            .expect("batch id")
            .to_string();
        assert_eq!(
            built.payload["result"]["live_evidence"],
            "synthetic_lab_readback"
        );
        assert!(directory
            .path()
            .join("imports")
            .join(format!("{batch_id}.xml"))
            .exists());
        let proof = server
            .verify_import(&json!({"company_guid":CAPTURED_GUID,"batch_id":batch_id}))
            .await
            .expect("verify");
        assert_eq!(proof.payload["result"]["counts"]["posted_verified"], 2);
        assert!(directory
            .path()
            .join("imports")
            .join(format!(
                "{}.proof.md",
                proof.payload["result"]["batch_id"]
                    .as_str()
                    .expect("batch id")
            ))
            .exists());
        assert_eq!(simulator.finish().expect("requests").len(), 42);
    }
}

fn import_cycle_plans() -> Vec<ScenarioPlan> {
    let company = format!("<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><COMPANY NAME=\"WR2 Unicode Lab\"><GUID>{CAPTURED_GUID}</GUID><COMPANYNUMBER>1</COMPANYNUMBER><BOOKSFROM>20260401</BOOKSFROM></COMPANY></COLLECTION></DATA></BODY></ENVELOPE>");
    // Replay byte-exact live catalogue; only the simulator inputs adapt to it.
    let bytes = include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-ledger-catalogue.utf16le.xml"
    );
    let words = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect::<Vec<_>>();
    let ledgers = String::from_utf16(&words).expect("captured native catalogue");
    let premark = format!("<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><COMPANY><GUID>{CAPTURED_GUID}</GUID><ALTVCHID>10</ALTVCHID><ALTMSTID>7</ALTMSTID></COMPANY></COLLECTION></DATA></BODY></ENVELOPE>");
    let status = "<RESPONSE>TallyPrime Server is Running</RESPONSE>".to_string();
    let readback = concat!(
        "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>",
        "<VOUCHER REMOTEID=\"tally-assigned-1\"><DATE>20260901</DATE><VOUCHERNUMBER>PV-1</VOUCHERNUMBER><VOUCHERTYPENAME>Journal</VOUCHERTYPENAME><GUID>g-1</GUID><MASTERID>1</MASTERID><ALTERID>12</ALTERID><NARRATION>Paid &amp; settled [BRIDGE:txn-001]</NARRATION><ISCANCELLED>No</ISCANCELLED><ISOPTIONAL>No</ISOPTIONAL><ALLLEDGERENTRIES.LIST><LEDGERNAME>Bridge Nested Debtor WR4</LEDGERNAME><ISDEEMEDPOSITIVE>Yes</ISDEEMEDPOSITIVE><AMOUNT>-12.50</AMOUNT></ALLLEDGERENTRIES.LIST><ALLLEDGERENTRIES.LIST><LEDGERNAME>Cash</LEDGERNAME><ISDEEMEDPOSITIVE>No</ISDEEMEDPOSITIVE><AMOUNT>12.50</AMOUNT></ALLLEDGERENTRIES.LIST></VOUCHER>",
        "<VOUCHER REMOTEID=\"tally-assigned-2\"><DATE>20260902</DATE><VOUCHERNUMBER>RV-1</VOUCHERNUMBER><VOUCHERTYPENAME>Journal</VOUCHERTYPENAME><GUID>g-2</GUID><MASTERID>2</MASTERID><ALTERID>13</ALTERID><NARRATION>[BRIDGE:txn-002]</NARRATION><ISCANCELLED>No</ISCANCELLED><ISOPTIONAL>No</ISOPTIONAL><ALLLEDGERENTRIES.LIST><LEDGERNAME>Cash</LEDGERNAME><ISDEEMEDPOSITIVE>Yes</ISDEEMEDPOSITIVE><AMOUNT>-7.50</AMOUNT></ALLLEDGERENTRIES.LIST><ALLLEDGERENTRIES.LIST><LEDGERNAME>WR2 Sales</LEDGERNAME><ISDEEMEDPOSITIVE>No</ISDEEMEDPOSITIVE><AMOUNT>7.50</AMOUNT></ALLLEDGERENTRIES.LIST></VOUCHER></COLLECTION></DATA></BODY></ENVELOPE>"
    )
    .to_string();
    vec![
        company.clone(),
        status.clone(),
        company.clone(),
        status.clone(),
        company.clone(),
        ledgers.clone(),
        status.clone(),
        ledgers,
        status.clone(),
        company.clone(),
        company.clone(),
        premark.clone(),
        status.clone(),
        premark,
        status.clone(),
        company.clone(),
        company.clone(),
        status.clone(),
        company.clone(),
        status.clone(),
        company.clone(),
        readback.clone(),
        status.clone(),
        readback.clone(),
        status.clone(),
        company.clone(),
        company.clone(),
        readback.clone(),
        status.clone(),
        readback,
        status.clone(),
        company,
    ]
    .into_iter()
    .enumerate()
    .map(|(index, xml)| {
        if matches!(index, 1 | 3 | 6 | 8 | 12 | 14 | 17 | 19 | 22 | 24 | 28 | 30) {
            ScenarioPlan::new(Fixture::ProductStatus(
                tally_protocol_simulator::ProductStatus::TallyPrime,
            ))
            .with_framing(ResponseFraming::ContentLength)
        } else {
            ScenarioPlan::new(Fixture::SyntheticXml(xml))
                .with_encoding(WireEncoding::Utf16Le)
                .with_framing(ResponseFraming::ContentLength)
        }
    })
    .collect()
}

#[test]
fn import_catalogue_fixture_is_byte_exact_live_capture() {
    let bytes = include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-ledger-catalogue.utf16le.xml"
    );
    let metadata: Value = serde_json::from_str(include_str!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-ledger-catalogue.json"
    ))
    .unwrap();
    assert_eq!(bytes.len(), 13_060);
    assert_eq!(
        sha256_hex(bytes),
        "f354993704f0feddc27a46d73b4ca10787b6028d9f385d8323994b18e5b34e0f"
    );
    assert_eq!(
        metadata["fixture_sha256"],
        metadata["source_response_sha256"]
    );
    assert_eq!(metadata["fixture_sha256"], sha256_hex(bytes));
    assert_eq!(metadata["transformation"], json!([]));
}

#[path = "agent_recovery_tests.rs"]
mod recovery_tests;

#[test]
fn native_cmpinfo_counter_does_not_become_an_import_verification_voucher() {
    let bytes = include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-empty-collection.utf16le.xml"
    );
    let words = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect::<Vec<_>>();
    let xml = String::from_utf16(&words).expect("captured UTF-16LE response");
    assert!(parse_import_vouchers(&xml)
        .expect("native empty verification collection")
        .rows
        .is_empty());
}

#[test]
fn native_captured_import_readback_keeps_direct_amounts_and_padded_identifiers() {
    let bytes = include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-three-vouchers.utf16le.xml"
    );
    let words = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect::<Vec<_>>();
    let xml = String::from_utf16(&words).expect("captured UTF-16LE response");
    let rows = parse_import_vouchers(&xml).expect("captured native readback");
    assert_eq!(rows.rows.len(), 3);
    for (row, (id, amount)) in
        rows.rows
            .iter()
            .zip([(1, "-101.01"), (2, "-102.02"), (3, "-103.03")])
    {
        assert_eq!(row.alter_id, Some(id));
        assert_eq!(row.entries.len(), 2);
        assert_eq!(row.entries[0].amount, amount);
    }
}

#[test]
fn master_match_bounds_suggestions_before_copying_names_and_preserves_ambiguity() {
    let catalogue = (0..100)
        .map(|index| format!("Ledger {index:03}"))
        .collect::<Vec<_>>();
    let matched = master_match("L", &catalogue);
    assert_eq!(matched["match_state"], "near_miss");
    assert_eq!(matched["candidate_count"], 100);
    assert_eq!(matched["candidates_truncated"], true);
    assert_eq!(matched["candidates"].as_array().unwrap().len(), 25);
    assert_eq!(
        master_match("Ledger 099", &catalogue)["match_state"],
        "exact"
    );
    let huge = format!("Large{}", "x".repeat(8192));
    let limited = master_match("L", std::slice::from_ref(&huge));
    assert_eq!(limited["match_state"], "near_miss");
    assert_eq!(limited["candidate_count"], 1);
    assert_eq!(limited["candidates_truncated"], true);
    assert!(limited["candidates"].as_array().unwrap().is_empty());
    assert!(!limited.to_string().contains(&huge));
}

#[tokio::test]
async fn import_bounds_distinct_ledger_names_before_tally_without_reducing_voucher_limit() {
    let mut repeated = payload();
    repeated.vouchers = (0..1000)
        .map(|index| {
            let mut voucher = repeated.vouchers[0].clone();
            voucher.bridge_txn_id = format!("txn-{index}");
            voucher
        })
        .collect();
    assert_eq!(validate_payload(&repeated), Ok(()));
    let mut unique = repeated.clone();
    unique.vouchers.truncate(100);
    for (index, voucher) in unique.vouchers.iter_mut().enumerate() {
        voucher.entries[0].ledger = format!("Synthetic Ledger {index}");
    }
    let mut at_limit = unique.clone();
    at_limit.vouchers.pop();
    assert_eq!(validate_payload(&at_limit), Ok(()));
    assert_eq!(
        validate_payload(&unique),
        Err("voucher_unique_ledger_limit_exceeded".into())
    );
    let directory = tempfile::tempdir().unwrap();
    let server = Server::new(super::super::Settings {
        endpoint: TallyEndpointConfig {
            host: "127.0.0.1".into(),
            port: 9,
        },
        data_dir: directory.path().to_path_buf(),
        max_rows: 500,
        max_bytes: 200_000,
        redaction: super::super::Redaction::None,
        import_enabled: true,
    });
    let response = server
        .call_tool_response("build_import_xml", serde_json::to_value(unique).unwrap())
        .await;
    assert_eq!(
        response.value["structuredContent"]["result"]["error"]["code"],
        "voucher_unique_ledger_limit_exceeded"
    );
    assert_eq!(response.value["structuredContent"]["evidence"]["bytes"], 0);
    assert!(!directory.path().join("imports").exists());
}

#[path = "agent_wire_evidence_tests.rs"]
mod wire_evidence_tests;

#[path = "agent_import_ledger_tests.rs"]
mod compact_ledger_tests;

#[path = "agent_failure_tests.rs"]
mod failure_tests;

#[path = "agent_import_boundary_tests.rs"]
mod boundary_tests;

#[path = "agent_import_multiplicity_tests.rs"]
mod multiplicity_tests;

fn verify_observed_batch(line: &ImportLedgerLine, rows: &[ReadVoucher]) -> Result<Value, String> {
    verify_batch(line, &ImportReadSource::admit(rows.to_vec())?)
}

fn corroborate_observed_window(
    observed: &[ReadVoucher],
    corroboration: &[ReadVoucher],
    from: &str,
    to: &str,
) -> Result<(), String> {
    corroborate_verification_window(
        &ImportReadSource::admit(observed.to_vec())?,
        &ImportReadSource::admit(corroboration.to_vec())?,
        from,
        to,
    )
}

#[path = "agent_import_source_tests.rs"]
mod source_tests;

#[path = "agent_import_index_tests.rs"]
mod index_tests;

fn test_duplicates(observed: &[ReadVoucher]) -> Result<Vec<Value>, String> {
    let identities = observed
        .iter()
        .map(observed_voucher_identity)
        .collect::<Result<Vec<_>, _>>()?;
    let fingerprints = observed
        .iter()
        .map(|voucher| sha256_json(&observed_fingerprint(voucher)))
        .collect::<Vec<_>>();
    Ok(duplicates(observed, &identities, &fingerprints))
}

#[path = "agent_import_qualification_tests.rs"]
mod qualification_tests;

fn qualified_import_cycle_plans() -> Vec<ScenarioPlan> {
    let cycle = import_cycle_plans();
    let probe = mode_tests::licensed_import_probe();
    [
        probe.clone(),
        cycle[..16].to_vec(),
        cycle[4..10].to_vec(),
        probe,
        cycle[16..].to_vec(),
    ]
    .concat()
}

#[path = "agent_import_mode_tests.rs"]
mod mode_tests;

#[test]
fn voucher_and_import_read_filters_use_literal_dates_independently_of_static_periods() {
    // The recorded Education refusal affected ##SVFromDate/##SVToDate predicates.
    // Guard both production renderers against restoring that dependency; this
    // request test does not simulate or qualify Tally's date behavior.
    let (from, to) = ("20260815", "20260822");
    let requests = [
        render_import_verification_read("Synthetic Book", from, to),
        super::super::render_agent_vouchers("Synthetic Book", from, to, None).unwrap(),
    ];
    for request in requests {
        let mut reader = quick_xml::Reader::from_str(&request);
        let mut formulae = Vec::new();
        let mut bounds = BTreeMap::new();
        loop {
            match reader.read_event().unwrap() {
                quick_xml::events::Event::Start(tag) => {
                    let name = tag.name();
                    if name.as_ref() == b"SYSTEM" {
                        let text = reader.read_text(name).unwrap();
                        let decoded = text.decode().unwrap();
                        formulae.push(quick_xml::escape::unescape(&decoded).unwrap().into_owned());
                    } else if matches!(name.as_ref(), b"SVFROMDATE" | b"SVTODATE") {
                        let value = reader
                            .read_text(name)
                            .unwrap()
                            .decode()
                            .unwrap()
                            .into_owned();
                        assert!(bounds.insert(name.as_ref().to_vec(), value).is_none());
                    }
                }
                quick_xml::events::Event::Eof => break,
                _ => {}
            }
        }
        assert_eq!(
            formulae,
            [format!(
                "$Date >= $$Date:\"{from}\" AND $Date <= $$Date:\"{to}\""
            )]
        );
        assert_eq!(
            bounds.get(b"SVFROMDATE".as_slice()).map(String::as_str),
            Some(from)
        );
        assert_eq!(
            bounds.get(b"SVTODATE".as_slice()).map(String::as_str),
            Some(to)
        );
    }
}
