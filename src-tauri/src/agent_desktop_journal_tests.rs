use super::desktop_journal::{DesktopJournalOperation, DesktopJournalService};
use super::*;
use bridge_tally_transport::TallyEndpointConfig;

#[test]
fn action_ipc_keeps_recovery_state_without_unbounded_voucher_details() {
    let large = "x".repeat(5_000_001);
    for state in [
        "posted_verified",
        "previous_attempt_reconciled",
        "reconciliation_required",
    ] {
        let mut payload = json!({"result":{
            "dispatch":{"state":state,"resent":false,"response":{"unused":large}},
            "attempt_recorded":true,
            "unrelated_duplicates_in_window":[large],
        }});
        if state == "reconciliation_required" {
            payload["result"]["error"] = json!({
                "code":"import_reconciliation_required",
                "message":"Reconcile the original batch without resending it.",
                "remediation":large,
            });
        }
        let operation = DesktopJournalOperation::from_outcome(ToolOutcome {
            payload,
            evidence: Evidence {
                request_sha256: "a".repeat(64),
                response_sha256: "b".repeat(64),
                bytes: large.len(),
                state: "complete",
                read_at: None,
                duration_ms: None,
                reason_code: None,
            },
            company_guid: None,
            truncated: false,
        });
        let result = &operation.result["result"];
        assert_eq!(result["dispatch"]["state"], state);
        assert_eq!(result["dispatch"]["resent"], false);
        assert_eq!(result["attempt_recorded"], true);
        assert!(result.get("unrelated_duplicates_in_window").is_none());
        assert!(result["dispatch"].get("response").is_none());
        if state == "reconciliation_required" {
            assert_eq!(result["error"]["code"], "import_reconciliation_required");
            assert_eq!(
                result["error"]["message"],
                "Reconcile the original batch without resending it."
            );
            assert!(result["error"]["remediation"].is_null());
        } else {
            assert!(result["error"].is_null());
        }
        assert!(serde_json::to_vec(&operation.result).unwrap().len() < 1024);
    }
}

fn service(root: PathBuf) -> (DesktopJournalService, ImportLedgerLine) {
    let endpoint = TallyEndpointConfig {
        host: "127.0.0.1".into(),
        port: 9001,
    };
    let mut line: ImportLedgerLine = serde_json::from_value(json!({
        "batch_id":"bridge-00000000-0000-4000-8000-000000000001", "identity_scheme":"batch_v1", "company_guid":"00000000-0000-4000-8000-000000000002", "endpoint_origin":super::super::canonical_loopback_origin(&endpoint).unwrap(),
        "company":{"name":"Synthetic Accounts","guid":"00000000-0000-4000-8000-000000000002","company_number":"100001","books_from":"20260401"}, "txn_ids":["journal-test"],"date_from":"20260901","date_to":"20260901","sha256":"","built_at":"2026-09-07T00:00:00Z","status":"built","pre_import_mark":{"kind":"company_high_water","value":1,"master_value":1},
        "vouchers":[{"bridge_txn_id":"journal-test","date":"20260901","voucher_type":"Journal","entries":[{"ledger":"Expense","amount":"12.50","side":"Dr"},{"ledger":"Cash","amount":"12.50","side":"Cr"}]}]
    })).unwrap();
    line.sha256 = sha256_hex(
        render_import_xml("Synthetic Accounts", &line.vouchers, &line.batch_id).as_bytes(),
    );
    super::super::ensure_private_directory(&root).unwrap();
    let server = Server::new(super::super::Settings {
        endpoint,
        data_dir: root,
        max_rows: 500,
        max_bytes: 5_000_000,
        redaction: super::super::Redaction::None,
        import_enabled: true,
        writes_enabled: true,
    });
    server.append_import_ledger(&line).unwrap();
    (DesktopJournalService { server }, line)
}

#[tokio::test]
async fn descriptor_company_mismatch_is_refused_before_any_tally_work() {
    let directory = tempfile::tempdir().unwrap();
    let (service, line) = service(directory.path().join("agent"));
    let operation = service
        .post(&line.batch_id, &line.sha256, "other-company")
        .await
        .unwrap();
    assert_eq!(
        operation.result["result"]["error"]["code"],
        "import_batch_company_mismatch"
    );
}

#[tokio::test]
async fn reconcile_without_durable_intent_never_enters_post_or_approval() {
    let directory = tempfile::tempdir().unwrap();
    let (service, line) = service(directory.path().join("agent"));
    let operation = service
        .reconcile(&line.batch_id, &line.sha256, &line.company_guid)
        .await
        .unwrap();
    assert_eq!(
        operation.result["result"]["error"]["code"],
        "import_not_dispatched"
    );
    assert_eq!(operation.result["result"]["attempt_recorded"], false);
}

#[test]
fn review_details_come_from_the_admitted_saved_journal() {
    let directory = tempfile::tempdir().unwrap();
    let (service, line) = service(directory.path().join("agent"));
    let xml = render_import_xml("Synthetic Accounts", &line.vouchers, &line.batch_id);
    std::fs::write(
        service
            .server
            .imports_dir()
            .unwrap()
            .join(format!("{}.xml", line.batch_id)),
        &xml,
    )
    .unwrap();

    let review = service.review_selected_xml(xml.as_bytes()).unwrap();
    assert_eq!(review.details.date, "20260901");
    assert_eq!(review.details.total_debit, "12.5");
    assert_eq!(review.details.total_credit, "12.5");
    assert_eq!(
        review
            .details
            .entries
            .iter()
            .map(|entry| (&entry.ledger, &entry.side, &entry.amount))
            .collect::<Vec<_>>(),
        vec![
            (
                &"Expense".to_string(),
                &"Dr".to_string(),
                &"12.50".to_string()
            ),
            (&"Cash".to_string(), &"Cr".to_string(), &"12.50".to_string())
        ]
    );
}
