//! bridge#583: the native post driven end to end through the `post_import`
//! tool call against the protocol simulator, the approval answered by the
//! test-only seam (`approved_import::test_seam`). No real Tally is involved.
use super::*;
use super::{SCRIPTED_REMOTE_ID, SCRIPTED_REMOTE_IDS};
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

/// The captured Currency masters of a book with exactly one (`I₹`).
fn single_currency() -> String {
    captured(include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/currency_inr_modern_live.utf16le.xml"
    ))
}

/// The captured Currency masters of a book with two (`$` and the base).
fn two_currencies() -> String {
    captured(include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/currency_multi_live.utf16le.xml"
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

/// The same marks, read as the queue's binding reads begin (#239): the aim
/// snapshot must show the target's master mark unchanged since.
fn marks_at_binding() -> ScenarioPlan {
    marks_before()
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
    before_approval_with_currencies(single_currency())
}

fn before_approval_with_currencies(currencies: String) -> Vec<ScenarioPlan> {
    let mut plans = Vec::new();
    // verify_import_for_post: opening mode, identity, the window's mark, the
    // window, its replay, and the closing mode an absence needs.
    plans.extend(probe());
    plans.extend(verified_company());
    plans.extend(paired(marks()));
    plans.extend(paired(empty_collection()));
    plans.extend(paired(empty_collection()));
    plans.extend(probe());
    // The post's own identity, ledger catalogue, Currency masters and
    // qualified mode.
    plans.extend(verified_company());
    plans.extend(paired(catalogue()));
    plans.extend(paired(currencies));
    plans.extend(probe());
    plans
}

/// Tally's answer to one created voucher.
/// Tally's answer to one created voucher: a captured live response
/// (licensed-lab import, sanitized), not a hand-written shape. The earlier
/// hand-written ENVELOPE did not parse as an import outcome at all.
fn created_one() -> String {
    include_str!(
        "../crates/bridge-tally-protocol/tests/fixtures/live_education_w4_voucher_sanitized.xml"
    )
    .to_string()
}

/// What the dispatch sends after approval, up to and including the import:
/// the opening mode and company admission, the marks at binding, the ledger
/// catalogue, the Currency
/// masters, the closing mode and admission, the absence read twice, the aim
/// snapshot, then the one POST. The index of the POST is
/// `before_approval().len() + after_approval(..).len() - 1`.
fn after_approval(post: ScenarioPlan) -> Vec<ScenarioPlan> {
    after_approval_with_currencies(single_currency(), post)
}

fn after_approval_with_currencies(currencies: String, post: ScenarioPlan) -> Vec<ScenarioPlan> {
    let mut plans = probe();
    plans.push(xml(companies()));
    plans.push(marks_at_binding());
    plans.extend(paired(catalogue()));
    plans.extend(paired(currencies));
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

/// The one dispatch response a run journaled, with Tally's answer parsed.
/// A POST answered with `created_one()` must journal a parsed, clean single
/// create; `None` here means the post ran without a parsed outcome.
fn journaled_outcome(
    directory: &std::path::Path,
) -> Option<bridge_tally_protocol::TallyImportOutcome> {
    let responses = String::from_utf8(journal(directory))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|record| record["record_type"] == "dispatch_response")
        .collect::<Vec<_>>();
    assert_eq!(responses.len(), 1, "{responses:?}");
    serde_json::from_value::<ledger::DispatchResponse>(responses[0]["response"].clone())
        .unwrap()
        .outcome
}

fn assert_journaled_clean_create(directory: &std::path::Path) {
    let outcome = journaled_outcome(directory).expect("the POST answer was parsed and journaled");
    assert_eq!(outcome.counters().created, 1);
    assert!(import_outcome_is_clean(Some(&outcome), 1));
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
        batch_post_enabled: false,
    })
}

/// Record the build's ledger binding as `build_import_xml` does (#239): each
/// named ledger with the GUID the captured catalogue gives it.
fn bind_to_captured_catalogue(line: &mut ImportLedgerLine) {
    let payload = ImportPayload {
        company_guid: line.company_guid.clone(),
        vouchers: line.vouchers.clone(),
        amends_batch_id: None,
    };
    let binding = bridge_tally_protocol::parse_standard_ledger_catalog_with_identities(
        &catalogue(),
        "WR2 Unicode Lab",
        GUID,
    )
    .unwrap()
    .bind_selected(requested_ledger_names(&payload))
    .unwrap();
    line.ledger_identities = Some(
        binding
            .pairs()
            .map(|(name, guid)| BoundLedger {
                name: name.to_string(),
                guid: guid.to_string(),
            })
            .collect(),
    );
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
    bind_to_captured_catalogue(&mut line);
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

/// Another batch, already dispatched with `remote_id`, in the journal.
fn journal_an_earlier_intent(server: &Server, line: &ImportLedgerLine, remote_id: Uuid) {
    let mut earlier = line.clone();
    earlier.batch_id = "bridge-00000000-0000-4000-8000-000000000584".into();
    server.append_import_ledger(&earlier).unwrap();
    let _lock = server.lock_import_admission().unwrap();
    server
        .append_import_record_while_admitted(&ledger::StatusRecord::dispatch_native(
            &earlier,
            "c".repeat(64),
            remote_id,
        ))
        .unwrap();
}

/// A REMOTEID the journal already records is never sent again, since a resend
/// undoes a person's cancel or delete (protocol reference §9.3). The post is
/// refused from the journal alone: no Tally request, nothing appended.
#[tokio::test]
async fn a_remoteid_the_journal_records_is_refused_before_any_tally_request() {
    let simulator = SequenceSimulator::spawn(with_sentinel(Vec::new())).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let server = server_at(simulator.address(), directory.path());
    let (line, args) = saved_batch(&server);
    let reused = Uuid::new_v4();
    journal_an_earlier_intent(&server, &line, reused);
    let before = journal(directory.path());
    let response = SCRIPTED_REMOTE_ID
        .scope(
            reused,
            SCRIPTED_APPROVAL.scope(
                ScriptedApproval::approving(),
                server.call_tool("post_import", args),
            ),
        )
        .await;
    let observed = sent(simulator);
    assert_eq!(
        response["structuredContent"]["result"]["error"]["code"], "import_remote_id_reused",
        "{response}"
    );
    assert_eq!(observed.len(), 0);
    assert_eq!(journal(directory.path()), before);
}

/// While the dialog is open, another process journals an intent carrying
/// `injected`; this post mints `minted`. Returns the response, the requests
/// Tally received, where the POST would be, and the batches with an intent.
async fn race_an_intent_during_approval(
    injected: Uuid,
    minted: Uuid,
) -> (Value, usize, usize, Vec<String>) {
    let mut plans = before_approval();
    let post_at = plans.len() + after_approval(xml(created_one())).len() - 1;
    plans.extend(after_approval(xml(created_one())));
    let simulator = SequenceSimulator::spawn(with_sentinel(plans)).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let server = server_at(simulator.address(), directory.path());
    let (line, args) = saved_batch(&server);
    let mut earlier = line.clone();
    earlier.batch_id = "bridge-00000000-0000-4000-8000-000000000585".into();
    let mut appended = serde_json::to_vec(&earlier).unwrap();
    appended.push(b'\n');
    appended.extend(
        serde_json::to_vec(&ledger::StatusRecord::dispatch_native(
            &earlier,
            "c".repeat(64),
            injected,
        ))
        .unwrap(),
    );
    appended.push(b'\n');
    let path = directory.path().join("agent-import-ledger.jsonl");
    let scripted = ScriptedApproval::approving_after(move || {
        use std::io::Write;
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(&appended)
            .unwrap();
    });
    let response = SCRIPTED_REMOTE_ID
        .scope(
            minted,
            SCRIPTED_APPROVAL.scope(scripted, server.call_tool("post_import", args)),
        )
        .await;
    let observed = sent(simulator).len();
    let intents = String::from_utf8(journal(directory.path()))
        .unwrap()
        .lines()
        .map(|record| serde_json::from_str::<Value>(record).unwrap())
        .filter(|record| record["record_type"] == "dispatch_intent")
        .map(|record| record["batch_id"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    (response, observed, post_at, intents)
}

fn batch_server_at(address: std::net::SocketAddr, directory: &std::path::Path) -> Server {
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
        batch_post_enabled: true,
    })
}

/// `saved_batch`, with a second Journal on the same ledgers and day.
fn saved_batch_of_two(server: &Server) -> (ImportLedgerLine, Value) {
    let (mut line, args) = saved_batch(server);
    let mut second = line.vouchers[0].clone();
    second.bridge_txn_id = "journal-583-2".into();
    second
        .entries
        .iter_mut()
        .for_each(|entry| entry.amount = "7.25".into());
    line.vouchers.push(second);
    line.txn_ids.push("journal-583-2".into());
    let rendered = render_import_xml("WR2 Unicode Lab", &line.vouchers, &line.batch_id);
    line.sha256 = sha256_hex(rendered.as_bytes());
    bind_to_captured_catalogue(&mut line);
    server.append_import_ledger(&line).unwrap();
    fs::write(
        server
            .imports_dir()
            .unwrap()
            .join(format!("{}.xml", line.batch_id)),
        rendered,
    )
    .unwrap();
    (line, args)
}

/// A batch of two, while another process records `injected` in an intent
/// during approval; the post mints `minted`.
async fn race_a_batch_id_during_approval(
    injected: Uuid,
    minted: [Uuid; 2],
) -> (Value, usize, usize) {
    let mut plans = before_approval();
    let post_at = plans.len() + after_approval(xml(created_one())).len() - 1;
    plans.extend(after_approval(xml(created_one())));
    let simulator = SequenceSimulator::spawn(with_sentinel(plans)).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let server = batch_server_at(simulator.address(), directory.path());
    let (line, args) = saved_batch_of_two(&server);
    let mut earlier = line.clone();
    earlier.batch_id = "bridge-00000000-0000-4000-8000-000000000585".into();
    earlier.vouchers.truncate(1);
    earlier.txn_ids.truncate(1);
    let mut appended = serde_json::to_vec(&earlier).unwrap();
    appended.push(b'\n');
    appended.extend(
        serde_json::to_vec(&ledger::StatusRecord::dispatch_native(
            &earlier,
            "c".repeat(64),
            injected,
        ))
        .unwrap(),
    );
    appended.push(b'\n');
    let path = directory.path().join("agent-import-ledger.jsonl");
    let scripted = ScriptedApproval::approving_after(move || {
        use std::io::Write;
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(&appended)
            .unwrap();
    });
    let response = SCRIPTED_REMOTE_IDS
        .scope(
            minted.to_vec(),
            SCRIPTED_APPROVAL.scope(scripted, server.call_tool("post_import", args)),
        )
        .await;
    (response, sent(simulator).len(), post_at)
}

/// A batch's SECOND REMOTEID, recorded by another process while the dialog
/// is open, is caught inside the queue: no POST. The control, with an
/// unrelated id recorded instead, posts. So every id is checked, not only
/// the first.
#[tokio::test]
async fn a_batch_whose_second_remote_id_is_recorded_during_approval_is_never_sent() {
    let minted = [Uuid::new_v4(), Uuid::new_v4()];
    let (response, observed, post_at) = race_a_batch_id_during_approval(minted[1], minted).await;
    assert_eq!(observed, post_at, "{response}");
    let (response, observed, post_at) =
        race_a_batch_id_during_approval(Uuid::new_v4(), minted).await;
    assert!(
        observed > post_at,
        "the control's POST was sent: {response}"
    );
}

/// A live batch post records its step verdict durably, before the readback. Tally's captured answer reports one create for this batch of
/// two, and the target's mark moves by two: the step doubt is recorded, and
/// the batch is not verified.
#[tokio::test]
async fn a_batch_post_records_its_step_verdict_before_the_readback() {
    let mut plans = before_approval();
    plans.extend(after_approval(xml(created_one())));
    plans.push(xml(company_marks(12, 50, "WR2 Unicode Lab")));
    let simulator = SequenceSimulator::spawn(with_sentinel(plans)).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let server = batch_server_at(simulator.address(), directory.path());
    let (line, args) = saved_batch_of_two(&server);
    let scripted = ScriptedApproval::approving();
    let response = SCRIPTED_APPROVAL
        .scope(scripted.clone(), server.call_tool("post_import", args))
        .await;
    let _ = sent(simulator);
    // The dialog was asked about both vouchers: the count its title and
    // button name (#746), whose words the approval's unit tests check.
    assert_eq!(scripted.counts(), [2]);
    let imports = server.imports_dir().unwrap();
    let doubt: Value = serde_json::from_slice(
        &fs::read(imports.join(format!("{}.batch_step_doubt.json", line.batch_id))).unwrap(),
    )
    .unwrap();
    assert_eq!(doubt["state"], "unmatched", "{response}");
    assert_eq!(doubt["target_voucher_step"]["step"], 2, "{doubt}");
    assert_eq!(
        doubt["target_voucher_step"]["reported_created"], 1,
        "{doubt}"
    );
    assert_eq!(
        super::super::read_masters_check(&imports, &line.batch_id).unwrap()["batch_step"]["state"],
        "unmatched"
    );
    assert_ne!(
        response["structuredContent"]["result"]["dispatch"]["state"], "posted_verified",
        "{response}"
    );
}

/// With batch posting off, a batch of two is refused before any request.
#[tokio::test]
async fn a_batch_is_refused_while_batch_posting_is_off() {
    let simulator = SequenceSimulator::spawn(with_sentinel(Vec::new())).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let server = server_at(simulator.address(), directory.path());
    let (_, args) = saved_batch_of_two(&server);
    let response = SCRIPTED_APPROVAL
        .scope(
            ScriptedApproval::approving(),
            server.call_tool("post_import", args),
        )
        .await;
    assert_eq!(
        response["structuredContent"]["result"]["error"]["code"],
        "import_post_requires_one_voucher",
        "{response}"
    );
    assert!(sent(simulator).is_empty());
}

/// The same REMOTEID recorded by another process while the dialog is open is
/// caught as the intent is written: no intent for this batch, and no POST. The
/// control, an injected intent with another REMOTEID, posts: so the match is
/// what stopped the first.
#[tokio::test]
async fn a_remoteid_recorded_while_approval_is_pending_is_never_sent() {
    let raced = Uuid::new_v4();
    let (response, observed, post_at, intents) = race_an_intent_during_approval(raced, raced).await;
    // A refusal under the admission lock still reads as an unknown outcome
    // (#711), though nothing was sent: the journal and the request count show
    // that.
    assert_eq!(
        response["structuredContent"]["result"]["error"]["code"], "import_dispatch_outcome_unknown",
        "{response}"
    );
    assert_eq!(observed, post_at, "{response}");
    assert_eq!(intents, ["bridge-00000000-0000-4000-8000-000000000585"]);

    let (response, observed, post_at, intents) =
        race_an_intent_during_approval(Uuid::new_v4(), raced).await;
    assert!(
        observed > post_at,
        "the control's POST was sent: {response}"
    );
    assert_eq!(
        intents,
        [
            "bridge-00000000-0000-4000-8000-000000000585",
            "bridge-00000000-0000-4000-8000-000000000583"
        ]
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
    let company_guid = args["company_guid"].as_str().unwrap().to_string();
    let scripted = ScriptedApproval::approving();
    let response = SCRIPTED_APPROVAL
        .scope(scripted.clone(), server.call_tool("post_import", args))
        .await;
    let observed = sent(simulator);
    assert!(observed.len() > post_at, "{response}");
    // The post drops every ledger listing snapshot of its company (#630).
    let dropped = server.listings.lock().unwrap().dropped_companies().to_vec();
    assert_eq!(dropped.len(), 1, "{dropped:?}");
    assert!(
        dropped[0].eq_ignore_ascii_case(&company_guid),
        "{dropped:?}"
    );

    let intent = dispatch_intent(directory.path());
    assert_journaled_clean_create(directory.path());
    let recorded_sha = intent["native_request_sha256"].as_str().unwrap();
    let recorded_id = intent["native_remote_id"].as_str().unwrap();
    assert_eq!(observed[post_at].request_body_sha256, recorded_sha);
    let remote_id = Uuid::parse_str(recorded_id).unwrap();
    let rendered = native_post_request(&line, RemoteIds::from_ids(vec![remote_id])).unwrap();
    assert_eq!(rendered.request_sha256, recorded_sha);
    assert!(rendered
        .xml
        .contains(&format!("<VOUCHER REMOTEID=\"{recorded_id}\"")));
    // The renderer is a function of the batch and the REMOTEID alone: the same
    // REMOTEID renders the same bytes, and another REMOTEID different ones, so
    // the match above could not come from anything else in the request.
    assert_eq!(
        native_post_request(&line, RemoteIds::from_ids(vec![remote_id]))
            .unwrap()
            .xml,
        rendered.xml
    );
    assert_ne!(
        native_post_request(&line, RemoteIds::from_ids(vec![Uuid::new_v4()]))
            .unwrap()
            .request_sha256,
        recorded_sha
    );
    assert_eq!(
        scripted.previews(),
        [admit_fresh_saved_voucher(&line, &server.settings.endpoint).unwrap()]
    );
}

/// #632: an amendment is never posted natively, and this refusal is what
/// keeps the amendment compare-and-swap a build-time check. That check admits
/// a voucher whose ALTERID equals the verified baseline of *any* build in its
/// lineage, which is sound only against the read it has just made. If this
/// test ever has to change because amendments are posted, the post-time check
/// must bind the exact (GUID, MASTERID, ALTERID) the approval showed, never
/// reuse that match (see #632 for the design). Refused through the tool, under
/// an approving script: no Tally request, no approval asked, no attempt.
#[tokio::test]
async fn post_import_refuses_an_amendment_before_any_read_or_approval() {
    let simulator = SequenceSimulator::spawn(with_sentinel(Vec::new())).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let server = server_at(simulator.address(), directory.path());
    let (original, _) = saved_batch(&server);
    let mut amendment = original.clone();
    amendment.batch_id = "bridge-00000000-0000-4000-8000-000000000632".into();
    amendment.amends_batch_id = Some(original.batch_id.clone());
    amendment.vouchers[0].entries[0].amount = "13.50".into();
    amendment.vouchers[0].entries[1].amount = "13.50".into();
    let rendered = render_import_xml(
        "WR2 Unicode Lab",
        &amendment.vouchers,
        amendment.identity_batch_id(),
    );
    amendment.sha256 = sha256_hex(rendered.as_bytes());
    server.append_import_ledger(&amendment).unwrap();
    fs::write(
        server
            .imports_dir()
            .unwrap()
            .join(format!("{}.xml", amendment.batch_id)),
        rendered,
    )
    .unwrap();
    let scripted = ScriptedApproval::approving();
    let response = SCRIPTED_APPROVAL
        .scope(
            scripted.clone(),
            server.call_tool(
                "post_import",
                json!({"company_guid":GUID,"batch_id":amendment.batch_id}),
            ),
        )
        .await;
    let observed = sent(simulator);
    let result = &response["structuredContent"]["result"];
    assert_eq!(
        result["error"]["code"], "import_post_amendment_requires_file_import",
        "{response}"
    );
    assert_eq!(result["attempt_recorded"], json!(false), "{response}");
    assert!(observed.is_empty(), "no Tally request: {response}");
    assert!(scripted.previews().is_empty(), "no approval asked");
}

/// bridge#626: before approval, the post reads the catalogue again and refuses
/// a named ledger that now folds equal to another live ledger, which Tally's
/// import lookup could take for it. Refused through the tool, under an
/// approving script: no approval asked, no request after that catalogue read,
/// no intent journaled. The twin is a test-local rewrite of the capture (an
/// unrelated ledger renamed `Cash` plus CR LF), no evidence of Tally behaviour.
#[tokio::test]
async fn a_folded_twin_refuses_the_post_before_any_approval() {
    let captured = catalogue();
    assert_eq!(
        captured.matches("Bridge Nested Debtor WR4").count(),
        2,
        "name and NAME.LIST"
    );
    let twinned = captured.replace("Bridge Nested Debtor WR4", "Cash&#13;&#10;");
    let mut plans = Vec::new();
    plans.extend(probe());
    plans.extend(verified_company());
    plans.extend(paired(marks()));
    plans.extend(paired(empty_collection()));
    plans.extend(paired(empty_collection()));
    plans.extend(probe());
    plans.extend(verified_company());
    plans.extend(paired(twinned));
    let expected = plans.len();
    let simulator = SequenceSimulator::spawn(with_sentinel(plans)).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let server = server_at(simulator.address(), directory.path());
    let (_, args) = saved_batch(&server);
    let scripted = ScriptedApproval::approving();
    let response = SCRIPTED_APPROVAL
        .scope(scripted.clone(), server.call_tool("post_import", args))
        .await;
    let observed = sent(simulator);
    let result = &response["structuredContent"]["result"];
    assert_eq!(
        result["error"]["code"], "ledger_has_folded_twin",
        "{response}"
    );
    assert_ne!(result["attempt_recorded"], json!(true), "{response}");
    assert_eq!(observed.len(), expected, "nothing after the catalogue read");
    assert!(scripted.previews().is_empty(), "no approval asked");
    assert!(!String::from_utf8(journal(directory.path()))
        .unwrap()
        .contains("\"dispatch_intent\""));
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
    assert_eq!(
        scripted.counts(),
        [1],
        "the dialog is asked about one voucher"
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
    assert_journaled_clean_create(directory.path());
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
            batch_post_enabled: false,
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
    bank_before_approval_with_currencies(catalogue, groups, single_currency())
}

fn bank_before_approval_with_currencies(
    catalogue: String,
    groups: String,
    currencies: String,
) -> Vec<ScenarioPlan> {
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
    plans.extend(paired(currencies));
    plans.extend(probe());
    plans
}

/// `after_approval`, for a bank voucher: the queue re-reads the group
/// collection right after the catalogue, inside the same admission brackets.
fn bank_after_approval(catalogue: String, groups: String, post: ScenarioPlan) -> Vec<ScenarioPlan> {
    bank_after_approval_with_currencies(catalogue, groups, single_currency(), post)
}

fn bank_after_approval_with_currencies(
    catalogue: String,
    groups: String,
    currencies: String,
    post: ScenarioPlan,
) -> Vec<ScenarioPlan> {
    let mut plans = probe();
    plans.push(xml(companies()));
    plans.push(marks_at_binding());
    plans.extend(paired(catalogue));
    plans.extend(paired(groups));
    plans.extend(paired(currencies));
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
    bind_to_captured_catalogue(&mut line);
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
        assert_journaled_clean_create(directory.path());
        let recorded_sha = intent["native_request_sha256"].as_str().unwrap();
        let remote_id = Uuid::parse_str(intent["native_remote_id"].as_str().unwrap()).unwrap();
        assert_eq!(
            observed[post_at].request_body_sha256, recorded_sha,
            "{type_name}"
        );
        let rendered = native_post_request(&line, RemoteIds::from_ids(vec![remote_id])).unwrap();
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
    assert_journaled_clean_create(directory.path());
    let recorded_sha = intent["native_request_sha256"].as_str().unwrap();
    assert_eq!(observed[post_at].request_body_sha256, recorded_sha);
    let remote_id = Uuid::parse_str(intent["native_remote_id"].as_str().unwrap()).unwrap();
    assert_eq!(
        native_post_request(&line, RemoteIds::from_ids(vec![remote_id]))
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

/// bridge#676: a group collection the classification cannot parse is refused
/// before approval as `group_export_invalid`, and its `cause` is the group
/// parser's own data-free code, not dropped. Nothing is read after it.
async fn refused_on_the_group_read(groups: String, cause: &str) {
    let mut plans = probe();
    plans.extend(verified_company());
    plans.extend(paired(marks()));
    plans.extend(paired(empty_collection()));
    plans.extend(paired(empty_collection()));
    plans.extend(probe());
    plans.extend(verified_company());
    plans.extend(paired(catalogue()));
    plans.extend(paired(groups));
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
    let error = &response["structuredContent"]["result"]["error"];
    assert_eq!(error["code"], "group_export_invalid", "{response}");
    assert_eq!(error["cause"], cause, "{response}");
    assert!(scripted.previews().is_empty(), "no approval asked");
    assert_eq!(observed.len(), expected, "{response}");
    assert!(!String::from_utf8(journal(directory.path()))
        .unwrap()
        .contains("\"dispatch_intent\""));
}

#[tokio::test]
async fn a_group_collection_of_another_company_is_refused_with_its_cause() {
    let groups = groups();
    let other = groups.replacen(
        ">61c6de69-1748-461c-ad3f-162cb949df9f</BRIDGECOMPANYGUID>",
        ">00000000-0000-4000-8000-000000000676</BRIDGECOMPANYGUID>",
        1,
    );
    assert_ne!(other, groups, "one row's company GUID changed");
    refused_on_the_group_read(other, "group_response_company_guid_mismatch").await;
}

#[tokio::test]
async fn a_group_collection_that_reports_failure_is_refused_with_its_cause() {
    let failed = replaced_once(&groups(), "<STATUS>1</STATUS>", "<STATUS>0</STATUS>");
    refused_on_the_group_read(failed, "group_status_not_success").await;
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
        // The Currency masters and the qualified mode probe after the group
        // read are never sent.
        plans.truncate(plans.len() - paired(single_currency()).len() - probe().len());
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
    located_after_response(created_one(), marks_after).await
}

async fn located_after_response(post_response: String, marks_after: String) -> Value {
    let mut plans = before_approval();
    plans.extend(after_approval(xml(post_response.clone())));
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
    let created = parse_import_outcome(&post_response)
        .expect("the POST answer parses")
        .counters()
        .created;
    assert_eq!(
        journaled_outcome(directory.path()).map(|outcome| outcome.counters().created),
        Some(created),
        "the journaled outcome is the parsed POST answer"
    );
    response["structuredContent"]["result"]["post_location"].clone()
}

#[tokio::test]
async fn only_the_target_moving_is_reported_as_the_landing() {
    let located = located_after(company_marks(11, 50, "WR2 Unicode Lab")).await;
    assert_eq!(located["state"], "target_only", "{located}");
    // The captured answer reports one create, and the target's mark moved by one.
    assert_eq!(
        located["target_voucher_step"],
        json!({"before": 10, "after": 11, "step": 1, "reported_created": 1, "matches_created": true}),
        "{located}"
    );
}

/// A step larger than Tally's CREATED means the post altered or cancelled
/// vouchers itself, or another voucher in the target changed around it
/// (protocol reference §11c.5). It is reported in `post_location`.
#[tokio::test]
async fn a_target_step_beyond_the_create_is_reported() {
    let located = located_after(company_marks(12, 50, "WR2 Unicode Lab")).await;
    assert_eq!(located["state"], "target_only", "{located}");
    assert_eq!(located["target_voucher_step"]["step"], 2, "{located}");
    assert_eq!(
        located["target_voucher_step"]["matches_created"], false,
        "{located}"
    );
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

#[tokio::test]
async fn a_snapshot_lost_in_transport_refuses_as_unconfirmed_not_as_an_unknown_outcome() {
    // The read that aims the post fails before any body arrives: nothing was
    // sent, so the code must say so rather than "outcome unknown".
    let mut plans = before_approval();
    let mut after = after_approval(xml(created_one()));
    let marks_at = after.len() - 2;
    after[marks_at] = marks_before().with_delivery(Delivery::ResetBeforeBody);
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
    let result = &response["structuredContent"]["result"];
    assert_eq!(
        result["error"]["code"], "post_company_scope_unconfirmed",
        "{response}"
    );
    assert_eq!(result["attempt_recorded"], json!(false), "{response}");
    assert_eq!(observed, through_marks, "{response}");
    assert_eq!(
        appended_kinds(&before, &journal(directory.path())),
        ["verification_status"]
    );
}

#[tokio::test]
async fn the_response_is_journaled_before_the_location_snapshot_is_answered() {
    // The snapshot after the POST is held for three seconds. While it is held
    // the journal already carries the dispatch response, so a slow or lost
    // location read cannot cost the record of the post.
    let mut plans = before_approval();
    let after = after_approval(xml(created_one()));
    let post_at = plans.len() + after.len() - 1;
    plans.extend(after);
    plans.push(
        xml(company_marks(11, 50, "WR2 Unicode Lab"))
            .with_delivery(Delivery::SlowHeaders(Duration::from_secs(3))),
    );
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
        while simulator.received() <= post_at + 1 {
            assert!(
                started.elapsed() < Duration::from_secs(20),
                "the location snapshot never arrived"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        appended_kinds(&before, &journal(directory.path()))
    };
    let (response, while_held) = tokio::join!(post, watch);
    assert!(
        while_held.iter().any(|kind| kind == "dispatch_response"),
        "{while_held:?} {response}"
    );
    let _ = sent(simulator);
    assert_journaled_clean_create(directory.path());
    assert_eq!(
        response["structuredContent"]["result"]["post_location"]["state"], "target_only",
        "{response}"
    );
}

#[tokio::test]
async fn another_company_moving_while_tally_created_nothing_is_not_blamed() {
    // Tally rejected the post while a colleague's voucher moved another
    // company's mark: that company must not be named as where this post went.
    // The rejection is a captured live response (CREATED 0, EXCEPTIONS 1, one
    // LINEERROR), the shape T7 measured for a post into a company lacking a
    // ledger.
    let rejected = include_str!(
        "../crates/bridge-tally-protocol/tests/fixtures/live_education_w7_baddate_sanitized.xml"
    )
    .to_string();
    assert!(parse_import_outcome(&rejected).is_ok_and(|outcome| outcome.counters().created == 0));
    let located = located_after_response(rejected, company_marks(10, 51, "WR2 Unicode Lab")).await;
    assert_eq!(located["state"], "no_creation_reported", "{located}");
}

#[tokio::test]
async fn a_post_whose_response_cannot_be_journaled_still_reports_where_it_landed() {
    // The POST is sent and answered, but another process holds the admission
    // lock when the response is journaled, so that local write fails. The
    // location of a post that was sent must survive that failure.
    let held = xml(created_one()).with_delivery(Delivery::SlowHeaders(Duration::from_secs(2)));
    let mut plans = before_approval();
    let after = after_approval(held);
    let post_at = plans.len() + after.len() - 1;
    plans.extend(after);
    plans.push(xml(company_marks(11, 50, "WR2 Unicode Lab")));
    let simulator = SequenceSimulator::spawn(plans).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let server = server_at(simulator.address(), directory.path());
    let (_, args) = saved_batch(&server);
    let other = server_at(simulator.address(), directory.path());
    let post = SCRIPTED_APPROVAL.scope(
        ScriptedApproval::approving(),
        server.call_tool("post_import", args),
    );
    let hold_lock = async {
        let started = std::time::Instant::now();
        // Wait until the POST has been received, so the intent's own use of
        // the lock is over, then hold the lock while the response is written.
        while simulator.received() <= post_at {
            assert!(
                started.elapsed() < Duration::from_secs(20),
                "the POST never arrived"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let lock = other
            .lock_import_admission()
            .expect("lock is free while the POST is held");
        tokio::time::sleep(Duration::from_secs(4)).await;
        drop(lock);
    };
    let (response, ()) = tokio::join!(post, hold_lock);
    let _ = sent(simulator);
    let result = &response["structuredContent"]["result"];
    assert_eq!(
        result["error"]["code"], "import_admission_busy",
        "the journal write must have failed: {response}"
    );
    assert_eq!(
        result["post_location"]["state"], "target_only",
        "{response}"
    );
}

/// The simulated POST answer every test above posts against must be one Tally
/// actually sends, and must parse as exactly one clean create. When it did not
/// parse, every test here ran the post with no parsed outcome, so none of them
/// exercised the clean-success path.
#[test]
fn the_simulated_post_answer_parses_as_one_clean_create() {
    let outcome = parse_import_outcome(&created_one()).expect("the POST answer parses");
    assert_eq!(outcome.counters().created, 1);
    assert!(import_outcome_is_clean(Some(&outcome), 1));
}

/// A Journal Bridge posted live (bridge#582's lab qualification), as its
/// export was captured: `native-namespaced-journal`. The batch below is the
/// one that produced it, so a readback serving this capture is the post's own
/// voucher, attributed by its `[BRIDGE:…]` marker.
fn captured_posted_journal() -> String {
    captured(include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-namespaced-journal.utf16le.xml"
    ))
}

fn saved_captured_batch(server: &Server) -> Value {
    let line = saved_captured_line(server);
    json!({"company_guid":GUID,"batch_id":line.batch_id})
}

fn saved_captured_line(server: &Server) -> ImportLedgerLine {
    let origin = super::super::super::canonical_loopback_origin(&server.settings.endpoint).unwrap();
    let mut line: ImportLedgerLine = serde_json::from_value(json!({
        "batch_id":"bridge-6c79872c-aab6-4be5-a181-18182c8148be", "identity_scheme":"batch_v1",
        "company_guid":GUID,
        "endpoint_origin":origin,
        "company":{"name":"WR2 Unicode Lab","guid":GUID,"company_number":"100004","books_from":"20260401"},
        "txn_ids":["BRIDGE_MCP_LIVE_20260906_A1"],"date_from":"20260907","date_to":"20260907",
        "sha256":"", "built_at":"2026-09-06T21:40:26.641Z", "status":"built",
        "pre_import_mark":{"kind":"company_high_water","value":8,"master_value":7},
        "vouchers":[{"bridge_txn_id":"BRIDGE_MCP_LIVE_20260906_A1","date":"20260907",
            "voucher_type":"Journal","narration":"Bridge MCP batch namespace qualification",
            "reference":null,"voucher_number":null,
            "entries":[{"ledger":"Bridge Nested Debtor WR4","amount":"12.61","side":"Dr"},
                {"ledger":"Cash","amount":"12.61","side":"Cr"}]}]
    }))
    .unwrap();
    let rendered = render_import_xml("WR2 Unicode Lab", &line.vouchers, &line.batch_id);
    line.sha256 = sha256_hex(rendered.as_bytes());
    bind_to_captured_catalogue(&mut line);
    server.append_import_ledger(&line).unwrap();
    fs::write(
        server
            .imports_dir()
            .unwrap()
            .join(format!("{}.xml", line.batch_id)),
        rendered,
    )
    .unwrap();
    line
}

/// The whole native post, end to end: absent before, one clean create, the
/// location snapshot, then the readback finds the post's own voucher. This is
/// the only simulator test that reaches `posted_verified`; the others stop at
/// the POST, so their final result is the readback failing for want of plans.
/// The verdict is the readback's: a target step of two, which does not match
/// the one create, is reported and changes nothing.
#[tokio::test]
async fn a_native_post_reads_back_as_posted_verified() {
    for (mark_after, step, matches_created) in [(11, 1, true), (12, 2, false)] {
        native_post_reads_back_as_posted_verified(mark_after, step, matches_created).await;
    }
}

async fn native_post_reads_back_as_posted_verified(
    mark_after: u64,
    step: u64,
    matches_created: bool,
) {
    let mut plans = before_approval();
    plans.extend(after_approval(xml(created_one())));
    plans.push(xml(company_marks(mark_after, 50, "WR2 Unicode Lab")));
    // The readback: the same verification read the pre-post check made, now
    // serving the captured voucher.
    plans.extend(probe());
    plans.extend(verified_company());
    plans.extend(paired(marks()));
    plans.extend(paired(captured_posted_journal()));
    plans.extend(paired(captured_posted_journal()));
    plans.extend(probe());
    let simulator = SequenceSimulator::spawn(with_sentinel(plans)).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let server = server_at(simulator.address(), directory.path());
    let args = saved_captured_batch(&server);
    let response = SCRIPTED_APPROVAL
        .scope(
            ScriptedApproval::approving(),
            server.call_tool("post_import", args),
        )
        .await;
    let _ = sent(simulator);
    let result = &response["structuredContent"]["result"];
    assert_eq!(response["isError"], json!(false), "{response}");
    assert_eq!(result["dispatch"]["state"], "posted_verified", "{response}");
    assert_eq!(result["counts"]["posted_verified"], 1, "{response}");
    assert_eq!(
        result["post_location"]["state"], "target_only",
        "{response}"
    );
    assert_eq!(
        result["post_location"]["target_voucher_step"]["step"], step,
        "{response}"
    );
    assert_eq!(
        result["post_location"]["target_voucher_step"]["matches_created"], matches_created,
        "{response}"
    );
    assert!(result.get("error").is_none(), "{response}");
    assert_journaled_clean_create(directory.path());
}

// bridge#551: a post goes only into a book with exactly one Currency master,
// checked before approval and again inside the queue.

/// The refusal both surfaces report for the captured two-master book: the
/// plain reason, naming both masters, with no attempt recorded.
fn assert_refused_as_multi_currency(result: &Value) {
    assert_eq!(
        result["error"]["code"], "import_multi_currency_unsupported",
        "{result}"
    );
    assert_eq!(result["attempt_recorded"], json!(false), "{result}");
    let message = result["error"]["message"].as_str().unwrap();
    assert!(
        message.starts_with("This company has more than one currency defined ("),
        "{message}"
    );
    assert!(
        message.contains("Bridge does not post into multi-currency books yet"),
        "{message}"
    );
    let names =
        bridge_tally_protocol::native_outstandings::parse_company_currency(&two_currencies())
            .unwrap()
            .names;
    assert_eq!(names.len(), 2);
    for name in &names {
        assert!(message.contains(name.as_str()), "{name} in {message}");
    }
}

#[tokio::test]
async fn a_book_with_two_currency_masters_is_refused_before_approval_on_both_surfaces() {
    for desktop in [false, true] {
        let mut plans = before_approval_with_currencies(two_currencies());
        // The qualified mode probe after the Currency read is never sent.
        plans.truncate(plans.len() - probe().len());
        let expected = plans.len();
        let simulator = SequenceSimulator::spawn(with_sentinel(plans)).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let server = server_at(simulator.address(), directory.path());
        let (line, args) = saved_batch(&server);
        let scripted = ScriptedApproval::approving();
        let result = if desktop {
            let outcome = SCRIPTED_APPROVAL
                .scope(
                    scripted.clone(),
                    server.post_import_checked(&args, Some(&line.sha256), PostScope::JournalOnly),
                )
                .await
                .expect("a refusal is reported as the post's outcome");
            // What the desktop's webview receives.
            super::super::desktop_journal::DesktopJournalOperation::from_outcome(outcome).result
                ["result"]
                .clone()
        } else {
            let response = SCRIPTED_APPROVAL
                .scope(scripted.clone(), server.call_tool("post_import", args))
                .await;
            let result = response["structuredContent"]["result"].clone();
            assert_eq!(
                result["error"]["currencies_seen"].as_array().map(Vec::len),
                Some(2),
                "{response}"
            );
            result
        };
        let observed = sent(simulator);
        assert_refused_as_multi_currency(&result);
        assert!(scripted.previews().is_empty(), "approval must not be asked");
        assert_eq!(observed.len(), expected, "{result}");
        assert!(!String::from_utf8(journal(directory.path()))
            .unwrap()
            .contains("\"dispatch_intent\""));
    }
}

/// A master added while approval waits: the queue's own Currency read refuses,
/// after the aim snapshot and before the intent, so the POST is never sent.
#[tokio::test]
async fn a_currency_master_added_after_approval_is_refused_in_the_queue() {
    let mut plans = before_approval();
    let mut after = after_approval_with_currencies(two_currencies(), xml(created_one()));
    after.pop();
    let expected = plans.len() + after.len();
    plans.extend(after);
    let simulator = SequenceSimulator::spawn(with_sentinel(plans)).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let server = server_at(simulator.address(), directory.path());
    let (_, args) = saved_batch(&server);
    let scripted = ScriptedApproval::approving();
    let response = SCRIPTED_APPROVAL
        .scope(scripted.clone(), server.call_tool("post_import", args))
        .await;
    let observed = sent(simulator);
    assert_refused_as_multi_currency(&response["structuredContent"]["result"]);
    assert_eq!(scripted.previews().len(), 1, "approval was asked once");
    assert_eq!(
        observed.len(),
        expected,
        "the POST is never sent: {response}"
    );
    assert!(!String::from_utf8(journal(directory.path()))
        .unwrap()
        .contains("\"dispatch_intent\""));
}

/// A Payment is held to the same gate as a Journal: refused before approval on
/// a two-master book, and in the queue when the master arrives after approval.
#[tokio::test]
async fn a_payment_into_a_book_with_two_currency_masters_is_refused_before_and_after_approval() {
    for in_queue in [false, true] {
        let plans = if in_queue {
            let mut plans = bank_before_approval(catalogue(), groups());
            let mut after = bank_after_approval_with_currencies(
                catalogue(),
                groups(),
                two_currencies(),
                xml(created_one()),
            );
            after.pop();
            plans.extend(after);
            plans
        } else {
            let mut plans =
                bank_before_approval_with_currencies(catalogue(), groups(), two_currencies());
            plans.truncate(plans.len() - probe().len());
            plans
        };
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
        assert_refused_as_multi_currency(&response["structuredContent"]["result"]);
        assert_eq!(
            scripted.previews().len(),
            usize::from(in_queue),
            "{response}"
        );
        assert_eq!(observed.len(), expected, "{response}");
        assert!(!String::from_utf8(journal(directory.path()))
            .unwrap()
            .contains("\"dispatch_intent\""));
    }
}

/// Currency masters the queue cannot name a base from (none, here: an explicit
/// edit of the one-master capture) refuse with their own code, and nothing is
/// posted.
#[tokio::test]
async fn currency_masters_without_a_base_are_refused_in_the_queue_with_their_own_code() {
    let single = single_currency();
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
    let mut plans = before_approval();
    let mut after = after_approval_with_currencies(no_master, xml(created_one()));
    after.pop();
    let expected = plans.len() + after.len();
    plans.extend(after);
    let simulator = SequenceSimulator::spawn(with_sentinel(plans)).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let server = server_at(simulator.address(), directory.path());
    let (_, args) = saved_batch(&server);
    let response = SCRIPTED_APPROVAL
        .scope(
            ScriptedApproval::approving(),
            server.call_tool("post_import", args),
        )
        .await;
    let observed = sent(simulator);
    let result = &response["structuredContent"]["result"];
    assert_eq!(
        result["error"]["code"], "import_base_currency_undetermined",
        "{response}"
    );
    assert_eq!(result["attempt_recorded"], json!(false), "{response}");
    assert!(
        result["error"].get("currencies_seen").is_none(),
        "{response}"
    );
    assert_eq!(
        observed.len(),
        expected,
        "the POST is never sent: {response}"
    );
}

// bridge#239: a master changed after the queue's catalogue re-read is refused
// before the POST, from the target's ALTMSTID in the aim snapshot compared
// with the one read as the binding reads began.

/// The approved Journal, with the queue's binding-time snapshot answered by
/// `at_binding` and its aim snapshot by `at_aim`. Returns the response, the
/// requests observed, and the number up to and including the aim snapshot.
async fn post_with_master_marks(at_binding: String, at_aim: String) -> (Value, usize, usize) {
    let mut plans = before_approval();
    let mut after = after_approval(xml(created_one()));
    // The binding snapshot follows the opening mode probe and company read.
    let binding_at = probe().len() + 1;
    after[binding_at] = xml(at_binding);
    let aim_at = after.len() - 2;
    after[aim_at] = xml(at_aim);
    let through_aim = plans.len() + aim_at + 1;
    plans.extend(after);
    let simulator = SequenceSimulator::spawn(with_sentinel(plans)).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let server = server_at(simulator.address(), directory.path());
    let (_, args) = saved_batch(&server);
    let response = SCRIPTED_APPROVAL
        .scope(
            ScriptedApproval::approving(),
            server.call_tool("post_import", args),
        )
        .await;
    let observed = sent(simulator).len();
    (response, observed, through_aim)
}

fn marks_with_target_masters(masters: u64) -> String {
    replaced_once(
        &company_marks(10, 50, "WR2 Unicode Lab"),
        "<ALTMSTID>7</ALTMSTID>",
        &format!("<ALTMSTID>{masters}</ALTMSTID>"),
    )
}

#[tokio::test]
async fn a_master_changed_after_the_catalogue_re_read_is_refused_before_the_post() {
    let (response, observed, through_aim) =
        post_with_master_marks(marks_with_target_masters(7), marks_with_target_masters(8)).await;
    let result = &response["structuredContent"]["result"];
    assert_eq!(result["error"]["code"], "post_masters_moved", "{response}");
    assert_eq!(result["attempt_recorded"], json!(false), "{response}");
    assert_eq!(observed, through_aim, "the POST is never sent: {response}");
}

/// The control: another company's masters moving is not the target's, so the
/// post goes ahead.
#[tokio::test]
async fn another_companys_master_change_does_not_refuse_the_post() {
    let at_aim = replaced_once(
        &company_marks(10, 50, "WR2 Unicode Lab"),
        "<ALTMSTID>3</ALTMSTID>",
        "<ALTMSTID>4</ALTMSTID>",
    );
    let (response, observed, through_aim) =
        post_with_master_marks(marks_with_target_masters(7), at_aim).await;
    assert!(observed > through_aim, "the POST is sent: {response}");
}

/// A binding-time snapshot without the target cannot be compared, so the post
/// is refused as unconfirmed.
#[tokio::test]
async fn a_binding_snapshot_without_the_target_is_refused_as_unconfirmed() {
    let (response, observed, through_aim) = post_with_master_marks(
        company_marks(10, 50, "Synthetic Renamed Lab"),
        marks_with_target_masters(7),
    )
    .await;
    let result = &response["structuredContent"]["result"];
    assert_eq!(
        result["error"]["code"], "post_masters_unconfirmed",
        "{response}"
    );
    assert_eq!(result["attempt_recorded"], json!(false), "{response}");
    assert_eq!(observed, through_aim, "{response}");
}

/// During the approval wait, a ledger renamed and a new one created under its
/// old name is caught by identity: the queue's catalogue holds the approved
/// name with a different GUID, and the post is refused before the intent.
#[tokio::test]
async fn a_new_ledger_under_an_approved_name_during_approval_is_refused_by_identity() {
    let replaced = replaced_once(
        &catalogue(),
        ">61c6de69-1748-461c-ad3f-162cb949df9f-0000001f</GUID>",
        ">61c6de69-1748-461c-ad3f-162cb949df9f-000000ff</GUID>",
    );
    let mut plans = before_approval();
    let mut after = after_approval(xml(created_one()));
    // The queue's catalogue: its first report and its replay.
    let catalogue_at = probe().len() + 2;
    after[catalogue_at + 1] = xml(replaced.clone());
    after[catalogue_at + 3] = xml(replaced);
    // The binding is compared once every queue read is in, after the aim
    // snapshot; only the POST is never sent.
    after.pop();
    let expected = plans.len() + after.len();
    plans.extend(after);
    let simulator = SequenceSimulator::spawn(with_sentinel(plans)).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let server = server_at(simulator.address(), directory.path());
    let (_, args) = saved_batch(&server);
    let response = SCRIPTED_APPROVAL
        .scope(
            ScriptedApproval::approving(),
            server.call_tool("post_import", args),
        )
        .await;
    let observed = sent(simulator);
    let result = &response["structuredContent"]["result"];
    assert_eq!(
        result["error"]["code"], "import_masters_changed",
        "{response}"
    );
    assert_eq!(result["attempt_recorded"], json!(false), "{response}");
    assert_eq!(observed.len(), expected, "{response}");
}

/// bridge#634, #641: the queue's catalogue re-read at post time holds a
/// repeated ledger. The admission recheck refuses before the intent and the
/// POST under its own code, not the catch-all that says the outcome is
/// unknown, nor #656's `post_queue_read_failed` (the named refusal wins), and
/// carries the catalogue's typed cause. Below the response budget the cause
/// is left out, as on the generic refusal, and the fields a caller acts on
/// survive. The name is never in the response.
#[tokio::test]
async fn a_post_time_catalogue_refusal_names_its_cause_and_no_ledger() {
    let repeated = crate::tally::standard_ledger_catalog::tests::catalogue_with_extra_ledgers(
        &catalogue(),
        [
            ("Twice Named".to_string(), "c0000001".to_string()),
            ("Twice Named".to_string(), "c0000002".to_string()),
        ],
    );
    for (max_bytes, cause) in [
        (200_000, json!("ledger_catalogue_duplicate_identity")),
        (
            crate::agent::REMEDIATION_MIN_RESPONSE_BUDGET - 1,
            Value::Null,
        ),
    ] {
        let mut plans = before_approval();
        let mut after = after_approval(xml(created_one()));
        let catalogue_at = probe().len() + 2;
        after[catalogue_at + 1] = xml(repeated.clone());
        after[catalogue_at + 3] = xml(repeated.clone());
        after.pop();
        let expected = plans.len() + after.len();
        plans.extend(after);
        let simulator = SequenceSimulator::spawn(with_sentinel(plans)).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let mut server = server_at(simulator.address(), directory.path());
        server.settings.max_bytes = max_bytes;
        let (_, args) = saved_batch(&server);
        let response = SCRIPTED_APPROVAL
            .scope(
                ScriptedApproval::approving(),
                server.call_tool("post_import", args),
            )
            .await;
        let observed = sent(simulator);
        let result = &response["structuredContent"]["result"];
        assert_eq!(result["error"]["cause"], cause, "{response}");
        assert_eq!(
            result["error"]["code"], "post_catalogue_unreadable",
            "{response}"
        );
        assert_eq!(result["attempt_recorded"], json!(false), "{response}");
        assert_eq!(
            observed.len(),
            expected,
            "the POST is never sent: {response}"
        );
        assert!(!response.to_string().contains("Twice"), "{response}");
    }
}

/// The binding-time snapshot lost in transport, or answered with T2's live
/// refusal: the masters cannot be compared, so the post is refused as
/// `post_masters_unconfirmed`, never as an unknown outcome. A lost read stops
/// the queue at once; an unreadable one is refused once the queue's reads are
/// in, after the aim snapshot. Neither sends the POST or records an intent.
#[tokio::test]
async fn an_unreadable_binding_snapshot_refuses_as_unconfirmed() {
    let refused = "<ENVELOPE><HEADER><VERSION>1</VERSION><STATUS>0</STATUS></HEADER><BODY><DATA>\
                   <LINEERROR>Could not set 'SVCurrentCompany' to 'WR2 Unicode Lab'</LINEERROR>\
                   </DATA></BODY></ENVELOPE>"
        .to_string();
    let binding_at = probe().len() + 1;
    for lost in [true, false] {
        let mut plans = before_approval();
        let mut after = after_approval(xml(created_one()));
        after[binding_at] = if lost {
            marks_at_binding().with_delivery(Delivery::ResetBeforeBody)
        } else {
            xml(refused.clone())
        };
        let expected = plans.len()
            + if lost {
                binding_at + 1
            } else {
                after.len() - 1
            };
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
        let result = &response["structuredContent"]["result"];
        assert_eq!(
            result["error"]["code"], "post_masters_unconfirmed",
            "{response}"
        );
        assert_eq!(result["attempt_recorded"], json!(false), "{response}");
        assert_eq!(observed, expected, "{response}");
        assert_eq!(
            appended_kinds(&before, &journal(directory.path())),
            ["verification_status"]
        );
    }
}

/// #656: a queue read that fails before the intent is refused under its own
/// code, not the catch-all that says the outcome is unknown. The queue's
/// catalogue legs are lost in transport (the queue stops at once), or disagree
/// (a pair drift); either way no intent is journaled, no POST is sent, and the
/// cause names the failure.
#[tokio::test]
async fn a_queue_read_failing_before_the_intent_is_refused_as_such() {
    let catalogue_at = probe().len() + 2;
    let drifted = replaced_once(
        &catalogue(),
        ">61c6de69-1748-461c-ad3f-162cb949df9f-0000001f</GUID>",
        ">61c6de69-1748-461c-ad3f-162cb949df9f-000000ff</GUID>",
    );
    for (lost, cause) in [
        (true, "response_truncated"),
        (false, "native_report_pair_changed"),
    ] {
        let mut plans = before_approval();
        let mut after = after_approval(xml(created_one()));
        let expected = if lost {
            after[catalogue_at + 1] = xml(catalogue()).with_delivery(Delivery::ResetBeforeBody);
            plans.len() + catalogue_at + 2
        } else {
            after[catalogue_at + 3] = xml(drifted.clone());
            // Each leg of the paired read is followed by a health check, and
            // the legs are compared only after the second one.
            plans.len() + catalogue_at + 5
        };
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
        let error = &response["structuredContent"]["result"]["error"];
        assert_eq!(error["code"], "post_queue_read_failed", "{response}");
        assert_eq!(error["cause"], cause, "{response}");
        assert_eq!(
            response["structuredContent"]["result"]["attempt_recorded"],
            json!(false),
            "{response}"
        );
        assert_eq!(observed, expected, "{response}");
        assert_eq!(
            appended_kinds(&before, &journal(directory.path())),
            ["verification_status"]
        );
    }
}

/// #656, the other direction: the pre-intent code must never reach a post
/// whose bytes were sent. The POST's response is lost in transport, after the
/// intent was journaled, so the outcome is unknown and the attempt recorded.
#[tokio::test]
async fn a_post_lost_after_the_intent_is_still_an_unknown_outcome() {
    let mut plans = before_approval();
    plans.extend(after_approval(
        xml(created_one()).with_delivery(Delivery::ResetBeforeBody),
    ));
    let simulator = SequenceSimulator::spawn(with_sentinel(plans)).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let server = server_at(simulator.address(), directory.path());
    let (_, args) = saved_batch(&server);
    let response = SCRIPTED_APPROVAL
        .scope(
            ScriptedApproval::approving(),
            server.call_tool("post_import", args),
        )
        .await;
    sent(simulator);
    let result = &response["structuredContent"]["result"];
    assert_eq!(
        result["error"]["code"], "import_dispatch_outcome_unknown",
        "{response}"
    );
    assert_eq!(result["attempt_recorded"], json!(true), "{response}");
}

// bridge#239: the ledgers a batch names must still carry the GUIDs its build
// bound them to; a name alone cannot tell a ledger renamed and replaced.

/// A saved Journal refused before approval by its build-time binding, on the
/// MCP tool or on the desktop's post. A changed GUID is found on the post's
/// catalogue read, the last one sent; a record without identities is refused
/// before any request. No intent is recorded either way.
async fn refused_by_build_binding(
    identities: Option<Vec<BoundLedger>>,
    desktop: bool,
) -> (Value, usize, usize, bool) {
    let mut plans = before_approval();
    let expected = if identities.is_some() {
        // The Currency read and mode probe after the catalogue are never sent.
        plans.truncate(plans.len() - paired(single_currency()).len() - probe().len());
        plans.len()
    } else {
        plans.clear();
        0
    };
    let simulator = SequenceSimulator::spawn(with_sentinel(plans)).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let server = server_at(simulator.address(), directory.path());
    let (mut line, args) = saved_batch(&server);
    line.ledger_identities = identities;
    server.append_import_ledger(&line).unwrap();
    let scripted = ScriptedApproval::approving();
    let result = if desktop {
        let outcome = SCRIPTED_APPROVAL
            .scope(
                scripted.clone(),
                server.post_import_checked(&args, Some(&line.sha256), PostScope::JournalOnly),
            )
            .await
            .expect("a refusal is reported as the post's outcome");
        // What the desktop's webview receives.
        super::super::desktop_journal::DesktopJournalOperation::from_outcome(outcome).result
            ["result"]
            .clone()
    } else {
        SCRIPTED_APPROVAL
            .scope(scripted.clone(), server.call_tool("post_import", args))
            .await["structuredContent"]["result"]
            .clone()
    };
    let observed = sent(simulator).len();
    let intent = String::from_utf8(journal(directory.path()))
        .unwrap()
        .contains("\"dispatch_intent\"");
    assert!(scripted.previews().is_empty(), "approval must not be asked");
    (result, observed, expected, intent)
}

#[tokio::test]
async fn a_ledger_replaced_under_its_name_since_the_build_is_refused_before_approval() {
    for desktop in [false, true] {
        let identities = vec![
            BoundLedger {
                name: "Cash".into(),
                guid: "61c6de69-1748-461c-ad3f-162cb949df9f-000000ff".into(),
            },
            BoundLedger {
                name: "WR2 Sales".into(),
                guid: "61c6de69-1748-461c-ad3f-162cb949df9f-000000d0".into(),
            },
        ];
        let (result, observed, expected, intent) =
            refused_by_build_binding(Some(identities), desktop).await;
        assert_eq!(
            result["error"]["code"], "import_masters_changed_since_build",
            "{result}"
        );
        assert_eq!(result["attempt_recorded"], json!(false), "{result}");
        if !desktop {
            assert_eq!(
                result["error"]["ledgers_changed"],
                json!(["Cash"]),
                "{result}"
            );
        }
        assert!(result["error"]["message"]
            .as_str()
            .unwrap()
            .contains("(Cash)"));
        assert_eq!(observed, expected, "{result}");
        assert!(!intent);
    }
}

#[tokio::test]
async fn a_batch_built_before_ledger_binding_is_refused_before_any_request() {
    for desktop in [false, true] {
        let (result, observed, expected, intent) = refused_by_build_binding(None, desktop).await;
        assert_eq!(
            result["error"]["code"], "import_batch_predates_ledger_binding",
            "{result}"
        );
        assert_eq!(result["attempt_recorded"], json!(false), "{result}");
        assert!(result["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Build the batch again"));
        assert_eq!(observed, expected, "{result}");
        assert!(!intent);
    }
}

/// The approval preview says the ledgers were checked by identity.
#[tokio::test]
async fn the_preview_says_the_ledgers_were_checked_by_identity() {
    let simulator = SequenceSimulator::spawn(with_sentinel(before_approval())).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let server = server_at(simulator.address(), directory.path());
    let (_, args) = saved_batch(&server);
    let scripted = ScriptedApproval::declining();
    let _ = SCRIPTED_APPROVAL
        .scope(scripted.clone(), server.call_tool("post_import", args))
        .await;
    let previews = scripted.previews();
    assert_eq!(previews.len(), 1);
    assert!(previews[0].contains("Ledgers checked by identity against the build"));
}

/// A batch dispatched before Bridge recorded ledger identities still
/// reconciles: `post_import` on it goes to the readback, never to the
/// "build it again" refusal, since a dispatched batch must not be rebuilt.
#[tokio::test]
async fn a_dispatched_batch_without_identities_still_reconciles() {
    let simulator = SequenceSimulator::spawn(with_sentinel(Vec::new())).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let server = server_at(simulator.address(), directory.path());
    let (mut line, args) = saved_batch(&server);
    line.ledger_identities = None;
    server.append_import_ledger(&line).unwrap();
    let native = native_post_request(&line, RemoteIds::from_ids(vec![Uuid::new_v4()])).unwrap();
    {
        let _lock = server.lock_import_admission().unwrap();
        server
            .append_import_record_while_admitted(&ledger::StatusRecord::dispatch_for(
                &line, &native,
            ))
            .unwrap();
    }
    let response = server.call_tool("post_import", args).await;
    let observed = sent(simulator).len();
    let result = &response["structuredContent"]["result"];
    assert_ne!(
        result["error"]["code"], "import_batch_predates_ledger_binding",
        "{response}"
    );
    assert_ne!(result["attempt_recorded"], json!(false), "{response}");
    assert!(observed > 0, "the readback reads Tally: {response}");
}

// bridge#239: the company's masters across the post. Only when the target's
// master mark moved between the aim snapshot and the snapshot after the POST
// is the catalogue read again, and the approved ledgers resolved by name.

/// The captured post read back, with `marks_after` answering the snapshot
/// after the POST and `catalogue` the extra read (if any) before the readback.
/// Returns the response, the requests observed and the requests scripted.
async fn post_with_masters_after(
    marks_after: String,
    catalogue: Vec<ScenarioPlan>,
) -> (Value, usize, usize) {
    let mut plans = before_approval();
    plans.extend(after_approval(xml(created_one())));
    plans.push(xml(marks_after));
    plans.extend(catalogue);
    plans.extend(reconcile_readback());
    let scripted = plans.len();
    let simulator = SequenceSimulator::spawn(with_sentinel(plans)).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let server = server_at(simulator.address(), directory.path());
    let args = saved_captured_batch(&server);
    let response = SCRIPTED_APPROVAL
        .scope(
            ScriptedApproval::approving(),
            server.call_tool("post_import", args.clone()),
        )
        .await;
    let observed = sent(simulator).len();
    // Every post leaves a record: the verdict it reports, or, when the check
    // could not run, the pending mark a later readback finishes.
    let recorded = masters_check_of(&server, args["batch_id"].as_str().unwrap());
    let reported = &response["structuredContent"]["result"]["masters_after_post"];
    if reported["state"] == "check_unavailable" {
        assert_eq!(recorded, json!({"state": "check_pending"}), "{response}");
    } else {
        assert_eq!(&recorded, reported, "{response}");
    }
    // A verified post is an amendment's baseline; a doubted one never is.
    let baseline = read_verified_baseline(
        &server.imports_dir().unwrap(),
        args["batch_id"].as_str().unwrap(),
    )
    .is_some();
    let result = &response["structuredContent"]["result"];
    if result["dispatch"]["state"] == "posted_verified" {
        assert!(baseline, "{response}");
    }
    if masters_doubt(Some(reported)).is_some() {
        assert!(!baseline, "{response}");
    }
    (response, observed, scripted)
}

/// What the batch's masters records say, as every reader reads them.
fn masters_check_of(server: &Server, batch_id: &str) -> Value {
    read_masters_check(&server.imports_dir().unwrap(), batch_id).unwrap()
}

/// A write to `path` fails: a directory with an entry is in its place.
fn block(path: std::path::PathBuf) {
    fs::create_dir_all(path.join("entry")).unwrap();
}

/// The readback after a post, and in a later reconcile of the posted batch.
fn reconcile_readback() -> Vec<ScenarioPlan> {
    let mut plans = probe();
    plans.extend(verified_company());
    plans.extend(paired(marks()));
    plans.extend(paired(captured_posted_journal()));
    plans.extend(paired(captured_posted_journal()));
    plans
}

/// The snapshot after the POST with the target's master mark at `masters`
/// (the aim snapshot has it at 7).
fn masters_moved_to(masters: u64) -> String {
    replaced_once(
        &company_marks(11, 50, "WR2 Unicode Lab"),
        "<ALTMSTID>7</ALTMSTID>",
        &format!("<ALTMSTID>{masters}</ALTMSTID>"),
    )
}

#[tokio::test]
async fn unmoved_masters_are_not_checked_and_cost_no_request() {
    let (response, observed, scripted) =
        post_with_masters_after(company_marks(11, 50, "WR2 Unicode Lab"), Vec::new()).await;
    let result = &response["structuredContent"]["result"];
    assert_eq!(
        result["masters_after_post"]["state"], "not_checked",
        "{response}"
    );
    assert_eq!(result["masters_after_post"]["reason"], "masters_unmoved");
    assert_eq!(result["dispatch"]["state"], "posted_verified", "{response}");
    assert_eq!(observed, scripted, "no extra read: {response}");
}

#[tokio::test]
async fn moved_masters_with_every_approved_ledger_unchanged_stay_verified() {
    let (response, observed, scripted) =
        post_with_masters_after(masters_moved_to(8), paired(catalogue())).await;
    let result = &response["structuredContent"]["result"];
    assert_eq!(
        result["masters_after_post"]["state"], "unchanged",
        "{response}"
    );
    assert_eq!(result["masters_after_post"]["trigger"], "masters_moved");
    assert_eq!(result["dispatch"]["state"], "posted_verified", "{response}");
    assert_eq!(observed, scripted, "{response}");
}

/// A snapshot after the POST that cannot be read proves nothing unmoved, so
/// the approved ledgers are still read again, never skipped.
#[tokio::test]
async fn an_unreadable_snapshot_after_the_post_still_checks_the_ledgers() {
    let unreadable = "<ENVELOPE><BODY><DATA></DATA></BODY></ENVELOPE>".to_string();
    let (response, observed, scripted) =
        post_with_masters_after(unreadable, paired(catalogue())).await;
    let result = &response["structuredContent"]["result"];
    assert_eq!(
        result["masters_after_post"],
        json!({"state":"unchanged","trigger":"masters_unconfirmed"}),
        "{response}"
    );
    assert_eq!(result["dispatch"]["state"], "posted_verified", "{response}");
    assert_eq!(observed, scripted, "{response}");
}

#[tokio::test]
async fn a_ledger_now_on_another_guid_after_the_post_is_flagged_not_verified() {
    let replaced = replaced_once(
        &catalogue(),
        ">61c6de69-1748-461c-ad3f-162cb949df9f-0000001f</GUID>",
        ">61c6de69-1748-461c-ad3f-162cb949df9f-000000ff</GUID>",
    );
    let (response, observed, scripted) =
        post_with_masters_after(masters_moved_to(8), paired(replaced)).await;
    let result = &response["structuredContent"]["result"];
    assert_eq!(
        result["masters_after_post"]["state"], "posted_under_changed_masters",
        "{response}"
    );
    assert_eq!(result["masters_after_post"]["ledgers"], json!(["Cash"]));
    assert_eq!(result["dispatch"]["state"], "reconciliation_required");
    assert_eq!(result["error"]["code"], "posted_under_changed_masters");
    let message = result["error"]["message"].as_str().unwrap();
    assert!(message.starts_with("Posted to Tally"), "{message}");
    assert!(message.contains("do not rebuild this event"), "{message}");
    assert_eq!(observed, scripted, "{response}");
}

#[tokio::test]
async fn moved_masters_that_cannot_be_re_read_are_not_verified() {
    // T2's live answer: the catalogue read is refused, so nothing confirms
    // the approved ledgers.
    let refused = "<ENVELOPE><HEADER><VERSION>1</VERSION><STATUS>0</STATUS></HEADER><BODY><DATA>\
                   <LINEERROR>Could not set 'SVCurrentCompany' to 'WR2 Unicode Lab'</LINEERROR>\
                   </DATA></BODY></ENVELOPE>"
        .to_string();
    let (response, _, _) = post_with_masters_after(masters_moved_to(8), paired(refused)).await;
    let result = &response["structuredContent"]["result"];
    assert_eq!(
        result["masters_after_post"]["state"], "check_unavailable",
        "{response}"
    );
    assert_eq!(result["dispatch"]["state"], "reconciliation_required");
    assert_eq!(result["error"]["code"], "masters_after_post_unconfirmed");
}

/// A later `post_import` of a dispatched batch only reconciles: it reads the
/// voucher back by name, and the masters check recorded at the post decides
/// whether that is enough. `after` is scripted after the readback.
async fn reconcile_with_masters_check(
    record: Option<&[u8]>,
    after: Vec<ScenarioPlan>,
) -> (Value, usize, usize, Server, tempfile::TempDir) {
    let mut plans = reconcile_readback();
    plans.extend(after);
    reconcile_scripted(record, plans).await
}

async fn reconcile_with_masters_check_and_doubt(
    record: &[u8],
    doubt: &Value,
    after: Vec<ScenarioPlan>,
) -> (Value, usize, usize, Server, tempfile::TempDir) {
    let mut plans = reconcile_readback();
    plans.extend(after);
    reconcile_seeded(Some(record), Some(doubt), plans).await
}

async fn reconcile_scripted(
    record: Option<&[u8]>,
    plans: Vec<ScenarioPlan>,
) -> (Value, usize, usize, Server, tempfile::TempDir) {
    reconcile_seeded(record, None, plans).await
}

async fn reconcile_seeded(
    record: Option<&[u8]>,
    doubt: Option<&Value>,
    plans: Vec<ScenarioPlan>,
) -> (Value, usize, usize, Server, tempfile::TempDir) {
    let scripted = plans.len();
    let simulator = SequenceSimulator::spawn(with_sentinel(plans)).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let server = server_at(simulator.address(), directory.path());
    let line = saved_captured_line(&server);
    let native = native_post_request(&line, RemoteIds::from_ids(vec![Uuid::new_v4()])).unwrap();
    {
        let _lock = server.lock_import_admission().unwrap();
        server
            .append_import_record_while_admitted(&ledger::StatusRecord::dispatch_for(
                &line, &native,
            ))
            .unwrap();
        // A clean response was saved, so only the readback and the check decide.
        server
            .append_import_record_while_admitted(&ledger::StatusRecord::response(
                &line,
                ledger::DispatchResponse {
                    request_sha256: native.request_sha256.clone(),
                    ..super::tests::dispatch_response("success", 1, 0)
                },
            ))
            .unwrap();
    }
    if let Some(record) = record {
        fs::write(
            server
                .imports_dir()
                .unwrap()
                .join(format!("{}.masters_check.json", line.batch_id)),
            record,
        )
        .unwrap();
    }
    if let Some(doubt) = doubt {
        fs::write(
            server
                .imports_dir()
                .unwrap()
                .join(format!("{}.masters_doubt.json", line.batch_id)),
            serde_json::to_vec(doubt).unwrap(),
        )
        .unwrap();
    }
    let args = json!({"company_guid":GUID,"batch_id":line.batch_id});
    let response = server.call_tool("post_import", args).await;
    let observed = sent(simulator).len();
    (response, observed, scripted, server, directory)
}

const BATCH: &str = "bridge-6c79872c-aab6-4be5-a181-18182c8148be";

fn replaced_cash() -> String {
    replaced_once(
        &catalogue(),
        ">61c6de69-1748-461c-ad3f-162cb949df9f-0000001f</GUID>",
        ">61c6de69-1748-461c-ad3f-162cb949df9f-000000ff</GUID>",
    )
}

#[tokio::test]
async fn a_doubted_post_stays_doubted_when_reconciled_later() {
    // The control: a batch dispatched before the record existed reconciles.
    let (response, observed, scripted, ..) = reconcile_with_masters_check(None, Vec::new()).await;
    let result = &response["structuredContent"]["result"];
    assert_eq!(
        result["dispatch"]["state"], "previous_attempt_reconciled",
        "{response}"
    );
    assert_eq!(observed, scripted, "{response}");
    let doubt = json!({"state":"posted_under_changed_masters","trigger":"masters_moved","ledgers":["Cash"]});
    let (response, observed, scripted, server, _directory) =
        reconcile_with_masters_check(Some(&serde_json::to_vec(&doubt).unwrap()), Vec::new()).await;
    let result = &response["structuredContent"]["result"];
    assert_eq!(
        result["dispatch"]["state"], "reconciliation_required",
        "{response}"
    );
    assert_eq!(result["error"]["code"], "posted_under_changed_masters");
    assert_eq!(result["masters_after_post"], doubt);
    assert_eq!(observed, scripted, "no second check: {response}");
    assert_eq!(masters_check_of(&server, BATCH), doubt);
}

/// A check the post could not finish (a crash, a lost read) is finished by the
/// next readback that finds the voucher, against the ledgers bound at build.
#[tokio::test]
async fn a_pending_check_is_finished_by_the_next_readback() {
    let pending = br#"{"state":"check_pending"}"#;
    let (response, observed, scripted, server, _directory) =
        reconcile_with_masters_check(Some(pending), paired(catalogue())).await;
    let result = &response["structuredContent"]["result"];
    assert_eq!(
        result["dispatch"]["state"], "previous_attempt_reconciled",
        "{response}"
    );
    assert_eq!(
        result["masters_after_post"],
        json!({"state":"unchanged","trigger":"check_pending"})
    );
    assert_eq!(observed, scripted, "{response}");
    assert_eq!(masters_check_of(&server, BATCH)["state"], "unchanged");

    let (response, _, _, server, _directory) =
        reconcile_with_masters_check(Some(pending), paired(replaced_cash())).await;
    let result = &response["structuredContent"]["result"];
    assert_eq!(
        result["error"]["code"], "posted_under_changed_masters",
        "{response}"
    );
    assert_eq!(result["masters_after_post"]["ledgers"], json!(["Cash"]));
    assert_eq!(
        masters_check_of(&server, BATCH)["state"],
        "posted_under_changed_masters"
    );

    // A readback that does not find the voucher finishes nothing: before
    // the POST lands, a verdict would vouch for a post not yet made. The
    // catalogue is scripted, so a check that ran anyway would be served.
    let mut plans = probe();
    plans.extend(verified_company());
    plans.extend(paired(marks()));
    plans.extend(paired(empty_collection()));
    plans.extend(paired(empty_collection()));
    plans.extend(probe());
    plans.extend(paired(catalogue()));
    let (response, _, _, server, _directory) = reconcile_scripted(Some(pending), plans).await;
    let result = &response["structuredContent"]["result"];
    assert_eq!(result["counts"]["not_found"], 1, "{response}");
    assert_eq!(
        masters_check_of(&server, BATCH),
        json!({"state":"check_pending"})
    );

    // A check that still cannot run leaves the record pending for next time.
    let (response, _, _, server, _directory) =
        reconcile_with_masters_check(Some(pending), Vec::new()).await;
    let result = &response["structuredContent"]["result"];
    assert_eq!(
        result["error"]["code"], "masters_after_post_unconfirmed",
        "{response}"
    );
    assert_eq!(
        masters_check_of(&server, BATCH),
        json!({"state":"check_pending"})
    );
}

/// A record that exists but cannot be opened or parsed is a pending check,
/// never an absent one.
#[tokio::test]
async fn an_unreadable_masters_record_is_still_a_doubt() {
    let (response, ..) = reconcile_with_masters_check(Some(b"{not json"), Vec::new()).await;
    let result = &response["structuredContent"]["result"];
    assert_eq!(
        result["dispatch"]["state"], "reconciliation_required",
        "{response}"
    );
    assert_eq!(result["error"]["code"], "masters_after_post_unconfirmed");
}

#[test]
fn an_unopenable_masters_record_reads_as_pending() {
    let directory = tempfile::tempdir().unwrap();
    let server = server_at("127.0.0.1:9".parse().unwrap(), directory.path());
    let imports = server.imports_dir().unwrap();
    fs::create_dir(imports.join("batch-a.masters_check.json")).unwrap();
    fs::create_dir(imports.join("batch-c.masters_doubt.json")).unwrap();
    for batch in ["batch-a", "batch-c"] {
        assert_eq!(
            read_masters_check(&imports, batch),
            Some(json!({"state":"check_pending"})),
            "{batch}"
        );
    }
    assert_eq!(read_masters_check(&imports, "batch-b"), None);
}

/// A readback that fails after a doubted post still reports and records it.
#[tokio::test]
async fn a_failed_readback_after_a_doubted_post_still_carries_the_doubt() {
    let mut plans = before_approval();
    plans.extend(after_approval(xml(created_one())));
    plans.push(xml(masters_moved_to(8)));
    plans.extend(paired(replaced_cash()));
    // No readback is scripted: the sentinel answers it, so it fails.
    let simulator = SequenceSimulator::spawn(with_sentinel(plans)).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let server = server_at(simulator.address(), directory.path());
    let args = saved_captured_batch(&server);
    let response = SCRIPTED_APPROVAL
        .scope(
            ScriptedApproval::approving(),
            server.call_tool("post_import", args.clone()),
        )
        .await;
    drop(sent(simulator));
    let result = &response["structuredContent"]["result"];
    assert_ne!(result["dispatch"]["state"], "posted_verified", "{response}");
    assert_eq!(
        result["masters_after_post"]["state"], "posted_under_changed_masters",
        "{response}"
    );
    assert_eq!(
        masters_check_of(&server, BATCH),
        result["masters_after_post"]
    );
}

/// A verdict replaces a pending check; an observed doubt outranks any later
/// verdict; a check that could not run is not recorded; a verdict that cannot
/// be written leaves the check, and the result, pending.
#[test]
fn a_masters_verdict_never_clears_an_observed_doubt() {
    let directory = tempfile::tempdir().unwrap();
    let server = server_at("127.0.0.1:9".parse().unwrap(), directory.path());
    let imports = server.imports_dir().unwrap();
    let pending = json!({"state":"check_pending"});
    let unchanged = json!({"state":"unchanged","trigger":"masters_moved"});
    let doubt = json!({"state":"posted_under_changed_masters","ledgers":["Cash"]});
    let unavailable = json!({"state":"check_unavailable","trigger":"masters_moved"});
    server.record_masters_check_pending("batch-a").unwrap();
    assert_eq!(
        server.record_masters_verdict("batch-a", unavailable.clone()),
        unavailable
    );
    assert_eq!(
        read_masters_check(&imports, "batch-a"),
        Some(pending.clone())
    );
    assert_eq!(
        server.record_masters_verdict("batch-a", unchanged.clone()),
        unchanged
    );
    assert_eq!(
        server.record_masters_verdict("batch-a", doubt.clone()),
        doubt
    );
    assert_eq!(
        server.record_masters_verdict("batch-a", unchanged.clone()),
        doubt
    );
    assert_eq!(read_masters_check(&imports, "batch-a"), Some(doubt.clone()));

    // The check record cannot be written: a clear verdict stays pending, and
    // an observed doubt is still kept by its own file.
    block(imports.join("batch-b.masters_check.json"));
    assert_eq!(server.record_masters_verdict("batch-b", unchanged), pending);
    assert_eq!(
        server.record_masters_verdict("batch-b", doubt.clone()),
        doubt
    );
    assert!(imports.join("batch-b.masters_doubt.json").is_file());
}

/// Reviewers' A1: the post's own check observes a changed ledger, but its
/// check record is left pending; the ledger is later renamed back. The next
/// readback finds the voucher and would find a clean catalogue, yet the
/// observed doubt stands, and no second check is made.
#[tokio::test]
async fn an_observed_doubt_outlives_a_later_clean_check() {
    let doubt = json!({"state":"posted_under_changed_masters","trigger":"masters_moved","ledgers":["Cash"]});
    let pending = br#"{"state":"check_pending"}"#;
    let (response, observed, scripted, server, _directory) =
        reconcile_with_masters_check_and_doubt(pending, &doubt, paired(catalogue())).await;
    let result = &response["structuredContent"]["result"];
    assert_eq!(
        result["dispatch"]["state"], "reconciliation_required",
        "{response}"
    );
    assert_eq!(result["error"]["code"], "posted_under_changed_masters");
    assert_eq!(
        observed,
        scripted - paired(catalogue()).len(),
        "no second check: {response}"
    );
    assert_eq!(masters_check_of(&server, BATCH), doubt);
    assert!(read_verified_baseline(&server.imports_dir().unwrap(), BATCH).is_none());
}

/// A post whose pending masters record cannot be written is refused before
/// its dispatch intent, so nothing is sent.
#[tokio::test]
async fn a_post_whose_masters_record_cannot_be_written_is_not_sent() {
    // The queue's plans stay in the sequence, so a post that went ahead would
    // be served and observed here.
    let mut plans = before_approval();
    let expected = plans.len();
    plans.extend(after_approval(xml(created_one())));
    let simulator = SequenceSimulator::spawn(with_sentinel(plans)).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let server = server_at(simulator.address(), directory.path());
    let args = saved_captured_batch(&server);
    let imports = server.imports_dir().unwrap();
    block(imports.join(format!("{BATCH}.masters_check.json")));
    let before = journal(directory.path());
    let response = SCRIPTED_APPROVAL
        .scope(
            ScriptedApproval::approving(),
            server.call_tool("post_import", args),
        )
        .await;
    let observed = sent(simulator);
    let result = &response["structuredContent"]["result"];
    assert_eq!(
        result["error"]["code"], "post_masters_record_unavailable",
        "{response}"
    );
    assert_eq!(result["attempt_recorded"], false, "{response}");
    assert_eq!(
        observed.len(),
        expected,
        "nothing past approval: {response}"
    );
    assert_eq!(
        appended_kinds(&before, &journal(directory.path())),
        ["verification_status"]
    );
}

/// A pending mark never erases an observed doubt: a process that lost the
/// dispatch race cannot erase the winner's.
#[test]
fn a_pending_mark_never_erases_a_doubt() {
    let directory = tempfile::tempdir().unwrap();
    let server = server_at("127.0.0.1:9".parse().unwrap(), directory.path());
    let imports = server.imports_dir().unwrap();
    let doubt = json!({"state":"posted_under_changed_masters","ledgers":["Cash"]});
    server.record_masters_check_pending("batch-a").unwrap();
    server.record_masters_verdict("batch-a", doubt.clone());
    server.record_masters_check_pending("batch-a").unwrap();
    assert_eq!(read_masters_check(&imports, "batch-a"), Some(doubt));
}

#[path = "agent_import_ack_tests.rs"]
mod ack_tests;
