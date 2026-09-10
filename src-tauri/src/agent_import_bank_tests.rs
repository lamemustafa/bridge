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
    // Section 8.2a: the reserved account root arrives control-marked, and it is
    // not a group row. Reaching it establishes nothing.
    assert_eq!(
        masters.classify("Profit & Loss A/c").state(),
        "not_established"
    );
    for (name, _) in &ledgers {
        assert_eq!(
            masters.classify(name).is_established(),
            name == "Cash",
            "{name} classified against the captured group tree"
        );
    }
}

#[test]
fn exactly_the_captured_cash_and_bank_reserved_groups_are_admitted() {
    // Sweep every predefined group the live capture contains. This is what
    // pins the admitted set to observation rather than to recollection.
    let groups = captured_groups();
    let admitted = groups
        .iter()
        .filter(|group| {
            observed(&under(&group.name), groups.clone())
                .classify("Probe Ledger")
                .is_established()
        })
        .map(|group| group.name.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        admitted,
        ["Bank Accounts", "Bank OD A/c", "Cash-in-Hand"],
        "captured group set admits only these as cash or bank"
    );
    // `Bank OCC A/c` is a documented Tally group that neither capture contains.
    // A book using one is refused rather than matched on an unobserved name.
    assert!(!groups.iter().any(|group| group.name == "Bank OCC A/c"));
    assert_eq!(
        observed(&under("Bank OCC A/c"), groups)
            .classify("Probe Ledger")
            .state(),
        "not_established"
    );
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
    let legs = result["legs"].as_array().expect("classified legs");
    assert_eq!(legs.len(), 2);
    // Both legs are reported, so a caller fixing them pays one build, not two.
    assert_eq!(legs[0]["side"], json!("Dr"));
    assert_eq!(legs[0]["state"], "not_cash_bank");
    assert_eq!(legs[1]["side"], json!("Cr"));
    assert_eq!(legs[1]["state"], "cash_bank");
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
