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
    // establishes `Bank Accounts` and `Cash-in-Hand` from captured ledgers,
    // `a_captured_ledger_under_bank_od_is_established_as_bank` establishes
    // `Bank OD A/c` the same way, and this one establishes that no *other*
    // predefined group joins them.
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
        ["Bank Accounts", "Bank OD A/c", "Cash-in-Hand"],
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
fn a_padded_parent_does_not_resolve_to_a_group_it_does_not_name() {
    // The exactness of the hop is only as good as what reaches it. The
    // catalogue validator used to trim a ledger's PARENT, which absorbed the
    // difference upstream of the walk and let a padded reference resolve to a
    // group it does not byte-match — the pair the walk exists to refuse.
    let mut ledgers = captured_demo_ledger_parents();
    ledgers.push(("Padded Bank".into(), Some(" Bank Accounts ".into())));
    let masters = observed(&ledgers, captured_demo_groups());
    assert_eq!(masters.classify("Padded Bank").state(), "not_established");
    let refusals = cash_bank_refusals(
        &demo_batch("Payment", "Gujarat Poly Industries", "Padded Bank"),
        &masters,
        200_000,
    );
    assert!(refusals.is_refused(), "a padded reference funds nothing");
}

#[test]
fn a_renamed_predefined_group_still_classifies_by_its_reserved_identity() {
    // Section 8.2b measured a predefined group being renamed over XML while
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
    // The stale-classification warning is bank-gated and belongs here, beside
    // the always-present company-identity warning — this batch is exactly the
    // Payment/Receipt shape §9.13 and agent_import_cash_bank.rs's module
    // header both describe as vulnerable to a regroup after this build.
    let warnings = result["warnings"]
        .as_array()
        .expect("warnings array")
        .iter()
        .map(|value| value.as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(
        warnings
            .iter()
            .any(|warning| warning.contains("Regrouping a ledger afterwards")),
        "stale-classification warning missing from a bank batch: {warnings:?}"
    );
    // §9.13's measured slice is one licensed instance; this batch's own
    // observed profile rides beside it rather than being asserted as a match.
    assert!(
        warnings
            .iter()
            .any(|warning| warning.contains("measured on licensed TallyPrime 7.1 Gold only")),
        "release-evidence warning missing from a bank batch: {warnings:?}"
    );
    assert!(
        warnings
            .iter()
            .any(|warning| warning.contains("Confirm the loaded company before importing")),
        "company-identity warning missing from a bank batch: {warnings:?}"
    );
    assert!(result["next_step"]
        .as_str()
        .unwrap()
        .starts_with("Confirm the loaded company matches this batch"));
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

/// The licensed-lab `List of Groups` and `StandardLedgerCatalogV1` reads of
/// `BRIDGE SHAPE LAB`, sent with the requests the build renders. Its `HDFC CC`
/// is the one captured ledger in this tree whose parent is `Bank OD A/c`.
const SHAPE_LAB_GUID: &str = "3a6bd6e1-b835-4bff-89dd-8a6af138c346";

fn utf16le(bytes: &[u8]) -> String {
    String::from_utf16(
        &bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>(),
    )
    .expect("captured BOM-less UTF-16LE")
}

fn captured_shape_lab_masters() -> (Vec<(String, Option<String>)>, Vec<TallyNamedMaster>) {
    let catalogue = utf16le(include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-shape-lab-ledger-catalogue.utf16le.xml"
    ));
    let ledgers =
        parse_standard_ledger_catalog_response(&catalogue, "BRIDGE SHAPE LAB", SHAPE_LAB_GUID)
            .expect("captured Shape Lab catalogue rows")
            .parents()
            .map(|(name, parent)| (name.to_string(), parent.map(str::to_string)))
            .collect();
    let groups = bridge_tally_protocol::native_outstandings::parse_native_group_snapshot(
        &utf16le(include_bytes!(
            "../crates/bridge-tally-protocol/tests/fixtures/agent/native-shape-lab-groups.utf16le.xml"
        )),
        SHAPE_LAB_GUID,
    )
    .expect("captured Shape Lab group rows");
    (ledgers, groups)
}

#[test]
fn a_captured_ledger_under_bank_od_is_established_as_bank() {
    // Both sides of the edge are verbatim captures parsed by the production
    // readers: the catalogue row naming `Bank OD A/c` as `HDFC CC`'s parent,
    // and the group row carrying that reserved identity. That is the evidence
    // an overdraft or cash-credit account needed before it could fund a
    // Payment or sit in a Contra.
    let (ledgers, groups) = captured_shape_lab_masters();
    assert_eq!(ledgers.len(), 43, "the whole captured catalogue is swept");
    assert!(ledgers
        .iter()
        .any(|(name, parent)| name == "HDFC CC" && parent.as_deref() == Some("Bank OD A/c")));
    assert!(groups.iter().any(|group| group.name == "Bank OD A/c"
        && group.reserved_name.as_deref() == Some("Bank OD A/c")));
    let masters = observed(&ledgers, groups);
    assert_eq!(
        masters.classify("HDFC CC"),
        CashBankState::Established {
            reserved_group: "Bank OD A/c"
        }
    );
    let money = ledgers
        .iter()
        .filter(|(name, _)| LegRequirement::Money.admits(&masters.classify(name)))
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        money,
        ["Bank of Baroda CA", "Cash", "HDFC CC"],
        "no other captured ledger of the 43 is admitted as money"
    );
    for (voucher_type, dr, cr) in [
        ("Contra", "HDFC CC", "Bank of Baroda CA"),
        ("Payment", "Power Charges", "HDFC CC"),
        ("Receipt", "HDFC CC", "Shape Buyer 1"),
    ] {
        let refusals = cash_bank_refusals(&demo_batch(voucher_type, dr, cr), &masters, 200_000);
        assert!(refusals.ledgers.is_empty(), "{voucher_type} {dr} / {cr}");
    }
    // Still money on the counterparty side: a Payment from one bank into the
    // overdraft is a Contra, and is refused as one.
    let refusals = cash_bank_refusals(
        &demo_batch("Payment", "HDFC CC", "Bank of Baroda CA"),
        &masters,
        200_000,
    );
    assert!(!refusals.ledgers.is_empty());
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
fn a_tiny_response_cap_cannot_admit_a_batch_whose_legs_failed() {
    // The refusal rows are presentation and are bounded by a display budget;
    // the verdict is the leg count. Reading the row vector as the verdict let
    // a batch with failing cash/bank legs write its file whenever the cap was
    // small enough to omit every row — an accounting check made switchable by
    // BRIDGE_AGENT_MAX_BYTES.
    let masters = observed(&captured_demo_ledger_parents(), captured_demo_groups());
    let batch = demo_batch("Payment", "Cash", "HDFC Bank Current Account");
    for cap in [200_000, 4_096, 256] {
        let refusals = cash_bank_refusals(&batch, &masters, cap);
        assert!(refusals.legs > 0, "the leg fails at every cap");
        assert!(refusals.is_refused(), "cap {cap} still refuses the batch");
    }
    // The cap that empties the rows is the case that regressed.
    let mut wide = batch.clone();
    let template = wide.vouchers[0].clone();
    let mut ledgers = captured_demo_ledger_parents();
    wide.vouchers.clear();
    for index in 0..8 {
        let ledger = format!("{index:02} {}", "N".repeat(MAX_MASTER_NAME_CHARS - 3));
        ledgers.push((ledger.clone(), Some("Cash-in-Hand".into())));
        let mut voucher = template.clone();
        voucher.bridge_txn_id = format!("txn-{index:02}");
        voucher.entries[0].ledger = ledger;
        wide.vouchers.push(voucher);
    }
    let refusals = cash_bank_refusals(&wide, &observed(&ledgers, captured_demo_groups()), 256);
    assert!(refusals.ledgers.is_empty(), "no row fits this cap");
    assert!(refusals.is_refused(), "and the batch is still refused");
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
fn a_known_non_money_reserved_identity_is_still_admitted_as_a_counterparty() {
    // Regression: NON_MONEY_RESERVED_GROUPS must not narrow admission for the
    // identities it already knows about. `Sundry Debtors` is one of the 25
    // observed non-money identities, captured in both companies, and a party
    // resolving to it is exactly the ordinary case the counterparty leg exists
    // to pass.
    let masters = observed(&under("Sundry Debtors"), captured_demo_groups());
    assert_eq!(
        masters.classify("Probe Ledger"),
        CashBankState::OtherReservedGroup {
            reserved_group: "Sundry Debtors".into()
        }
    );
    assert!(LegRequirement::Counterparty.admits(&masters.classify("Probe Ledger")));
    // And the same holds through the full batch path: a real captured party
    // under Sundry Creditors funds nothing and is admitted as a counterparty.
    let masters = observed(&captured_demo_ledger_parents(), captured_demo_groups());
    let refusals = cash_bank_refusals(
        &demo_batch(
            "Payment",
            "Gujarat Poly Industries",
            "HDFC Bank Current Account",
        ),
        &masters,
        200_000,
    );
    assert!(
        refusals.ledgers.is_empty(),
        "a known non-money counterparty must still be admitted: {:?}",
        refusals.ledgers
    );
}

#[test]
fn an_unknown_reserved_identity_is_refused_as_a_counterparty_rather_than_admitted() {
    // An identity in neither table is unknown, not non-money — see
    // NON_MONEY_RESERVED_GROUPS's doc comment. Before this change the final
    // `None` arm of `classify` read any such identity as proof of "holds no
    // money", which would admit it here. The invented RESERVEDNAME below is
    // deliberately not one of the 28 either captured company exhibits.
    let mut groups = captured_demo_groups();
    groups.push(TallyNamedMaster {
        name: "Escrow Holdback A/c".into(),
        parent: PartyLedgerMasterFieldObservation::Returned("Current Liabilities".into()),
        reserved_name: Some("Escrow Holdback A/c".into()),
    });
    let mut ledgers = captured_demo_ledger_parents();
    ledgers.push(("Retention Party".into(), Some("Escrow Holdback A/c".into())));
    let masters = observed(&ledgers, groups);
    let state = masters.classify("Retention Party");
    assert_eq!(state.state(), "not_established");
    assert!(
        state.detail().contains("no captured response exhibits"),
        "unexpected detail: {}",
        state.detail()
    );
    assert!(
        !LegRequirement::Counterparty.admits(&state),
        "an unknown reserved identity must not be admitted as a counterparty"
    );
    let refusals = cash_bank_refusals(
        &demo_batch("Payment", "Retention Party", "HDFC Bank Current Account"),
        &masters,
        200_000,
    );
    assert_eq!(
        refusals.legs, 1,
        "an unknown counterparty refuses the build"
    );
    assert_eq!(refusals.ledgers.len(), 1);
    let refused = &refusals.ledgers[0];
    assert_eq!(refused["requires"], "not_cash_bank");
    assert_eq!(refused["state"], "not_established");
    let because = refused["refused_because"].as_str().unwrap();
    // Distinguishes it from the money-side refusal in two ways: the wording
    // names a counterparty specifically, and it never suggests a Contra —
    // this ledger is not known money, only unclassified, and advising a
    // Contra about a ledger nobody has classified would be wrong.
    assert!(because.contains("no captured response exhibits"));
    assert!(because.contains("could not be classified either way"));
    assert!(!because.contains("Contra"));
    // The money-side refusal for the very same unresolved identity carries
    // only the bare detail sentence, with none of the counterparty wrapping —
    // that difference is what "distinguishes the refusal" means here.
    let money_refusal = LegRequirement::Money
        .refusal(&state, "Payment")
        .expect("money side also refuses an unresolved identity");
    assert!(!money_refusal.contains("could not be classified either way"));
    assert!(!money_refusal.contains("counterparty"));
    assert_eq!(money_refusal, state.detail());
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
    // No row is exempted to guarantee "at least one". An exempt row reproduced
    // the very size failure the budget prevents, and a name trimmed to fit is
    // worse than an absent one because a ledger matches by exact codepoint.
    // The counts are integers and always survive, so a caller on a tiny cap
    // still learns how many refusals it cannot see.
    assert!(refusal_diagnostic_budget(256) < refusal_diagnostic_budget(200_000));
    let tight = cash_bank_refusals(&batch, &masters, 256);
    assert_eq!(tight.legs, MAX_MASTER_NAMES);
    assert_eq!(
        tight.ledgers.len() + tight.omitted,
        MAX_MASTER_NAMES,
        "every failure is reported or counted, at any cap"
    );
    for row in &tight.ledgers {
        assert!(
            serde_json::to_string(row).unwrap().len() <= refusal_diagnostic_budget(256),
            "no row is exempt from the budget"
        );
    }
    // One oversized row does not discard the shorter refusals behind it. The
    // rows are sorted by ledger, so a long name early in the order would
    // otherwise take every later one down with it.
    let mut mixed = batch.clone();
    let short = "AA Bank";
    ledgers.push((short.to_string(), Some("Migrated Debtors".into())));
    let mut voucher = template.clone();
    voucher.bridge_txn_id = "txn-short".into();
    voucher.entries[0].ledger = short.into();
    mixed.vouchers.push(voucher);
    let refusals = cash_bank_refusals(&mixed, &observed(&ledgers, captured_demo_groups()), 2_048);
    assert!(
        refusals
            .ledgers
            .iter()
            .any(|row| row["ledger"] == serde_json::to_value(party_name(short)).unwrap()),
        "the short row survives a budget the long ones exhaust"
    );
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
            amends_batch_id: None,
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
            amends_batch_id: None,
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

/// Publish `payload`'s vouchers as a parse_bank_statement proposals file,
/// returning its id and digest as that tool would.
fn published_proposals(directory: &std::path::Path, payload: &ImportPayload) -> (String, String) {
    let proposals_id = format!("statement-{}", uuid::Uuid::new_v4());
    let document = json!({
        "schema": "bridge.bank_statement.proposals.v1",
        "proposals_id": proposals_id,
        "vouchers": payload.vouchers,
    });
    let bytes = serde_json::to_vec_pretty(&document).unwrap();
    let statements = directory.join("bank-statements");
    std::fs::create_dir_all(&statements).unwrap();
    std::fs::write(statements.join(format!("{proposals_id}.json")), &bytes).unwrap();
    (proposals_id, sha256_hex(&bytes))
}

/// The import file without the parts every build draws afresh: its REMOTEIDs
/// and the batch marker in each narration.
fn without_batch_identity(xml: &str) -> String {
    let mut out = xml.to_string();
    for (open, close) in [("REMOTEID=\"", "\""), ("[BRIDGE:", "]")] {
        let mut kept = String::new();
        let mut rest = out.as_str();
        while let Some(start) = rest.find(open) {
            let after = &rest[start + open.len()..];
            let end = after.find(close).expect("closed identity");
            kept.push_str(&rest[..start + open.len()]);
            rest = &after[end..];
        }
        kept.push_str(rest);
        out = kept;
    }
    out
}

#[tokio::test]
async fn a_proposals_file_builds_through_tools_call_exactly_as_its_inline_vouchers_do() {
    let payload = captured_bank_payload();

    let inline_simulator = SequenceSimulator::spawn(bank_build_plans()).expect("inline plan");
    let inline_directory = tempfile::tempdir().unwrap();
    let inline = bank_server(inline_directory.path(), inline_simulator.address().port())
        .call_tool_response("build_import_xml", serde_json::to_value(&payload).unwrap())
        .await
        .value;
    let inline_result = &inline["structuredContent"]["result"];
    assert_eq!(inline_result["voucher_count"], 2, "{inline}");

    let simulator = SequenceSimulator::spawn(bank_build_plans()).expect("proposals plan");
    let directory = tempfile::tempdir().unwrap();
    let (proposals_id, digest) = published_proposals(directory.path(), &payload);
    let response = bank_server(directory.path(), simulator.address().port())
        .call_tool_response(
            "build_import_xml",
            json!({"company_guid": CAPTURED_GUID, "proposals_id": proposals_id, "proposals_sha256": digest}),
        )
        .await
        .value;
    let result = &response["structuredContent"]["result"];
    assert_eq!(result["voucher_count"], 2, "{response}");
    // the same request sequence was consumed, so the same admission ran
    assert_eq!(simulator.finish().expect("requests").len(), 44);
    assert_eq!(inline_simulator.finish().expect("requests").len(), 44);

    let read = |directory: &std::path::Path, result: &Value| {
        std::fs::read_to_string(
            directory
                .join("imports")
                .join(format!("{}.xml", result["batch_id"].as_str().unwrap())),
        )
        .unwrap()
    };
    let from_proposals = read(directory.path(), result);
    assert_ne!(result["batch_id"], inline_result["batch_id"]);
    assert_eq!(
        without_batch_identity(&from_proposals),
        without_batch_identity(&read(inline_directory.path(), inline_result))
    );
    assert!(from_proposals.contains("<PARTYLEDGERNAME>Bridge Nested Debtor WR4</PARTYLEDGERNAME>"));
}

#[tokio::test]
async fn a_proposals_file_changed_since_its_parse_is_refused_before_any_tally_read() {
    let directory = tempfile::tempdir().unwrap();
    // port 9: any Tally read would fail differently from the refusals below
    let server = bank_server(directory.path(), 9);
    let payload = captured_bank_payload();
    let (proposals_id, digest) = published_proposals(directory.path(), &payload);
    let path = directory
        .path()
        .join("bank-statements")
        .join(format!("{proposals_id}.json"));
    let code = |response: Value| {
        response["structuredContent"]["result"]["error"]["code"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    };

    let mut edited = std::fs::read(&path).unwrap();
    let at = edited
        .windows(5)
        .position(|window| window == b"12.50")
        .unwrap();
    edited[at..at + 5].copy_from_slice(b"99.50");
    std::fs::write(&path, &edited).unwrap();
    let args = json!({"company_guid": CAPTURED_GUID, "proposals_id": proposals_id, "proposals_sha256": digest});
    assert_eq!(
        code(
            server
                .call_tool_response("build_import_xml", args.clone())
                .await
                .value
        ),
        "proposals_changed"
    );

    for (arguments, expected) in [
        (
            json!({"company_guid": CAPTURED_GUID, "proposals_id": proposals_id}),
            "proposals_sha256_required",
        ),
        (
            json!({"company_guid": CAPTURED_GUID, "proposals_sha256": digest}),
            "proposals_id_required",
        ),
        (json!({"company_guid": CAPTURED_GUID}), "vouchers_required"),
        (
            json!({"company_guid": CAPTURED_GUID, "proposals_id": proposals_id, "proposals_sha256": digest,
                "vouchers": serde_json::to_value(&payload).unwrap()["vouchers"]}),
            "proposals_id_with_vouchers",
        ),
        (
            json!({"company_guid": CAPTURED_GUID, "proposals_id": format!("statement-{}", uuid::Uuid::new_v4()),
                "proposals_sha256": digest}),
            "proposals_not_found",
        ),
        // exactly the published length, so only the resolver's own check can refuse it
        (
            json!({"company_guid": CAPTURED_GUID, "proposals_id": "statement-../../../imports/0000000000000000000",
                "proposals_sha256": digest}),
            "argument_invalid:proposals_id",
        ),
        (
            json!({"company_guid": CAPTURED_GUID, "proposals_id": proposals_id,
                "proposals_sha256": digest.to_uppercase()}),
            "argument_invalid:proposals_sha256",
        ),
    ] {
        assert_eq!(
            code(
                server
                    .call_tool_response("build_import_xml", arguments)
                    .await
                    .value
            ),
            expected
        );
    }

    // a file with the right digest but not this tool's schema or id
    let foreign = json!({"schema": "something.else", "proposals_id": proposals_id, "vouchers": []});
    let bytes = serde_json::to_vec(&foreign).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    let args = json!({"company_guid": CAPTURED_GUID, "proposals_id": proposals_id, "proposals_sha256": sha256_hex(&bytes)});
    assert_eq!(
        code(
            server
                .call_tool_response("build_import_xml", args)
                .await
                .value
        ),
        "proposals_file_invalid"
    );

    // a link to a file elsewhere is not a file this tool published
    #[cfg(unix)]
    {
        let elsewhere = directory.path().join("elsewhere.json");
        std::fs::write(&elsewhere, &edited).unwrap();
        std::fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &path).unwrap();
        let args = json!({"company_guid": CAPTURED_GUID, "proposals_id": proposals_id,
                          "proposals_sha256": sha256_hex(&edited)});
        assert_eq!(
            code(
                server
                    .call_tool_response("build_import_xml", args)
                    .await
                    .value
            ),
            "proposals_file_unreadable"
        );
    }
}

#[test]
fn an_amendment_is_admitted_through_tools_call_argument_validation() {
    let batch = format!("bridge-{}", uuid::Uuid::new_v4());
    let args = |amends: &str| {
        json!({"company_guid": CAPTURED_GUID, "amends_batch_id": amends,
               "vouchers": serde_json::to_value(captured_bank_payload()).unwrap()["vouchers"]})
    };
    // this was argument_invalid:amends_batch_id for every value before the
    // validator knew the pattern the schema publishes
    assert_eq!(
        crate::agent::catalog::validate_tool_arguments("build_import_xml", &args(&batch)),
        Ok(())
    );
    for malformed in [
        batch.to_uppercase(),
        format!("bridge-{}", uuid::Uuid::nil()),
        batch.replace("bridge-", "batch-"),
        format!("{batch}0"),
    ] {
        assert_eq!(
            crate::agent::catalog::validate_tool_arguments("build_import_xml", &args(&malformed)),
            Err("argument_invalid:amends_batch_id".to_string()),
            "{malformed}"
        );
    }
}

/// Statement PDF → `parse_bank_statement` → `build_import_xml` by
/// `proposals_id`, both through tool dispatch. The synthetic statement is
/// mapped onto ledgers the captured catalogue holds: `Cash` as the bank and a
/// captured debtor as suspense, so every row is admitted by the same
/// cash/bank rules a real statement meets.
#[tokio::test]
#[ignore = "needs PDFium: set BRIDGE_PDFIUM_LIBRARY and run with --ignored"]
async fn a_parsed_statement_builds_an_import_file_by_proposals_id() {
    assert!(std::env::var_os("BRIDGE_PDFIUM_LIBRARY").is_some());
    let directory = tempfile::tempdir().unwrap();
    let statement = directory.path().join("statement.pdf");
    std::fs::copy(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("crates/bridge-bank-statement/tests/fixtures/hdfc-synthetic.pdf"),
        &statement,
    )
    .unwrap();
    let password = directory.path().join("statement.password");
    std::fs::write(&password, "synthetic-user-4321\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&password, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let data = directory.path().join("agent");
    std::fs::create_dir_all(&data).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&data, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let simulator = SequenceSimulator::spawn(bank_build_plans()).expect("bank build plan");
    let server = bank_server(&data, simulator.address().port());

    let parsed = server
        .call_tool_response(
            "parse_bank_statement",
            json!({
                "statement_path": statement.to_str().unwrap(),
                "password_file": password.to_str().unwrap(),
                "bank": "hdfc", "account_label": "Synthetic CA xx4321",
                "opening_balance": "1,000.00", "closing_balance": "1,02,200.00",
                "total_debits": "8,800.00", "total_credits": "1,10,000.00",
                "bank_ledger": "Cash", "suspense_ledger": "Bridge Nested Debtor WR4"
            }),
        )
        .await
        .value;
    let summary = &parsed["structuredContent"]["result"];
    assert_eq!(summary["vouchers"], 6, "{parsed}");

    let built = server
        .call_tool_response(
            "build_import_xml",
            json!({"company_guid": CAPTURED_GUID,
                   "proposals_id": summary["proposals_id"],
                   "proposals_sha256": summary["sha256"]}),
        )
        .await
        .value;
    let result = &built["structuredContent"]["result"];
    assert_eq!(result["voucher_count"], 6, "{built}");
    let xml = std::fs::read_to_string(
        data.join("imports")
            .join(format!("{}.xml", result["batch_id"].as_str().unwrap())),
    )
    .unwrap();
    assert_eq!(xml.matches("VCHTYPE=\"Payment\"").count(), 4);
    assert_eq!(xml.matches("VCHTYPE=\"Receipt\"").count(), 2);
    // the 12-digit reference a cell wrap split in the PDF reaches the narration whole
    assert!(
        xml.contains("UPI 612345678901 from NORTHWIND TRADERS"),
        "{xml}"
    );
    assert_eq!(simulator.finish().expect("requests").len(), 44);
}
