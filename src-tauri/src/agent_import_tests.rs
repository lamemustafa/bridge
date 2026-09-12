use super::super::ToolResponse;
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

/// The shared valid batch. Its Payment and Receipt carry no `reference`,
/// which these types refuse: the qualified bank shape has no such element.
/// `agent_import_post_tests` covers reference rendering on a Journal.
fn payload() -> ImportPayload {
    serde_json::from_value(json!({"company_guid":GUID,"vouchers":[
        {"bridge_txn_id":"txn-001","date":"2026-09-01","voucher_type":"Payment","narration":"Paid & settled","entries":[{"ledger":"Expense","amount":"12.50","side":"Dr"},{"ledger":"Bank","amount":"12.50","side":"Cr"}]},
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
        writes_enabled: false,
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
fn external_import_ledger_read_refuses_busy_admission_without_waiting() {
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
        writes_enabled: false,
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
    let result = done_rx.recv_timeout(std::time::Duration::from_secs(2));
    drop(append_admission); // Release even if the assertion fails, so no reader is stranded.
    assert_eq!(
        result.expect("reader must not wait for the lock").err(),
        Some("import_admission_busy".into())
    );
    assert!(server.import_ledger().unwrap().is_empty());
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
        writes_enabled: false,
    };
    let server = Server::new(settings.clone());
    let initial = ImportLedgerLine {
        endpoint_origin: None,
        identity_scheme: None,
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
    let generation = server
        .latest_import_snapshot(&initial.batch_id)
        .unwrap()
        .unwrap()
        .generation;
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
            let result =
                Server::new(settings).persist_import_verification(&proof, &update, generation);
            done.send(result).expect("writer result");
        })
    });
    for _ in 0..2 {
        started_rx.recv().expect("both writers started");
    }
    let results: Vec<_> = (0..2)
        .map(|_| done_rx.recv_timeout(std::time::Duration::from_secs(2)))
        .collect();
    let json_path = directory.path().join("imports/batch-proof.proof.json");
    let md_path = directory.path().join("imports/batch-proof.proof.md");
    assert!(!json_path.exists() && !md_path.exists());
    drop(admission);
    for writer in writers {
        writer.join().expect("publication writer");
    }
    for result in results {
        assert_eq!(
            result.expect("busy writer must return"),
            Err("import_admission_busy".into())
        );
    }
    for (index, state) in ["first", "second"].into_iter().enumerate() {
        let mut update = initial.clone();
        update.status = state.into();
        let proof = json!({"batch_id":"batch-proof", "company":{"name":state}, "writer":state});
        let result = server.persist_import_verification(&proof, &update, generation);
        if index == 0 {
            result.unwrap();
        } else {
            assert_eq!(result, Err("import_verification_conflict_retry".into()));
        }
    }
    let proof: Value =
        serde_json::from_slice(&fs::read(json_path).expect("JSON proof")).expect("parseable proof");
    let markdown = fs::read_to_string(md_path).expect("Markdown proof");
    let latest = server
        .latest_import_snapshot("batch-proof")
        .expect("ledger read")
        .expect("published status");
    assert_eq!(proof["writer"], latest.batch.status);
    assert!(markdown.contains(&format!("- Company: `{}`", latest.batch.status)));
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
    // ASCII case and one trailing space are transformations
    // `TALLY_PROTOCOL_REFERENCE.md` §9.4b measured Tally performing, so they
    // name the same live ledger: they bind and report its exact spelling. Only
    // byte equality is `exact`, which is what build_import_xml admits.
    for wanted in ["bank ", "bank", "BANK"] {
        let matched = one_master_match(wanted, &["Bank"]);
        assert_eq!(matched["match_state"], "normalized");
        assert_eq!(
            matched["exact_live_spelling"][super::super::PARTY_NAME_MARKER],
            "Bank"
        );
    }
    // A non-breaking space, an en dash and a curly quote are **not** on that
    // list. Each still reaches its master through the wide fold, so the caller
    // sees one candidate and confirms it; what it no longer gets is an answer
    // and a live spelling to copy into the exact-only write gate.
    for (wanted, live) in [
        ("A\u{a0}B", "A B"),
        ("Fees-Admin", "Fees\u{2013}Admin"),
        ("Bob's", "Bob\u{2019}s"),
    ] {
        let matched = one_master_match(wanted, &[live]);
        assert_eq!(
            matched["match_state"], "near_miss",
            "{wanted} resolved on an unverified fold"
        );
        assert!(matched.get("exact_live_spelling").is_none());
        assert_eq!(
            matched["candidates"][0]["name"][super::super::PARTY_NAME_MARKER],
            live
        );
    }
    assert_eq!(one_master_match("Bank", &["Bank"])["match_state"], "exact");
    // A shorter name that a live ledger extends is a near-miss with one
    // candidate, and one candidate is still not a decision.
    let near = one_master_match("Bank", &["Bank Charges"]);
    assert_eq!(near["match_state"], "near_miss");
    assert_eq!(near["reason"], "master_binding_near_miss");
    assert!(near.get("exact_live_spelling").is_none());
    assert_eq!(
        near["candidates"][0]["name"][super::super::PARTY_NAME_MARKER],
        "Bank Charges"
    );
    let xml = render_import_xml("Book & Co", &input.vouchers, "batch-render");
    assert!(xml.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
    assert!(xml.contains(&format!(
        "<NARRATION>Paid &amp; settled [BRIDGE:{}]</NARRATION>",
        import_identity("batch-render", "txn-001")
    )));
    assert!(xml.contains(&format!(
        "<NARRATION>[BRIDGE:{}]</NARRATION>",
        import_identity("batch-render", "txn-002")
    )));
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
        writes_enabled: false,
    });
    let line = ImportLedgerLine {
        endpoint_origin: None,
        identity_scheme: None,
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
    let results = [
        first.join().expect("first writer"),
        second.join().expect("second writer"),
    ];
    let accepted = results.iter().filter(|result| result.is_ok()).count();
    assert!(accepted >= 1);
    for result in results.into_iter().filter_map(Result::err) {
        assert_eq!(result, "import_admission_busy");
    }
    let lines = server.import_ledger().expect("parseable ledger lines");
    assert_eq!(lines.len(), 1 + accepted);
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
        endpoint_origin: None,
        identity_scheme: None,
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
        endpoint_origin: None,
        identity_scheme: None,
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
        parse_import_vouchers("", CAPTURED_GUID),
        Err("import_verification_protocol_invalid".to_string())
    );
    assert_eq!(
        parse_import_vouchers("<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><RESPONSE>error</RESPONSE></BODY></ENVELOPE>", CAPTURED_GUID),
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
    let observed = parse_import_vouchers(xml, "guid").expect("verification response");
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
        writes_enabled: false,
    });
    let input = payload();
    let line = ImportLedgerLine {
        endpoint_origin: None,
        identity_scheme: None,
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
        endpoint_origin: None,
        identity_scheme: None,
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
        endpoint_origin: None,
        identity_scheme: None,
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
        endpoint_origin: None,
        identity_scheme: None,
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
        endpoint_origin: None,
        identity_scheme: None,
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
        endpoint_origin: None,
        identity_scheme: None,
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
        endpoint_origin: None,
        identity_scheme: None,
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
        endpoint_origin: None,
        identity_scheme: None,
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
    let observed = parse_import_vouchers(xml, "guid").expect("escaped export parses");
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
            parse_import_vouchers(xml, CAPTURED_GUID),
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
            parse_import_vouchers(&xml, CAPTURED_GUID),
            Err("import_verification_export_invalid".to_string())
        );
    }
}

#[test]
fn optional_voucher_number_is_rendered_only_when_valid_and_supplied() {
    let mut input = payload();
    input.vouchers[0].voucher_number = Some("PV-0001".to_string());
    let rendered = render_import_xml("Book", &input.vouchers, "batch-render");
    assert!(rendered.contains("<VOUCHERNUMBER>PV-0001</VOUCHERNUMBER>"));
    assert_eq!(rendered.matches("<VOUCHERNUMBER>").count(), 1);
    input.vouchers[0].voucher_number = Some("bad$number".to_string());
    assert_eq!(
        validate_payload(&input),
        Err("voucher_number_invalid".to_string())
    );
}

#[test]
fn voucher_number_length_counts_unicode_characters_and_preserves_safety_checks() {
    assert_eq!(
        voucher_input_schema()["properties"]["vouchers"]["items"]["properties"]["voucher_number"]
            ["maxLength"],
        32
    );
    let eleven = "क".repeat(11);
    assert_eq!(eleven.chars().count(), 11);
    assert_eq!(eleven.len(), 33);
    let mut input = captured_catalogue_payload();
    for number in [eleven, "क".repeat(32), "A".repeat(32)] {
        input.vouchers[0].voucher_number = Some(number);
        assert_eq!(validate_payload(&input), Ok(()));
    }
    for (number, code) in [
        ("क".repeat(33), "voucher_number_invalid"),
        ("A".repeat(33), "voucher_number_invalid"),
        ("bad$number".into(), "voucher_number_invalid"),
        ("bad\nnumber".into(), "voucher_text_invalid"),
        ("bad\u{007f}number".into(), "voucher_text_invalid"),
        (String::new(), "voucher_text_invalid"),
    ] {
        input.vouchers[0].voucher_number = Some(number);
        assert_eq!(validate_payload(&input), Err(code.to_string()));
    }
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
            writes_enabled: false,
        });
        let built = server
            .build_import_xml(&serde_json::to_value(captured_catalogue_payload()).expect("json"))
            .await
            .expect("build");
        let batch_id = built.payload["result"]["batch_id"]
            .as_str()
            .expect("batch id")
            .to_string();
        assert_eq!(built.payload["result"]["identity_scheme"], "batch_v1");
        let saved = server
            .latest_import_snapshot(&batch_id)
            .unwrap()
            .unwrap()
            .batch;
        assert!(matches!(
            saved.identity_scheme,
            Some(ImportIdentityScheme::BatchV1)
        ));
        assert_eq!(saved.txn_ids, ["txn-001", "txn-002"]);
        assert_eq!(
            built.payload["result"]["live_evidence"],
            json!([{"observation":"synthetic_lab_readback",
                "report":"docs/agent/ASSESSMENT-2026-09-06.md",
                "voucher_types":["Journal"]}])
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
        // These historical replay rows have caller-selected raw tags. They can
        // corroborate contents but cannot prove attribution to this fresh batch.
        assert_eq!(proof.payload["result"]["counts"]["posted_verified"], 0);
        assert_eq!(
            proof.payload["result"]["counts"]["matching_content_observed"],
            2
        );
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
        assert_eq!(simulator.finish().expect("requests").len(), 50);
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
        "<VOUCHER REMOTEID=\"tally-assigned-1\"><DATE>20260901</DATE><VOUCHERNUMBER>PV-1</VOUCHERNUMBER><VOUCHERTYPENAME>Journal</VOUCHERTYPENAME><GUID>61c6de69-1748-461c-ad3f-162cb949df9f-00000001</GUID><MASTERID>1</MASTERID><ALTERID>12</ALTERID><NARRATION>Paid &amp; settled [BRIDGE:txn-001]</NARRATION><ISCANCELLED>No</ISCANCELLED><ISOPTIONAL>No</ISOPTIONAL><ALLLEDGERENTRIES.LIST><LEDGERNAME>Bridge Nested Debtor WR4</LEDGERNAME><ISDEEMEDPOSITIVE>Yes</ISDEEMEDPOSITIVE><AMOUNT>-12.50</AMOUNT></ALLLEDGERENTRIES.LIST><ALLLEDGERENTRIES.LIST><LEDGERNAME>Cash</LEDGERNAME><ISDEEMEDPOSITIVE>No</ISDEEMEDPOSITIVE><AMOUNT>12.50</AMOUNT></ALLLEDGERENTRIES.LIST></VOUCHER>",
        "<VOUCHER REMOTEID=\"tally-assigned-2\"><DATE>20260902</DATE><VOUCHERNUMBER>RV-1</VOUCHERNUMBER><VOUCHERTYPENAME>Journal</VOUCHERTYPENAME><GUID>61c6de69-1748-461c-ad3f-162cb949df9f-00000002</GUID><MASTERID>2</MASTERID><ALTERID>13</ALTERID><NARRATION>[BRIDGE:txn-002]</NARRATION><ISCANCELLED>No</ISCANCELLED><ISOPTIONAL>No</ISOPTIONAL><ALLLEDGERENTRIES.LIST><LEDGERNAME>Cash</LEDGERNAME><ISDEEMEDPOSITIVE>Yes</ISDEEMEDPOSITIVE><AMOUNT>-7.50</AMOUNT></ALLLEDGERENTRIES.LIST><ALLLEDGERENTRIES.LIST><LEDGERNAME>WR2 Sales</LEDGERNAME><ISDEEMEDPOSITIVE>No</ISDEEMEDPOSITIVE><AMOUNT>7.50</AMOUNT></ALLLEDGERENTRIES.LIST></VOUCHER></COLLECTION></DATA></BODY></ENVELOPE>"
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
    assert!(parse_import_vouchers(&xml, CAPTURED_GUID)
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
    let rows = parse_import_vouchers(&xml, CAPTURED_GUID).expect("captured native readback");
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

/// Binds one name against a fabricated catalogue and returns its rendered row.
fn one_master_match(wanted: &str, catalogue: &[&str]) -> Value {
    let catalogue = catalogue
        .iter()
        .map(|name| (*name).to_string())
        .collect::<Vec<_>>();
    master_report(
        &source_entities(&[wanted.to_string()]).expect("fabricated name parses"),
        &catalogue,
    )
    .expect("fabricated catalogue binds")
    .remove(0)
}

#[test]
fn master_match_bounds_suggestions_before_copying_names_and_preserves_ambiguity() {
    let catalogue = (0..100)
        .map(|index| format!("Ledger {index:03}"))
        .collect::<Vec<_>>();
    let borrowed = catalogue.iter().map(String::as_str).collect::<Vec<_>>();
    // A name reaching a whole family distinguishes none of it. Measured against
    // live books, listing an arbitrary capped slice omitted the right master
    // about a third of the time, so the family is counted and not listed.
    let matched = one_master_match("Ledger", &borrowed);
    assert_eq!(matched["match_state"], "near_miss");
    assert_eq!(
        matched["reason"],
        "master_binding_no_discriminating_candidate"
    );
    assert_eq!(matched["candidate_count"], 100);
    assert_eq!(matched["candidates_truncated"], true);
    assert!(matched["candidates"].as_array().unwrap().is_empty());
    // A family inside the bound is still listed in full.
    let small = (0..5)
        .map(|index| format!("Small {index:02}"))
        .collect::<Vec<_>>();
    let small_ref = small.iter().map(String::as_str).collect::<Vec<_>>();
    let listed = one_master_match("Small", &small_ref);
    assert_eq!(listed["candidates"].as_array().unwrap().len(), 5);
    assert_eq!(
        one_master_match("Ledger 099", &borrowed)["match_state"],
        "exact"
    );
    // A single pathological live name is bounded by bytes before it is copied
    // into a result, and its true count is still reported.
    let huge = format!("Large{}", "x".repeat(8192));
    let limited = one_master_match("Large", &[huge.as_str()]);
    assert_eq!(limited["match_state"], "near_miss");
    assert_eq!(limited["candidate_count"], 1);
    assert_eq!(limited["candidates_truncated"], true);
    assert!(limited["candidates"].as_array().unwrap().is_empty());
    assert!(!limited.to_string().contains(&huge));
}

#[test]
fn a_catalogue_that_was_never_read_refuses_instead_of_reporting_everything_missing() {
    // "Nobody read the ledger list out of Tally first" is the recorded cause
    // of the one failed engagement, so an empty catalogue must not look like
    // an answer. P5: nothing-found and request-failed stay distinguishable.
    assert_eq!(
        master_report(&source_entities(&["Bank".to_string()]).expect("valid"), &[]),
        Err("master_catalog_empty".to_string())
    );
}

#[test]
fn an_embedded_identifier_decides_where_the_name_offers_wrong_candidates() {
    // Fabricated from a placeholder alphabet: the live ledger carries a number
    // the operator typed into its name, and the requested name matches no live
    // spelling. The number is the key; the name is a hint.
    let matched = one_master_match(
        "GAMMA. EPSILON 5550000001",
        &["GAMMA (5550000001)", "GAMMA ALPHA", "GAMMA BETA"],
    );
    assert_eq!(matched["match_state"], "identifier");
    assert_eq!(
        matched["exact_live_spelling"][super::super::PARTY_NAME_MARKER],
        "GAMMA (5550000001)"
    );
}

#[test]
fn a_near_miss_never_names_a_live_spelling_and_retains_its_identity() {
    let matched = one_master_match(
        "PARTY 5550000001",
        &["ALPHA (5550000001)", "BETA (5550000001)"],
    );
    assert_eq!(matched["match_state"], "near_miss");
    assert_eq!(matched["reason"], "master_binding_identifier_conflict");
    assert!(matched.get("exact_live_spelling").is_none());
    assert_eq!(matched["candidate_count"], 2);
    assert_eq!(
        matched["unresolved_identity"][0]["value"][super::super::PARTY_NAME_MARKER],
        "5550000001"
    );
}

#[test]
fn nothing_defensible_is_reported_missing_with_no_candidate() {
    let matched = one_master_match("Zeta Placeholder", &["Bank", "Cash"]);
    assert_eq!(matched["match_state"], "missing");
    assert_eq!(matched["reason"], "master_binding_no_candidate");
    assert!(matched["candidates"].as_array().unwrap().is_empty());
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
        writes_enabled: false,
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

#[tokio::test]
async fn built_batch_guidance_matches_the_saved_native_admission() {
    for (writes_enabled, voucher_count, numbered, native) in [
        (true, 1, false, true),
        (true, 2, false, false),
        (true, 1, true, false),
        (false, 1, false, false),
    ] {
        let simulator = SequenceSimulator::spawn(qualified_import_cycle_plans()[..32].to_vec())
            .expect("captured build plan");
        let directory = tempfile::tempdir().unwrap();
        let server = Server::new(crate::agent::Settings {
            endpoint: TallyEndpointConfig {
                host: "127.0.0.1".into(),
                port: simulator.address().port(),
            },
            data_dir: directory.path().into(),
            max_rows: 10,
            max_bytes: 200_000,
            redaction: crate::agent::Redaction::None,
            import_enabled: true,
            writes_enabled,
        });
        let mut input = captured_catalogue_payload();
        input.vouchers.truncate(voucher_count);
        if numbered {
            input.vouchers[0].voucher_number = Some("TEST-1".into());
        }
        let built = server
            .build_import_xml(&serde_json::to_value(input).unwrap())
            .await
            .unwrap();
        let result = &built.payload["result"];
        assert_eq!(result["voucher_count"], voucher_count);
        let next_step = result["next_step"].as_str().unwrap();
        assert_eq!(next_step.starts_with("Call post_import"), native);
        assert_eq!(
            next_step.starts_with("Confirm the loaded company matches this batch"),
            !native
        );
        if writes_enabled {
            assert!(result["warnings"][0]
                .as_str()
                .unwrap()
                .contains("do not call post_import"));
        }
        // The company-identity warning is unconditional: present for this
        // Journal-only batch exactly as it would be for a bank one.
        let warnings = result["warnings"]
            .as_array()
            .expect("warnings array")
            .iter()
            .map(|value| value.as_str().unwrap())
            .collect::<Vec<_>>();
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("Confirm the loaded company before importing")),
            "company-identity warning missing from a Journal-only batch: {warnings:?}"
        );
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("including a Journal")
                    && warning.contains("verify_import for read-only reconciliation")),
            "unknown-outcome guidance must remain read-only for Journal: {warnings:?}"
        );
        // The check the warning asks for is §9.13's five-element tuple, and
        // the operator can only perform it if the fifth element is visible:
        // endpoint_origin is recorded on the batch and compared on dispatch,
        // but a hand import never reaches that check.
        assert!(
            warnings.iter().any(|warning| warning
                .contains("endpoint origin, name, GUID, company number and books-from, all five")),
            "the identity warning must enumerate the whole tuple: {warnings:?}"
        );
        assert!(
            result["endpoint_origin"].is_string(),
            "the batch must expose the endpoint origin the warning tells the operator to compare"
        );
        // The stale-classification warning is bank-gated and must not appear
        // for a Journal-only batch.
        assert!(
            !warnings
                .iter()
                .any(|warning| warning.contains("Regrouping a ledger afterwards")),
            "stale-classification warning leaked into a Journal-only batch: {warnings:?}"
        );
        // The release-evidence warning is bank-gated too: §9.13's licensed
        // 7.1 Gold measurement has nothing to do with a Journal-only batch.
        assert!(
            !warnings
                .iter()
                .any(|warning| warning.contains("measured on licensed TallyPrime 7.1 Gold only")),
            "release-evidence warning leaked into a Journal-only batch: {warnings:?}"
        );
        assert_eq!(simulator.finish().unwrap().len(), 32);
    }
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

#[path = "agent_import_bank_tests.rs"]
mod bank_tests;

fn qualified_import_cycle_plans() -> Vec<ScenarioPlan> {
    let cycle = import_cycle_plans();
    let probe = mode_tests::licensed_import_probe();
    [
        probe.clone(),
        cycle[..16].to_vec(),
        cycle[4..10].to_vec(),
        build_preflight_plans(),
        probe.clone(),
        probe,
        cycle[16..].to_vec(),
    ]
    .concat()
}

fn build_preflight_plans() -> Vec<ScenarioPlan> {
    let captured = include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-empty-collection.utf16le.xml"
    );
    let empty = String::from_utf16(
        &captured
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let mut plans = import_cycle_plans()[4..10].to_vec();
    for index in [1, 3] {
        plans[index].fixture = Fixture::SyntheticXml(empty.clone());
        plans[index].encoding = WireEncoding::Utf16LeNoBom;
        assert!(
            tally_protocol_simulator::encode(&plans[index].fixture.body(), plans[index].encoding)
                == captured,
            "preflight response must preserve the captured bytes"
        );
    }
    plans
}

#[path = "agent_import_preflight_tests.rs"]
mod preflight_tests;

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

#[path = "agent_import_verify_mode_tests.rs"]
mod verify_mode_tests;

#[path = "agent_import_identity_tests.rs"]
mod identity_tests;

#[path = "agent_import_text_tests.rs"]
mod text_tests;

#[tokio::test]
async fn dispatched_verification_requires_its_saved_endpoint_before_tally_reads() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("loopback listener");
    listener
        .set_nonblocking(true)
        .expect("nonblocking listener");
    let endpoint = TallyEndpointConfig {
        host: "127.0.0.1".into(),
        port: listener.local_addr().expect("listener address").port(),
    };
    let directory = tempfile::tempdir().expect("temporary data directory");
    let server = Server::new(super::super::Settings {
        endpoint: endpoint.clone(),
        data_dir: directory.path().to_path_buf(),
        max_rows: 10,
        max_bytes: 200_000,
        redaction: super::super::Redaction::None,
        import_enabled: true,
        writes_enabled: false,
    });
    let line = ImportLedgerLine {
        batch_id: "batch-dispatched-endpoint".into(),
        identity_scheme: Some(ImportIdentityScheme::BatchV1),
        company_guid: GUID.into(),
        endpoint_origin: Some("http://127.0.0.1:9002".into()),
        company: None,
        txn_ids: vec![],
        date_from: "20260901".into(),
        date_to: "20260901".into(),
        sha256: "hash".into(),
        built_at: now(),
        status: "built".into(),
        pre_import_mark: PreImportMark {
            kind: "company_high_water".into(),
            value: Some(1),
            master_value: Some(1),
        },
        vouchers: vec![],
    };
    server.append_import_ledger(&line).expect("saved batch");
    let admission = server.lock_import_admission().expect("admission lock");
    server
        .append_import_record_while_admitted(&ledger::StatusRecord::dispatch(&line))
        .expect("durable dispatch intent");
    drop(admission);

    let failure = match server
        .verify_import(&json!({"company_guid":GUID,"batch_id":line.batch_id}))
        .await
    {
        Err(failure) => failure,
        Ok(_) => panic!("mismatched dispatched endpoint must be refused"),
    };
    assert_eq!(failure.code, "import_post_endpoint_mismatch");
    assert!(matches!(
        listener.accept(),
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock
    ));

    let matching = ImportLedgerLine {
        endpoint_origin: Some(super::super::canonical_loopback_origin(&endpoint).unwrap()),
        ..line.clone()
    };
    assert_eq!(
        validate_dispatched_import_endpoint(&matching, true, &endpoint),
        Ok(())
    );
    let manual = ImportLedgerLine {
        endpoint_origin: None,
        ..line
    };
    assert_eq!(
        validate_dispatched_import_endpoint(&manual, false, &endpoint),
        Ok(())
    );
}

fn persisted_dispatch_response(created: u64, altered: u64) -> ledger::DispatchResponse {
    ledger::DispatchResponse {
        request_sha256: "a".repeat(64),
        response_sha256: "b".repeat(64),
        bytes: 1,
        outcome: Some(
            serde_json::from_value(json!({
                "application_status":"success",
                "counters": {
                    "created":created, "altered":altered, "deleted":0, "ignored":0,
                    "errors":0, "cancelled":0, "exceptions":0, "line_error_count":0
                },
                "exceptions_were_reported":true
            }))
            .unwrap(),
        ),
    }
}

async fn verify_saved_batch_after_dispatch(
    dispatched: bool,
    dispatch_response: Option<ledger::DispatchResponse>,
) -> (ToolResponse, ledger::BatchSnapshot, String) {
    let simulator = SequenceSimulator::spawn(qualified_import_cycle_plans()).expect("simulator");
    let directory = tempfile::tempdir().expect("temporary data directory");
    let server = Server::new(super::super::Settings {
        endpoint: TallyEndpointConfig {
            host: "127.0.0.1".into(),
            port: simulator.address().port(),
        },
        data_dir: directory.path().to_path_buf(),
        max_rows: 10,
        max_bytes: 200_000,
        redaction: super::super::Redaction::None,
        import_enabled: true,
        writes_enabled: false,
    });
    let built = server
        .build_import_xml(&serde_json::to_value(captured_catalogue_payload()).expect("input"))
        .await
        .expect("build");
    let batch_id = built.payload["result"]["batch_id"]
        .as_str()
        .expect("batch id")
        .to_string();
    let saved = server
        .latest_import_snapshot(&batch_id)
        .expect("saved snapshot")
        .expect("batch");
    if dispatched {
        let admission = server.lock_import_admission().expect("admission lock");
        server
            .append_import_record_while_admitted(&ledger::StatusRecord::dispatch(&saved.batch))
            .expect("dispatch intent");
        if let Some(response) = dispatch_response {
            server
                .append_import_record_while_admitted(&ledger::StatusRecord::response(
                    &saved.batch,
                    response,
                ))
                .expect("dispatch response");
        }
        drop(admission);
    }
    let response = server
        .call_tool_response(
            "verify_import",
            json!({"company_guid":CAPTURED_GUID,"batch_id":batch_id}),
        )
        .await;
    let snapshot = server
        .latest_import_snapshot(&batch_id)
        .expect("persisted snapshot")
        .expect("batch");
    assert_eq!(
        simulator.finish().expect("captured plan requests").len(),
        50
    );
    let markdown = fs::read_to_string(
        server
            .imports_dir()
            .unwrap()
            .join(format!("{batch_id}.proof.md")),
    )
    .unwrap();
    (response, snapshot, markdown)
}

#[tokio::test]
async fn dispatched_verification_persists_reconciliation_for_missing_or_dirty_response() {
    for response in [None, Some(persisted_dispatch_response(0, 1))] {
        let (outcome, snapshot, markdown) = verify_saved_batch_after_dispatch(true, response).await;
        assert_eq!(outcome.value["isError"], true);
        assert_eq!(
            outcome.value["structuredContent"]["result"]["dispatch"]["state"],
            "reconciliation_required"
        );
        assert_eq!(snapshot.batch.status, "verification_incomplete");
        assert!(markdown.contains("Dispatch verdict: `reconciliation_required`"));
        assert!(markdown.contains("this report does not confirm posting"));
        assert!(markdown.contains("Error: `import_reconciliation_required`"));
    }
}

#[tokio::test]
async fn current_dispatch_persists_its_reconciliation_verdict_before_returning_the_proof() {
    let simulator = SequenceSimulator::spawn(qualified_import_cycle_plans()).expect("simulator");
    let directory = tempfile::tempdir().expect("temporary data directory");
    let server = Server::new(super::super::Settings {
        endpoint: TallyEndpointConfig {
            host: "127.0.0.1".into(),
            port: simulator.address().port(),
        },
        data_dir: directory.path().to_path_buf(),
        max_rows: 10,
        max_bytes: 200_000,
        redaction: super::super::Redaction::None,
        import_enabled: true,
        writes_enabled: true,
    });
    let built = server
        .build_import_xml(&serde_json::to_value(captured_catalogue_payload()).expect("input"))
        .await
        .expect("build");
    let batch_id = built.payload["result"]["batch_id"]
        .as_str()
        .expect("batch id")
        .to_string();
    let saved = server
        .latest_import_snapshot(&batch_id)
        .expect("saved snapshot")
        .expect("batch");
    let response = persisted_dispatch_response(1, 0);
    let admission = server.lock_import_admission().expect("admission lock");
    server
        .append_import_record_while_admitted(&ledger::StatusRecord::dispatch(&saved.batch))
        .expect("dispatch intent");
    server
        .append_import_record_while_admitted(&ledger::StatusRecord::response(
            &saved.batch,
            response,
        ))
        .expect("dispatch response");
    drop(admission);

    let outcome = server
        .verify_import_after_current_dispatch(
            &json!({"company_guid":CAPTURED_GUID,"batch_id":batch_id}),
        )
        .await
        .expect("current dispatch verification");
    let persisted: Value = serde_json::from_slice(
        &std::fs::read(
            server
                .imports_dir()
                .expect("imports directory")
                .join(format!("{batch_id}.proof.json")),
        )
        .expect("persisted proof"),
    )
    .expect("proof JSON");
    let latest = server
        .latest_import_snapshot(&batch_id)
        .expect("latest snapshot")
        .expect("batch");
    assert_eq!(
        outcome.payload["result"]["dispatch"]["state"],
        "reconciliation_required"
    );
    assert_eq!(persisted["dispatch"], outcome.payload["result"]["dispatch"]);
    assert_eq!(persisted["dispatch"]["counters"]["created"], 1);
    assert_eq!(persisted["dispatch"]["automatic_retry"], false);
    assert_eq!(latest.batch.status, "verification_incomplete");
    assert_eq!(
        simulator.finish().expect("captured plan requests").len(),
        50
    );
}
