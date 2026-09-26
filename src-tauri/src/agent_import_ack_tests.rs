//! `acknowledge_post_review` (#239): a person records that they reviewed a
//! post whose masters check found a changed ledger. Every test drives the
//! tool call, against the captured readback the post tests use.
use super::*;

const DOUBT: &str =
    r#"{"state":"posted_under_changed_masters","trigger":"masters_moved","ledgers":["Cash"]}"#;
const ACK: &str = "acknowledge_post_review";

/// A dispatched batch whose saved response is `response`, with the masters
/// records `check` and `doubt` written as given.
fn seeded(
    simulator: &SequenceSimulator,
    directory: &std::path::Path,
    response: ledger::DispatchResponse,
    check: Option<&[u8]>,
    doubt: Option<&[u8]>,
) -> (Server, Value) {
    let server = server_at(simulator.address(), directory);
    let line = saved_captured_line(&server);
    let native = native_post_request(&line, RemoteIds::from_ids(vec![Uuid::new_v4()])).unwrap();
    {
        let _lock = server.lock_import_admission().unwrap();
        server
            .append_import_record_while_admitted(&ledger::StatusRecord::dispatch_for(
                &line, &native,
            ))
            .unwrap();
        server
            .append_import_record_while_admitted(&ledger::StatusRecord::response(
                &line,
                ledger::DispatchResponse {
                    request_sha256: native.request_sha256.clone(),
                    ..response
                },
            ))
            .unwrap();
    }
    let imports = server.imports_dir().unwrap();
    if let Some(check) = check {
        fs::write(imports.join(format!("{BATCH}.masters_check.json")), check).unwrap();
    }
    if let Some(doubt) = doubt {
        fs::write(imports.join(format!("{BATCH}.masters_doubt.json")), doubt).unwrap();
    }
    let args = json!({"company_guid":GUID,"batch_id":line.batch_id});
    (server, args)
}

fn clean() -> ledger::DispatchResponse {
    super::super::tests::dispatch_response("success", 1, 0)
}

fn ack_path(server: &Server) -> std::path::PathBuf {
    server
        .imports_dir()
        .unwrap()
        .join(format!("{BATCH}.masters_ack.json"))
}

fn doubted(simulator: &SequenceSimulator, directory: &std::path::Path) -> (Server, Value) {
    seeded(
        simulator,
        directory,
        clean(),
        Some(DOUBT.as_bytes()),
        Some(DOUBT.as_bytes()),
    )
}

/// The captured readback with the voucher's ALTERID replaced.
fn readback_at_alter_id(alter_id: u64) -> Vec<ScenarioPlan> {
    readback_of(replaced_once(
        &captured_posted_journal(),
        "<ALTERID TYPE=\"Number\"> 10</ALTERID>",
        &format!("<ALTERID TYPE=\"Number\"> {alter_id}</ALTERID>"),
    ))
}

/// The captured readback with the voucher's narration changed and its
/// ALTERID left as it was, as an edit that did not move it would read.
fn readback_with_edited_narration() -> Vec<ScenarioPlan> {
    readback_of(replaced_once(
        &captured_posted_journal(),
        "Bridge MCP batch namespace qualification [BRIDGE:",
        "Bridge MCP batch namespace qualification edited [BRIDGE:",
    ))
}

fn readback_of(journal: String) -> Vec<ScenarioPlan> {
    let mut plans = probe();
    plans.extend(verified_company());
    plans.extend(paired(marks()));
    plans.extend(paired(journal.clone()));
    plans.extend(paired(journal));
    plans
}

async fn acknowledge(server: &Server, args: Value, scripted: ScriptedApproval) -> Value {
    SCRIPTED_APPROVAL
        .scope(scripted, server.call_tool(ACK, args))
        .await
}

#[tokio::test]
async fn an_approved_review_is_recorded_once_and_changes_no_verdict() {
    let mut plans = reconcile_readback();
    plans.extend(reconcile_readback());
    plans.extend(reconcile_readback());
    let scripted_plans = plans.len();
    let simulator = SequenceSimulator::spawn(with_sentinel(plans)).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let (server, args) = doubted(&simulator, directory.path());
    let approval = ScriptedApproval::approving();

    let response = acknowledge(&server, args.clone(), approval.clone()).await;
    let result = &response["structuredContent"]["result"];
    assert_eq!(result["operator_review"]["state"], "current", "{response}");
    assert_eq!(approval.reviews().len(), 1, "{response}");
    assert!(approval.previews().is_empty(), "no post dialog: {response}");
    let review = &approval.reviews()[0];
    assert!(review.contains("Cash"), "the doubt is shown: {review}");
    assert!(
        review.contains("ALTERID: 10"),
        "the voucher as read: {review}"
    );
    let record: Value = serde_json::from_slice(&fs::read(ack_path(&server)).unwrap()).unwrap();
    assert_eq!(record["batch_id"], BATCH);
    assert_eq!(record["alter_id"], 10);
    assert_eq!(
        record["doubt_sha256"],
        crate::agent::sha256_hex(DOUBT.as_bytes())
    );

    // A later readback keeps every verdict it had, and reports the review.
    let verified = server.call_tool("verify_import", args).await;
    let result = &verified["structuredContent"]["result"];
    assert_eq!(
        result["dispatch"]["state"], "reconciliation_required",
        "{verified}"
    );
    assert_eq!(result["error"]["code"], "posted_under_changed_masters");
    assert_eq!(result["operator_review"]["state"], "current", "{verified}");
    assert_eq!(
        server
            .latest_import_snapshot(BATCH)
            .unwrap()
            .unwrap()
            .batch
            .status,
        "verification_incomplete"
    );
    assert_eq!(sent(simulator).len(), scripted_plans);
}

#[tokio::test]
async fn a_declined_review_writes_nothing() {
    let simulator = SequenceSimulator::spawn(with_sentinel(reconcile_readback())).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let (server, args) = doubted(&simulator, directory.path());
    let approval = ScriptedApproval::declining();
    let response = acknowledge(&server, args, approval.clone()).await;
    let result = &response["structuredContent"]["result"];
    assert_eq!(result["error"]["code"], "ack_review_declined", "{response}");
    assert_eq!(approval.reviews().len(), 1);
    assert!(!ack_path(&server).exists());
}

/// Each refusal names its reason, shows no dialog and writes nothing.
async fn refused(
    plans: Vec<ScenarioPlan>,
    response: ledger::DispatchResponse,
    check: Option<&[u8]>,
    doubt: Option<&[u8]>,
    code: &str,
) {
    let simulator = SequenceSimulator::spawn(with_sentinel(plans)).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let (server, args) = seeded(&simulator, directory.path(), response, check, doubt);
    let approval = ScriptedApproval::approving();
    let outcome = acknowledge(&server, args, approval.clone()).await;
    assert_eq!(
        outcome["structuredContent"]["result"]["error"]["code"], code,
        "{outcome}"
    );
    assert!(approval.reviews().is_empty(), "{code}: no dialog");
    assert!(!ack_path(&server).exists(), "{code}: nothing written");
}

#[tokio::test]
async fn a_batch_without_an_observed_doubt_is_refused() {
    let unchanged = br#"{"state":"unchanged","trigger":"masters_moved"}"#;
    refused(
        reconcile_readback(),
        clean(),
        Some(unchanged),
        None,
        "ack_no_observed_doubt",
    )
    .await;
}

#[tokio::test]
async fn a_pending_check_is_refused_not_acknowledged() {
    // The readback finishes a pending check only with the catalogue it
    // scripts; none is scripted, so the check stays pending.
    let pending = br#"{"state":"check_pending"}"#;
    refused(
        reconcile_readback(),
        clean(),
        Some(pending),
        None,
        "ack_check_pending",
    )
    .await;
}

/// A doubt outranks a check left pending beside it (a write that failed
/// between the two): nothing would ever finish that check, so refusing would
/// refuse the batch forever.
#[tokio::test]
async fn a_doubt_beside_a_check_left_pending_is_reviewable() {
    let mut plans = reconcile_readback();
    plans.extend(reconcile_readback());
    let simulator = SequenceSimulator::spawn(with_sentinel(plans)).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let (server, args) = seeded(
        &simulator,
        directory.path(),
        clean(),
        Some(br#"{"state":"check_pending"}"#),
        Some(DOUBT.as_bytes()),
    );
    let response = acknowledge(&server, args, ScriptedApproval::approving()).await;
    assert_eq!(
        response["structuredContent"]["result"]["operator_review"]["state"], "current",
        "{response}"
    );
}

#[tokio::test]
async fn an_unreadable_masters_record_is_refused() {
    refused(
        reconcile_readback(),
        clean(),
        Some(b"not json"),
        None,
        "ack_masters_record_unreadable",
    )
    .await;
    // A doubt must name each of its ledgers, as Bridge writes it.
    let unnamed = br#"{"state":"posted_under_changed_masters","ledgers":["Cash",7]}"#;
    refused(
        reconcile_readback(),
        clean(),
        Some(unnamed),
        Some(unnamed),
        "ack_masters_record_unreadable",
    )
    .await;
    refused(
        reconcile_readback(),
        clean(),
        Some(DOUBT.as_bytes()),
        Some(b"{\"state\":"),
        "ack_masters_record_unreadable",
    )
    .await;
    // A doubt file holding anything but that verdict is not one to bind to.
    refused(
        reconcile_readback(),
        clean(),
        Some(DOUBT.as_bytes()),
        Some(br#"{"state":"unchanged"}"#),
        "ack_masters_record_unreadable",
    )
    .await;
    // A doubt that names no ledger shows nothing to review.
    let nameless = br#"{"state":"posted_under_changed_masters","ledgers":[]}"#;
    refused(
        reconcile_readback(),
        clean(),
        Some(nameless),
        Some(nameless),
        "ack_masters_record_unreadable",
    )
    .await;
}

#[tokio::test]
async fn a_response_that_was_not_clean_is_refused() {
    let altered = super::super::tests::dispatch_response("success", 1, 1);
    refused(
        reconcile_readback(),
        altered,
        Some(DOUBT.as_bytes()),
        Some(DOUBT.as_bytes()),
        "ack_response_not_clean",
    )
    .await;
}

#[tokio::test]
async fn a_readback_that_does_not_match_is_refused() {
    let mut plans = probe();
    plans.extend(verified_company());
    plans.extend(paired(marks()));
    plans.extend(paired(empty_collection()));
    plans.extend(paired(empty_collection()));
    plans.extend(probe());
    refused(
        plans,
        clean(),
        Some(DOUBT.as_bytes()),
        Some(DOUBT.as_bytes()),
        "ack_readback_not_matched",
    )
    .await;
}

#[tokio::test]
async fn a_review_is_recorded_only_once() {
    let mut plans = reconcile_readback();
    plans.extend(reconcile_readback());
    plans.extend(reconcile_readback());
    let simulator = SequenceSimulator::spawn(with_sentinel(plans)).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let (server, args) = doubted(&simulator, directory.path());
    let first = acknowledge(&server, args.clone(), ScriptedApproval::approving()).await;
    assert_eq!(
        first["structuredContent"]["result"]["operator_review"]["state"], "current",
        "{first}"
    );
    let recorded = fs::read(ack_path(&server)).unwrap();
    let approval = ScriptedApproval::approving();
    let second = acknowledge(&server, args, approval.clone()).await;
    assert_eq!(
        second["structuredContent"]["result"]["error"]["code"], "ack_already_recorded",
        "{second}"
    );
    assert!(approval.reviews().is_empty(), "refused before the dialog");
    assert_eq!(fs::read(ack_path(&server)).unwrap(), recorded);
}

#[tokio::test]
async fn a_doubt_that_changes_while_the_dialog_is_open_is_refused() {
    let mut plans = reconcile_readback();
    plans.extend(reconcile_readback());
    let simulator = SequenceSimulator::spawn(with_sentinel(plans)).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let (server, args) = doubted(&simulator, directory.path());
    let doubt_path = server
        .imports_dir()
        .unwrap()
        .join(format!("{BATCH}.masters_doubt.json"));
    let approval = ScriptedApproval::approving_after(move || {
        let other = r#"{"state":"posted_under_changed_masters","trigger":"masters_moved","ledgers":["Sales"]}"#;
        fs::write(&doubt_path, other).unwrap();
    });
    let response = acknowledge(&server, args, approval).await;
    assert_eq!(
        response["structuredContent"]["result"]["error"]["code"], "ack_changed_while_reviewing",
        "{response}"
    );
    assert!(!ack_path(&server).exists());
}

#[tokio::test]
async fn a_voucher_that_changes_while_the_dialog_is_open_is_refused() {
    for second in [readback_at_alter_id(11), readback_with_edited_narration()] {
        let mut plans = reconcile_readback();
        plans.extend(second);
        let simulator = SequenceSimulator::spawn(with_sentinel(plans)).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let (server, args) = doubted(&simulator, directory.path());
        let response = acknowledge(&server, args, ScriptedApproval::approving()).await;
        assert_eq!(
            response["structuredContent"]["result"]["error"]["code"], "ack_changed_while_reviewing",
            "{response}"
        );
        assert!(!ack_path(&server).exists());
    }
}

/// A recorded review covers only the doubt it showed and the voucher as it
/// was: each binding, changed alone, makes it stale.
#[tokio::test]
async fn a_recorded_review_goes_stale_when_anything_it_bound_changes() {
    type Change = fn(&Server);
    let untouched: Change = |_| {};
    let new_doubt: Change = |server| {
        let other = r#"{"state":"posted_under_changed_masters","trigger":"masters_moved","ledgers":["Sales"]}"#;
        let path = server
            .imports_dir()
            .unwrap()
            .join(format!("{BATCH}.masters_doubt.json"));
        fs::write(path, other).unwrap();
    };
    let other_batch: Change = |server| {
        let path = ack_path(server);
        let mut record: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        record["batch_id"] = json!("bridge-00000000-0000-4000-8000-000000000001");
        fs::write(path, serde_json::to_vec(&record).unwrap()).unwrap();
    };
    let other_company: Change = |server| {
        edit_record(
            server,
            "company_guid",
            json!("00000000-0000-4000-8000-000000000002"),
        )
    };
    let other_fields: Change =
        |server| edit_record(server, "voucher_fingerprint_fields", json!("v0:guid"));
    let other_guid: Change = |server| {
        edit_record(
            server,
            "voucher_guid",
            json!("61c6de69-1748-461c-ad3f-162cb949df9f-00000006"),
        )
    };
    let other_master_id: Change = |server| edit_record(server, "voucher_master_id", json!("6"));
    let cases: [(&str, Vec<ScenarioPlan>, Change); 8] = [
        ("alter_id", readback_at_alter_id(11), untouched),
        ("fingerprint", readback_with_edited_narration(), untouched),
        ("doubt", reconcile_readback(), new_doubt),
        ("identity", reconcile_readback(), other_batch),
        ("company", reconcile_readback(), other_company),
        ("fingerprint_fields", reconcile_readback(), other_fields),
        ("voucher_guid", reconcile_readback(), other_guid),
        ("voucher_master_id", reconcile_readback(), other_master_id),
    ];
    for (name, later, change) in cases {
        let mut plans = reconcile_readback();
        plans.extend(reconcile_readback());
        plans.extend(later);
        let simulator = SequenceSimulator::spawn(with_sentinel(plans)).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let (server, args) = doubted(&simulator, directory.path());
        let recorded = acknowledge(&server, args.clone(), ScriptedApproval::approving()).await;
        assert_eq!(
            recorded["structuredContent"]["result"]["operator_review"]["state"], "current",
            "{name}: {recorded}"
        );
        change(&server);
        let verified = server.call_tool("verify_import", args).await;
        let result = &verified["structuredContent"]["result"];
        assert_eq!(
            result["operator_review"]["state"], "stale",
            "{name}: {verified}"
        );
        assert_eq!(
            result["dispatch"]["state"], "reconciliation_required",
            "{name}"
        );
    }
}

#[tokio::test]
async fn the_tool_needs_the_posting_opt_in() {
    let listed = |writes: bool| {
        crate::agent::catalog::registered_tool_definitions(true, writes)
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["name"] == ACK)
    };
    assert!(listed(true));
    assert!(!listed(false));
    let simulator = SequenceSimulator::spawn(with_sentinel(Vec::new())).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let server = Server::new(crate::agent::Settings {
        writes_enabled: false,
        batch_post_enabled: false,
        ..server_at(simulator.address(), directory.path())
            .settings
            .clone()
    });
    let approval = ScriptedApproval::approving();
    let response = acknowledge(
        &server,
        json!({"company_guid":GUID,"batch_id":BATCH}),
        approval.clone(),
    )
    .await;
    assert_eq!(
        response["structuredContent"]["result"]["error"]["code"], "import_posting_disabled",
        "{response}"
    );
    assert!(approval.reviews().is_empty());
    assert!(sent(simulator).is_empty());
}

fn edit_record(server: &Server, field: &str, value: Value) {
    let path = ack_path(server);
    let mut record: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    record[field] = value;
    fs::write(path, serde_json::to_vec(&record).unwrap()).unwrap();
}

/// A record in a format this build does not know is reported unreadable,
/// never matched.
#[tokio::test]
async fn a_record_this_build_cannot_read_is_reported_unreadable() {
    type Change = fn(&Server);
    let newer: Change = |server| edit_record(server, "version", json!(2));
    let unknown_field: Change = |server| edit_record(server, "approved", json!(true));
    let garbage: Change = |server| fs::write(ack_path(server), b"not json").unwrap();
    let doubt_unreadable: Change = |server| {
        let path = server
            .imports_dir()
            .unwrap()
            .join(format!("{BATCH}.masters_doubt.json"));
        fs::write(path, b"not json").unwrap();
    };
    for (name, change) in [
        ("version", newer),
        ("field", unknown_field),
        ("garbage", garbage),
        ("doubt_unreadable", doubt_unreadable),
    ] {
        let mut plans = reconcile_readback();
        plans.extend(reconcile_readback());
        plans.extend(reconcile_readback());
        let simulator = SequenceSimulator::spawn(with_sentinel(plans)).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let (server, args) = doubted(&simulator, directory.path());
        let recorded = acknowledge(&server, args.clone(), ScriptedApproval::approving()).await;
        assert_eq!(
            recorded["structuredContent"]["result"]["operator_review"]["state"], "current",
            "{name}: {recorded}"
        );
        change(&server);
        let verified = server.call_tool("verify_import", args).await;
        let result = &verified["structuredContent"]["result"];
        assert_eq!(
            result["operator_review"]["state"], "unreadable",
            "{name}: {verified}"
        );
    }
}

#[tokio::test]
async fn a_batch_never_posted_is_refused_before_any_request() {
    let simulator = SequenceSimulator::spawn(with_sentinel(Vec::new())).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let server = server_at(simulator.address(), directory.path());
    let line = saved_captured_line(&server);
    let approval = ScriptedApproval::approving();
    let response = acknowledge(
        &server,
        json!({"company_guid":GUID,"batch_id":line.batch_id}),
        approval.clone(),
    )
    .await;
    assert_eq!(
        response["structuredContent"]["result"]["error"]["code"], "ack_batch_not_posted",
        "{response}"
    );
    assert!(approval.reviews().is_empty());
    assert!(sent(simulator).is_empty());
}

/// A readback that finds the marked voucher but not as posted is not a match.
#[tokio::test]
async fn a_readback_that_diverges_is_refused() {
    let diverged = readback_of(replaced_once(
        &captured_posted_journal(),
        "Bridge Nested Debtor WR4",
        "Bridge Nested Debtor WR5",
    ));
    refused(
        diverged,
        clean(),
        Some(DOUBT.as_bytes()),
        Some(DOUBT.as_bytes()),
        "ack_readback_not_matched",
    )
    .await;
}

/// A record that appears while the dialog is open is never overwritten.
#[tokio::test]
async fn a_record_written_while_the_dialog_is_open_is_kept() {
    let mut plans = reconcile_readback();
    plans.extend(reconcile_readback());
    let simulator = SequenceSimulator::spawn(with_sentinel(plans)).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let (server, args) = doubted(&simulator, directory.path());
    let path = ack_path(&server);
    let planted = path.clone();
    let approval = ScriptedApproval::approving_after(move || {
        fs::write(&planted, b"written elsewhere").unwrap();
    });
    let response = acknowledge(&server, args, approval).await;
    assert_eq!(
        response["structuredContent"]["result"]["error"]["code"], "ack_already_recorded",
        "{response}"
    );
    assert_eq!(fs::read(&path).unwrap(), b"written elsewhere");
}

/// A doubt outranks a check record that cannot be read beside it, as it does
/// for the verdict, which never rewrites that record once a doubt exists.
#[tokio::test]
async fn a_doubt_beside_an_unreadable_check_is_reviewable() {
    let mut plans = reconcile_readback();
    plans.extend(reconcile_readback());
    let simulator = SequenceSimulator::spawn(with_sentinel(plans)).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let (server, args) = seeded(
        &simulator,
        directory.path(),
        clean(),
        Some(b"not json"),
        Some(DOUBT.as_bytes()),
    );
    let response = acknowledge(&server, args, ScriptedApproval::approving()).await;
    assert_eq!(
        response["structuredContent"]["result"]["operator_review"]["state"], "current",
        "{response}"
    );
}

/// With a doubt and no record, the review reads absent; with a record and
/// no doubt it can bind to, it reads stale, never nothing.
#[tokio::test]
async fn a_review_reads_absent_before_a_record_and_stale_without_its_doubt() {
    let mut plans = reconcile_readback();
    plans.extend(reconcile_readback());
    plans.extend(reconcile_readback());
    plans.extend(reconcile_readback());
    let simulator = SequenceSimulator::spawn(with_sentinel(plans)).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let (server, args) = doubted(&simulator, directory.path());
    let before = server.call_tool("verify_import", args.clone()).await;
    assert_eq!(
        before["structuredContent"]["result"]["operator_review"],
        json!({"state":"absent"}),
        "{before}"
    );
    acknowledge(&server, args.clone(), ScriptedApproval::approving()).await;
    let imports = server.imports_dir().unwrap();
    fs::remove_file(imports.join(format!("{BATCH}.masters_doubt.json"))).unwrap();
    fs::write(
        imports.join(format!("{BATCH}.masters_check.json")),
        br#"{"state":"unchanged"}"#,
    )
    .unwrap();
    let after = server.call_tool("verify_import", args).await;
    assert_eq!(
        after["structuredContent"]["result"]["operator_review"]["state"], "stale",
        "{after}"
    );
}

/// A two-voucher batch whose dispatch intent is journaled, as `post_import`
/// leaves one just before its POST.
fn dispatched_batch(server: &Server) -> ImportLedgerLine {
    let mut line = saved_captured_line(server);
    let mut second = line.vouchers[0].clone();
    second.bridge_txn_id = "BRIDGE_MCP_LIVE_20260906_A2".into();
    line.vouchers.push(second);
    line.txn_ids.push("BRIDGE_MCP_LIVE_20260906_A2".into());
    server.append_import_ledger(&line).unwrap();
    // One REMOTEID per voucher: the intent records the batch's two.
    let native = native_post_request(
        &line,
        RemoteIds::from_ids(vec![Uuid::new_v4(), Uuid::new_v4()]),
    )
    .unwrap();
    {
        let _lock = server.lock_import_admission().unwrap();
        server
            .append_import_record_while_admitted(&ledger::StatusRecord::dispatch_for(
                &line, &native,
            ))
            .unwrap();
    }
    line
}

/// A batch of several vouchers with no doubt recorded is refused before any
/// request: there is nothing to review. (Before batch reviews, slice D2b,
/// any batch was refused here as `ack_batch_not_posted`.)
#[tokio::test]
async fn a_batch_of_several_vouchers_with_no_doubt_is_refused_before_any_request() {
    let simulator = SequenceSimulator::spawn(with_sentinel(Vec::new())).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let server = server_at(simulator.address(), directory.path());
    let line = dispatched_batch(&server);
    let response = acknowledge(
        &server,
        json!({"company_guid":GUID,"batch_id":line.batch_id}),
        ScriptedApproval::approving(),
    )
    .await;
    assert_eq!(
        response["structuredContent"]["result"]["error"]["code"], "ack_no_observed_doubt",
        "{response}"
    );
    assert!(sent(simulator).is_empty());
}

/// A batch whose step verdict is still pending is refused before any request:
/// only a post records that verdict, so no read could finish it.
#[tokio::test]
async fn a_batch_step_review_left_pending_is_refused_before_any_request() {
    let simulator = SequenceSimulator::spawn(with_sentinel(Vec::new())).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let server = server_at(simulator.address(), directory.path());
    let line = dispatched_batch(&server);
    server
        .record_post_checks_pending(&line.batch_id, true)
        .unwrap();
    let response = acknowledge(
        &server,
        json!({"company_guid":GUID,"batch_id":line.batch_id,"doubt":"batch_step"}),
        ScriptedApproval::approving(),
    )
    .await;
    assert_eq!(
        response["structuredContent"]["result"]["error"]["code"], "ack_check_pending",
        "{response}"
    );
    assert!(sent(simulator).is_empty());
}

/// A doubt whose own file cannot be written (#722): the write is made to fail
/// by a directory standing where the file goes. The check record keeps the
/// doubt and says why no review can find it, and a review of that doubt is
/// refused before any request, for each kind. The control is the same doubt
/// with its file written, whose check record carries no mark; that such a
/// doubt reads as reviewable is the unit tests' control.
#[tokio::test]
async fn a_batch_doubt_whose_own_file_was_not_written_is_refused_before_any_request() {
    let changed = json!({"state":"posted_under_changed_masters","ledgers":["Cash"]});
    let step =
        json!({"before":10,"after":13,"step":3,"reported_created":2,"matches_created":false});
    for (kind, blocked) in [
        ("masters", "masters_doubt.json"),
        ("batch_step", "batch_step_doubt.json"),
    ] {
        for write_fails in [true, false] {
            let simulator = SequenceSimulator::spawn(with_sentinel(Vec::new())).unwrap();
            let directory = tempfile::tempdir().unwrap();
            let server = server_at(simulator.address(), directory.path());
            let line = dispatched_batch(&server);
            let imports = server.imports_dir().unwrap();
            server
                .record_post_checks_pending(&line.batch_id, true)
                .unwrap();
            let doubt_path = imports.join(format!("{}.{blocked}", line.batch_id));
            if write_fails {
                block(doubt_path.clone());
            }
            // In the post's own order: the step verdict, then the masters
            // verdict, which rewrites the check record around the step.
            let (step, masters) = if kind == "masters" {
                (json!({"matches_created":true}), changed.clone())
            } else {
                (step.clone(), json!({"state":"unchanged"}))
            };
            server.record_batch_step_verdict(&line.batch_id, &step);
            server.record_masters_verdict_for(&line.batch_id, masters, true);
            if write_fails {
                fs::remove_dir_all(&doubt_path).unwrap();
            }
            assert_eq!(doubt_path.is_file(), !write_fails, "{kind}");
            let check: Value = serde_json::from_slice(
                &fs::read(imports.join(format!("{}.masters_check.json", line.batch_id))).unwrap(),
            )
            .unwrap();
            let verdict = if kind == "masters" {
                &check
            } else {
                &check["batch_step"]
            };
            // The verdict recorded is the doubt itself, for either kind.
            assert_eq!(
                verdict["state"],
                if kind == "masters" {
                    "posted_under_changed_masters"
                } else {
                    "unmatched"
                },
                "{kind}: {check}"
            );
            assert_eq!(
                verdict["doubt_record"],
                if write_fails {
                    json!("unavailable")
                } else {
                    Value::Null
                },
                "{kind}: {check}"
            );
            // Marked or not, the verdict is still doubt.
            let expected = if kind == "masters" {
                "posted_under_changed_masters"
            } else {
                "batch_step_unconfirmed"
            };
            assert_eq!(
                post_doubt(
                    read_masters_check(&imports, &line.batch_id).as_ref(),
                    line.vouchers.len()
                )
                .map(|(code, _)| code),
                Some(expected),
                "{kind}: {check}"
            );
            if !write_fails {
                continue;
            }
            let response = acknowledge(
                &server,
                json!({"company_guid":GUID,"batch_id":line.batch_id,"doubt":kind}),
                ScriptedApproval::approving(),
            )
            .await;
            assert_eq!(
                response["structuredContent"]["result"]["error"]["code"],
                "ack_doubt_record_unavailable",
                "{kind}: {response}"
            );
            assert!(sent(simulator).is_empty(), "{kind}: no request");
        }
    }
}

/// With no doubt named, a batch whose masters doubt is held only by the check
/// record, beside a step doubt with its own file, needs a name; one whose two
/// doubts are both held only by the check record is refused as unavailable,
/// since no name could be reviewed. Either way, before any request.
#[tokio::test]
async fn an_unnamed_review_beside_a_doubt_without_its_file_is_refused_before_any_request() {
    let changed = json!({"state":"posted_under_changed_masters","ledgers":["Cash"]});
    let step =
        json!({"before":10,"after":13,"step":3,"reported_created":2,"matches_created":false});
    for (step_file_fails, code) in [
        (false, "ack_doubt_ambiguous"),
        (true, "ack_doubt_record_unavailable"),
    ] {
        let simulator = SequenceSimulator::spawn(with_sentinel(Vec::new())).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let server = server_at(simulator.address(), directory.path());
        let line = dispatched_batch(&server);
        let imports = server.imports_dir().unwrap();
        server
            .record_post_checks_pending(&line.batch_id, true)
            .unwrap();
        let masters_doubt = imports.join(format!("{}.masters_doubt.json", line.batch_id));
        let step_doubt = imports.join(format!("{}.batch_step_doubt.json", line.batch_id));
        block(masters_doubt.clone());
        if step_file_fails {
            block(step_doubt.clone());
        }
        server.record_batch_step_verdict(&line.batch_id, &step);
        server.record_masters_verdict_for(&line.batch_id, changed.clone(), true);
        fs::remove_dir_all(&masters_doubt).unwrap();
        if step_file_fails {
            fs::remove_dir_all(&step_doubt).unwrap();
        }
        assert_eq!(step_doubt.is_file(), !step_file_fails, "{code}");
        let response = acknowledge(
            &server,
            json!({"company_guid":GUID,"batch_id":line.batch_id}),
            ScriptedApproval::approving(),
        )
        .await;
        let error = &response["structuredContent"]["result"]["error"];
        assert_eq!(error["code"], code, "{response}");
        if code == "ack_doubt_ambiguous" {
            assert_eq!(error["cause"], "masters_and_batch_step", "{response}");
        }
        assert!(sent(simulator).is_empty(), "{code}: no request");
    }
}

/// One voucher's doubt recorded only in the check record is refused after
/// the read, as every single-voucher refusal is, and never as no doubt.
#[tokio::test]
async fn a_doubt_recorded_only_in_the_check_record_is_refused() {
    let marked = br#"{"state":"posted_under_changed_masters","ledgers":["Cash"],"doubt_record":"unavailable"}"#;
    refused(
        reconcile_readback(),
        clean(),
        Some(marked),
        None,
        "ack_doubt_record_unavailable",
    )
    .await;
}

/// The live batch post of slice D3 (a licensed TallyPrime 7.1 Silver lab, 50
/// Journals on a synthetic company), as the journal and saved file recorded it.
const D3_BATCH: &str = "bridge-1e5b2cd7-f5c6-4d51-adc9-53a4dfa370bb";
const D3_GUID: &str = "17a10910-773c-42c6-bd66-7bba9a392536";

/// One `verify_import` of that batch, answered in the captured order: `S` the
/// status probe, then the company extents (`E`), the company high water
/// (`H`), the voucher census (`C`) and the import verification read (`V`),
/// each response as Tally sent it (fixtures `d3-batch-*`, one capture).
fn d3_batch_readback() -> Vec<ScenarioPlan> {
    let extent = captured(include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/d3-batch-company-extent.utf16le.xml"
    ));
    let high_water = captured(include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/d3-batch-company-high-water.utf16le.xml"
    ));
    let census = captured(include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/d3-batch-voucher-census.utf16le.xml"
    ));
    let readback = captured(include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/d3-batch-import-verification.utf16le.xml"
    ));
    "SEESESEHSHSEECSCSEEVSVSEEVSVSE"
        .chars()
        .map(|step| match step {
            'S' => status(),
            'E' => xml(extent.clone()),
            'H' => xml(high_water.clone()),
            'C' => xml(census.clone()),
            _ => xml(readback.clone()),
        })
        .collect()
}

/// The sha256 of each request that capture carried, in the same order; `None`
/// for the status probe.
fn d3_batch_requests() -> Vec<Option<&'static str>> {
    "SEESESEHSHSEECSCSEEVSVSEEVSVSE"
        .chars()
        .map(|step| match step {
            'S' => None,
            'E' => Some("9df2a53f085dac2636e9435462b612c1487ec6f903677815036c9f39163f7dd8"),
            'H' => Some("0930288f6eb531926d018cc4762288084831b8fad16a554e48fafbd694e245c2"),
            'C' => Some("5a9690e84fef530985b2444a0bc66cd5784a4467571da04e31559c72c6ba11f1"),
            _ => Some("c25ed5f689596b2620c58f417af74217985fbbe54a12f2c317fd537f901ed85f"),
        })
        .collect()
}

/// A person reviews the doubted 50-voucher batch through the whole path: the
/// read, the dialog, the second read and the write. The record binds every
/// voucher as Tally holds it, in batch order, and a later readback reports it
/// current. The step doubt is a local record written here; every Tally
/// response is captured, and every request sent equals the captured one.
#[tokio::test]
async fn a_review_of_the_captured_50_voucher_batch_binds_every_voucher() {
    let mut plans = d3_batch_readback();
    plans.extend(d3_batch_readback());
    plans.extend(d3_batch_readback());
    let simulator = SequenceSimulator::spawn(with_sentinel(plans)).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let server = server_at(simulator.address(), directory.path());
    let origin =
        super::super::super::super::canonical_loopback_origin(&server.settings.endpoint).unwrap();
    fs::write(
        directory.path().join("agent-import-ledger.jsonl"),
        include_str!("../crates/bridge-tally-protocol/tests/fixtures/agent/d3-batch-journal.jsonl")
            .replace("http://127.0.0.1:9001", &origin),
    )
    .unwrap();
    let imports = server.imports_dir().unwrap();
    fs::write(
        imports.join(format!("{D3_BATCH}.xml")),
        include_bytes!("../crates/bridge-tally-protocol/tests/fixtures/agent/d3-batch-import.xml"),
    )
    .unwrap();
    let step = json!({"state":"unmatched","target_voucher_step":{
        "before":1419,"after":1470,"step":51,"reported_created":50,"matches_created":false}});
    fs::write(
        imports.join(format!("{D3_BATCH}.batch_step_doubt.json")),
        serde_json::to_vec(&step).unwrap(),
    )
    .unwrap();
    fs::write(
        imports.join(format!("{D3_BATCH}.masters_check.json")),
        serde_json::to_vec(
            &json!({"state":"not_checked","reason":"masters_unmoved","batch_step":step}),
        )
        .unwrap(),
    )
    .unwrap();
    let args = json!({"company_guid":D3_GUID,"batch_id":D3_BATCH,"doubt":"batch_step"});
    let approval = ScriptedApproval::approving();

    let response = acknowledge(&server, args.clone(), approval.clone()).await;
    assert!(
        response["structuredContent"]["result"]["error"].is_null(),
        "{response}"
    );
    assert_eq!(approval.reviews().len(), 1, "{response}");
    let review = &approval.reviews()[0];
    for shown in [
        "Record that you reviewed 50 vouchers in \"BRIDGE AMEND LAB\"",
        "its voucher mark moved by 51 (from 1419 to 1470); Tally reported creating 50.",
        "Dates: 20260401 to 20260401  ALTERIDs: 1420 to 1469",
        "Dr 1275  Cr 0  50 entries  \"Test Expense B\"",
        "Dr 0  Cr 1275  50 entries  \"Cash\"",
        "I reviewed these 50 vouchers in Tally.",
    ] {
        assert!(review.contains(shown), "{shown}: {review}");
    }

    // The batch record: version 2, the step doubt, every voucher in order.
    let record: Value = serde_json::from_slice(
        &fs::read(imports.join(format!("{D3_BATCH}.batch_step_ack.json"))).unwrap(),
    )
    .unwrap();
    assert_eq!(record["version"], 2, "{record}");
    assert_eq!(record["doubt"], "batch_step");
    assert_eq!(
        record["doubt_sha256"],
        crate::agent::sha256_hex(&serde_json::to_vec(&step).unwrap())
    );
    let vouchers = record["vouchers"].as_array().unwrap();
    assert_eq!(
        vouchers
            .iter()
            .map(|voucher| (
                voucher["bridge_txn_id"].as_str().unwrap().to_string(),
                voucher["alter_id"].as_u64().unwrap()
            ))
            .collect::<Vec<_>>(),
        (1..=50)
            .map(|index| (format!("D3-{index:03}"), 1419 + index))
            .collect::<Vec<_>>()
    );
    assert!(!imports
        .join(format!("{D3_BATCH}.masters_ack.json"))
        .exists());

    // A later readback keeps the batch's verdict and reports the review.
    let verified = server
        .call_tool(
            "verify_import",
            json!({"company_guid":D3_GUID,"batch_id":D3_BATCH}),
        )
        .await;
    let result = &verified["structuredContent"]["result"];
    assert_eq!(result["counts"]["posted_verified"], 50, "{verified}");
    assert_eq!(
        result["dispatch"]["state"], "reconciliation_required",
        "{verified}"
    );
    assert_eq!(
        result["operator_review"]["batch_step"]["state"], "current",
        "{verified}"
    );
    assert_eq!(
        result["operator_review"]["masters"],
        Value::Null,
        "{verified}"
    );

    // Every request Bridge sent is the one the capture answered.
    let requests = sent(simulator);
    let expected = [
        d3_batch_requests(),
        d3_batch_requests(),
        d3_batch_requests(),
    ]
    .concat();
    assert_eq!(requests.len(), expected.len(), "{requests:?}");
    for (index, (request, expected)) in requests.iter().zip(expected).enumerate() {
        match expected {
            None => assert_eq!(request.method, "GET", "request {index}"),
            Some(sha256) => assert_eq!(request.request_body_sha256, sha256, "request {index}"),
        }
    }
}
