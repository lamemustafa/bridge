//! An amendment reuses a batch's wire identity, and only under compare-and-swap.
use super::*;

const ORIGINAL: &str = "bridge-2b1c9f4e-9d3a-4f71-8c2e-5a6b7c8d9e01";
const AMENDMENT: &str = "bridge-7d4e0a1b-3c2f-4e5d-9a8b-1c2d3e4f5a6b";
const UNRELATED: &str = "bridge-0f1e2d3c-4b5a-4968-8776-655443322110";

fn endpoint() -> TallyEndpointConfig {
    TallyEndpointConfig {
        host: "127.0.0.1".into(),
        port: 9001,
    }
}

fn origin() -> String {
    super::super::super::canonical_loopback_origin(&endpoint()).unwrap()
}

fn build(batch_id: &str, amends: Option<&str>, amount: &str, date: &str) -> ImportLedgerLine {
    let mut line: ImportLedgerLine = serde_json::from_value(json!({
        "batch_id":batch_id, "identity_scheme":"batch_v1", "company_guid":GUID,
        "endpoint_origin":origin(),
        "company":{"name":"Synthetic Accounts","guid":GUID,"company_number":"100001","books_from":"20260401"},
        "txn_ids":["txn-001"],"date_from":date,"date_to":date,
        "sha256":"", "built_at":"2026-09-16T00:00:00Z", "status":"built",
        "pre_import_mark":{"kind":"company_high_water","value":1,"master_value":1},
        "vouchers":[{"bridge_txn_id":"txn-001","date":date,"voucher_type":"Payment",
            "entries":[{"ledger":"Expense","amount":amount,"side":"Dr"},
                       {"ledger":"Bank","amount":amount,"side":"Cr"}]}]
    }))
    .unwrap();
    line.amends_batch_id = amends.map(str::to_string);
    line.sha256 = sha256_hex(
        render_import_xml(
            "Synthetic Accounts",
            &line.vouchers,
            line.identity_batch_id(),
        )
        .as_bytes(),
    );
    line
}

fn journal(records: &[&dyn erased::Record]) -> String {
    records.iter().map(|record| record.line()).collect()
}

mod erased {
    pub(super) trait Record {
        fn line(&self) -> String;
    }
    impl<T: serde::Serialize> Record for T {
        fn line(&self) -> String {
            format!("{}\n", serde_json::to_string(self).unwrap())
        }
    }
}

fn lineage_of(text: &str, named: &str) -> Result<amend::Lineage, String> {
    let named = ledger::read_snapshot(std::io::Cursor::new(text.as_bytes()), Some(named))?
        .ok_or("import_amend_batch_not_found")?;
    let builds = ledger::read_lineage(
        std::io::Cursor::new(text.as_bytes()),
        named.batch.identity_batch_id(),
    )?;
    amend::admit_lineage(&named, builds, GUID, &origin())
}

/// A book row as Tally returns it after importing `line`'s only voucher.
fn book_row(line: &ImportLedgerLine) -> ReadVoucher {
    let voucher = &line.vouchers[0];
    ReadVoucher {
        remote_id: Some(format!("{GUID}-00000005")),
        guid: Some(format!("{GUID}-00000005")),
        master_id: Some("5".into()),
        alter_id: Some(40),
        date: Some(voucher.date.clone()),
        voucher_type: Some(voucher.voucher_type.as_str().into()),
        narration: Some(format!("[BRIDGE:{}]", line.attribution_tag(voucher))),
        voucher_number: Some("7".into()),
        cancelled: Some(false),
        optional: Some(false),
        effective_date: None,
        // Tally does not promise the order it was sent (§12a.4).
        entries: voucher
            .entries
            .iter()
            .rev()
            .map(|entry| ReadEntry {
                ledger: entry.ledger.clone(),
                amount: match entry.side {
                    EntrySide::Dr => format!("-{}", entry.amount),
                    EntrySide::Cr => entry.amount.clone(),
                },
                is_deemed_positive: entry.side.tally_positive().into(),
            })
            .collect(),
    }
}

/// Every build of `lineage` as Bridge first verified it: each voucher at the
/// ALTERID `book_row` gives it, so the book matches its baseline (#239).
fn verified_as_booked(lineage: &amend::Lineage) -> amend::VerifiedBaselines {
    amend::VerifiedBaselines(
        lineage
            .builds
            .iter()
            .map(|build| {
                (
                    build.batch.batch_id.clone(),
                    amend::VerifiedBaseline {
                        vouchers: build
                            .batch
                            .vouchers
                            .iter()
                            .map(|voucher| (voucher.bridge_txn_id.clone(), 40))
                            .collect(),
                    },
                )
            })
            .collect(),
    )
}

fn book(rows: Vec<ReadVoucher>) -> ImportReadSource {
    ImportReadSource::admit(rows).unwrap()
}

#[test]
fn an_amendment_carries_the_original_batch_identity_and_nothing_else_does() {
    let original = build(ORIGINAL, None, "12.50", "20260901");
    let amendment = build(AMENDMENT, Some(ORIGINAL), "15.00", "20260901");
    let unrelated = build(UNRELATED, None, "15.00", "20260901");
    let tag = |line: &ImportLedgerLine| line.attribution_tag(&line.vouchers[0]);
    assert_eq!(tag(&amendment), tag(&original));
    assert_ne!(tag(&unrelated), tag(&original));
    // The file itself carries the original REMOTEID, which is what makes a
    // file import alter the voucher rather than create another.
    let xml = render_import_xml(
        "Synthetic Accounts",
        &amendment.vouchers,
        amendment.identity_batch_id(),
    );
    assert!(xml.contains(&format!("REMOTEID=\"{}\"", tag(&original))));
    assert!(xml.contains(&format!("[BRIDGE:{}]", tag(&original))));
}

#[test]
fn an_amendment_record_round_trips_and_an_older_record_reads_as_no_amendment() {
    let amendment = build(AMENDMENT, Some(ORIGINAL), "15.00", "20260901");
    let encoded = serde_json::to_value(&amendment).unwrap();
    assert_eq!(encoded["amends_batch_id"], ORIGINAL);
    let decoded: ImportLedgerLine = serde_json::from_value(encoded).unwrap();
    assert_eq!(decoded.identity_batch_id(), ORIGINAL);

    let original = build(ORIGINAL, None, "12.50", "20260901");
    let encoded = serde_json::to_value(&original).unwrap();
    assert!(encoded.get("amends_batch_id").is_none());
    assert_eq!(
        serde_json::from_value::<ImportLedgerLine>(encoded)
            .unwrap()
            .identity_batch_id(),
        ORIGINAL
    );
}

#[test]
fn a_lineage_holds_the_original_and_its_amendments_and_no_other_batch() {
    let original = build(ORIGINAL, None, "12.50", "20260901");
    let first = build(AMENDMENT, Some(ORIGINAL), "15.00", "20260901");
    let unrelated = build(UNRELATED, None, "15.00", "20260901");
    let text = journal(&[&original, &unrelated, &first]);
    // Naming the amendment resolves to the same lineage as naming the original.
    for named in [ORIGINAL, AMENDMENT] {
        let lineage = lineage_of(&text, named).unwrap();
        assert_eq!(lineage.identity_batch_id, ORIGINAL);
        assert_eq!(
            lineage
                .builds
                .iter()
                .map(|build| build.batch.batch_id.as_str())
                .collect::<Vec<_>>(),
            [ORIGINAL, AMENDMENT]
        );
    }
}

#[test]
fn any_native_dispatch_in_the_lineage_refuses_the_amendment() {
    // A native post carried the marker beside a random private REMOTEID, so an
    // import under the batch identity would create a second voucher.
    let original = build(ORIGINAL, None, "12.50", "20260901");
    let first = build(AMENDMENT, Some(ORIGINAL), "15.00", "20260901");
    let text = journal(&[
        &original,
        &ledger::StatusRecord::dispatch(&original),
        &first,
    ]);
    assert_eq!(
        lineage_of(&text, AMENDMENT).err().unwrap(),
        "import_amend_natively_posted"
    );
    // Control: the same lineage without the dispatch is admitted.
    assert!(lineage_of(&journal(&[&original, &first]), AMENDMENT).is_ok());
}

#[test]
fn a_lineage_from_another_company_endpoint_or_identity_scheme_is_refused() {
    let original = build(ORIGINAL, None, "12.50", "20260901");
    let text = journal(&[&original]);
    let named = ledger::read_snapshot(std::io::Cursor::new(text.as_bytes()), Some(ORIGINAL))
        .unwrap()
        .unwrap();
    let builds = || ledger::read_lineage(std::io::Cursor::new(text.as_bytes()), ORIGINAL).unwrap();
    assert_eq!(
        amend::admit_lineage(
            &named,
            builds(),
            "00000000-0000-4000-8000-000000000009",
            &origin()
        )
        .err()
        .unwrap(),
        "import_batch_company_mismatch"
    );
    assert_eq!(
        amend::admit_lineage(&named, builds(), GUID, "http://127.0.0.1:9000")
            .err()
            .unwrap(),
        "import_amend_endpoint_mismatch"
    );
    let mut legacy = original.clone();
    legacy.identity_scheme = None;
    let text = journal(&[&legacy]);
    assert_eq!(
        lineage_of(&text, ORIGINAL).err().unwrap(),
        "import_amend_identity_scheme_unsupported"
    );
    // An amendment whose original is not in this journal has no lineage root.
    let orphan = build(AMENDMENT, Some(ORIGINAL), "15.00", "20260901");
    assert_eq!(
        lineage_of(&journal(&[&orphan]), AMENDMENT).err().unwrap(),
        "import_amend_lineage_invalid"
    );
}

#[test]
fn a_proposal_may_correct_amounts_and_dates_but_not_type_number_or_membership() {
    let original = build(ORIGINAL, None, "12.50", "20260901");
    let lineage = lineage_of(&journal(&[&original]), ORIGINAL).unwrap();
    let mut proposal = build(AMENDMENT, None, "20.00", "20260903").vouchers;
    assert_eq!(lineage.admit_proposal(&proposal), Ok(()));

    let mut retyped = proposal.clone();
    retyped[0].voucher_type = VoucherType::Receipt;
    assert_eq!(
        lineage.admit_proposal(&retyped).err().unwrap(),
        "import_amend_voucher_type_changed"
    );
    let mut renumbered = proposal.clone();
    renumbered[0].voucher_number = Some("9".into());
    assert_eq!(
        lineage.admit_proposal(&renumbered).err().unwrap(),
        "import_amend_voucher_number_changed"
    );
    proposal[0].bridge_txn_id = "txn-999".into();
    assert_eq!(
        lineage.admit_proposal(&proposal).err().unwrap(),
        "import_amend_txn_not_in_batch"
    );
}

#[test]
fn the_window_holds_where_a_voucher_is_and_where_the_amendment_moves_it() {
    let original = build(ORIGINAL, None, "12.50", "20260910");
    let lineage = lineage_of(&journal(&[&original]), ORIGINAL).unwrap();
    let earlier = build(AMENDMENT, None, "12.50", "20260902").vouchers;
    assert_eq!(
        lineage.window(&earlier),
        ("20260902".into(), "20260910".into())
    );
    let later = build(AMENDMENT, None, "12.50", "20260920").vouchers;
    assert_eq!(
        lineage.window(&later),
        ("20260910".into(), "20260920".into())
    );
}

#[test]
fn a_voucher_still_as_any_build_wrote_it_is_admitted() {
    let original = build(ORIGINAL, None, "12.50", "20260901");
    let first = build(AMENDMENT, Some(ORIGINAL), "15.00", "20260902");
    let lineage = lineage_of(&journal(&[&original, &first]), ORIGINAL).unwrap();
    let proposal = build(UNRELATED, None, "18.00", "20260903").vouchers;
    // The book may hold the original (the first amendment was never imported)
    // or the first amendment (it was): both are versions Bridge wrote.
    for (row, expected) in [
        (book_row(&original), ORIGINAL),
        (book_row(&first), AMENDMENT),
    ] {
        let admitted = lineage
            .compare_and_swap(&proposal, &book(vec![row]), &verified_as_booked(&lineage))
            .unwrap()
            .expect("book holds a version Bridge built");
        assert_eq!(admitted[0]["book_matches_batch_id"], expected);
    }
}

#[test]
fn an_edited_missing_or_cancelled_voucher_refuses_the_amendment() {
    let original = build(ORIGINAL, None, "12.50", "20260901");
    let lineage = lineage_of(&journal(&[&original]), ORIGINAL).unwrap();
    let proposal = build(AMENDMENT, None, "18.00", "20260901").vouchers;
    let reason = |rows: Vec<ReadVoucher>| {
        lineage
            .compare_and_swap(&proposal, &book(rows), &verified_as_booked(&lineage))
            .unwrap()
            .expect_err("refused")[0]["reason"]
            .clone()
    };

    // Someone changed the amount in Tally after Bridge built it: an amendment
    // would silently overwrite that change.
    let mut edited = book_row(&original);
    edited.entries[0].amount = "13.00".into();
    edited.entries[1].amount = "-13.00".into();
    assert_eq!(reason(vec![edited]), "book_voucher_diverged");
    // The import rewrites the narration as well, so an edit to it is a change.
    let mut renarrated = book_row(&original);
    renarrated.narration = Some(format!(
        "Paid by cheque [BRIDGE:{}]",
        original.attribution_tag(&original.vouchers[0])
    ));
    assert_eq!(reason(vec![renarrated]), "book_voucher_diverged");
    let mut redated = book_row(&original);
    redated.date = Some("20260905".into());
    assert_eq!(reason(vec![redated]), "book_voucher_diverged");
    // an edited effective date is a change the amendment would overwrite
    let mut effective_redated = book_row(&original);
    effective_redated.effective_date = Some("20260905".into());
    let refused = lineage
        .compare_and_swap(
            &proposal,
            &book(vec![effective_redated]),
            &verified_as_booked(&lineage),
        )
        .unwrap()
        .expect_err("refused");
    assert_eq!(refused[0]["reason"], "book_voucher_diverged");
    assert_eq!(
        refused[0]["diffs_from_latest_build"],
        json!(["effective_date"])
    );
    // one that came back unchanged is admitted without a caveat; one that did
    // not come back is admitted with it
    let mut effective_kept = book_row(&original);
    effective_kept.effective_date = effective_kept.date.clone();
    let admitted = lineage
        .compare_and_swap(
            &proposal,
            &book(vec![effective_kept]),
            &verified_as_booked(&lineage),
        )
        .unwrap()
        .unwrap();
    assert!(admitted[0].get("not_observed").is_none());
    let admitted = lineage
        .compare_and_swap(
            &proposal,
            &book(vec![book_row(&original)]),
            &verified_as_booked(&lineage),
        )
        .unwrap()
        .unwrap();
    assert_eq!(admitted[0]["not_observed"], json!(["effective_date"]));

    assert_eq!(reason(vec![]), "not_in_book");
    // A voucher carrying another batch's marker is not this one.
    let unrelated = build(UNRELATED, None, "12.50", "20260901");
    assert_eq!(reason(vec![book_row(&unrelated)]), "not_in_book");

    let mut cancelled = book_row(&original);
    cancelled.cancelled = Some(true);
    assert_eq!(reason(vec![cancelled]), "voucher_cancelled_or_optional");

    // Control: the unedited row is admitted by the same lineage and proposal.
    assert!(lineage
        .compare_and_swap(
            &proposal,
            &book(vec![book_row(&original)]),
            &verified_as_booked(&lineage)
        )
        .unwrap()
        .is_ok());
}

#[test]
fn one_diverged_voucher_refuses_the_whole_amendment() {
    let mut original = build(ORIGINAL, None, "12.50", "20260901");
    let mut second = original.vouchers[0].clone();
    second.bridge_txn_id = "txn-002".into();
    original.vouchers.push(second);
    let lineage = lineage_of(&journal(&[&original]), ORIGINAL).unwrap();
    let mut rows = vec![book_row(&original)];
    let mut other = book_row(&original);
    other.guid = Some(format!("{GUID}-00000006"));
    other.master_id = Some("6".into());
    other.narration = Some(format!(
        "[BRIDGE:{}]",
        original.attribution_tag(&original.vouchers[1])
    ));
    other.entries[0].amount = "99.00".into();
    other.entries[1].amount = "-99.00".into();
    rows.push(other);
    let refused = lineage
        .compare_and_swap(
            &original.vouchers,
            &book(rows),
            &verified_as_booked(&lineage),
        )
        .unwrap()
        .expect_err("one divergence refuses the batch");
    assert_eq!(refused.len(), 1);
    assert_eq!(refused[0]["bridge_txn_id"], "txn-002");
}

#[test]
fn an_amendment_is_never_eligible_for_native_posting() {
    // Native posting renders a fresh private REMOTEID and can only create.
    let mut line: ImportLedgerLine = serde_json::from_value(json!({
        "batch_id":AMENDMENT, "identity_scheme":"batch_v1", "amends_batch_id":ORIGINAL,
        "company_guid":GUID, "endpoint_origin":origin(),
        "company":{"name":"Synthetic Accounts","guid":GUID,"company_number":"100001","books_from":"20260401"},
        "txn_ids":["journal-test"],"date_from":"20260901","date_to":"20260901",
        "sha256":"", "built_at":"2026-09-16T00:00:00Z", "status":"built",
        "pre_import_mark":{"kind":"company_high_water","value":1,"master_value":1},
        "vouchers":[{"bridge_txn_id":"journal-test","date":"20260901","voucher_type":"Journal",
            "entries":[{"ledger":"Expense","amount":"12.50","side":"Dr"},
                       {"ledger":"Cash","amount":"12.50","side":"Cr"}]}]
    }))
    .unwrap();
    line.sha256 = sha256_hex(
        render_import_xml(
            "Synthetic Accounts",
            &line.vouchers,
            line.identity_batch_id(),
        )
        .as_bytes(),
    );
    assert_eq!(
        post::admit_saved_journal_integrity(&line, &endpoint())
            .err()
            .unwrap(),
        "import_post_amendment_requires_file_import"
    );
    // Control: the same Journal as an original batch is admitted.
    line.amends_batch_id = None;
    line.batch_id = ORIGINAL.into();
    line.sha256 = sha256_hex(
        render_import_xml("Synthetic Accounts", &line.vouchers, &line.batch_id).as_bytes(),
    );
    assert!(post::admit_saved_journal_integrity(&line, &endpoint()).is_ok());
}

#[test]
fn the_build_schema_admits_amends_batch_id_and_the_payload_parses_it() {
    let schema = voucher_input_schema();
    // The schema states the rule the server enforces with valid_batch_id: the
    // lowercase hyphenated spelling of a random (version 4) UUID.
    assert_eq!(
        schema["properties"]["amends_batch_id"]["pattern"],
        "^bridge-[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$"
    );
    assert!(valid_batch_id(ORIGINAL));
    assert!(!valid_batch_id(
        "bridge-2B1C9F4E-9D3A-4F71-8C2E-5A6B7C8D9E01"
    ));
    let mut input = serde_json::to_value(payload()).unwrap();
    input["amends_batch_id"] = json!(ORIGINAL);
    assert_eq!(
        parse_payload(&input).unwrap().amends_batch_id.as_deref(),
        Some(ORIGINAL)
    );
}

#[tokio::test]
async fn an_unknown_amendment_target_is_refused_before_any_tally_read() {
    let directory = tempfile::tempdir().unwrap();
    // Port 9 is never answered; reaching Tally would fail differently.
    let server = Server::new(crate::agent::Settings {
        endpoint: endpoint_on_port(9),
        data_dir: directory.path().to_path_buf(),
        max_rows: 10,
        max_bytes: 200_000,
        redaction: crate::agent::Redaction::None,
        import_enabled: true,
        writes_enabled: false,
    });
    let mut input = serde_json::to_value(payload()).unwrap();
    input["amends_batch_id"] = json!(ORIGINAL);
    let failure = server.build_import_xml(&input).await.err().unwrap();
    assert_eq!(failure.code, "import_amend_batch_not_found");
    assert!(failure.evidence.is_none());
    input["amends_batch_id"] = json!("bridge-not-a-uuid");
    let failure = server.build_import_xml(&input).await.err().unwrap();
    assert_eq!(failure.code, "import_amend_batch_id_invalid");
}

fn endpoint_on_port(port: u16) -> TallyEndpointConfig {
    TallyEndpointConfig {
        host: "127.0.0.1".into(),
        port,
    }
}

fn simulated_server(directory: &std::path::Path, port: u16) -> Server {
    Server::new(crate::agent::Settings {
        endpoint: endpoint_on_port(port),
        data_dir: directory.to_path_buf(),
        max_rows: 10,
        max_bytes: 200_000,
        redaction: crate::agent::Redaction::None,
        import_enabled: true,
        writes_enabled: false,
    })
}

/// The Journal build sequence, with the preflight read answered by `book`.
fn build_plans_reading(book: Option<String>) -> Vec<ScenarioPlan> {
    let mut plans = qualified_import_cycle_plans()[..32].to_vec();
    if let Some(book) = book {
        // probe (2) + first cycle (16) + repeated catalogue (6), then the
        // preflight read whose responses sit at offsets 1 and 3.
        for index in [25, 27] {
            plans[index].fixture = Fixture::SyntheticXml(book.clone());
            plans[index].encoding = WireEncoding::Utf16Le;
        }
    }
    plans
}

fn book_holding(tag: &str, amount: &str) -> String {
    format!(
        "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>\
         <VOUCHER REMOTEID=\"{CAPTURED_GUID}-00000001\"><DATE>20260901</DATE><VOUCHERNUMBER>1</VOUCHERNUMBER>\
         <VOUCHERTYPENAME>Journal</VOUCHERTYPENAME><GUID>{CAPTURED_GUID}-00000001</GUID><MASTERID>1</MASTERID>\
         <ALTERID>12</ALTERID><NARRATION>Paid &amp; settled [BRIDGE:{tag}]</NARRATION>\
         <ISCANCELLED>No</ISCANCELLED><ISOPTIONAL>No</ISOPTIONAL>\
         <ALLLEDGERENTRIES.LIST><LEDGERNAME>Cash</LEDGERNAME><ISDEEMEDPOSITIVE>No</ISDEEMEDPOSITIVE><AMOUNT>{amount}</AMOUNT></ALLLEDGERENTRIES.LIST>\
         <ALLLEDGERENTRIES.LIST><LEDGERNAME>Bridge Nested Debtor WR4</LEDGERNAME><ISDEEMEDPOSITIVE>Yes</ISDEEMEDPOSITIVE><AMOUNT>-{amount}</AMOUNT></ALLLEDGERENTRIES.LIST>\
         </VOUCHER></COLLECTION></DATA></BODY></ENVELOPE>"
    )
}

/// Record the captured two-Journal batch as this server built it: same
/// company tuple and endpoint, so only the book decides the amendment.
fn seed_original(server: &Server) {
    let mut vouchers = captured_catalogue_payload().vouchers;
    for voucher in &mut vouchers {
        voucher.date = normalized_date(&voucher.date).unwrap();
    }
    let line: ImportLedgerLine = serde_json::from_value(json!({
        "batch_id":ORIGINAL, "identity_scheme":"batch_v1", "company_guid":CAPTURED_GUID,
        "endpoint_origin":super::super::super::canonical_loopback_origin(&server.settings.endpoint).unwrap(),
        "company":{"name":"WR2 Unicode Lab","guid":CAPTURED_GUID,"company_number":"1","books_from":"20260401"},
        "txn_ids":["txn-001","txn-002"],"date_from":"20260901","date_to":"20260902",
        "sha256":"a".repeat(64), "built_at":"2026-09-16T00:00:00Z", "status":"posted_verified",
        "pre_import_mark":{"kind":"company_high_water","value":1,"master_value":1},
        "vouchers":vouchers
    }))
    .unwrap();
    server.append_import_ledger(&line).unwrap();
}

/// Record the original batch's first verification, as verify_import does,
/// with `txn-001` at `alter_id` (#239).
fn seed_baseline(server: &Server, alter_id: u64) {
    record_verified_baseline(
        &server.imports_dir().unwrap(),
        ORIGINAL,
        &json!({"vouchers":[{"bridge_txn_id":"txn-001","status":"posted_verified","alter_id":alter_id}]}),
    )
    .unwrap();
}

fn amendment_payload(original: &str, amount: &str) -> Value {
    let mut input = captured_catalogue_payload();
    input.vouchers.truncate(1);
    for entry in &mut input.vouchers[0].entries {
        entry.amount = amount.into();
    }
    input.amends_batch_id = Some(original.into());
    serde_json::to_value(input).unwrap()
}

#[tokio::test]
async fn an_amendment_built_against_an_unchanged_book_reuses_the_original_remote_id() {
    let directory = tempfile::tempdir().unwrap();
    let original = ORIGINAL.to_string();
    let tag = import_identity(&original, "txn-001").to_string();
    let simulator =
        SequenceSimulator::spawn(build_plans_reading(Some(book_holding(&tag, "12.50")))).unwrap();
    let server = simulated_server(directory.path(), simulator.address().port());
    seed_original(&server);
    // The simulated book serves the voucher at ALTERID 12, as first verified.
    seed_baseline(&server, 12);
    let built = server
        .build_import_xml(&amendment_payload(&original, "15.00"))
        .await
        .unwrap();
    assert_eq!(simulator.finish().unwrap().len(), 32);
    let result = &built.payload["result"];
    let batch_id = result["batch_id"].as_str().unwrap();
    assert_ne!(batch_id, original, "an amendment is its own build");
    assert_eq!(result["amendment"]["identity_batch_id"], original.as_str());
    assert_eq!(
        result["amendment"]["vouchers"][0]["book_matches_batch_id"],
        original.as_str()
    );
    assert!(result["warnings"][0]
        .as_str()
        .unwrap()
        .starts_with("This file amends an earlier batch."));
    // The warning names what is not compared, and the next message says why
    // the file must be imported by hand, not the generic native-post reason.
    let warning = result["warnings"][0].as_str().unwrap();
    for named in [
        "reference",
        "bill-wise or cost-centre allocations",
        "Import promptly",
    ] {
        assert!(warning.contains(named), "{named} in {warning}");
    }
    assert!(result["warnings"][1]
        .as_str()
        .unwrap()
        .contains("Bridge does not post amendments"));
    assert!(result["next_step"]
        .as_str()
        .unwrap()
        .starts_with("Import promptly"));

    let xml = std::fs::read_to_string(
        directory
            .path()
            .join("imports")
            .join(format!("{batch_id}.xml")),
    )
    .unwrap();
    assert!(xml.contains(&format!("REMOTEID=\"{tag}\"")));
    assert!(xml.contains("<AMOUNT>15.00</AMOUNT>"));

    let recorded = server
        .import_ledger()
        .unwrap()
        .into_iter()
        .find(|line| line.batch_id == batch_id)
        .unwrap();
    assert_eq!(recorded.amends_batch_id.as_deref(), Some(original.as_str()));
    assert_eq!(recorded.attribution_tag(&recorded.vouchers[0]), tag);
}

#[tokio::test]
async fn an_amendment_of_a_voucher_edited_in_tally_writes_nothing() {
    let directory = tempfile::tempdir().unwrap();
    let original = ORIGINAL.to_string();
    let tag = import_identity(&original, "txn-001").to_string();
    // Someone changed 12.50 to 13.00 in Tally after Bridge built it. The
    // refusal follows the preflight read, before the closing mode probe.
    let simulator = SequenceSimulator::spawn(
        build_plans_reading(Some(book_holding(&tag, "13.00")))[..30].to_vec(),
    )
    .unwrap();
    let server = simulated_server(directory.path(), simulator.address().port());
    seed_original(&server);
    let imports = server.imports_dir().unwrap();
    let files_before = std::fs::read_dir(&imports).unwrap().count();
    let batches_before = server.import_ledger().unwrap().len();
    let refused = server
        .build_import_xml(&amendment_payload(&original, "15.00"))
        .await
        .unwrap();
    let result = &refused.payload["result"];
    assert_eq!(result["state"], "refused");
    assert_eq!(result["reason"], "amended_vouchers_not_as_built");
    assert_eq!(
        result["refused_vouchers"][0]["reason"],
        "book_voucher_diverged"
    );
    assert_eq!(std::fs::read_dir(&imports).unwrap().count(), files_before);
    assert_eq!(server.import_ledger().unwrap().len(), batches_before);
    assert_eq!(simulator.finish().unwrap().len(), 30);
}

#[test]
fn the_amendment_admission_module_stays_pinned() {
    // The compatibility gate cannot see a pin dropped by a merge resolution.
    // This module decides what an import file may overwrite; its reason sits
    // beside MAX_SURFACE_FILES.
    let surface: Value = serde_json::from_str(include_str!(
        "../../docs/tally/compatibility/compatibility-surface.json"
    ))
    .unwrap();
    assert!(surface["files"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["path"] == "src-tauri/src/agent_import_amend.rs"));
}

// bridge#239: an amendment also refuses a voucher Tally altered since Bridge
// first verified it, which catches edits to fields the read does not carry.

#[test]
fn a_voucher_altered_since_its_first_verification_refuses_the_amendment() {
    let original = build(ORIGINAL, None, "12.50", "20260901");
    let lineage = lineage_of(&journal(&[&original]), ORIGINAL).unwrap();
    let proposal = build(AMENDMENT, None, "18.00", "20260901").vouchers;
    let verified = verified_as_booked(&lineage);
    let refusal = |alter_id: Option<u64>, baselines: &amend::VerifiedBaselines| {
        let mut row = book_row(&original);
        row.alter_id = alter_id;
        lineage
            .compare_and_swap(&proposal, &book(vec![row]), baselines)
            .unwrap()
            .err()
            .map(|refused| refused[0]["reason"].clone())
    };
    // The book's ALTERID equals the one first verified: admitted.
    assert_eq!(refusal(Some(40), &verified), None);
    // Altered since, by a reference or allocation edit the fields cannot
    // show; a lower mark is a change too.
    for moved in [Some(41), Some(39), None] {
        assert_eq!(
            refusal(moved, &verified),
            Some(json!("voucher_altered_since_verified")),
            "{moved:?}"
        );
    }
    // No verified baseline for the build the book matches.
    assert_eq!(
        refusal(Some(40), &amend::VerifiedBaselines::default()),
        Some(json!("voucher_never_verified"))
    );
}

#[test]
fn a_verified_baseline_records_each_voucher_once_and_never_changes_it() {
    let proof = |alter_id: u64, txn_id: &str, status: &str| json!({"vouchers":[{"bridge_txn_id":txn_id,"status":status,"alter_id":alter_id}]});
    let mut baseline = amend::VerifiedBaseline::default();
    // Only posted_verified is recorded.
    assert!(!amend::record_first_verified(
        &mut baseline,
        &proof(38, "txn-001", "posted_divergent")
    ));
    assert!(amend::record_first_verified(
        &mut baseline,
        &proof(40, "txn-001", "posted_verified")
    ));
    // A later verification, after an edit, cannot move it.
    assert!(!amend::record_first_verified(
        &mut baseline,
        &proof(41, "txn-001", "posted_verified")
    ));
    assert!(amend::record_first_verified(
        &mut baseline,
        &proof(52, "txn-002", "posted_verified")
    ));
    assert_eq!(
        baseline.vouchers,
        [("txn-001".to_string(), 40), ("txn-002".to_string(), 52)]
            .into_iter()
            .collect()
    );
}

/// An amendment of a voucher whose fields still match but whose ALTERID moved
/// since Bridge first verified it (a reference or allocation edit), or of a
/// build Bridge never verified, writes nothing (#239).
#[tokio::test]
async fn an_amendment_of_a_voucher_altered_or_never_verified_writes_nothing() {
    for (baseline, reason) in [
        (Some(11), "voucher_altered_since_verified"),
        (None, "voucher_never_verified"),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let original = ORIGINAL.to_string();
        let tag = import_identity(&original, "txn-001").to_string();
        let simulator = SequenceSimulator::spawn(
            build_plans_reading(Some(book_holding(&tag, "12.50")))[..30].to_vec(),
        )
        .unwrap();
        let server = simulated_server(directory.path(), simulator.address().port());
        seed_original(&server);
        if let Some(alter_id) = baseline {
            seed_baseline(&server, alter_id);
        }
        let imports = server.imports_dir().unwrap();
        let files_before = std::fs::read_dir(&imports).unwrap().count();
        let refused = server
            .build_import_xml(&amendment_payload(&original, "15.00"))
            .await
            .unwrap();
        let result = &refused.payload["result"];
        assert_eq!(
            result["reason"], "amended_vouchers_not_as_built",
            "{result}"
        );
        assert_eq!(result["refused_vouchers"][0]["reason"], reason, "{result}");
        assert!(result["next_step"]
            .as_str()
            .unwrap()
            .contains("Correct the voucher in Tally directly"));
        assert_eq!(std::fs::read_dir(&imports).unwrap().count(), files_before);
        assert_eq!(simulator.finish().unwrap().len(), 30);
    }
}

#[test]
fn a_verified_baseline_file_is_written_once_per_voucher() {
    let directory = tempfile::tempdir().unwrap();
    let proof = |alter_id: u64| json!({"vouchers":[{"bridge_txn_id":"txn-001","status":"posted_verified","alter_id":alter_id}]});
    record_verified_baseline(directory.path(), ORIGINAL, &proof(40)).unwrap();
    let first = std::fs::read(directory.path().join(format!("{ORIGINAL}.baseline.json"))).unwrap();
    // A later verification after an edit reports the voucher posted at a
    // higher ALTERID; the file keeps the first.
    record_verified_baseline(directory.path(), ORIGINAL, &proof(41)).unwrap();
    assert_eq!(
        std::fs::read(directory.path().join(format!("{ORIGINAL}.baseline.json"))).unwrap(),
        first
    );
    assert_eq!(
        read_verified_baseline(directory.path(), ORIGINAL)
            .unwrap()
            .vouchers["txn-001"],
        40
    );
    // An unreadable file is never rewritten.
    std::fs::write(
        directory.path().join(format!("{ORIGINAL}.baseline.json")),
        b"{",
    )
    .unwrap();
    assert!(record_verified_baseline(directory.path(), ORIGINAL, &proof(42)).is_err());
    assert_eq!(read_verified_baseline(directory.path(), ORIGINAL), None);
}

fn baselines(pairs: &[(&str, u64)]) -> amend::VerifiedBaselines {
    amend::VerifiedBaselines(
        pairs
            .iter()
            .map(|(batch_id, alter_id)| {
                (
                    batch_id.to_string(),
                    amend::VerifiedBaseline {
                        vouchers: [("txn-001".to_string(), *alter_id)].into_iter().collect(),
                    },
                )
            })
            .collect(),
    )
}

/// An amendment built but never imported, differing only in a field the read
/// does not carry, matches the book too; it has no verification of its own and
/// must not hide the build that was verified (#239).
#[test]
fn an_unimported_amendment_does_not_hide_the_verified_build() {
    let original = build(ORIGINAL, None, "12.50", "20260901");
    // Same compared fields as the original: it matches the untouched book.
    let stale = build(AMENDMENT, Some(ORIGINAL), "12.50", "20260901");
    let lineage = lineage_of(&journal(&[&original, &stale]), ORIGINAL).unwrap();
    let proposal = build(UNRELATED, None, "18.00", "20260901").vouchers;
    let admitted = lineage
        .compare_and_swap(
            &proposal,
            &book(vec![book_row(&original)]),
            &baselines(&[(ORIGINAL, 40)]),
        )
        .unwrap()
        .expect("the verified original still vouches for the book");
    assert_eq!(admitted[0]["book_matches_batch_id"], ORIGINAL);
}

/// The usual flow: the original verified, an amendment imported and verified,
/// then a second amendment. The book matches the first amendment only, and its
/// own verification decides (#239).
#[test]
fn a_second_amendment_is_decided_by_the_first_amendments_verification() {
    let original = build(ORIGINAL, None, "12.50", "20260901");
    let first = build(AMENDMENT, Some(ORIGINAL), "15.00", "20260901");
    let lineage = lineage_of(&journal(&[&original, &first]), ORIGINAL).unwrap();
    let proposal = build(UNRELATED, None, "18.00", "20260901").vouchers;
    let refusal = |alter_id: u64, recorded: &[(&str, u64)]| {
        let mut row = book_row(&first);
        row.alter_id = Some(alter_id);
        lineage
            .compare_and_swap(&proposal, &book(vec![row]), &baselines(recorded))
            .unwrap()
            .err()
            .map(|refused| refused[0]["reason"].clone())
    };
    // Imported and verified at 41: admitted.
    assert_eq!(refusal(41, &[(ORIGINAL, 40), (AMENDMENT, 41)]), None);
    // Altered since that verification.
    assert_eq!(
        refusal(42, &[(ORIGINAL, 40), (AMENDMENT, 41)]),
        Some(json!("voucher_altered_since_verified"))
    );
    // The amendment was imported but never verified: the original's record
    // does not vouch for a book that no longer matches the original.
    assert_eq!(
        refusal(41, &[(ORIGINAL, 40)]),
        Some(json!("voucher_never_verified"))
    );
}

/// A reference-only amendment, imported and verified: the book matches both
/// builds' compared fields, and only the amendment's record equals the book's
/// ALTERID. The equal record admits it, whichever build comes first (#239).
#[test]
fn the_build_whose_record_equals_the_book_admits_even_when_not_first() {
    let original = build(ORIGINAL, None, "12.50", "20260901");
    let reference_only = build(AMENDMENT, Some(ORIGINAL), "12.50", "20260901");
    let lineage = lineage_of(&journal(&[&original, &reference_only]), ORIGINAL).unwrap();
    let proposal = build(UNRELATED, None, "18.00", "20260901").vouchers;
    let mut row = book_row(&original);
    row.alter_id = Some(41);
    let admitted = lineage
        .compare_and_swap(
            &proposal,
            &book(vec![row]),
            &baselines(&[(ORIGINAL, 40), (AMENDMENT, 41)]),
        )
        .unwrap()
        .expect("the amendment's own record equals the book");
    assert_eq!(admitted[0]["book_matches_batch_id"], AMENDMENT);
    // Altered since both readings: refused, naming the latest matching build
    // with a record.
    let mut altered = book_row(&original);
    altered.alter_id = Some(42);
    let refused = lineage
        .compare_and_swap(
            &proposal,
            &book(vec![altered]),
            &baselines(&[(ORIGINAL, 40), (AMENDMENT, 41)]),
        )
        .unwrap()
        .expect_err("no record equals the book");
    assert_eq!(refused[0]["reason"], "voucher_altered_since_verified");
    assert_eq!(refused[0]["book_matches_batch_id"], AMENDMENT);
    assert_eq!(refused[0]["verified_alter_id"], 41);
    // An unread ALTERID never matches a missing record: nothing is verified.
    let mut unread = book_row(&original);
    unread.alter_id = None;
    let refused = lineage
        .compare_and_swap(&proposal, &book(vec![unread]), &baselines(&[]))
        .unwrap()
        .expect_err("an unread ALTERID and no record prove nothing");
    assert_eq!(refused[0]["reason"], "voucher_never_verified");
}

#[test]
fn a_failed_baseline_write_leaves_the_previous_file_whole() {
    let directory = tempfile::tempdir().unwrap();
    let proof = |txn_id: &str, alter_id: u64| json!({"vouchers":[{"bridge_txn_id":txn_id,"status":"posted_verified","alter_id":alter_id}]});
    record_verified_baseline(directory.path(), ORIGINAL, &proof("txn-001", 40)).unwrap();
    let path = directory.path().join(format!("{ORIGINAL}.baseline.json"));
    let before = std::fs::read(&path).unwrap();
    // The staged file cannot be written: the write fails before the rename.
    std::fs::create_dir(
        directory
            .path()
            .join(format!("{ORIGINAL}.baseline.json.next")),
    )
    .unwrap();
    assert!(record_verified_baseline(directory.path(), ORIGINAL, &proof("txn-002", 52)).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
}
