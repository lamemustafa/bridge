//! Payment, Receipt and Contra: the file shape, and the cash/bank leg gate.
//!
//! The masters here are two byte-exact live captures of the same company, so
//! the group semantics under test — reserved identities, a user-created group
//! between a ledger and its predefined ancestor, the control-marked account
//! root — are Tally's own and not this repository's idea of them.
use super::*;
use bridge_tally_protocol::{PartyLedgerMasterFieldObservation, TallyNamedMaster};

fn captured_group_collection() -> String {
    let bytes = include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-party-groups.utf16le.xml"
    );
    String::from_utf16(
        &bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>(),
    )
    .expect("captured native group collection")
}

fn captured_groups() -> Vec<TallyNamedMaster> {
    bridge_tally_protocol::native_outstandings::parse_native_group_snapshot(
        &captured_group_collection(),
        CAPTURED_GUID,
    )
    .expect("captured group rows")
}

fn captured_ledger_parents() -> Vec<(String, Option<String>)> {
    let bytes = include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-ledger-catalogue.utf16le.xml"
    );
    let xml = String::from_utf16(
        &bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>(),
    )
    .expect("captured native catalogue");
    parse_standard_ledger_catalog_response(&xml, "WR2 Unicode Lab", CAPTURED_GUID)
        .expect("captured catalogue rows")
        .parents()
        .map(|(name, parent)| (name.to_string(), parent.map(str::to_string)))
        .collect()
}

fn observed(
    ledgers: &[(String, Option<String>)],
    groups: Vec<TallyNamedMaster>,
) -> ObservedMasters {
    ObservedMasters::new(
        ledgers
            .iter()
            .map(|(name, parent)| (name.as_str(), parent.as_deref())),
        groups,
    )
}

fn under(group: &str) -> Vec<(String, Option<String>)> {
    vec![("Probe Ledger".to_string(), Some(group.to_string()))]
}

/// The 28-group `List of Groups` capture from the licensed 7.1 demo company,
/// and the 88-row ledger capture from that same company. Together they carry
/// real ledgers sitting under real cash and bank groups, which the smaller
/// company used elsewhere in this file does not: its only money ledger is
/// `Cash`. See `fixtures/native/PROVENANCE.md`.
const AARAV_GUID: &str = "bb8ad19e-6aef-4239-a917-87fec0c6215e";

fn captured_demo_groups() -> Vec<TallyNamedMaster> {
    bridge_tally_protocol::native_outstandings::parse_native_group_snapshot(
        include_str!(
            "../crates/bridge-tally-protocol/tests/fixtures/native/group_snapshot_aarav_with_computed_company_guid.xml"
        ),
        AARAV_GUID,
    )
    .expect("captured demo group rows")
}

fn captured_demo_ledger_parents() -> Vec<(String, Option<String>)> {
    let bytes = include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/native/ledgers_native_aarav.utf16le.xml"
    );
    let xml = bridge_tally_protocol::decode_tally_xml_response_bytes_limited(
        bytes,
        "text/xml; charset=utf-16",
        bridge_tally_protocol::ExpectedTallyTextEncoding::Utf16Le,
        bytes.len(),
    )
    .expect("captured BOM-less UTF-16LE ledger response")
    .text;
    bridge_tally_protocol::parse_native_ledger_source_records_with_evidence(&xml, AARAV_GUID)
        .expect("captured demo ledger rows")
        .records
        .into_iter()
        .map(|source| {
            (
                source.record.name,
                source
                    .record
                    .parent
                    .nonempty_returned_text()
                    .map(str::to_string),
            )
        })
        .collect()
}

#[test]
fn captured_ledgers_under_captured_money_groups_are_established() {
    // Both sides of every edge here are verbatim live captures of one company,
    // parsed by the production readers. Nothing about a ledger's relationship
    // to `Bank Accounts` or `Cash-in-Hand` is authored by this test, so a
    // captured bank row that did not look the way the gate assumes would fail
    // it rather than be assumed away.
    let ledgers = captured_demo_ledger_parents();
    assert_eq!(ledgers.len(), 88, "the whole captured catalogue is swept");
    let masters = observed(&ledgers, captured_demo_groups());
    for (ledger, reserved_group) in [
        ("HDFC Bank Current Account", "Bank Accounts"),
        ("ICICI Bank CA 4471", "Bank Accounts"),
        ("Cash", "Cash-in-Hand"),
        ("Petty Cash", "Cash-in-Hand"),
        ("Petty Cash Counter", "Cash-in-Hand"),
    ] {
        assert_eq!(
            masters.classify(ledger),
            CashBankState::Established { reserved_group },
            "captured ledger {ledger}"
        );
    }
    let established = ledgers
        .iter()
        .filter(|(name, _)| LegRequirement::Money.admits(&masters.classify(name)))
        .count();
    assert_eq!(
        established, 5,
        "no other captured ledger of the 88 is admitted as money"
    );
}

#[test]
fn captured_masters_establish_cash_and_refuse_every_other_captured_ledger() {
    let ledgers = captured_ledger_parents();
    let masters = observed(&ledgers, captured_groups());
    // `Cash` sits directly under the reserved Cash-in-Hand group.
    assert_eq!(
        masters.classify("Cash"),
        CashBankState::Established {
            reserved_group: "Cash-in-Hand"
        }
    );
    // A user-created group carries an empty RESERVEDNAME, so the walk climbs
    // through it to the predefined ancestor that actually classifies.
    assert_eq!(
        masters.classify("Bridge Nested Debtor WR4"),
        CashBankState::OtherReservedGroup {
            reserved_group: "Sundry Debtors".into()
        }
    );
    assert_eq!(
        masters.classify("WR2 Sales"),
        CashBankState::OtherReservedGroup {
            reserved_group: "Sales Accounts".into()
        }
    );
    // `Profit & Loss A/c` is the one captured ledger parented on the reserved
    // account root. The catalogue reader refuses a control-bearing parent
    // outright rather than returning it, so the classifier sees no parent at
    // all — which is still a refusal, and the reason says the true thing.
    let root = masters.classify("Profit & Loss A/c");
    assert_eq!(root.state(), "not_established");
    assert!(
        root.detail().contains("no parent group"),
        "an unreturned parent is reported as one: {}",
        root.detail()
    );
    for (name, _) in &ledgers {
        assert_eq!(
            LegRequirement::Money.admits(&masters.classify(name)),
            name == "Cash",
            "{name} classified against the captured group tree"
        );
    }
}

#[test]
fn exactly_the_captured_cash_and_bank_reserved_groups_are_admitted() {
    // Sweep every predefined group the live capture contains, to pin the
    // admitted set to observation rather than to recollection. The ledger side
    // of each edge is synthetic here, which is the point: the test above
    // establishes `Bank Accounts` and `Cash-in-Hand` from captured ledgers, and
    // this one establishes that no *other* predefined group joins them.
    //
    // `Bank OD A/c` is captured as a group in both companies and still not
    // admitted, because no captured ledger sits beneath it: admission needs the
    // whole edge, not just its far end.
    let groups = captured_groups();
    let admitted = groups
        .iter()
        .filter(|group| {
            LegRequirement::Money
                .admits(&observed(&under(&group.name), groups.clone()).classify("Probe Ledger"))
        })
        .map(|group| group.name.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        admitted,
        ["Bank Accounts", "Cash-in-Hand"],
        "captured group set admits only these as cash or bank"
    );
    // `Bank OCC A/c` is a documented Tally group that neither capture contains,
    // so a ledger under it is refused rather than matched on an unobserved
    // name. Bridge still knows it holds money — see
    // `a_money_group_bridge_will_not_admit_is_still_money_on_the_counterparty_side`.
    assert!(!groups.iter().any(|group| group.name == "Bank OCC A/c"));
    assert_eq!(
        observed(&under("Bank OCC A/c"), groups)
            .classify("Probe Ledger")
            .state(),
        "not_established"
    );
}

#[test]
fn a_user_created_group_at_the_account_root_ends_the_walk_as_the_root() {
    // A user may create a group directly under the account root, and a ledger
    // under it then walks to a parent that is no group row at all. The root
    // reaches this module through the tolerant reader, which repairs the
    // illegal `&#4;` character reference into a replacement marker — so it is
    // NOT a raw control character, and a classifier testing for one would
    // report "absent from the group collection" and hide the real shape.
    // The captured Group collection is what pins the spelling.
    let captured_root = captured_groups()
        .into_iter()
        .find(|group| group.name == "Capital Account")
        .and_then(|group| group.parent.nonempty_returned_text().map(str::to_string))
        .expect("a captured top-level group names the reserved root");
    assert!(captured_root.contains("Primary") && captured_root != "Primary");
    // The raw `U+0004` form is deliberately not recognised — the observed
    // PARENT carries the character reference, and the crate's root test is
    // defined against that. It still refuses, as an absent group.
    let raw = observed(&under("\u{4} Primary"), captured_demo_groups()).classify("Probe Ledger");
    assert_eq!(raw.state(), "not_established");
    for spelling in [captured_root.as_str(), "Primary"] {
        let mut rows = captured_groups();
        rows.push(TallyNamedMaster {
            name: "House Accounts".into(),
            parent: PartyLedgerMasterFieldObservation::Returned(spelling.to_string()),
            reserved_name: Some(String::new()),
        });
        let state = observed(&under("House Accounts"), rows).classify("Probe Ledger");
        assert_eq!(state.state(), "not_established");
        assert!(
            state.detail().contains("account root"),
            "{spelling:?} reads as the reserved root: {}",
            state.detail()
        );
    }
}

#[test]
fn a_renamed_predefined_group_still_classifies_by_its_reserved_identity() {
    // Section 8.2a measured a predefined group being renamed over XML while
    // RESERVEDNAME kept its original identity. Classification must survive it.
    let mut groups = captured_groups();
    let bank = groups
        .iter_mut()
        .find(|group| group.name == "Bank Accounts")
        .expect("captured bank group");
    bank.name = "Current Account".into();
    assert_eq!(bank.reserved_name.as_deref(), Some("Bank Accounts"));
    assert_eq!(
        observed(&under("Current Account"), groups.clone()).classify("Probe Ledger"),
        CashBankState::Established {
            reserved_group: "Bank Accounts"
        }
    );
    // The old name is now nobody's group, and resolves to nothing.
    assert_eq!(
        observed(&under("Bank Accounts"), groups)
            .classify("Probe Ledger")
            .state(),
        "not_established"
    );
}

#[test]
fn an_absent_ambiguous_or_unattributed_ancestor_is_refused() {
    let groups = captured_groups();
    for (ledgers, rows) in [
        // A parent group missing from the observed collection.
        (under("Imported Bank Group"), groups.clone()),
        // Tally returned no parent at all for the ledger.
        (vec![("Probe Ledger".to_string(), None)], groups.clone()),
        // Two rows share one name, so the hop has no single answer.
        (under("Bank Accounts"), {
            let mut rows = groups.clone();
            rows.push(TallyNamedMaster {
                name: "Bank Accounts".into(),
                parent: PartyLedgerMasterFieldObservation::Returned("Current Assets".into()),
                reserved_name: Some("Bank Accounts".into()),
            });
            rows
        }),
        // A reader that never captured RESERVEDNAME carries no identity claim,
        // which is not the same as an empty user-created marker.
        (under("Bank Accounts"), {
            let mut rows = groups.clone();
            for row in &mut rows {
                row.reserved_name = None;
            }
            rows
        }),
        // A cycle in the observed ancestry terminates instead of spinning.
        (under("Looping Group"), {
            let mut rows = groups.clone();
            rows.push(TallyNamedMaster {
                name: "Looping Group".into(),
                parent: PartyLedgerMasterFieldObservation::Returned("Looping Group".into()),
                reserved_name: Some(String::new()),
            });
            rows
        }),
    ] {
        let state = observed(&ledgers, rows).classify("Probe Ledger");
        assert_eq!(state.state(), "not_established");
        assert!(!state.detail().is_empty());
    }
    // A ledger absent from the catalogue cannot be classified from it either.
    assert_eq!(
        observed(&[], captured_groups())
            .classify("Probe Ledger")
            .state(),
        "not_established"
    );
}

fn rendered(voucher_type: &str, dr: &str, cr: &str) -> String {
    let voucher: ImportVoucher = serde_json::from_value(json!({
        "bridge_txn_id":"txn-001","date":"2026-09-01","voucher_type":voucher_type,
        "narration":"Transfer to A & B","entries":[
            {"ledger":dr,"amount":"1000.00","side":"Dr"},
            {"ledger":cr,"amount":"1000.00","side":"Cr"}]
    }))
    .expect("voucher");
    render_import_xml(
        "Synthetic Book",
        std::slice::from_ref(&voucher),
        "batch-render",
    )
}

#[test]
fn a_bank_voucher_renders_the_verified_element_shape() {
    // Section 9.13: DATE and EFFECTIVEDATE both present and equal, the party
    // named on the side opposite the money, no VOUCHERNUMBER, and a debit as
    // ISDEEMEDPOSITIVE Yes with a negative amount.
    for (voucher_type, dr, cr, party) in [
        (
            "Payment",
            "Supplier A & B",
            "Bank",
            Some("Supplier A &amp; B"),
        ),
        (
            "Receipt",
            "Bank",
            "Customer A & B",
            Some("Customer A &amp; B"),
        ),
        ("Contra", "Cash", "Bank", None),
    ] {
        let xml = rendered(voucher_type, dr, cr);
        assert!(xml.contains(&format!("VCHTYPE=\"{voucher_type}\"")));
        assert!(xml.contains("<DATE>20260901</DATE><EFFECTIVEDATE>20260901</EFFECTIVEDATE>"));
        assert!(!xml.contains("<VOUCHERNUMBER>"));
        assert!(xml.contains("<ISDEEMEDPOSITIVE>Yes</ISDEEMEDPOSITIVE><AMOUNT>-1000.00</AMOUNT>"));
        assert!(xml.contains("<ISDEEMEDPOSITIVE>No</ISDEEMEDPOSITIVE><AMOUNT>1000.00</AMOUNT>"));
        match party {
            Some(party) => assert!(
                xml.contains(&format!("<PARTYLEDGERNAME>{party}</PARTYLEDGERNAME>")),
                "{voucher_type} names its party"
            ),
            // A Contra moves money between the company's own accounts.
            None => assert!(!xml.contains("<PARTYLEDGERNAME>")),
        }
        // Section 9.1b: one unescaped ampersand rejects the whole file.
        assert!(!xml.contains("A & B"));
    }
}

#[test]
fn a_bank_voucher_renders_its_debit_first_whatever_order_the_caller_used() {
    // Every measured file put the debit first. A caller's ordering is not a
    // fact about the batch, so it is canonicalised rather than refused — which
    // costs nothing and removes the variance instead of pushing it back.
    let credit_first: ImportVoucher = serde_json::from_value(json!({
        "bridge_txn_id":"txn-001","date":"2026-09-01","voucher_type":"Payment",
        "entries":[{"ledger":"Bank","amount":"1000.00","side":"Cr"},
                   {"ledger":"Supplier","amount":"1000.00","side":"Dr"}]
    }))
    .expect("voucher");
    let xml = render_import_xml(
        "Synthetic Book",
        std::slice::from_ref(&credit_first),
        "batch-render",
    );
    let debit = xml
        .find("<LEDGERNAME>Supplier</LEDGERNAME>")
        .expect("debit leg");
    let credit = xml
        .find("<LEDGERNAME>Bank</LEDGERNAME>")
        .expect("credit leg");
    assert!(debit < credit, "the debit is rendered first");
    // Reordering is a bank-shape rule; a Journal keeps the caller's order,
    // because its own measured file is what its byte-identity claim rests on.
    let mut journal = credit_first.clone();
    journal.voucher_type = VoucherType::Journal;
    let xml = render_import_xml(
        "Synthetic Book",
        std::slice::from_ref(&journal),
        "batch-render",
    );
    assert!(
        xml.find("<LEDGERNAME>Bank</LEDGERNAME>") < xml.find("<LEDGERNAME>Supplier</LEDGERNAME>")
    );
}

#[test]
fn a_batch_mixing_the_two_rendered_shapes_is_refused() {
    // Payment and Receipt shared a file in the measured import — 61 and 54 in
    // one, 20 and 8 in another — so a heterogeneous bank file is qualified.
    // The reallocation Journals went in on their own, and no file has mixed a
    // Journal's shape with a bank one. Holding both citations at once is not
    // evidence for their union.
    let mut mixed = payload();
    mixed.vouchers[0].voucher_type = VoucherType::Journal;
    mixed.vouchers[1].voucher_type = VoucherType::Payment;
    assert_eq!(
        validate_payload(&mixed),
        Err("voucher_type_shapes_mixed".to_string())
    );
    // Within a family, mixing stays allowed in both directions.
    for (first, second) in [
        (VoucherType::Payment, VoucherType::Receipt),
        (VoucherType::Contra, VoucherType::Payment),
        (VoucherType::Journal, VoucherType::Journal),
    ] {
        let mut same = payload();
        same.vouchers[0].voucher_type = first;
        same.vouchers[1].voucher_type = second;
        assert_eq!(refuse_mixed_shapes(&same.vouchers), Ok(()));
    }
}

#[test]
fn a_journal_file_keeps_the_shape_its_own_measurement_ran_on() {
    let xml = rendered("Journal", "Expense", "Bank");
    assert!(xml.contains("<DATE>20260901</DATE><VOUCHERTYPENAME>Journal</VOUCHERTYPENAME>"));
    assert!(!xml.contains("<EFFECTIVEDATE>"));
    assert!(!xml.contains("<PARTYLEDGERNAME>"));
}

/// One paired group read, shaped exactly like the catalogue read beside it and
/// answered with the captured live group collection.
fn group_read_plans() -> Vec<ScenarioPlan> {
    let captured = include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-party-groups.utf16le.xml"
    );
    let groups = captured_group_collection();
    let mut plans = import_cycle_plans()[4..10].to_vec();
    for index in [1, 3] {
        plans[index].fixture = Fixture::SyntheticXml(groups.clone());
        plans[index].encoding = WireEncoding::Utf16LeNoBom;
        assert!(
            tally_protocol_simulator::encode(&plans[index].fixture.body(), plans[index].encoding)
                == captured,
            "the group response must preserve the captured bytes"
        );
    }
    plans
}

/// The build request sequence for a payload that carries a cash/bank voucher:
/// the Journal cycle plus a paired group read after each catalogue read.
fn bank_build_plans() -> Vec<ScenarioPlan> {
    let cycle = import_cycle_plans();
    let probe = mode_tests::licensed_import_probe();
    [
        probe.clone(),
        cycle[..10].to_vec(),
        group_read_plans(),
        cycle[10..16].to_vec(),
        cycle[4..10].to_vec(),
        group_read_plans(),
        build_preflight_plans(),
        probe,
    ]
    .concat()
}

fn captured_bank_payload() -> ImportPayload {
    serde_json::from_value(json!({"company_guid":CAPTURED_GUID,"vouchers":[
        {"bridge_txn_id":"txn-001","date":"2026-09-01","voucher_type":"Payment","narration":"Settled on account",
         "entries":[{"ledger":"Bridge Nested Debtor WR4","amount":"12.50","side":"Dr"},
                    {"ledger":"Cash","amount":"12.50","side":"Cr"}]},
        {"bridge_txn_id":"txn-002","date":"2026-09-02","voucher_type":"Receipt",
         "entries":[{"ledger":"Cash","amount":"7.50","side":"Dr"},
                    {"ledger":"WR2 Sales","amount":"7.50","side":"Cr"}]}
    ]}))
    .expect("captured bank payload")
}

fn bank_server(directory: &std::path::Path, port: u16) -> Server {
    Server::new(crate::agent::Settings {
        endpoint: TallyEndpointConfig {
            host: "127.0.0.1".into(),
            port,
        },
        data_dir: directory.into(),
        max_rows: 10,
        max_bytes: 200_000,
        redaction: crate::agent::Redaction::None,
        import_enabled: true,
        writes_enabled: false,
    })
}

#[tokio::test]
async fn a_payment_and_receipt_batch_builds_against_the_captured_masters() {
    let simulator = SequenceSimulator::spawn(bank_build_plans()).expect("bank build plan");
    let directory = tempfile::tempdir().unwrap();
    let server = bank_server(directory.path(), simulator.address().port());
    let built = server
        .build_import_xml(&serde_json::to_value(captured_bank_payload()).unwrap())
        .await
        .unwrap();
    let result = &built.payload["result"];
    assert_eq!(result["voucher_count"], 2);
    let batch_id = result["batch_id"].as_str().expect("batch id");
    let xml = std::fs::read_to_string(
        directory
            .path()
            .join("imports")
            .join(format!("{batch_id}.xml")),
    )
    .expect("written import file");
    assert!(xml.contains("VCHTYPE=\"Payment\""));
    assert!(xml.contains("VCHTYPE=\"Receipt\""));
    assert!(xml.contains("<PARTYLEDGERNAME>Bridge Nested Debtor WR4</PARTYLEDGERNAME>"));
    assert!(xml.contains("<PARTYLEDGERNAME>WR2 Sales</PARTYLEDGERNAME>"));
    assert!(xml.contains("<EFFECTIVEDATE>20260901</EFFECTIVEDATE>"));
    assert!(!xml.contains("<VOUCHERNUMBER>"));
    // A cash/bank payload reads the group collection twice, exactly as it reads
    // the catalogue twice, and the whole sequence is consumed.
    assert_eq!(simulator.finish().expect("requests").len(), 44);
}

fn demo_batch(voucher_type: &str, dr: &str, cr: &str) -> ImportPayload {
    serde_json::from_value(json!({"company_guid":AARAV_GUID,"vouchers":[
        {"bridge_txn_id":"txn-001","date":"2026-09-01","voucher_type":voucher_type,
         "entries":[{"ledger":dr,"amount":"1000.00","side":"Dr"},
                    {"ledger":cr,"amount":"1000.00","side":"Cr"}]}
    ]}))
    .expect("demo batch")
}

#[test]
fn a_payment_between_two_money_ledgers_is_refused_as_a_contra() {
    // `Dr Cash, Cr HDFC Bank Current Account` balances, names two distinct
    // captured ledgers, and puts money on the side Tally requires — every
    // structural rule passes. It is still a Contra, and admitting it as a
    // Payment files it in the Payment register: exactly the misfiling this
    // gate exists to stop. Only classifying the counterparty leg catches it.
    let masters = observed(&captured_demo_ledger_parents(), captured_demo_groups());
    let party = "Gujarat Poly Industries";
    for (voucher_type, dr, cr, counterparty_side) in [
        ("Payment", "Cash", "HDFC Bank Current Account", "Dr"),
        ("Receipt", "HDFC Bank Current Account", "Cash", "Cr"),
    ] {
        let refusals = cash_bank_refusals(&demo_batch(voucher_type, dr, cr), &masters, 200_000);
        let refused = refusals
            .ledgers
            .iter()
            .find(|leg| leg["requires"] == "not_cash_bank")
            .expect("the counterparty leg is classified too");
        assert_eq!(refused["side"], json!(counterparty_side));
        assert!(refused["refused_because"]
            .as_str()
            .expect("a refused leg says why")
            .contains("Contra"));
    }
    // The same two ledgers as a Contra are exactly right, and each type is
    // admitted when its counterparty really is one.
    for (voucher_type, dr, cr) in [
        ("Contra", "Cash", "HDFC Bank Current Account"),
        ("Payment", party, "HDFC Bank Current Account"),
        ("Receipt", "HDFC Bank Current Account", party),
    ] {
        let refusals = cash_bank_refusals(&demo_batch(voucher_type, dr, cr), &masters, 200_000);
        assert!(refusals.ledgers.is_empty(), "{voucher_type} {dr} / {cr}");
    }
}

#[test]
fn a_captured_money_group_with_no_captured_ledger_is_not_admitted() {
    // `Bank OD A/c` is a captured group row in both companies, so its identity
    // is not in doubt — but no captured ledger's `PARENT` resolves to it, and
    // that edge is what classification reads. Admitting it on the group row
    // alone would ship the money leg's whole claim on half its evidence.
    let mut ledgers = captured_demo_ledger_parents();
    ledgers.push(("Overdraft Account".into(), Some("Bank OD A/c".into())));
    let masters = observed(&ledgers, captured_demo_groups());
    assert!(captured_demo_groups()
        .iter()
        .any(|group| group.reserved_name.as_deref() == Some("Bank OD A/c")));
    assert_eq!(
        masters.classify("Overdraft Account"),
        CashBankState::UnadmittedMoney {
            reserved_group: "Bank OD A/c",
            gap: "that group is captured, but no captured ledger sits under it, and the ledger-to-parent edge is what this classification reads",
        }
    );
    // The refusal names the gap that actually exists. Saying "never appeared in
    // a captured group set" here would send an operator looking for a group
    // capture this tree already has.
    let detail = masters.classify("Overdraft Account").detail();
    assert!(detail.contains("no captured ledger sits under it"));
    assert!(!detail.contains("never appeared"));
    // Refused as funding, and refused as a counterparty, exactly as any other
    // money group Bridge will not admit.
    for (voucher_type, dr, cr) in [
        ("Payment", "Gujarat Poly Industries", "Overdraft Account"),
        ("Payment", "Overdraft Account", "HDFC Bank Current Account"),
        ("Contra", "Overdraft Account", "HDFC Bank Current Account"),
    ] {
        let refusals = cash_bank_refusals(&demo_batch(voucher_type, dr, cr), &masters, 200_000);
        assert!(!refusals.ledgers.is_empty(), "{voucher_type} {dr} / {cr}");
    }
}

#[test]
fn a_money_group_bridge_will_not_admit_is_still_money_on_the_counterparty_side() {
    // `Bank OCC A/c` is documented by Tally and appears in neither captured
    // group set, so a ledger under it is refused on a leg that must hold
    // money. Asking only whether the counterparty was *admitted* would then
    // read that same refusal as "not money" and wave through exactly the
    // bank-to-bank Payment the counterparty leg exists to catch.
    //
    // The group row here is synthetic — that is the whole scenario, a book
    // carrying a group no capture in this tree contains.
    let mut groups = captured_demo_groups();
    groups.push(TallyNamedMaster {
        name: "Bank OCC A/c".into(),
        parent: PartyLedgerMasterFieldObservation::Returned("Loans (Liability)".into()),
        reserved_name: Some("Bank OCC A/c".into()),
    });
    let mut ledgers = captured_demo_ledger_parents();
    ledgers.push(("Cash Credit Account".into(), Some("Bank OCC A/c".into())));
    let masters = observed(&ledgers, groups);
    assert_eq!(
        masters.classify("Cash Credit Account"),
        CashBankState::UnadmittedMoney {
            reserved_group: "Bank OCC A/c",
            gap: "that identity has never appeared in a captured group set",
        }
    );
    // Refused on the money leg: the identity has never been observed.
    let refusals = cash_bank_refusals(
        &demo_batch("Payment", "Gujarat Poly Industries", "Cash Credit Account"),
        &masters,
        200_000,
    );
    assert!(
        !refusals.ledgers.is_empty(),
        "an unobserved money group funds nothing"
    );
    // And refused on the counterparty leg: it is money, so this is a Contra.
    let refusals = cash_bank_refusals(
        &demo_batch(
            "Payment",
            "Cash Credit Account",
            "HDFC Bank Current Account",
        ),
        &masters,
        200_000,
    );
    assert!(
        !refusals.ledgers.is_empty(),
        "money on both sides is a Contra"
    );
    let counterparty = refusals
        .ledgers
        .iter()
        .find(|leg| leg["requires"] == "not_cash_bank")
        .expect("the counterparty leg is classified");
    assert_eq!(counterparty["state"], "cash_bank_unadmitted");
    assert!(counterparty["refused_because"]
        .as_str()
        .expect("a refused leg says why")
        .contains("Contra"));
    // A Contra between the two is refused as well, because the money leg's
    // rule still applies. Both refusals are the same ignorance, and neither
    // side quietly assumes the other's answer.
    let refusals = cash_bank_refusals(
        &demo_batch("Contra", "Cash Credit Account", "HDFC Bank Current Account"),
        &masters,
        200_000,
    );
    assert!(!refusals.ledgers.is_empty());
}

#[test]
fn one_misfiled_ledger_reports_once_however_many_vouchers_repeat_it() {
    // A refusal has to stay small enough to survive the response cap, or the
    // caller gets a generic size error instead of the thing to fix. Reporting
    // per leg had no bound — a 1,000-voucher batch produced up to 2,000 rows.
    // Per distinct ledger, the payload's own MAX_MASTER_NAMES limit caps it.
    let masters = observed(&captured_demo_ledger_parents(), captured_demo_groups());
    let mut batch = demo_batch("Payment", "Cash", "HDFC Bank Current Account");
    let template = batch.vouchers[0].clone();
    for index in 1..200 {
        let mut voucher = template.clone();
        voucher.bridge_txn_id = format!("txn-{index:03}");
        batch.vouchers.push(voucher);
    }
    let refusals = cash_bank_refusals(&batch, &masters, 200_000);
    assert_eq!(refusals.legs, 200, "every failing leg is still counted");
    assert_eq!(
        refusals.ledgers.len(),
        1,
        "200 copies of one problem is one problem"
    );
    // The count is what tells a caller this poisons the batch rather than one
    // voucher, and the first label is where to look.
    assert_eq!(refusals.ledgers[0]["first_bridge_txn_id"], "txn-001");
    // Deduplication is per (ledger, requirement), not per ledger: one ledger
    // can fail as funding in one voucher and as counterparty in another, and
    // those are two different things to fix. `Cash Credit Account` is money
    // Bridge will not admit, so it fails on both.
    let mut groups = captured_demo_groups();
    groups.push(TallyNamedMaster {
        name: "Bank OCC A/c".into(),
        parent: PartyLedgerMasterFieldObservation::Returned("Loans (Liability)".into()),
        reserved_name: Some("Bank OCC A/c".into()),
    });
    let mut ledgers = captured_demo_ledger_parents();
    ledgers.push(("Cash Credit Account".into(), Some("Bank OCC A/c".into())));
    let masters = observed(&ledgers, groups);
    let mut both_ways = demo_batch("Payment", "Gujarat Poly Industries", "Cash Credit Account");
    let mut counterparty = demo_batch(
        "Payment",
        "Cash Credit Account",
        "HDFC Bank Current Account",
    )
    .vouchers
    .remove(0);
    counterparty.bridge_txn_id = "txn-002".into();
    both_ways.vouchers.push(counterparty);
    let refusals = cash_bank_refusals(&both_ways, &masters, 200_000);
    assert_eq!(refusals.legs, 2);
    assert_eq!(refusals.ledgers.len(), 2, "one ledger, two things to fix");
    let named = serde_json::to_value(party_name("Cash Credit Account")).unwrap();
    assert!(refusals.ledgers.iter().all(|row| row["ledger"] == named));
    let mut requirements = refusals
        .ledgers
        .iter()
        .map(|row| row["requires"].as_str().unwrap())
        .collect::<Vec<_>>();
    requirements.sort();
    assert_eq!(requirements, ["cash_bank", "not_cash_bank"]);
}

#[test]
fn a_counterparty_that_cannot_be_classified_is_refused() {
    // Both legs need a positive fact; they differ only in which one. An
    // unresolved counterparty is not evidence that it holds no money, and the
    // failure it hides is the silent one — a misjudged money leg makes Tally
    // reject the import, while a misjudged counterparty files a Contra into the
    // Payment register and is found later, in the wrong place.
    //
    // The cost is small because an ordinary party never lands here: one under
    // `Sundry Debtors` resolves directly, and one under a user-created group
    // walks up to its reserved ancestor. Only anomalies reach this state.
    let mut ledgers = captured_demo_ledger_parents();
    ledgers.push(("Imported Party".into(), Some("Migrated Debtors".into())));
    let masters = observed(&ledgers, captured_demo_groups());
    assert_eq!(
        masters.classify("Imported Party").state(),
        "not_established"
    );
    let refusals = cash_bank_refusals(
        &demo_batch("Payment", "Imported Party", "HDFC Bank Current Account"),
        &masters,
        200_000,
    );
    assert_eq!(refusals.ledgers.len(), 1);
    assert_eq!(refusals.ledgers[0]["requires"], "not_cash_bank");
    // The refusal distinguishes the two ways a counterparty fails, because the
    // fixes differ: one is the wrong voucher type, the other an unresolvable
    // group. Saying "book it as a Contra" here would be wrong advice.
    let because = refusals.ledgers[0]["refused_because"].as_str().unwrap();
    assert!(because.contains("could not be classified"));
    assert!(!because.contains("Contra"));
    // A party the walk *does* resolve to a non-money identity still passes.
    let refusals = cash_bank_refusals(
        &demo_batch(
            "Payment",
            "Gujarat Poly Industries",
            "HDFC Bank Current Account",
        ),
        &masters,
        200_000,
    );
    assert!(refusals.ledgers.is_empty());
    assert_eq!(refusals.legs, 0);
}

#[test]
fn refusal_diagnostics_stay_inside_a_byte_budget() {
    // Deduplication bounds the row count, not their size: a batch may name
    // MAX_MASTER_NAMES ledgers of MAX_MASTER_NAME_CHARS each, which passes the
    // response cap on names alone. Whole rows are dropped and counted rather
    // than truncating a ledger name, because the exact live spelling is the
    // one thing a caller needs to fix the batch.
    let mut ledgers = captured_demo_ledger_parents();
    let mut batch = demo_batch(
        "Payment",
        "Gujarat Poly Industries",
        "HDFC Bank Current Account",
    );
    let template = batch.vouchers[0].clone();
    batch.vouchers.clear();
    for index in 0..MAX_MASTER_NAMES {
        let ledger = format!("{index:03} {}", "N".repeat(MAX_MASTER_NAME_CHARS - 4));
        ledgers.push((ledger.clone(), Some("Migrated Debtors".into())));
        let mut voucher = template.clone();
        voucher.bridge_txn_id = format!("txn-{index:03}");
        voucher.entries[0].ledger = ledger;
        batch.vouchers.push(voucher);
    }
    let masters = observed(&ledgers, captured_demo_groups());
    let refusals = cash_bank_refusals(&batch, &masters, 200_000);
    assert_eq!(refusals.legs, MAX_MASTER_NAMES);
    assert!(refusals.omitted > 0, "this batch does not fit");
    assert_eq!(
        refusals.ledgers.len() + refusals.omitted,
        MAX_MASTER_NAMES,
        "every distinct failure is either reported or counted"
    );
    assert!(!refusals.ledgers.is_empty(), "at least one is actionable");
    let bytes = serde_json::to_string(&json!(refusals.ledgers))
        .unwrap()
        .len();
    assert!(
        bytes <= refusal_diagnostic_budget(200_000) + MAX_MASTER_NAME_CHARS * 2,
        "diagnostics stayed within budget, got {bytes}"
    );
    // A caller configured far below the default gets a proportionally smaller
    // budget, and still gets one actionable row rather than a bare count.
    let tight = cash_bank_refusals(&batch, &masters, 256);
    assert_eq!(tight.ledgers.len(), 1);
    assert_eq!(tight.omitted, MAX_MASTER_NAMES - 1);
    assert!(refusal_diagnostic_budget(256) < refusal_diagnostic_budget(200_000));
    // A ledger name is reported whole, never trimmed to fit.
    let reported = refusals.ledgers[0]["ledger"].to_string();
    assert!(reported.contains(&"N".repeat(MAX_MASTER_NAME_CHARS - 4)));
}

#[tokio::test]
async fn a_bank_voucher_carrying_a_reference_is_refused_before_any_read() {
    // Section 9.13's measured shape has no REFERENCE element, and nothing
    // downstream would notice if Tally dropped or rewrote one: verify_import
    // compares accounting entries, not this annotation.
    for voucher_type in [
        VoucherType::Payment,
        VoucherType::Receipt,
        VoucherType::Contra,
    ] {
        let directory = tempfile::tempdir().unwrap();
        let server = bank_server(directory.path(), 9);
        let mut input = captured_bank_payload();
        input.vouchers.truncate(1);
        input.vouchers[0].voucher_type = voucher_type;
        input.vouchers[0].reference = Some("NEFT-REF".into());
        let error = server
            .build_import_xml(&serde_json::to_value(input).unwrap())
            .await
            .err()
            .unwrap();
        assert_eq!(error.code, "voucher_reference_unqualified_for_type");
        assert!(error.evidence.is_none(), "refusal precedes any source read");
    }
    // A Journal still carries one; this is a per-type rule, not a new global.
    let mut journal = payload().vouchers.remove(0);
    journal.voucher_type = VoucherType::Journal;
    journal.reference = Some("JV-REF".into());
    assert_eq!(
        validate_payload(&ImportPayload {
            company_guid: GUID.into(),
            vouchers: vec![journal],
        }),
        Ok(())
    );
}

#[tokio::test]
async fn a_contra_leg_outside_cash_and_bank_is_refused_without_writing_a_file() {
    let plans = bank_build_plans();
    // The refusal lands on the first group read, so the plan stops there.
    let simulator = SequenceSimulator::spawn(plans[..18].to_vec()).expect("bank refusal plan");
    let directory = tempfile::tempdir().unwrap();
    let server = bank_server(directory.path(), simulator.address().port());
    let mut input = captured_bank_payload();
    input.vouchers.truncate(1);
    input.vouchers[0].voucher_type = VoucherType::Contra;
    let refused = server
        .build_import_xml(&serde_json::to_value(input).unwrap())
        .await
        .unwrap();
    let result = &refused.payload["result"];
    assert_eq!(result["state"], "refused");
    assert_eq!(result["reason"], "cash_bank_ledger_not_established");
    // Only the failing leg is reported, and it is reported per ledger rather
    // than per voucher: the admitted `Cash` leg needs no action, and a ledger
    // in the wrong group fails identically in every voucher that names it.
    let refused = result["refused_ledgers"]
        .as_array()
        .expect("refused ledgers");
    assert_eq!(refused.len(), 1);
    assert_eq!(refused[0]["side"], json!("Dr"));
    assert_eq!(refused[0]["state"], "not_cash_bank");
    assert_eq!(refused[0]["requires"], "cash_bank");
    assert_eq!(refused[0]["first_bridge_txn_id"], "txn-001");
    assert_eq!(result["refused_leg_count"], 1);
    assert!(result["group_evidence_sha256"].as_str().is_some_and(
        |digest| digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
    ));
    assert!(!directory.path().join("imports").exists());
    assert!(!directory.path().join("agent-import-ledger.jsonl").exists());
    assert_eq!(simulator.finish().expect("requests").len(), 18);
}

#[tokio::test]
async fn a_journal_only_batch_reads_no_group_collection() {
    // The Journal path keeps the exact request sequence its own qualification
    // was measured on; nothing here widened it.
    let simulator =
        SequenceSimulator::spawn(qualified_import_cycle_plans()[..32].to_vec()).expect("simulator");
    let directory = tempfile::tempdir().unwrap();
    let server = bank_server(directory.path(), simulator.address().port());
    server
        .build_import_xml(&serde_json::to_value(captured_catalogue_payload()).unwrap())
        .await
        .unwrap();
    assert_eq!(simulator.finish().expect("requests").len(), 32);
}

#[tokio::test]
async fn a_bank_batch_verifies_through_the_rewrites_tally_makes_to_it() {
    // Two rewrites are already recorded against the import path: under
    // automatic numbering Tally assigns its own voucher number (section 9.8),
    // and it does not promise to return entries in the order they were sent
    // (section 12a.4). Readback must survive both — a batch that posts
    // correctly and then fails its own verification is worse than useless.
    for voucher_type in [
        VoucherType::Payment,
        VoucherType::Receipt,
        VoucherType::Contra,
    ] {
        let mut voucher = payload().vouchers.remove(0);
        voucher.voucher_type = voucher_type.clone();
        voucher.voucher_number = None;
        let line = ImportLedgerLine {
            endpoint_origin: None,
            identity_scheme: None,
            batch_id: "batch-bank".into(),
            company_guid: GUID.into(),
            company: None,
            txn_ids: vec![voucher.bridge_txn_id.clone()],
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
            vouchers: vec![voucher.clone()],
        };
        let mut entries = voucher
            .entries
            .iter()
            .map(|entry| ReadEntry {
                ledger: entry.ledger.clone(),
                amount: match entry.side {
                    EntrySide::Dr => format!("-{}", entry.amount),
                    EntrySide::Cr => entry.amount.clone(),
                },
                is_deemed_positive: entry.side.tally_positive().into(),
            })
            .collect::<Vec<_>>();
        entries.reverse();
        let observed = vec![ReadVoucher {
            remote_id: Some("remote-bank".into()),
            guid: Some("guid-bank".into()),
            master_id: Some("41".into()),
            alter_id: Some(63),
            date: Some(normalized_date(&voucher.date).unwrap()),
            voucher_type: Some(voucher_type.as_str().into()),
            narration: Some(format!("[BRIDGE:{}]", voucher.bridge_txn_id)),
            // Tally's own number, which Bridge never sent and must not compare.
            voucher_number: Some("463".into()),
            cancelled: Some(false),
            optional: Some(false),
            entries,
        }];
        let result = verify_observed_batch(&line, &observed).unwrap();
        assert_eq!(result["counts"]["posted_verified"], 1);
        assert_eq!(verification_status(&result, 1), "posted_verified");
        assert_eq!(result["duplicates"], json!([]));
        // A readback that disagrees on the type is a different voucher, and
        // says so rather than passing on matching amounts alone.
        let mut mistyped = observed.clone();
        mistyped[0].voucher_type = Some("Journal".into());
        let result = verify_observed_batch(&line, &mistyped).unwrap();
        assert_eq!(result["counts"]["posted_verified"], 0);
        assert_ne!(verification_status(&result, 1), "posted_verified");
    }
}
