use super::*;

#[test]
fn opposing_unallocated_direction_is_preserved_and_only_gross_exposure_is_ranked() {
    let bills = vec![OpenBillRow {
        party: "Synthetic Party".into(),
        reference: "INV-1".into(),
        bill_date: "20260901".into(),
        due_date: "20260901".into(),
        amount: bridge_tally_core::ExactDecimal::parse("100").unwrap(),
        age_days: Some(5),
        kind: ExposureDirection::Receivable,
    }];
    let unallocated = vec![UnallocatedParty {
        party: "Synthetic Party".into(),
        amount: bridge_tally_core::ExactDecimal::parse("30").unwrap(),
        direction: ExposureDirection::Payable,
    }];
    let ranked = ranked_parties_from_exposure(&bills, &unallocated, 1).unwrap();
    let party = &ranked[0];
    assert_eq!(party["gross_exposure"], "130");
    assert_eq!(party["gross_billed"], "100");
    assert_eq!(party["billed_receivable"], "100");
    assert_eq!(party["billed_payable"], "0");
    assert_eq!(party["unallocated_receivable"], "0");
    assert_eq!(party["unallocated_payable"], "30");
    assert_eq!(party["gross_unallocated"], "30");
    for absent in ["outstanding_total", "net_due", "billed", "unallocated"] {
        assert!(party.get(absent).is_none(), "ambiguous total {absent}");
    }
    let residuals = unallocated_totals_from_parties(&unallocated).unwrap();
    assert_eq!(
        residuals,
        json!({"receivable":"0", "payable":"30", "gross_unallocated":"30"})
    );
    let billed = outstanding_totals_from_open_bills(&bills).unwrap();
    assert_eq!(billed["scope"], "open_bills_only");
    assert_eq!(billed["receivable"], "100");
    assert_eq!(billed["payable"], "0");
}

/// bridge#551: a currency refusal names its ledger on the MCP result, masked
/// like any party name; a reason without a ledger carries none.
#[test]
fn a_withheld_result_names_its_ledger_under_redaction() {
    let mut reason =
        crate::tally::OutstandingsPartialReason::code("ledger_currency_base_unmatched");
    reason.foreign_currency_ledger_name = Some("Synthetic FX Debtor".to_string());
    let plain = partial_payload(&reason, Redaction::None);
    assert_eq!(plain["partial_reason"], "ledger_currency_base_unmatched");
    assert_eq!(plain["ledger"], "Synthetic FX Debtor");
    let masked = partial_payload(&reason, Redaction::MaskParties);
    assert_eq!(masked["ledger"], json!(mask("Synthetic FX Debtor")));
    assert_ne!(masked["ledger"], "Synthetic FX Debtor");
    let unnamed = partial_payload(
        &crate::tally::OutstandingsPartialReason::code("native_bills_report_drifted"),
        Redaction::MaskParties,
    );
    assert_eq!(
        unnamed,
        json!({"state":"partial","partial_reason":"native_bills_report_drifted"})
    );
}

/// bridge#551: the foreign-balance refusal, which predates the currency
/// classification, names its ledger on the MCP result through the same
/// party-name redaction.
#[test]
fn the_foreign_balance_refusal_names_its_ledger_under_redaction() {
    let reason = crate::tally::OutstandingsPartialReason::foreign_currency_ledger_balance(
        "Synthetic FX Debtor".to_string(),
    );
    let plain = partial_payload(&reason, Redaction::None);
    assert_eq!(
        plain,
        json!({
            "state": "partial",
            "partial_reason": "company_foreign_currency_ledger_balance",
            "ledger": "Synthetic FX Debtor",
        })
    );
    let masked = partial_payload(&reason, Redaction::MaskParties);
    assert_eq!(masked["ledger"], json!(mask("Synthetic FX Debtor")));
    assert_ne!(masked["ledger"], "Synthetic FX Debtor");
}

/// bridge#551, through the MCP tool itself on FOREX's captures, with party
/// names masked: a book with an `I₹` and a `$` master comes back partial,
/// with the rupee ledgers' figures under `base_currency_ledgers`, no figure
/// for the whole book, and the three `$` ledgers listed with their currency,
/// their names masked like any party's.
#[tokio::test]
async fn mcp_outstandings_report_base_currency_ledgers_only_on_forex() {
    use tally_protocol_simulator::{
        Fixture, ProductStatus, ScenarioPlan, SequenceSimulator, WireEncoding,
    };
    fn decode(bytes: &[u8]) -> String {
        String::from_utf16(
            &bytes
                .chunks_exact(2)
                .map(|unit| u16::from_le_bytes([unit[0], unit[1]]))
                .collect::<Vec<_>>(),
        )
        .unwrap()
    }
    let xml = |body: String| {
        ScenarioPlan::new(Fixture::SyntheticXml(body)).with_encoding(WireEncoding::Utf16Le)
    };
    let status = || ScenarioPlan::new(Fixture::ProductStatus(ProductStatus::TallyPrime));
    let pair = |plans: &mut Vec<ScenarioPlan>, source: ScenarioPlan| {
        plans.extend([source.clone(), status(), source, status()]);
    };
    let companies = xml(decode(include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-licensed-release-companies.utf16le.xml"
    )));
    let extent = xml(
        include_str!(
            "../crates/bridge-tally-protocol/tests/fixtures/agent/native-company-book-extents-with-number.utf8.xml"
        )
        .to_string(),
    );
    let captured = |bytes: &[u8]| xml(decode(bytes));

    let mut plans = Vec::new();
    pair(&mut plans, companies.clone());
    // The classified currency read: plain, then with ORIGINALNAME, then the
    // Company collection.
    plans.push(companies.clone());
    pair(&mut plans, extent.clone());
    for source in [
        captured(include_bytes!(
            "../crates/bridge-tally-protocol/tests/fixtures/currency_multi_live.utf16le.xml"
        )),
        captured(include_bytes!(
            "../crates/bridge-tally-protocol/tests/fixtures/currency_originalname_forex_live.utf16le.xml"
        )),
        captured(include_bytes!(
            "../crates/bridge-tally-protocol/tests/fixtures/company_currencyname_forex_edited.utf16le.xml"
        )),
    ] {
        pair(&mut plans, source);
    }
    pair(&mut plans, extent.clone());
    plans.push(companies.clone());
    // The native outstandings read.
    plans.extend([status(), companies.clone(), companies.clone()]);
    pair(&mut plans, extent.clone());
    for source in [
        captured(include_bytes!(
            "../crates/bridge-tally-protocol/tests/fixtures/bills_receivable_forex_live.utf16le.xml"
        )),
        captured(include_bytes!(
            "../crates/bridge-tally-protocol/tests/fixtures/groups_forex_live.utf16le.xml"
        )),
        captured(include_bytes!(
            "../crates/bridge-tally-protocol/tests/fixtures/bills_payable_forex_live.utf16le.xml"
        )),
        captured(include_bytes!(
            "../crates/bridge-tally-protocol/tests/fixtures/ledgers_currency_forex_live.utf16le.xml"
        )),
    ] {
        pair(&mut plans, source);
    }
    pair(&mut plans, extent);
    plans.extend([companies.clone(), status(), companies]);
    let plan_count = plans.len();

    let simulator = SequenceSimulator::spawn(plans).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let server = Server::new(Settings {
        endpoint: TallyEndpointConfig {
            host: "127.0.0.1".into(),
            port: simulator.address().port(),
        },
        data_dir: directory.path().into(),
        max_rows: 500,
        max_bytes: 200_000,
        redaction: Redaction::MaskParties,
        import_enabled: false,
        writes_enabled: false,
    });
    let response = server
        .call_tool(
            "outstandings",
            json!({"company_guid":"b14e9b2d-8a63-4779-804d-25d59eb787eb","as_of":"20250930"}),
        )
        .await;
    assert_eq!(simulator.finish().unwrap().len(), plan_count);
    assert_eq!(response["isError"], false, "{response}");
    let content = &response["structuredContent"];
    assert_eq!(content["evidence"]["state"], "partial");
    let result = &content["result"];
    assert_eq!(result["state"], "partial");
    assert_eq!(
        result["partial_reason"],
        "foreign_currency_ledgers_excluded"
    );
    for book_level in [
        "totals",
        "ageing_buckets",
        "top_parties",
        "open_bills",
        "unallocated",
    ] {
        assert!(
            result.get(book_level).is_none(),
            "{book_level} at book level"
        );
    }
    let base = &result["base_currency_ledgers"];
    assert_eq!(base["totals"]["receivable"], "34500");
    assert_eq!(base["open_bills"].as_array().unwrap().len(), 14);
    let excluded = &result["foreign_currency_ledgers_excluded"];
    assert_eq!(excluded["count"], 3);
    let ledgers = excluded["ledgers"].as_array().unwrap();
    assert_eq!(ledgers.len(), 3);
    assert!(ledgers.iter().all(|ledger| ledger["currency"] == "$"));
    let text = response.to_string();
    for name in [
        "BRIDGE FX DEBTOR A",
        "FX USD Debtor 01",
        "FX USD Debtor 02",
        "FX Party",
    ] {
        assert!(!text.contains(name), "{name} unmasked");
    }
}
