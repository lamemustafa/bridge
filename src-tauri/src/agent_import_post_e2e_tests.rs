//! bridge#583: the native post driven end to end through the `post_import`
//! tool call against the protocol simulator, the approval answered by the
//! test-only seam (`approved_import::test_seam`). No real Tally is involved.
use super::*;
use crate::tally::approved_import::test_seam::{ScriptedApproval, SCRIPTED_APPROVAL};
use bridge_tally_transport::TallyEndpointConfig;
use std::time::Duration;
use tally_protocol_simulator::{
    Delivery, Fixture, ProductStatus, ResponseFraming, ScenarioPlan, SequenceSimulator,
    WireEncoding,
};

const GUID: &str = "61c6de69-1748-461c-ad3f-162cb949df9f";

fn captured(bytes: &[u8]) -> String {
    String::from_utf16(
        &bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>(),
    )
    .unwrap()
}

fn companies() -> String {
    captured(include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-licensed-release-companies.utf16le.xml"
    ))
}

fn catalogue() -> String {
    captured(include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-ledger-catalogue.utf16le.xml"
    ))
}

fn empty_collection() -> String {
    captured(include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-empty-collection.utf16le.xml"
    ))
}

fn marks() -> String {
    format!(
        "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><COMPANY>\\
         <GUID>{GUID}</GUID><ALTVCHID>10</ALTVCHID><ALTMSTID>7</ALTMSTID>\\
         </COMPANY></COLLECTION></DATA></BODY></ENVELOPE>"
    )
}

/// Every loaded company's change marks, shaped as the high-water collection
/// returns them (a `NAME` attribute per row; measured 2026-09-21). The target
/// is the captured laboratory company; the other two are invented.
fn company_marks(target_vouchers: u64, other_vouchers: u64, target_name: &str) -> String {
    format!(
        "<ENVELOPE><HEADER><VERSION>1</VERSION><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>\
         <COMPANY NAME=\"{target_name}\" RESERVEDNAME=\"\"><GUID>{GUID}</GUID><ALTVCHID>{target_vouchers}</ALTVCHID><ALTMSTID>7</ALTMSTID></COMPANY>\
         <COMPANY NAME=\"Synthetic Other Lab\" RESERVEDNAME=\"\"><GUID>22222222-2222-4222-8222-222222222222</GUID><ALTVCHID>{other_vouchers}</ALTVCHID><ALTMSTID>3</ALTMSTID></COMPANY>\
         <COMPANY NAME=\"Synthetic Empty Lab\" RESERVEDNAME=\"\"><GUID>33333333-3333-4333-8333-333333333333</GUID><ALTMSTID>1</ALTMSTID></COMPANY>\
         </COLLECTION></DATA></BODY></ENVELOPE>"
    )
}

fn marks_before() -> ScenarioPlan {
    xml(company_marks(10, 50, "WR2 Unicode Lab"))
}

fn xml(body: String) -> ScenarioPlan {
    ScenarioPlan::new(Fixture::SyntheticXml(body))
        .with_encoding(WireEncoding::Utf16Le)
        .with_framing(ResponseFraming::ContentLength)
}

fn status() -> ScenarioPlan {
    ScenarioPlan::new(Fixture::ProductStatus(ProductStatus::TallyPrime))
        .with_framing(ResponseFraming::ContentLength)
}

/// One identity-bracketed paired read of `body`.
fn paired(body: String) -> Vec<ScenarioPlan> {
    vec![
        xml(companies()),
        xml(body.clone()),
        status(),
        xml(body),
        status(),
        xml(companies()),
    ]
}

/// A product and mode probe: status, then the company list.
fn probe() -> Vec<ScenarioPlan> {
    vec![status(), xml(companies())]
}

/// The company's verified identity.
fn verified_company() -> Vec<ScenarioPlan> {
    vec![xml(companies()), status(), xml(companies()), status()]
}

/// Every read `post_import` makes before it asks for approval, on a book where
/// the batch's vouchers are absent.
fn before_approval() -> Vec<ScenarioPlan> {
    let mut plans = Vec::new();
    // verify_import_for_post: opening mode, identity, the window's mark, the
    // window, its replay, and the closing mode an absence needs.
    plans.extend(probe());
    plans.extend(verified_company());
    plans.extend(paired(marks()));
    plans.extend(paired(empty_collection()));
    plans.extend(paired(empty_collection()));
    plans.extend(probe());
    // The post's own identity, ledger catalogue and qualified mode.
    plans.extend(verified_company());
    plans.extend(paired(catalogue()));
    plans.extend(probe());
    plans
}

/// Tally's answer to one created voucher.
fn created_one() -> String {
    "<ENVELOPE><HEADER><VERSION>1</VERSION><STATUS>1</STATUS></HEADER><BODY><DATA>\
     <IMPORTRESULT><CREATED>1</CREATED><ALTERED>0</ALTERED><DELETED>0</DELETED>\
     <LASTVCHID>11</LASTVCHID><LASTMID>0</LASTMID><COMBINED>0</COMBINED>\
     <IGNORED>0</IGNORED><ERRORS>0</ERRORS><CANCELLED>0</CANCELLED></IMPORTRESULT>\
     </DATA></BODY></ENVELOPE>"
        .to_string()
}

/// What the dispatch sends after approval, up to and including the import:
/// the opening mode and company admission, the ledger catalogue, the closing
/// mode and admission, the absence read twice, then the one POST. The index of
/// the POST is `before_approval().len() + after_approval(..).len() - 1`.
fn after_approval(post: ScenarioPlan) -> Vec<ScenarioPlan> {
    let mut plans = probe();
    plans.push(xml(companies()));
    plans.extend(paired(catalogue()));
    plans.extend(probe());
    plans.push(xml(companies()));
    plans.extend(paired(empty_collection()));
    plans.extend(paired(empty_collection()));
    plans.push(marks_before());
    plans.push(post);
    plans
}

/// The one dispatch record a run journaled.
fn dispatch_intent(directory: &std::path::Path) -> Value {
    let intents = String::from_utf8(journal(directory))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|record| record["record_type"] == "dispatch_intent")
        .collect::<Vec<_>>();
    assert_eq!(intents.len(), 1, "{intents:?}");
    intents[0].clone()
}

fn server_at(address: std::net::SocketAddr, directory: &std::path::Path) -> Server {
    Server::new(crate::agent::Settings {
        endpoint: TallyEndpointConfig {
            host: address.ip().to_string(),
            port: address.port(),
        },
        data_dir: directory.to_path_buf(),
        max_rows: 10,
        max_bytes: 200_000,
        redaction: crate::agent::Redaction::None,
        import_enabled: true,
        writes_enabled: true,
    })
}

/// A built, never-dispatched batch for the captured laboratory company, with
/// its XML file, exactly as `build_import_xml` leaves one.
fn saved_batch(server: &Server) -> (ImportLedgerLine, Value) {
    let origin = super::super::super::canonical_loopback_origin(&server.settings.endpoint).unwrap();
    let mut line: ImportLedgerLine = serde_json::from_value(json!({
        "batch_id":"bridge-00000000-0000-4000-8000-000000000583", "identity_scheme":"batch_v1",
        "company_guid":GUID,
        "endpoint_origin":origin,
        "company":{"name":"WR2 Unicode Lab","guid":GUID,"company_number":"100004","books_from":"20260401"},
        "txn_ids":["journal-583"],"date_from":"20260901","date_to":"20260901",
        "sha256":"", "built_at":"2026-09-22T00:00:00Z", "status":"built",
        "pre_import_mark":{"kind":"company_high_water","value":10,"master_value":7},
        "vouchers":[{"bridge_txn_id":"journal-583","date":"20260901","voucher_type":"Journal",
            "narration":"Synthetic test only","entries":[
                {"ledger":"WR2 Sales","amount":"12.50","side":"Dr"},
                {"ledger":"Cash","amount":"12.50","side":"Cr"}]}]
    }))
    .unwrap();
    let rendered = render_import_xml("WR2 Unicode Lab", &line.vouchers, &line.batch_id);
    line.sha256 = sha256_hex(rendered.as_bytes());
    server.append_import_ledger(&line).unwrap();
    fs::write(
        server
            .imports_dir()
            .unwrap()
            .join(format!("{}.xml", line.batch_id)),
        rendered,
    )
    .unwrap();
    let args = json!({"company_guid":GUID,"batch_id":line.batch_id});
    (line, args)
}

fn journal(directory: &std::path::Path) -> Vec<u8> {
    fs::read(directory.join("agent-import-ledger.jsonl")).unwrap()
}

/// The `record_type` of every record appended to the journal since `before`.
/// The pre-post absence check writes one `verification_status` record; only a
/// dispatch writes `dispatch_intent` or `dispatch_response`.
fn appended_kinds(before: &[u8], after: &[u8]) -> Vec<String> {
    assert!(after.starts_with(before), "the journal is append-only");
    String::from_utf8(after[before.len()..].to_vec())
        .unwrap()
        .lines()
        .map(|line| {
            serde_json::from_str::<Value>(line).unwrap()["record_type"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect()
}

/// `plans` and one more, so a request past the last expected one is served
/// and observed rather than refused by a simulator that has stopped listening.
fn with_sentinel(mut plans: Vec<ScenarioPlan>) -> Vec<ScenarioPlan> {
    plans.push(status());
    plans
}

/// Data requests a run actually made (a cancel's wake-up connection has no method).
fn sent(simulator: SequenceSimulator) -> Vec<tally_protocol_simulator::ObservedRequest> {
    simulator.cancel();
    simulator
        .finish()
        .unwrap()
        .into_iter()
        .filter(|request| !request.method.is_empty())
        .collect()
}

/// With no scripted decision in scope the test build's approval declines at
/// once and starts no process: no request follows the pre-approval reads, no
/// intent is journaled, and the journal is byte-identical.
#[tokio::test]
async fn an_unscripted_approval_declines_and_nothing_is_sent_or_journaled() {
    let expected = before_approval().len();
    let simulator = SequenceSimulator::spawn(with_sentinel(before_approval())).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let server = server_at(simulator.address(), directory.path());
    let (_, args) = saved_batch(&server);
    let before = journal(directory.path());
    let response = server.call_tool("post_import", args).await;
    let observed = sent(simulator);
    let result = &response["structuredContent"]["result"];
    assert_eq!(
        result["error"]["code"], "import_approval_declined",
        "{response}"
    );
    assert_ne!(result["attempt_recorded"], json!(true), "{response}");
    assert_eq!(observed.len(), expected);
    assert_eq!(
        appended_kinds(&before, &journal(directory.path())),
        ["verification_status"]
    );
}

/// Approved, the post sends exactly the request its dispatch intent recorded:
/// the POST's body hashes to the journal's `native_request_sha256`, and the
/// code's own renderer, given the journal's `native_remote_id`, produces those
/// same bytes. The approval was asked about the preview the real dialog shows.
#[tokio::test]
async fn an_approved_post_sends_exactly_the_request_its_intent_recorded() {
    let mut plans = before_approval();
    let post_at = plans.len() + after_approval(xml(created_one())).len() - 1;
    plans.extend(after_approval(xml(created_one())));
    let simulator = SequenceSimulator::spawn(plans).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let server = server_at(simulator.address(), directory.path());
    let (line, args) = saved_batch(&server);
    let scripted = ScriptedApproval::approving();
    let response = SCRIPTED_APPROVAL
        .scope(scripted.clone(), server.call_tool("post_import", args))
        .await;
    let observed = sent(simulator);
    assert!(observed.len() > post_at, "{response}");

    let intent = dispatch_intent(directory.path());
    let recorded_sha = intent["native_request_sha256"].as_str().unwrap();
    let recorded_id = intent["native_remote_id"].as_str().unwrap();
    assert_eq!(observed[post_at].request_body_sha256, recorded_sha);
    let remote_id = Uuid::parse_str(recorded_id).unwrap();
    let rendered = native_post_request(&line, remote_id).unwrap();
    assert_eq!(rendered.request_sha256, recorded_sha);
    assert!(rendered
        .xml
        .contains(&format!("<VOUCHER REMOTEID=\"{recorded_id}\"")));
    // The renderer is a function of the batch and the REMOTEID alone: the same
    // REMOTEID renders the same bytes, and another REMOTEID different ones, so
    // the match above could not come from anything else in the request.
    assert_eq!(
        native_post_request(&line, remote_id).unwrap().xml,
        rendered.xml
    );
    assert_ne!(
        native_post_request(&line, Uuid::new_v4())
            .unwrap()
            .request_sha256,
        recorded_sha
    );
    assert_eq!(
        scripted.previews(),
        [admit_fresh_saved_voucher(&line, &server.settings.endpoint).unwrap()]
    );
}

/// Declined, the post sends nothing past the pre-approval reads and journals
/// no intent; the approval was asked once.
#[tokio::test]
async fn a_declined_post_sends_nothing_and_journals_no_intent() {
    let expected = before_approval().len();
    let simulator = SequenceSimulator::spawn(with_sentinel(before_approval())).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let server = server_at(simulator.address(), directory.path());
    let (line, args) = saved_batch(&server);
    let before = journal(directory.path());
    let scripted = ScriptedApproval::declining();
    let response = SCRIPTED_APPROVAL
        .scope(scripted.clone(), server.call_tool("post_import", args))
        .await;
    let observed = sent(simulator);
    let result = &response["structuredContent"]["result"];
    assert_eq!(
        result["error"]["code"], "import_approval_declined",
        "{response}"
    );
    assert_ne!(result["attempt_recorded"], json!(true), "{response}");
    assert_eq!(observed.len(), expected);
    assert_eq!(
        appended_kinds(&before, &journal(directory.path())),
        ["verification_status"]
    );
    assert_eq!(
        scripted.previews(),
        [admit_fresh_saved_voucher(&line, &server.settings.endpoint).unwrap()]
    );
}

/// The dispatch intent is in the journal before the POST is received. The
/// simulator holds the POST's response for three seconds; while it is held,
/// with the POST read and nothing answered, the journal already carries the
/// intent. This proves an append-before-send order, not the fsync itself.
#[tokio::test]
async fn the_dispatch_intent_is_journaled_before_the_post_is_received() {
    let held = xml(created_one()).with_delivery(Delivery::SlowHeaders(Duration::from_secs(3)));
    let mut plans = before_approval();
    let post_at = plans.len() + after_approval(held.clone()).len() - 1;
    plans.extend(after_approval(held));
    let simulator = SequenceSimulator::spawn(plans).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let server = server_at(simulator.address(), directory.path());
    let (_, args) = saved_batch(&server);
    let before = journal(directory.path());
    let post = SCRIPTED_APPROVAL.scope(
        ScriptedApproval::approving(),
        server.call_tool("post_import", args),
    );
    let watch = async {
        let started = std::time::Instant::now();
        while simulator.received() <= post_at {
            assert!(
                started.elapsed() < Duration::from_secs(20),
                "the POST never arrived"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        appended_kinds(&before, &journal(directory.path()))
    };
    let (response, while_held) = tokio::join!(post, watch);
    assert_eq!(
        while_held.last().map(String::as_str),
        Some("dispatch_intent"),
        "{response}"
    );
    assert!(!while_held.iter().any(|kind| kind == "dispatch_response"));
    let observed = sent(simulator);
    assert!(observed.len() > post_at);
}

/// A dispatch admission that fails stops the send. While the approval is
/// pending the batch's record changes, so the admission that journals the
/// intent refuses it: no intent is written and nothing follows the absence
/// reads. The POST's plan, and one after it, stay in the sequence, so a post
/// that sent anyway would be served and observed here, not refused unseen.
#[tokio::test]
async fn a_dispatch_admission_that_fails_sends_nothing() {
    let mut plans = before_approval();
    let expected = plans.len() + after_approval(xml(created_one())).len() - 1;
    plans.extend(after_approval(xml(created_one())));
    let simulator = SequenceSimulator::spawn(with_sentinel(plans)).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let server = server_at(simulator.address(), directory.path());
    let (line, args) = saved_batch(&server);
    let mut changed = line.clone();
    changed.sha256 = "0".repeat(64);
    let data_dir = directory.path().to_path_buf();
    let endpoint = server.settings.endpoint.clone();
    let scripted = ScriptedApproval::approving_after(move || {
        let other = Server::new(crate::agent::Settings {
            endpoint: endpoint.clone(),
            data_dir: data_dir.clone(),
            max_rows: 10,
            max_bytes: 200_000,
            redaction: crate::agent::Redaction::None,
            import_enabled: true,
            writes_enabled: true,
        });
        other.append_import_ledger(&changed).unwrap();
    });
    let response = SCRIPTED_APPROVAL
        .scope(scripted, server.call_tool("post_import", args))
        .await;
    let observed = sent(simulator);
    assert_eq!(observed.len(), expected, "{response}");
    assert!(!String::from_utf8(journal(directory.path()))
        .unwrap()
        .contains("\"dispatch_intent\""));
    assert_ne!(
        response["structuredContent"]["result"]["attempt_recorded"],
        json!(true),
        "{response}"
    );
}

// ADR 0004 as amended 2026-09-22: one Payment, Receipt or Contra posts under
// every Journal safeguard, and its legs are classified again from the ledgers'
// parents and the group tree before approval and inside the queue.

const BANK_BATCH: &str = "bridge-00000000-0000-4000-8000-000000000466";

fn groups() -> String {
    captured(include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-party-groups.utf16le.xml"
    ))
}

/// `body` with the one occurrence of `from` replaced by `to`.
fn replaced_once(body: &str, from: &str, to: &str) -> String {
    assert_eq!(body.matches(from).count(), 1, "{from} must occur once");
    body.replace(from, to)
}

/// The counterparty ledger moved directly under a cash group.
fn catalogue_with_debtor_under_cash() -> String {
    replaced_once(
        &catalogue(),
        ">Bridge Nested Debtors WR4</PARENT>",
        ">Cash-in-Hand</PARENT>",
    )
}

/// The counterparty ledger's own group moved under Bank Accounts; the ledger
/// row itself is byte-identical.
fn groups_with_debtor_group_under_bank() -> String {
    replaced_once(
        &groups(),
        ">Sundry Debtors</PARENT>",
        ">Bank Accounts</PARENT>",
    )
}

/// A second money ledger, for a Contra: `WR2 Sales` read as a bank account in
/// every read of the run.
fn catalogue_with_sales_as_bank() -> String {
    replaced_once(
        &catalogue(),
        ">Sales Accounts</PARENT>",
        ">Bank Accounts</PARENT>",
    )
}

/// `before_approval`, for a bank voucher: the post's classification reads the
/// group collection after the catalogue, before the qualified mode.
fn bank_before_approval(catalogue: String, groups: String) -> Vec<ScenarioPlan> {
    let mut plans = Vec::new();
    plans.extend(probe());
    plans.extend(verified_company());
    plans.extend(paired(marks()));
    plans.extend(paired(empty_collection()));
    plans.extend(paired(empty_collection()));
    plans.extend(probe());
    plans.extend(verified_company());
    plans.extend(paired(catalogue));
    plans.extend(paired(groups));
    plans.extend(probe());
    plans
}

/// `after_approval`, for a bank voucher: the queue re-reads the group
/// collection right after the catalogue, inside the same admission brackets.
fn bank_after_approval(catalogue: String, groups: String, post: ScenarioPlan) -> Vec<ScenarioPlan> {
    let mut plans = probe();
    plans.push(xml(companies()));
    plans.extend(paired(catalogue));
    plans.extend(paired(groups));
    plans.extend(probe());
    plans.push(xml(companies()));
    plans.extend(paired(empty_collection()));
    plans.extend(paired(empty_collection()));
    plans.push(marks_before());
    plans.push(post);
    plans
}

/// A built, never-dispatched single-voucher bank batch, as `build_import_xml`
/// leaves one.
fn saved_bank_batch(server: &Server, voucher: Value) -> (ImportLedgerLine, Value) {
    let origin = super::super::super::canonical_loopback_origin(&server.settings.endpoint).unwrap();
    let mut line: ImportLedgerLine = serde_json::from_value(json!({
        "batch_id":BANK_BATCH, "identity_scheme":"batch_v1",
        "company_guid":GUID,
        "endpoint_origin":origin,
        "company":{"name":"WR2 Unicode Lab","guid":GUID,"company_number":"100004","books_from":"20260401"},
        "txn_ids":[voucher["bridge_txn_id"].clone()],"date_from":"20260901","date_to":"20260901",
        "sha256":"", "built_at":"2026-09-22T00:00:00Z", "status":"built",
        "pre_import_mark":{"kind":"company_high_water","value":10,"master_value":7},
        "vouchers":[voucher]
    }))
    .unwrap();
    let rendered = render_import_xml("WR2 Unicode Lab", &line.vouchers, &line.batch_id);
    line.sha256 = sha256_hex(rendered.as_bytes());
    server.append_import_ledger(&line).unwrap();
    fs::write(
        server
            .imports_dir()
            .unwrap()
            .join(format!("{}.xml", line.batch_id)),
        rendered,
    )
    .unwrap();
    let args = json!({"company_guid":GUID,"batch_id":line.batch_id});
    (line, args)
}

fn payment() -> Value {
    json!({"bridge_txn_id":"payment-466","date":"20260901","voucher_type":"Payment",
        "narration":"Synthetic test only","entries":[
            {"ledger":"Bridge Nested Debtor WR4","amount":"12.50","side":"Dr"},
            {"ledger":"Cash","amount":"12.50","side":"Cr"}]})
}

fn receipt() -> Value {
    json!({"bridge_txn_id":"receipt-466","date":"20260901","voucher_type":"Receipt",
        "narration":"Synthetic test only","entries":[
            {"ledger":"Cash","amount":"7.50","side":"Dr"},
            {"ledger":"Bridge Nested Debtor WR4","amount":"7.50","side":"Cr"}]})
}

fn contra() -> Value {
    json!({"bridge_txn_id":"contra-466","date":"20260901","voucher_type":"Contra",
        "narration":"Synthetic test only","entries":[
            {"ledger":"WR2 Sales","amount":"5.00","side":"Dr"},
            {"ledger":"Cash","amount":"5.00","side":"Cr"}]})
}

/// Each bank type posts through the tool call exactly as a Journal does: the
/// POST is the request its intent recorded, rendered by the code's own
/// renderer from the recorded REMOTEID, and the approval was asked about a
/// preview that names the type and the classification it relies on.
#[tokio::test]
async fn each_bank_type_posts_the_request_its_intent_recorded() {
    for (voucher, catalogue, type_name, rule) in [
        (
            payment(),
            catalogue(),
            "Payment",
            "every Cr ledger is bank/cash; every Dr ledger holds no money",
        ),
        (
            receipt(),
            catalogue(),
            "Receipt",
            "every Dr ledger is bank/cash; every Cr ledger holds no money",
        ),
        (
            contra(),
            catalogue_with_sales_as_bank(),
            "Contra",
            "every ledger is bank/cash",
        ),
    ] {
        let mut plans = bank_before_approval(catalogue.clone(), groups());
        let after = bank_after_approval(catalogue.clone(), groups(), xml(created_one()));
        let post_at = plans.len() + after.len() - 1;
        plans.extend(after);
        let simulator = SequenceSimulator::spawn(plans).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let server = server_at(simulator.address(), directory.path());
        let (line, args) = saved_bank_batch(&server, voucher);
        let scripted = ScriptedApproval::approving();
        let response = SCRIPTED_APPROVAL
            .scope(scripted.clone(), server.call_tool("post_import", args))
            .await;
        let observed = sent(simulator);
        assert_eq!(observed.len(), post_at + 1, "{type_name}: {response}");

        let intent = dispatch_intent(directory.path());
        let recorded_sha = intent["native_request_sha256"].as_str().unwrap();
        let remote_id = Uuid::parse_str(intent["native_remote_id"].as_str().unwrap()).unwrap();
        assert_eq!(
            observed[post_at].request_body_sha256, recorded_sha,
            "{type_name}"
        );
        let rendered = native_post_request(&line, remote_id).unwrap();
        assert_eq!(rendered.request_sha256, recorded_sha, "{type_name}");
        assert!(
            rendered.xml.contains(&format!("VCHTYPE=\"{type_name}\"")),
            "{type_name}: {}",
            rendered.xml
        );
        let previews = scripted.previews();
        assert_eq!(
            previews,
            [admit_fresh_saved_voucher(&line, &server.settings.endpoint).unwrap()]
        );
        assert!(
            previews[0].starts_with(&format!("Create ONE {type_name} in ")),
            "{}",
            previews[0]
        );
        assert!(
            previews[0].contains(&format!("Checked in Tally: {rule}.")),
            "{}",
            previews[0]
        );
    }
}

fn three_entry_receipt() -> Value {
    json!({"bridge_txn_id":"receipt-3-466","date":"20260901","voucher_type":"Receipt",
        "narration":"Synthetic test only","entries":[
            {"ledger":"Cash","amount":"3.00","side":"Dr"},
            {"ledger":"Bridge Nested Debtor WR4","amount":"1.00","side":"Cr"},
            {"ledger":"Café Naïve Traders","amount":"2.00","side":"Cr"}]})
}

/// The SECOND counterparty of the three-entry Receipt moved under a cash group;
/// the first counterparty's row is byte-identical.
fn catalogue_with_second_counterparty_under_cash() -> String {
    let body = catalogue();
    let start = body
        .find("<LEDGER NAME=\"Café Naïve Traders\"")
        .expect("ledger row");
    let end = start + body[start..].find("</LEDGER>").expect("row end");
    let row = &body[start..end];
    let moved = replaced_once(row, ">Sundry Debtors</PARENT>", ">Cash-in-Hand</PARENT>");
    format!("{}{}{}", &body[..start], moved, &body[end..])
}

/// A multi-entry bank voucher (bridge#466, #585) posts like any other: the
/// preview lists every entry, and the POST is the recorded request.
#[tokio::test]
async fn a_three_entry_receipt_posts_the_request_its_intent_recorded() {
    let mut plans = bank_before_approval(catalogue(), groups());
    let after = bank_after_approval(catalogue(), groups(), xml(created_one()));
    let post_at = plans.len() + after.len() - 1;
    plans.extend(after);
    let simulator = SequenceSimulator::spawn(plans).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let server = server_at(simulator.address(), directory.path());
    let (line, args) = saved_bank_batch(&server, three_entry_receipt());
    let scripted = ScriptedApproval::approving();
    let response = SCRIPTED_APPROVAL
        .scope(scripted.clone(), server.call_tool("post_import", args))
        .await;
    let observed = sent(simulator);
    assert_eq!(observed.len(), post_at + 1, "{response}");
    let intent = dispatch_intent(directory.path());
    let recorded_sha = intent["native_request_sha256"].as_str().unwrap();
    assert_eq!(observed[post_at].request_body_sha256, recorded_sha);
    let remote_id = Uuid::parse_str(intent["native_remote_id"].as_str().unwrap()).unwrap();
    assert_eq!(
        native_post_request(&line, remote_id)
            .unwrap()
            .request_sha256,
        recorded_sha
    );
    let previews = scripted.previews();
    assert_eq!(previews.len(), 1);
    for entry in [
        "Dr 3.00  \"Cash\"",
        "Cr 1.00  \"Bridge Nested Debtor WR4\"",
        "Cr 2.00  \"Café Naïve Traders\"",
    ] {
        assert!(previews[0].contains(entry), "{entry}: {}", previews[0]);
    }
}

/// Every leg is classified again in the queue, not only the first on each
/// side: the second counterparty turning into money refuses the post.
#[tokio::test]
async fn a_second_counterparty_moved_under_cash_after_approval_is_refused() {
    let mut plans = bank_before_approval(catalogue(), groups());
    let after = bank_after_approval(
        catalogue_with_second_counterparty_under_cash(),
        groups(),
        xml(created_one()),
    );
    let expected = plans.len() + after.len() - 1;
    plans.extend(after);
    let simulator = SequenceSimulator::spawn(with_sentinel(plans)).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let server = server_at(simulator.address(), directory.path());
    let (_, args) = saved_bank_batch(&server, three_entry_receipt());
    let response = SCRIPTED_APPROVAL
        .scope(
            ScriptedApproval::approving(),
            server.call_tool("post_import", args),
        )
        .await;
    let observed = sent(simulator);
    assert_eq!(
        response["structuredContent"]["result"]["error"]["code"],
        "import_bank_classification_changed",
        "{response}"
    );
    assert_eq!(observed.len(), expected, "{response}");
    assert!(!String::from_utf8(journal(directory.path()))
        .unwrap()
        .contains("\"dispatch_intent\""));
}

/// Refused inside the queue, after approval: no intent, and nothing past the
/// queued reads. The POST's plan and one after it stay in the sequence, so a
/// post that went ahead would be served and observed here.
async fn refused_in_the_queue(queued_catalogue: String, queued_groups: String) {
    let mut plans = bank_before_approval(catalogue(), groups());
    let after = bank_after_approval(queued_catalogue, queued_groups, xml(created_one()));
    let expected = plans.len() + after.len() - 1;
    plans.extend(after);
    let simulator = SequenceSimulator::spawn(with_sentinel(plans)).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let server = server_at(simulator.address(), directory.path());
    let (_, args) = saved_bank_batch(&server, payment());
    let before = journal(directory.path());
    let scripted = ScriptedApproval::approving();
    let response = SCRIPTED_APPROVAL
        .scope(scripted.clone(), server.call_tool("post_import", args))
        .await;
    let observed = sent(simulator);
    let result = &response["structuredContent"]["result"];
    assert_eq!(
        result["error"]["code"], "import_bank_classification_changed",
        "{response}"
    );
    assert_eq!(scripted.previews().len(), 1, "approval was asked once");
    assert_eq!(observed.len(), expected, "{response}");
    assert_eq!(
        appended_kinds(&before, &journal(directory.path())),
        ["verification_status"]
    );
}

/// A ledger re-parented after approval keeps its name and GUID, so the
/// catalogue binding still matches; only the classification sees it.
#[tokio::test]
async fn a_counterparty_moved_under_cash_after_approval_is_refused_before_the_post() {
    refused_in_the_queue(catalogue_with_debtor_under_cash(), groups()).await;
}

/// A group re-parented after approval changes no ledger row at all.
#[tokio::test]
async fn a_counterparty_group_moved_under_bank_after_approval_is_refused_before_the_post() {
    refused_in_the_queue(catalogue(), groups_with_debtor_group_under_bank()).await;
}

/// Already changed since the build: refused before approval is asked, and no
/// request follows the classification reads.
#[tokio::test]
async fn a_classification_changed_since_the_build_is_refused_before_approval() {
    for (catalogue, groups) in [
        (catalogue_with_debtor_under_cash(), groups()),
        (catalogue(), groups_with_debtor_group_under_bank()),
    ] {
        let mut plans = bank_before_approval(catalogue, groups);
        // The qualified mode probe after the group read is never sent.
        plans.truncate(plans.len() - probe().len());
        let expected = plans.len();
        let simulator = SequenceSimulator::spawn(with_sentinel(plans)).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let server = server_at(simulator.address(), directory.path());
        let (_, args) = saved_bank_batch(&server, payment());
        let scripted = ScriptedApproval::approving();
        let response = SCRIPTED_APPROVAL
            .scope(scripted.clone(), server.call_tool("post_import", args))
            .await;
        let observed = sent(simulator);
        assert_eq!(
            response["structuredContent"]["result"]["error"]["code"],
            "import_bank_classification_changed",
            "{response}"
        );
        assert!(scripted.previews().is_empty(), "approval must not be asked");
        assert_eq!(observed.len(), expected, "{response}");
        assert!(!String::from_utf8(journal(directory.path()))
            .unwrap()
            .contains("\"dispatch_intent\""));
    }
}

/// The desktop's post stays Journal-only: the same saved Payment the agent can
/// post is refused before any request.
#[tokio::test]
async fn the_desktop_scope_refuses_a_bank_voucher_before_any_request() {
    let simulator = SequenceSimulator::spawn(with_sentinel(Vec::new())).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let server = server_at(simulator.address(), directory.path());
    let (line, args) = saved_bank_batch(&server, payment());
    let refused = server
        .post_import_checked(&args, Some(&line.sha256), PostScope::JournalOnly)
        .await
        .expect("a refusal is reported as the post's outcome");
    assert_eq!(
        refused.payload["result"]["error"]["code"], "import_post_requires_one_journal",
        "{}",
        refused.payload
    );
    assert!(sent(simulator).is_empty());
}

// bridge#574: the aim is confirmed on the snapshot sent last before the POST,
// and where the voucher went is read straight after it.

/// The approved Journal post, with `marks` in place of the snapshot sent last
/// before the POST. Returns the response, the requests observed, and the
/// number of requests up to and including that snapshot.
async fn post_with_marks_before(marks: String) -> (Value, usize, usize, Vec<u8>, Vec<u8>) {
    let mut plans = before_approval();
    let mut after = after_approval(xml(created_one()));
    let marks_at = after.len() - 2;
    after[marks_at] = xml(marks);
    let through_marks = plans.len() + marks_at + 1;
    plans.extend(after);
    let simulator = SequenceSimulator::spawn(with_sentinel(plans)).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let server = server_at(simulator.address(), directory.path());
    let (_, args) = saved_batch(&server);
    let before = journal(directory.path());
    let response = SCRIPTED_APPROVAL
        .scope(
            ScriptedApproval::approving(),
            server.call_tool("post_import", args),
        )
        .await;
    let observed = sent(simulator).len();
    (
        response,
        observed,
        through_marks,
        before,
        journal(directory.path()),
    )
}

#[tokio::test]
async fn another_company_renamed_to_the_target_refuses_before_the_post() {
    // The residual #574 case: another loaded company now carries the target's
    // name, so a post named by SVCURRENTCOMPANY could land in it.
    let namesake =
        company_marks(10, 50, "WR2 Unicode Lab").replace("Synthetic Other Lab", "WR2 UNICODE LAB");
    let (response, observed, through_marks, before, after) = post_with_marks_before(namesake).await;
    assert_eq!(
        response["structuredContent"]["result"]["error"]["code"], "post_company_scope_changed",
        "{response}"
    );
    assert_eq!(
        observed, through_marks,
        "nothing after the snapshot: {response}"
    );
    assert_eq!(appended_kinds(&before, &after), ["verification_status"]);
}

#[tokio::test]
async fn the_target_renamed_since_admission_refuses_before_the_post() {
    let renamed = company_marks(10, 50, "WR2 Unicode Lab Renamed");
    let (response, observed, through_marks, before, after) = post_with_marks_before(renamed).await;
    assert_eq!(
        response["structuredContent"]["result"]["error"]["code"], "post_company_scope_changed",
        "{response}"
    );
    assert_eq!(observed, through_marks, "{response}");
    assert_eq!(appended_kinds(&before, &after), ["verification_status"]);
}

#[tokio::test]
async fn an_unreadable_snapshot_refuses_before_the_post() {
    // T2's live answer to an unmatched SVCURRENTCOMPANY.
    let refused = "<ENVELOPE><HEADER><VERSION>1</VERSION><STATUS>0</STATUS></HEADER><BODY><DATA>\
                   <LINEERROR>Could not set 'SVCurrentCompany' to 'WR2 Unicode Lab'</LINEERROR>\
                   </DATA></BODY></ENVELOPE>"
        .to_string();
    let (response, observed, through_marks, before, after) = post_with_marks_before(refused).await;
    assert_eq!(
        response["structuredContent"]["result"]["error"]["code"], "post_company_scope_unconfirmed",
        "{response}"
    );
    assert_eq!(observed, through_marks, "{response}");
    assert_eq!(appended_kinds(&before, &after), ["verification_status"]);
}

/// After the POST, the snapshot says which companies' voucher marks moved,
/// and the result carries it even when the readback that follows fails.
async fn located_after(marks_after: String) -> Value {
    let mut plans = before_approval();
    plans.extend(after_approval(xml(created_one())));
    plans.push(xml(marks_after));
    let simulator = SequenceSimulator::spawn(plans).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let server = server_at(simulator.address(), directory.path());
    let (_, args) = saved_batch(&server);
    let response = SCRIPTED_APPROVAL
        .scope(
            ScriptedApproval::approving(),
            server.call_tool("post_import", args),
        )
        .await;
    let _ = sent(simulator);
    assert_eq!(
        dispatch_intent(directory.path())["record_type"],
        "dispatch_intent"
    );
    response["structuredContent"]["result"]["post_location"].clone()
}

#[tokio::test]
async fn only_the_target_moving_is_reported_as_the_landing() {
    let located = located_after(company_marks(11, 50, "WR2 Unicode Lab")).await;
    assert_eq!(located["state"], "target_only", "{located}");
}

#[tokio::test]
async fn another_company_moving_instead_is_named_in_the_result() {
    let located = located_after(company_marks(10, 51, "WR2 Unicode Lab")).await;
    assert_eq!(located["state"], "suspected_other_company", "{located}");
    assert_eq!(
        located["other_companies_moved"][0]["name"],
        "Synthetic Other Lab"
    );
}
