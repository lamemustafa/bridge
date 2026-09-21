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
        [admit_fresh_saved_journal(&line, &server.settings.endpoint).unwrap()]
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
        [admit_fresh_saved_journal(&line, &server.settings.endpoint).unwrap()]
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
