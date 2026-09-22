//! Admission tests for locally generated batches, not authored Tally responses.
use super::*;
use bridge_tally_transport::TallyEndpointConfig;

fn batch() -> (ImportLedgerLine, TallyEndpointConfig) {
    let endpoint = TallyEndpointConfig {
        host: "127.0.0.1".into(),
        port: 9001,
    };
    let mut line: ImportLedgerLine = serde_json::from_value(json!({
        "batch_id":"bridge-00000000-0000-4000-8000-000000000001", "identity_scheme":"batch_v1",
        "company_guid":"00000000-0000-4000-8000-000000000002",
        "endpoint_origin":super::super::super::canonical_loopback_origin(&endpoint).unwrap(),
        "company":{"name":"Synthetic Accounts","guid":"00000000-0000-4000-8000-000000000002","company_number":"100001","books_from":"20260401"},
        "txn_ids":["journal-test"],"date_from":"20260901","date_to":"20260901",
        "sha256":"", "built_at":"2026-09-07T00:00:00Z", "status":"built",
        "pre_import_mark":{"kind":"company_high_water","value":1,"master_value":1},
        "vouchers":[{"bridge_txn_id":"journal-test","date":"20260901","voucher_type":"Journal",
            "narration":"Synthetic test only","reference":"REF-1","entries":[
                {"ledger":"Expense","amount":"12.50","side":"Dr"},
                {"ledger":"Cash","amount":"12.50","side":"Cr"}]}]
    })).unwrap();
    line.sha256 = sha256_hex(
        render_import_xml("Synthetic Accounts", &line.vouchers, &line.batch_id).as_bytes(),
    );
    (line, endpoint)
}

#[test]
fn native_preview_contains_all_accounting_inputs_and_pinned_destination() {
    let (line, endpoint) = batch();
    let (_, preview) = admit_saved_journal(&line, &endpoint).unwrap();
    for field in [
        "Synthetic Accounts",
        "9001",
        "20260901",
        "Expense",
        "Cash",
        "12.50",
        "Dr",
        "Cr",
        "REF-1",
        "Synthetic test only",
        "Pause other edits/imports; keep this company and Tally mode unchanged until Bridge finishes.",
        &line.batch_id,
    ] {
        assert!(preview.contains(field), "missing {field}");
    }
}

#[test]
fn contended_post_returns_without_waiting_or_recording_a_dispatch() {
    let directory = tempfile::tempdir().unwrap();
    let (line, mut endpoint) = batch();
    // A regressed blocking lock is released below to let the worker exit.
    // Keep its endpoint different from the saved batch so that fallback path
    // fails local admission before any network access or native approval.
    endpoint.port = 9;
    let server = Server::new(crate::agent::Settings {
        endpoint,
        data_dir: directory.path().to_path_buf(),
        max_rows: 10,
        max_bytes: 200_000,
        redaction: crate::agent::Redaction::None,
        import_enabled: true,
        writes_enabled: true,
    });
    server.append_import_ledger(&line).unwrap();
    let journal = directory.path().join("agent-import-ledger.jsonl");
    let before = fs::read(&journal).unwrap();
    let admission = server.lock_import_admission().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = runtime.block_on(server.post_import(&json!({
            "company_guid":line.company_guid,"batch_id":line.batch_id
        })));
        tx.send(result.err().map(|failure| failure.code)).unwrap();
    });
    let result = rx.recv_timeout(std::time::Duration::from_secs(2));
    drop(admission);
    worker.join().unwrap();
    assert_eq!(result.unwrap(), Some("import_admission_busy".into()));
    assert_eq!(fs::read(journal).unwrap(), before);
}

#[test]
fn old_or_changed_batches_cannot_be_posted() {
    let (mut line, mut endpoint) = batch();
    endpoint.port = 9002;
    assert_eq!(
        admit_saved_journal_integrity(&line, &endpoint).unwrap_err(),
        "import_post_endpoint_mismatch"
    );
    endpoint.port = 9001;
    line.endpoint_origin = None;
    assert_eq!(
        admit_saved_journal_integrity(&line, &endpoint).unwrap_err(),
        "import_post_endpoint_mismatch"
    );
    let (mut line, endpoint) = batch();
    line.vouchers[0].narration = Some("Changed after build".into());
    assert_eq!(
        admit_saved_journal_integrity(&line, &endpoint).unwrap_err(),
        "import_batch_changed"
    );
}

#[test]
fn saved_integrity_allows_dispatched_recovery_without_reopening_native_preview_checks() {
    let (mut line, endpoint) = batch();
    line.vouchers[0].narration = Some("safe\u{200b}hidden".into());
    refresh_batch_sha256(&mut line);
    assert!(admit_saved_journal_integrity(&line, &endpoint).is_ok());
    assert_eq!(
        admit_saved_journal(&line, &endpoint).unwrap_err(),
        "import_review_format_text"
    );
}

#[test]
fn multiple_or_unqualified_vouchers_require_manual_workflow() {
    let (mut line, endpoint) = batch();
    line.vouchers.push(line.vouchers[0].clone());
    assert_eq!(
        admit_saved_journal(&line, &endpoint).unwrap_err(),
        "import_post_requires_one_journal"
    );
    line.vouchers.pop();
    line.vouchers[0].voucher_type = VoucherType::Payment;
    assert_eq!(
        admit_saved_journal(&line, &endpoint).unwrap_err(),
        "import_post_requires_one_journal"
    );
}

fn refresh_batch_sha256(line: &mut ImportLedgerLine) {
    line.sha256 = sha256_hex(
        render_import_xml(
            &line.company.as_ref().unwrap().name,
            &line.vouchers,
            &line.batch_id,
        )
        .as_bytes(),
    );
}

#[test]
fn native_preview_refuses_line_and_paragraph_separators_in_every_operator_text_field() {
    for separator in ['\u{2028}', '\u{2029}'] {
        for field in ["narration", "reference", "ledger"] {
            let (mut line, endpoint) = batch();
            let value = format!("safe{separator}hidden");
            match field {
                "narration" => line.vouchers[0].narration = Some(value),
                "reference" => line.vouchers[0].reference = Some(value),
                "ledger" => line.vouchers[0].entries[0].ledger = value,
                _ => unreachable!(),
            }
            refresh_batch_sha256(&mut line);
            assert_eq!(
                admit_saved_journal(&line, &endpoint).unwrap_err(),
                "import_review_layout_text",
                "{field} {separator:?}"
            );
        }
    }
}

#[test]
fn native_preview_refuses_unreviewable_formatting_in_every_operator_text_field() {
    for mark in [
        '\u{061c}',
        '\u{200e}',
        '\u{200f}',
        '\u{202a}',
        '\u{202b}',
        '\u{202c}',
        '\u{202d}',
        '\u{202e}',
        '\u{2066}',
        '\u{2067}',
        '\u{2068}',
        '\u{2069}',
        '\u{200b}',
        '\u{200c}',
        '\u{200d}',
        '\u{2060}',
        '\u{206a}',
        '\u{206b}',
        '\u{206c}',
        '\u{206d}',
        '\u{206e}',
        '\u{206f}',
        '\u{feff}',
        '\u{00ad}',
        '\u{034f}',
        '\u{115f}',
        '\u{180e}',
        '\u{fe0f}',
        '\u{e0100}',
        '\u{e007f}',
        '\u{0600}',
        '\u{fff0}',
    ] {
        for field in ["company", "number", "narration", "reference", "ledger"] {
            let (mut line, endpoint) = batch();
            let value = format!("safe{mark}hidden");
            match field {
                "company" => line.company.as_mut().unwrap().name = value,
                "number" => line.vouchers[0].voucher_number = Some(value),
                "narration" => line.vouchers[0].narration = Some(value),
                "reference" => line.vouchers[0].reference = Some(value),
                "ledger" => line.vouchers[0].entries[0].ledger = value,
                _ => unreachable!(),
            }
            refresh_batch_sha256(&mut line);
            assert_eq!(
                admit_saved_journal(&line, &endpoint).unwrap_err(),
                "import_review_format_text",
                "{field} {mark:?}"
            );
        }
    }
}

#[test]
fn native_preview_preserves_visible_multilingual_text() {
    let (mut line, endpoint) = batch();
    line.company.as_mut().unwrap().name = "मराठी खाते".into();
    line.vouchers[0].entries[0].ledger = "किराया".into();
    line.vouchers[0].narration = Some("Cafe\u{0301} – مصروف ₹12.50".into());
    refresh_batch_sha256(&mut line);
    let (_, preview) = admit_saved_journal(&line, &endpoint).unwrap();
    for text in ["मराठी खाते", "किराया", "Cafe\u{0301} – مصروف ₹12.50"]
    {
        assert!(preview.contains(text));
    }
}

fn dispatch_response(
    application_status: &str,
    created: u64,
    altered: u64,
) -> ledger::DispatchResponse {
    ledger::DispatchResponse {
        request_sha256: "a".repeat(64),
        response_sha256: "b".repeat(64),
        bytes: 1,
        outcome: Some(
            serde_json::from_value(json!({
                "application_status":application_status,
                "counters": {
                    "created":created,
                    "altered":altered,
                    "deleted":0,
                    "ignored":0,
                    "errors":0,
                    "cancelled":0,
                    "exceptions":0,
                    "line_error_count":0,
                    "counter_presence": {
                        "created":true, "altered":true, "deleted":true,
                        "ignored":true, "errors":true, "cancelled":true, "exceptions":true
                    }
                },
                "exceptions_were_reported":true
            }))
            .unwrap(),
        ),
    }
}

#[test]
fn exact_readback_requires_a_clean_persisted_response_to_reconcile() {
    let clean = dispatch_response("success", 1, 0);
    let altered = dispatch_response("success", 0, 1);
    for (response, response_state, expected_state) in [
        (
            Some(&clean),
            "response_clean",
            "previous_attempt_reconciled",
        ),
        (
            Some(&altered),
            "response_not_clean",
            "reconciliation_required",
        ),
        (None, "response_missing", "reconciliation_required"),
    ] {
        let mut payload = json!({
            "result": {"counts": {"posted_verified": 1}, "duplicates": []}
        });
        finalize_previous_attempt_reconciliation(&mut payload, response);
        assert_eq!(payload["result"]["dispatch"]["state"], expected_state);
        assert_eq!(
            payload["result"]["dispatch"]["response_state"],
            response_state
        );
        if expected_state == "previous_attempt_reconciled" {
            assert!(payload["result"].get("error").is_none());
        } else {
            assert_eq!(
                payload["result"]["error"]["code"],
                "import_reconciliation_required"
            );
        }
        let markdown = render_proof_markdown(&payload["result"]);
        assert!(markdown.contains(&format!("Dispatch verdict: `{expected_state}`")));
        assert!(markdown.contains(&format!("Response state: `{response_state}`")));
        assert!(markdown.contains("Readback counts: matching 1"));
        assert!(!markdown.contains("Proof-of-Post"));
        assert_eq!(
            markdown.contains("this report does not confirm posting"),
            expected_state == "reconciliation_required"
        );
        assert_eq!(
            markdown.contains("Error: `import_reconciliation_required`"),
            expected_state == "reconciliation_required"
        );
    }
}

#[test]
fn recovery_failure_retains_the_saved_dispatch_response() {
    let response = dispatch_response("success", 1, 0);
    let payload = reconciliation_failure_payload(
        "bridge-test",
        Some(true),
        Some(&response),
        "verification_transport_failed",
    );
    let result = &payload["result"];
    assert_eq!(result["attempt_recorded"], true);
    assert_eq!(
        result["dispatch_response"]["request_sha256"],
        response.request_sha256
    );
    assert_eq!(
        result["dispatch_response"]["response_sha256"],
        response.response_sha256
    );
    assert_eq!(
        result["dispatch_response"]["outcome"]["application_status"],
        "success"
    );
    assert_eq!(
        result["dispatch_response"]["outcome"]["counters"]["created"],
        1
    );
    assert_eq!(result["error"]["code"], "verification_transport_failed");
}

#[test]
fn endpoint_lease_contention_keeps_negative_post_and_cancellation_results_uncertain() {
    let directory = tempfile::tempdir().unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let (mut line, mut endpoint) = batch();
    endpoint.port = listener.local_addr().unwrap().port();
    line.endpoint_origin = Some(super::super::super::canonical_loopback_origin(&endpoint).unwrap());
    let server = Server::new(crate::agent::Settings {
        endpoint,
        data_dir: directory.path().to_path_buf(),
        max_rows: 10,
        max_bytes: 200_000,
        redaction: crate::agent::Redaction::None,
        import_enabled: true,
        writes_enabled: true,
    });
    server.append_import_ledger_while_admitted(&line).unwrap();
    let args = json!({"batch_id":&line.batch_id,"company_guid":&line.company_guid});
    let snapshot = server
        .latest_import_snapshot(&line.batch_id)
        .unwrap()
        .unwrap();
    let snapshot_lease = dispatch_lease::acquire_snapshot(&server.settings.endpoint).unwrap();

    assert_eq!(server.recorded_import_attempt(&args).unwrap(), Some(false));
    assert_eq!(
        server.post_failure_attempt_observation(&line.batch_id, &line.company_guid, None, None),
        None
    );
    let attempted = server.post_failure_attempt_observation(
        &line.batch_id,
        &line.company_guid,
        Some(&snapshot),
        None,
    );
    assert_eq!(attempted, Some(false));
    for code in ["import_admission_busy", "import_approval_declined"] {
        let outcome = post_failure_outcome(
            &line.batch_id,
            &line.company_guid,
            ToolFailure::from(code.to_string()),
            crate::agent::evidence_from_runtime_read(
                crate::tally::runtime::RuntimeReadEvidence::empty(),
            ),
            Some(&snapshot),
            None,
            attempted,
        );
        assert_eq!(outcome.payload["result"]["attempt_recorded"], false);
    }
    let cancellation = server.cancelled_import_for_response(&args).unwrap();
    assert_eq!(cancellation.payload["result"]["attempt_recorded"], false);
    drop(snapshot_lease);

    let writer_lease = dispatch_lease::acquire(&server.settings.endpoint).unwrap();
    assert!(server
        .post_failure_attempt_observation(&line.batch_id, &line.company_guid, Some(&snapshot), None)
        .is_none());
    let cancellation = server.cancelled_import_for_response(&args).unwrap();
    assert!(cancellation.payload["result"]["attempt_recorded"].is_null());
    drop(writer_lease);
    assert_eq!(
        server.post_failure_attempt_observation(
            &line.batch_id,
            &line.company_guid,
            Some(&snapshot),
            None,
        ),
        Some(false)
    );

    let mut wrong_endpoint = snapshot;
    wrong_endpoint.batch.endpoint_origin = Some("http://127.0.0.1:9".into());
    assert_eq!(
        server.post_failure_attempt_observation(
            &line.batch_id,
            &line.company_guid,
            Some(&wrong_endpoint),
            None,
        ),
        None
    );

    let lease = dispatch_lease::acquire(&server.settings.endpoint).unwrap();
    server
        .append_import_record_while_admitted(&ledger::StatusRecord::dispatch(&line))
        .unwrap();
    let intent_snapshot = server
        .latest_import_snapshot(&line.batch_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        server.post_failure_attempt_observation(
            &line.batch_id,
            &line.company_guid,
            Some(&intent_snapshot),
            None,
        ),
        Some(true)
    );
    assert_eq!(
        server.post_failure_attempt_observation(
            &line.batch_id,
            &line.company_guid,
            None,
            Some(&dispatch_response("success", 1, 0)),
        ),
        Some(true)
    );
    drop(lease);
}

#[tokio::test]
async fn received_response_survives_an_injected_response_journal_failure() {
    let directory = tempfile::tempdir().unwrap();
    let (line, endpoint) = batch();
    let server = Server::new(crate::agent::Settings {
        endpoint,
        data_dir: directory.path().to_path_buf(),
        max_rows: 10,
        max_bytes: 200_000,
        redaction: crate::agent::Redaction::None,
        import_enabled: true,
        writes_enabled: true,
    });
    server.append_import_ledger(&line).unwrap();
    let admission = server.lock_import_admission().unwrap();
    server
        .append_import_record_while_admitted(&ledger::StatusRecord::dispatch(&line))
        .unwrap();
    drop(admission);

    let journal = directory.path().join("agent-import-ledger.jsonl");
    std::fs::rename(&journal, directory.path().join("preserved-ledger.jsonl")).unwrap();
    std::fs::create_dir(&journal).unwrap();
    let response = dispatch_response("success", 1, 0);
    let admission = server.lock_import_admission().unwrap();
    assert_eq!(
        server.append_import_record_while_admitted(&ledger::StatusRecord::response(
            &line,
            response.clone(),
        )),
        Err("import_ledger_unavailable".into())
    );
    drop(admission);

    let outcome = post_failure_outcome(
        &line.batch_id,
        &line.company_guid,
        ToolFailure::from("import_ledger_unavailable".to_string()),
        crate::agent::evidence_from_runtime_read(
            crate::tally::runtime::RuntimeReadEvidence::empty(),
        ),
        None,
        Some(&response),
        Some(true),
    );
    let formatted = server.finish_tool_response(
        "post_import",
        &json!({"batch_id":line.batch_id,"company_guid":line.company_guid}),
        chrono::Utc::now(),
        Ok(outcome),
    );
    let mut output = Vec::new();
    std::fs::create_dir(server.settings.data_dir.join("agent-egress.jsonl")).unwrap();
    crate::agent::agent_protocol::finish_response(
        &server,
        &mut output,
        json!(7),
        Ok(formatted.value),
        Some(formatted.egress),
        formatted.recovery_batch_id,
        true,
    )
    .await
    .unwrap();
    let framed: Value = serde_json::from_slice(&output).unwrap();
    assert!(output.len() <= server.settings.max_bytes);
    assert_eq!(framed["error"]["message"], "egress_record_write_failed");
    let payload = &framed["error"]["data"];
    assert_eq!(payload["batch_id"], line.batch_id);
    assert_eq!(payload["attempt_recorded"], true);
    assert_eq!(payload["dispatch"]["state"], "reconciliation_required");
    assert_eq!(payload["dispatch"]["resent"], false);
    assert_eq!(
        payload["dispatch_response"]["request_sha256"],
        response.request_sha256
    );
    assert_eq!(
        payload["dispatch_response"]["response_sha256"],
        response.response_sha256
    );
    assert_eq!(
        payload["dispatch_response"]["outcome"]["application_status"],
        "success"
    );
    assert_eq!(
        payload["dispatch_response"]["outcome"]["counters"]["created"],
        1
    );
}

#[test]
fn current_dispatch_finalizer_marks_only_a_clean_response_posted() {
    let response = dispatch_response("success", 1, 0);
    let mut payload = json!({
        "result": {"counts": {"posted_verified": 1}, "duplicates": []}
    });
    finalize_current_dispatch(&mut payload, Some(&response));
    assert_eq!(payload["result"]["dispatch"]["state"], "posted_verified");
    assert_eq!(
        payload["result"]["dispatch"]["response_state"],
        "response_clean"
    );
    assert_eq!(payload["result"]["dispatch"]["counters"]["created"], 1);
    assert!(payload["result"].get("error").is_none());
}

#[test]
fn missing_counter_evidence_cannot_confirm_current_or_previous_dispatch() {
    // Mutate only the presence marker in saved response evidence. This tests
    // classification, not a newly claimed live response profile.
    for missing in [
        "legacy",
        "created",
        "altered",
        "deleted",
        "ignored",
        "errors",
        "cancelled",
        "exceptions",
        "exception_marker",
    ] {
        let mut saved = serde_json::to_value(dispatch_response("success", 1, 0)).unwrap();
        match missing {
            "legacy" => {
                saved["outcome"]["counters"]
                    .as_object_mut()
                    .unwrap()
                    .remove("counter_presence");
            }
            "exception_marker" => saved["outcome"]["exceptions_were_reported"] = json!(false),
            counter => saved["outcome"]["counters"]["counter_presence"][counter] = json!(false),
        }
        let response: ledger::DispatchResponse = serde_json::from_value(saved).unwrap();
        for finalize in [
            finalize_current_dispatch,
            finalize_previous_attempt_reconciliation,
        ] {
            let mut payload = json!({"result":{"counts":{"posted_verified":1},"duplicates":[]}});
            finalize(&mut payload, Some(&response));
            assert_eq!(
                payload["result"]["dispatch"]["state"],
                "reconciliation_required"
            );
            assert_eq!(
                payload["result"]["dispatch"]["response_state"],
                "response_not_clean"
            );
            assert_eq!(
                payload["result"]["error"]["code"],
                "import_reconciliation_required"
            );
            assert!(render_proof_markdown(&payload["result"]).contains("does not confirm posting"));
        }
    }
}

#[test]
fn post_date_refusal_retains_the_completed_profile_probe_evidence() {
    let (mut line, _) = batch();
    line.vouchers[0].date = "20260907".into();
    let payload = ImportPayload {
        company_guid: line.company_guid.clone(),
        vouchers: line.vouchers,
        amends_batch_id: None,
    };
    let profile_evidence = Evidence {
        request_sha256: "profile-request".into(),
        response_sha256: "profile-response".into(),
        bytes: 42,
        state: "complete",
        read_at: None,
        duration_ms: None,
        reason_code: None,
    };
    let profile = ImportProfileObservation {
        qualification: Ok(()),
        evidence: profile_evidence.clone(),
        observed_profile: json!({}),
        admission_key: ("tallyprime".into(), "education".into()),
    };
    let mut accumulated = Evidence {
        request_sha256: String::new(),
        response_sha256: String::new(),
        bytes: 0,
        state: "complete",
        read_at: None,
        duration_ms: None,
        reason_code: None,
    };
    let expected = combine_evidence(accumulated.clone(), profile_evidence);
    assert_eq!(
        validate_post_profile_with_evidence(&payload, &profile, &mut accumulated).unwrap_err(),
        "education_voucher_date_unsupported"
    );
    assert_eq!(accumulated.request_sha256, expected.request_sha256);
    assert_eq!(accumulated.response_sha256, expected.response_sha256);
    assert_eq!(accumulated.bytes, expected.bytes);
}

#[test]
fn absence_is_required_before_a_first_attempt() {
    for payload in [
        json!({}),
        json!({"counts":{"posted_verified":1},"vouchers":[{}]}),
        json!({"counts":{"not_found":1},"vouchers":[]}),
        json!({"result":{"counts":{"not_found":1},"vouchers":[{}]}}),
    ] {
        assert_eq!(
            require_absent_verification_result(&payload).unwrap_err(),
            "import_preexisting_identity"
        );
    }
    assert!(
        require_absent_verification_result(&json!({"counts":{"not_found":1},"vouchers":[{}]}))
            .is_ok()
    );
}

#[test]
fn native_request_uses_a_private_remote_identity_but_preserves_batch_attribution() {
    let (line, _) = batch();
    let voucher = &line.vouchers[0];
    let public = render_import_xml("Synthetic Accounts", &line.vouchers, &line.batch_id);
    let first = render_native_voucher_xml(
        "Synthetic Accounts",
        voucher,
        &line.batch_id,
        Uuid::new_v4(),
    );
    let second = render_native_voucher_xml(
        "Synthetic Accounts",
        voucher,
        &line.batch_id,
        Uuid::new_v4(),
    );
    let remote_id = |xml: &str| {
        let mut reader = quick_xml::Reader::from_str(xml);
        loop {
            match reader.read_event().unwrap() {
                quick_xml::events::Event::Start(tag) if tag.name().as_ref() == b"VOUCHER" => {
                    let attribute = tag
                        .attributes()
                        .map(Result::unwrap)
                        .find(|attribute| attribute.key.as_ref() == b"REMOTEID")
                        .unwrap();
                    break Uuid::parse_str(std::str::from_utf8(&attribute.value).unwrap()).unwrap();
                }
                quick_xml::events::Event::Eof => panic!("voucher missing"),
                _ => {}
            }
        }
    };
    let public_id = remote_id(&public);
    let first_id = remote_id(&first);
    let second_id = remote_id(&second);
    assert_ne!(first_id, public_id);
    assert_ne!(second_id, public_id);
    assert_ne!(first_id, second_id);
    // Only the client mutation selector changes; accounting, company and stable
    // readback attribution remain byte-for-byte the reviewed public document.
    assert_eq!(
        first.replace(
            &format!("REMOTEID=\"{first_id}\""),
            &format!("REMOTEID=\"{public_id}\"")
        ),
        public
    );
    assert_eq!(
        second.replace(
            &format!("REMOTEID=\"{second_id}\""),
            &format!("REMOTEID=\"{public_id}\"")
        ),
        public
    );
    assert_eq!(sha256_hex(public.as_bytes()), line.sha256);
}

#[test]
fn native_post_refuses_supplied_numbers_without_disabling_manual_files() {
    let (mut line, endpoint) = batch();
    assert!(require_native_numbering(&line.vouchers[0]).is_ok());
    line.vouchers[0].voucher_number = Some("MANUAL-1".into());
    line.sha256 = sha256_hex(
        render_import_xml("Synthetic Accounts", &line.vouchers, &line.batch_id).as_bytes(),
    );
    assert_eq!(
        admit_saved_journal(&line, &endpoint).err().as_deref(),
        Some("import_post_numbered_journal_unsupported")
    );
}

#[test]
fn queued_absence_recheck_distinguishes_an_attributed_journal_from_a_new_candidate() {
    let company_guid = "61c6de69-1748-461c-ad3f-162cb949df9f";
    let bytes = include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-namespaced-journal.utf16le.xml"
    );
    let captured = String::from_utf16(
        &bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let line: ImportLedgerLine = serde_json::from_value(json!({
        "batch_id":"bridge-6c79872c-aab6-4be5-a181-18182c8148be",
        "identity_scheme":"batch_v1", "company_guid":company_guid,
        "txn_ids":["BRIDGE_MCP_LIVE_20260906_A1"],
        "date_from":"20260907", "date_to":"20260907", "sha256":"e39eb3c0bfe53144bdd9c0f4afcb88c3d63a2050214233ee77465d42a54245ef",
        "built_at":"2026-09-06T21:40:26.641Z", "status":"built",
        "pre_import_mark":{"kind":"company_high_water","value":8,"master_value":219},
        "vouchers":[{"bridge_txn_id":"BRIDGE_MCP_LIVE_20260906_A1","date":"20260907",
            "voucher_type":"Journal","narration":"Bridge MCP batch namespace qualification",
            "reference":null,"voucher_number":null,
            "entries":[{"ledger":"Bridge Nested Debtor WR4","amount":"12.61","side":"Dr"},
                {"ledger":"Cash","amount":"12.61","side":"Cr"}]}]
    }))
    .unwrap();
    let catalogue_bytes = include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-ledger-catalogue.utf16le.xml"
    );
    let catalogue = String::from_utf16(
        &catalogue_bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let single_currency = captured_currencies(include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/currency_inr_modern_live.utf16le.xml"
    ));
    let ledger_binding = bridge_tally_protocol::parse_standard_ledger_catalog_with_identities(
        &catalogue,
        "WR2 Unicode Lab",
        company_guid,
    )
    .unwrap()
    .bind_selected(requested_ledger_names(&ImportPayload {
        company_guid: company_guid.into(),
        vouchers: line.vouchers.clone(),
        amends_batch_id: None,
    }))
    .unwrap();
    let error = recheck_import_admission(
        &line,
        company_guid,
        "WR2 Unicode Lab",
        &captured,
        &captured,
        &catalogue,
        None,
        &single_currency,
        &ledger_binding,
    )
    .expect_err("captured attributed Journal must block the queued native attempt");
    assert!(matches!(
        error.downcast_ref::<ApprovedImportAdmissionError>(),
        Some(ApprovedImportAdmissionError::PreexistingIdentity)
    ));

    // Change the expected local batch, not the captured Tally source. Neither
    // attribution nor accounting content now matches the captured Journal.
    let mut absent = line;
    absent.batch_id = "bridge-00000000-0000-4000-8000-000000000003".into();
    for entry in &mut absent.vouchers[0].entries {
        entry.amount = "12.62".into();
    }
    recheck_import_admission(
        &absent,
        company_guid,
        "WR2 Unicode Lab",
        &captured,
        &captured,
        &catalogue,
        None,
        &single_currency,
        &ledger_binding,
    )
    .expect("paired captured source establishes absence of the new candidate");

    // A bank voucher is classified from the group collection read beside the
    // catalogue. Without that read the queue refuses rather than post on half
    // a check, and a Journal that somehow carries one is a wiring fault too.
    let groups_bytes = include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-party-groups.utf16le.xml"
    );
    let groups = String::from_utf16(
        &groups_bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let mut payment = absent.clone();
    payment.vouchers[0].voucher_type = VoucherType::Payment;
    payment.vouchers[0].reference = None;
    let recheck = |line: &ImportLedgerLine, groups: Option<&str>| {
        recheck_import_admission(
            line,
            company_guid,
            "WR2 Unicode Lab",
            &captured,
            &captured,
            &catalogue,
            groups,
            &single_currency,
            &ledger_binding,
        )
    };
    recheck(&payment, Some(&groups)).expect("the captured masters classify this Payment");
    for (line, groups) in [(&payment, None), (&absent, Some(groups.as_str()))] {
        let error = recheck(line, groups).expect_err("a missing or stray group read must refuse");
        assert!(matches!(
            error.downcast_ref::<ApprovedImportAdmissionError>(),
            Some(ApprovedImportAdmissionError::AdmissionInconsistent)
        ));
    }
}

#[test]
fn the_whole_window_pre_post_request_is_admitted_on_the_verification_measurement() {
    // Review of #520. The request sent whole inside the dispatch lease is
    // admitted on what verify_import's read of the same window measured.
    let evidence = |bytes: usize| Evidence {
        request_sha256: String::new(),
        response_sha256: String::new(),
        bytes,
        state: "complete",
        read_at: None,
        duration_ms: None,
        reason_code: None,
    };
    let divided = [
        crate::agent::WindowPart {
            from: "20260801".into(),
            to: "20260815".into(),
            span: None,
        },
        crate::agent::WindowPart {
            from: "20260816".into(),
            to: "20260831".into(),
            span: None,
        },
    ];
    let budget = usize::try_from(crate::agent::WINDOW_READ_BUDGET_BYTES).unwrap();
    let light = crate::agent::WindowServed::of(&divided, &evidence(2 * budget), false);
    let heavy = crate::agent::WindowServed::of(&divided, &evidence(2 * budget + 2), false);
    assert_eq!(admit_post_window(Some(light)), Ok(()));
    assert_eq!(
        admit_post_window(Some(heavy)),
        Err(IMPORT_POST_WINDOW_NOT_BOUNDED.to_string())
    );
    // A verification that reported nothing is refused, not assumed small.
    assert_eq!(
        admit_post_window(None),
        Err(IMPORT_POST_WINDOW_NOT_BOUNDED.to_string())
    );
}

/// bridge#575. The post path used to compare a journal record only with
/// itself; the saved XML file Bridge built was never read. A record and file
/// that disagree must stop the post before anything is sent to Tally.
#[tokio::test]
async fn a_saved_xml_file_that_differs_from_the_record_is_refused_before_tally() {
    let directory = tempfile::tempdir().unwrap();
    let (mut line, mut endpoint) = batch();
    // Port 9 is not a Tally endpoint: a regression that reaches the network
    // fails there with a transport error, never with the codes below.
    endpoint.port = 9;
    line.endpoint_origin = Some(super::super::super::canonical_loopback_origin(&endpoint).unwrap());
    let server = Server::new(crate::agent::Settings {
        endpoint,
        data_dir: directory.path().to_path_buf(),
        max_rows: 10,
        max_bytes: 200_000,
        redaction: crate::agent::Redaction::None,
        import_enabled: true,
        writes_enabled: true,
    });
    server.append_import_ledger(&line).unwrap();
    let path = server
        .imports_dir()
        .unwrap()
        .join(format!("{}.xml", line.batch_id));
    let args = json!({"company_guid":line.company_guid,"batch_id":line.batch_id});

    // The record is self-consistent; the file holds a different Journal.
    let mut other = line.vouchers.clone();
    other[0].entries[0].amount = "99.00".into();
    other[0].entries[1].amount = "99.00".into();
    fs::write(
        &path,
        render_import_xml("Synthetic Accounts", &other, &line.batch_id),
    )
    .unwrap();
    assert!(admit_saved_journal_integrity(&line, &server.settings.endpoint).is_ok());
    assert_eq!(post_error(&server, &args).await, "import_batch_changed");

    fs::remove_file(&path).unwrap();
    assert_eq!(
        post_error(&server, &args).await,
        "import_persisted_file_unavailable"
    );

    // The exact file passes this check; the post then fails later, at the
    // network, because port 9 is not Tally.
    fs::write(
        &path,
        render_import_xml("Synthetic Accounts", &line.vouchers, &line.batch_id),
    )
    .unwrap();
    let later = post_error(&server, &args).await;
    assert!(
        !matches!(
            later.as_str(),
            "import_batch_changed" | "import_persisted_file_unavailable"
        ),
        "{later}"
    );
}

/// The error code a post reports, whether it failed before or inside its
/// operation (the latter comes back as an outcome whose result names the
/// error), and that no attempt was recorded.
async fn post_error(server: &Server, args: &Value) -> String {
    match server.post_import(args).await {
        Ok(outcome) => {
            let result = &outcome.payload["result"];
            assert_ne!(result["attempt_recorded"], json!(true), "{result}");
            result["error"]["code"]
                .as_str()
                .unwrap_or("unexpected_success")
                .to_string()
        }
        Err(failure) => failure.code,
    }
}

/// bridge#579. Tally deletes a voucher only by the client REMOTEID it was
/// created with and exports its own GUID in that attribute, so the native
/// post must record the REMOTEID it sends, with its request hash, before
/// sending. The recorded value must be exactly the one in the request bytes.
#[test]
fn the_dispatch_intent_records_the_remoteid_the_native_request_carries() {
    let (line, _) = batch();
    let remote_id = Uuid::new_v4();
    let request = native_post_request(&line, remote_id).unwrap();
    assert_eq!(request.remote_id, remote_id);
    let sent = request
        .xml
        .split("<VOUCHER REMOTEID=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap();
    assert_eq!(request.xml.matches("REMOTEID=").count(), 1);
    assert_eq!(sent, remote_id.hyphenated().to_string());
    assert_eq!(
        request.request_sha256,
        sha256_hex(&bridge_tally_protocol::encode_tally_xml_request_utf16le(
            &request.xml
        ))
    );

    let intent = serde_json::to_value(ledger::StatusRecord::dispatch_for(&line, &request)).unwrap();
    assert_eq!(intent["native_remote_id"], json!(sent));
    assert_eq!(
        intent["native_request_sha256"],
        json!(request.request_sha256)
    );

    // Two posts never share a REMOTEID: reuse could make Tally upsert.
    let other = native_post_request(&line, Uuid::new_v4()).unwrap();
    assert_ne!(other.remote_id, request.remote_id);
    assert_ne!(other.request_sha256, request.request_sha256);
}

/// A Payment with one bank credit and `parties` debits named by `name`.
fn payment_with(parties: usize, name: impl Fn(usize) -> String) -> ImportLedgerLine {
    let (mut line, _) = batch();
    let voucher = &mut line.vouchers[0];
    voucher.voucher_type = VoucherType::Payment;
    voucher.reference = None;
    voucher.entries = (0..parties)
        .map(|index| ImportEntry {
            ledger: name(index),
            amount: "1.00".into(),
            side: EntrySide::Dr,
        })
        .chain(std::iter::once(ImportEntry {
            ledger: "Cash".into(),
            amount: format!("{parties}.00"),
            side: EntrySide::Cr,
        }))
        .collect();
    line
}

/// The native dialog cannot scroll, so a preview past its caps is refused,
/// never truncated. Multi-entry bank vouchers reach the caps: seventeen fixed
/// lines plus one per entry, so seven entries fit 24 lines and eight do not.
#[test]
fn a_bank_preview_is_refused_at_each_cap_rather_than_truncated() {
    let (_, endpoint) = batch();
    let seven = admit_fresh_saved_voucher(&payment_with(6, |i| format!("Party {i}")), &endpoint)
        .expect("seven entries fit");
    assert_eq!(seven.lines().count(), 24, "{seven}");
    assert_eq!(
        admit_fresh_saved_voucher(&payment_with(7, |i| format!("Party {i}")), &endpoint)
            .unwrap_err(),
        "import_review_too_large"
    );
    // One entry line is `Dr 1.00  "<name>"`: 11 characters around the name.
    let fits = admit_fresh_saved_voucher(&payment_with(1, |_| "N".repeat(89)), &endpoint)
        .expect("a 100-character line fits");
    assert!(
        fits.lines().any(|line| line.chars().count() == 100),
        "{fits}"
    );
    assert_eq!(
        admit_fresh_saved_voucher(&payment_with(1, |_| "N".repeat(90)), &endpoint).unwrap_err(),
        "import_review_too_large"
    );
}

fn captured_currencies(bytes: &[u8]) -> String {
    String::from_utf16(
        &bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>(),
    )
    .unwrap()
}

#[test]
fn a_post_is_admitted_only_into_a_book_with_one_currency_master() {
    // Both single-master spellings measured on 7.1 are admitted, whatever the
    // base is called: the gate asks how many masters there are, not which.
    for single in [
        &include_bytes!(
            "../crates/bridge-tally-protocol/tests/fixtures/currency_inr_modern_live.utf16le.xml"
        )[..],
        &include_bytes!(
            "../crates/bridge-tally-protocol/tests/fixtures/currency_inr_legacy_live.utf16le.xml"
        )[..],
    ] {
        admit_post_currency(&captured_currencies(single)).expect("one master admits");
    }
    // The captured two-master book refuses, naming both masters as read.
    let multi = captured_currencies(include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/currency_multi_live.utf16le.xml"
    ));
    let names = bridge_tally_protocol::native_outstandings::parse_company_currency(&multi)
        .unwrap()
        .names;
    assert_eq!(names.len(), 2, "{names:?}");
    assert_eq!(
        admit_post_currency(&multi),
        Err(ApprovedImportAdmissionError::MultiCurrencyBook { currencies: names })
    );
    // Kept out of every serialized output that carries the struct.
    let serialized = serde_json::to_value(
        bridge_tally_protocol::native_outstandings::parse_company_currency(&multi).unwrap(),
    )
    .unwrap();
    assert!(serialized.get("names").is_none(), "{serialized}");
    // No base can be named from a response that does not parse, from one with
    // no master, or from one whose only master has no NAME (which the parser
    // itself refuses). The last two are explicit edits of the one-master
    // capture, not live evidence.
    let single = captured_currencies(include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/currency_inr_modern_live.utf16le.xml"
    ));
    // The row's own close: CMPINFO's `<CURRENCY>0</CURRENCY>` comes earlier.
    let row_start = single.find("<CURRENCY NAME=").unwrap();
    let row_end =
        row_start + single[row_start..].find("</CURRENCY>").unwrap() + "</CURRENCY>".len();
    let no_master = format!("{}{}", &single[..row_start], &single[row_end..]);
    assert_eq!(
        bridge_tally_protocol::native_outstandings::parse_company_currency(&no_master)
            .unwrap()
            .currency_count,
        0,
        "the edit must leave a readable collection with no master"
    );
    assert_eq!(single.matches(" NAME=\"I₹\"").count(), 1);
    let nameless = single.replace(" NAME=\"I₹\"", " NAME=\"\"");
    assert!(bridge_tally_protocol::native_outstandings::parse_company_currency(&nameless).is_err());
    for undetermined in ["<ENVELOPE/>", no_master.as_str(), nameless.as_str()] {
        assert_eq!(
            admit_post_currency(undetermined),
            Err(ApprovedImportAdmissionError::BaseCurrencyUndetermined),
            "{undetermined}"
        );
    }
}

#[test]
fn a_multi_currency_refusal_names_the_masters_in_plain_words_only_when_nothing_was_attempted() {
    let currencies = (1..=10).map(|n| format!("C{n}")).collect::<Vec<_>>();
    let refused = |attempted: Value| {
        let mut payload = json!({"result":{"attempt_recorded":attempted,
            "error":{"code":"import_multi_currency_unsupported","message":"generic"}}});
        name_refused_currencies(&mut payload, &currencies);
        payload["result"]["error"].clone()
    };
    let error = refused(json!(false));
    assert_eq!(
        error["message"],
        "This company has more than one currency defined (C1, C2, C3, C4, C5, C6, C7, C8 and 2 \
         more); Bridge does not post into multi-currency books yet. Nothing was posted."
    );
    assert_eq!(error["currencies_seen"].as_array().unwrap().len(), 8);
    assert_eq!(error["currencies_total"], 10);
    // An unknown attempt keeps its instruction to reconcile.
    for attempted in [json!(true), Value::Null] {
        assert_eq!(refused(attempted)["message"], "generic");
    }
}

/// The captured catalogue's binding of `names`, as a post binds them.
fn captured_binding(names: &[&str]) -> bridge_tally_protocol::StandardLedgerCatalogBinding {
    let catalogue = captured_currencies(include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-ledger-catalogue.utf16le.xml"
    ));
    bridge_tally_protocol::parse_standard_ledger_catalog_with_identities(
        &catalogue,
        "WR2 Unicode Lab",
        "61c6de69-1748-461c-ad3f-162cb949df9f",
    )
    .unwrap()
    .bind_selected(names.iter().map(|name| name.to_string()))
    .unwrap()
}

fn recorded(pairs: &[(&str, &str)]) -> Vec<BoundLedger> {
    pairs
        .iter()
        .map(|(name, guid)| BoundLedger {
            name: name.to_string(),
            guid: guid.to_string(),
        })
        .collect()
}

#[test]
fn a_post_admits_only_the_ledgers_its_build_bound() {
    // The captured catalogue's GUIDs for these two ledgers.
    const A: &str = "61c6de69-1748-461c-ad3f-162cb949df9f-000000d5";
    const B: &str = "61c6de69-1748-461c-ad3f-162cb949df9f-0000001f";
    let now = captured_binding(&["Bridge Nested Debtor WR4", "Cash"]);
    // Same ledgers, same GUIDs (GUID case is not identity).
    assert_eq!(
        admit_build_binding(
            Some(&recorded(&[
                ("Cash", &B.to_uppercase()),
                ("Bridge Nested Debtor WR4", A)
            ])),
            &now
        ),
        Ok(())
    );
    // Cash renamed and a new Cash created since the build: named.
    assert_eq!(
        admit_build_binding(
            Some(&recorded(&[
                ("Bridge Nested Debtor WR4", A),
                ("Cash", "61c6de69-1748-461c-ad3f-162cb949df9f-000000ff")
            ])),
            &now
        ),
        Err(BuildBindingRefusal::Changed(vec!["Cash".into()]))
    );
    // A ledger the record does not hold is not admitted by name alone.
    assert_eq!(
        admit_build_binding(Some(&recorded(&[("Bridge Nested Debtor WR4", A)])), &now),
        Err(BuildBindingRefusal::Changed(vec!["Cash".into()]))
    );
    // A record built before binding existed has nothing to compare.
    assert_eq!(
        admit_build_binding(None, &now),
        Err(BuildBindingRefusal::Unbound)
    );
}

#[test]
fn a_record_without_ledger_identities_reads_as_built_before_binding() {
    // An older record, exactly as it was serialized: no `ledger_identities`.
    let (mut line, _) = batch();
    line.ledger_identities = None;
    let json = serde_json::to_value(&line).unwrap();
    assert!(json.get("ledger_identities").is_none(), "{json}");
    let reread: ImportLedgerLine = serde_json::from_value(json).unwrap();
    assert_eq!(reread.ledger_identities, None);
    // And a current record keeps them across a round trip.
    line.ledger_identities = Some(recorded(&[("Cash", "g")]));
    let reread: ImportLedgerLine =
        serde_json::from_value(serde_json::to_value(&line).unwrap()).unwrap();
    assert_eq!(reread.ledger_identities, line.ledger_identities);
}

#[test]
fn a_changed_ledger_is_named_in_plain_words_only_when_nothing_was_attempted() {
    let ledgers = (1..=9).map(|n| format!("L{n}")).collect::<Vec<_>>();
    let refused = |attempted: Value| {
        let mut payload = json!({"result":{"attempt_recorded":attempted,
            "error":{"code":"import_masters_changed_since_build","message":"generic"}}});
        name_changed_ledgers(&mut payload, &ledgers);
        payload["result"]["error"].clone()
    };
    let error = refused(json!(false));
    let message = error["message"].as_str().unwrap();
    assert!(
        message.starts_with(
            "A ledger this batch names is no longer the one it was built against (L1, L2, L3, L4, L5, L6, L7, L8 and 1 more)"
        ),
        "{message}"
    );
    assert_eq!(error["ledgers_changed_total"], 9);
    for attempted in [json!(true), Value::Null] {
        assert_eq!(refused(attempted)["message"], "generic");
    }
    // An unbound batch is told to rebuild, only when nothing was attempted.
    let mut unbound = json!({"result":{"attempt_recorded":false,
        "error":{"code":"import_batch_predates_ledger_binding","message":"generic"}}});
    explain_unbound_batch(&mut unbound);
    assert!(unbound["result"]["error"]["message"]
        .as_str()
        .unwrap()
        .contains("Build the batch again"));
}
