use super::*;

#[test]
fn missing_opening_is_partial_even_at_book_start() {
    let (row, partial) = ledger_movement_row(
        LedgerMovementRow {
            name: "Ledger".into(),
            parent: None,
            opening: None,
            debit: "-12".into(),
            credit: "4".into(),
            vouchers_touching: 2,
        },
        Redaction::None,
    )
    .unwrap();
    assert!(partial);
    assert_eq!(row["state"], "partial");
    assert_eq!(row["opening"], Value::Null);
    assert_eq!(row["closing"], Value::Null);
}

#[test]
fn movement_emptiness_uses_raw_presence_before_accounting_exclusions() {
    for (cancelled, optional) in [(true, false), (false, true), (true, true)] {
        let page = parse_movement_rows(
            vec![json!({"date":"20260901", "cancelled":cancelled,
            "optional":optional,"amounts":[]})],
            "20260901",
            "20260902",
        )
        .unwrap();
        assert_eq!(page.observed_rows, 1);
        assert_eq!(page.rows.is_empty(), cancelled || optional);
    }
    let empty = parse_movement_rows(vec![], "20260901", "20260902").unwrap();
    assert_eq!(empty.observed_rows, 0);
}

#[test]
fn movement_uses_observed_period_opening_without_inferring_account_classification() {
    for (opening, closing) in [("0", "12.5"), ("-12.5", "0")] {
        let (row, partial) = ledger_movement_row(
            LedgerMovementRow {
                name: "Unclassified ledger".into(),
                parent: None,
                opening: Some(opening.into()),
                debit: "0".into(),
                credit: "12.5".into(),
                vouchers_touching: 1,
            },
            Redaction::None,
        )
        .unwrap();
        assert!(!partial);
        assert_eq!(row["opening"], opening);
        assert_eq!(row["closing"], closing);
        assert_eq!(row["reason"], Value::Null);
    }
}

#[test]
fn active_entryless_movement_voucher_is_refused() {
    let result = parse_movement_rows(
        vec![json!({"date":"20260901", "cancelled":false,
            "optional":false,"amounts":[]})],
        "20260901",
        "20260902",
    );
    assert_eq!(
        result.err(),
        Some("ledger_movement_entries_not_observed".into())
    );
}

#[test]
fn movement_snapshot_rejects_renames_even_when_the_selected_name_returns() {
    let ledger = |name: &str| TallyLedger {
        name: name.into(),
        parent: Default::default(),
        party_gstin: Default::default(),
        opening_balance: Some("0".into()),
    };
    let opening = vec![ledger("Cash"), ledger("Sales")];
    let vouchers = |name: &str| {
        vec![MovementVoucher {
            date: "20260901".into(),
            cancelled: false,
            optional: false,
            ledger_entries: vec![MovementEntry {
                ledger_name: name.into(),
                amount: "-10".into(),
                is_deemed_positive: true,
            }],
        }]
    };
    let renamed = vec![ledger("New Cash"), ledger("Sales")];
    assert_eq!(
        validate_movement_snapshot(&opening, &renamed, &vouchers("New Cash")),
        Err("ledger_snapshot_drifted".into())
    );
    assert_eq!(
        validate_movement_snapshot(&opening, &opening, &vouchers("New Cash")),
        Err("ledger_snapshot_drifted".into())
    );
    // Known entries outside the selected Cash ledger remain admissible.
    assert_eq!(
        validate_movement_snapshot(&opening, &opening, &vouchers("Sales")),
        Ok(())
    );
    let mut changed_opening = opening.clone();
    changed_opening[0].opening_balance = Some("1".into());
    assert_eq!(
        validate_movement_snapshot(&opening, &changed_opening, &vouchers("Cash")),
        Err("ledger_snapshot_drifted".into())
    );
}

#[tokio::test]
async fn movement_read_preserves_observed_count_after_accounting_exclusions() {
    use tally_protocol_simulator::{
        Fixture, ProductStatus, ResponseFraming, ScenarioPlan, SequenceSimulator, WireEncoding,
    };
    let bytes = include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-three-vouchers.utf16le.xml"
    );
    let words = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect::<Vec<_>>();
    let captured = String::from_utf16(&words).unwrap();
    let company = crate::agent::tests::company_collection_xml();
    let companies = bridge_tally_protocol::parse_companies_from_collection(&company).unwrap();
    let observed = &companies[0];
    let identity = VerifiedCompanyIdentity::from_observed_companies(
        observed.name.clone(),
        observed.guid.clone().unwrap(),
        observed.company_number.clone().unwrap(),
        observed.books_from.clone().unwrap(),
        &companies,
    )
    .unwrap();
    for flag in ["ISCANCELLED", "ISOPTIONAL"] {
        // Deliberate accounting-state mutation of captured bytes tests count
        // propagation; it makes no claim about a new Tally response shape.
        let xml = captured.replace(
            &format!("<{flag} TYPE=\"Logical\">No</{flag}>"),
            &format!("<{flag} TYPE=\"Logical\">Yes</{flag}>"),
        );
        assert_ne!(xml, captured);
        let date = parse_agent_changed_rows(&xml).unwrap()[0]["date"]
            .as_str()
            .unwrap()
            .to_string();
        let plan = |xml: String| {
            ScenarioPlan::new(Fixture::SyntheticXml(xml))
                .with_encoding(WireEncoding::Utf16Le)
                .with_framing(ResponseFraming::ContentLength)
        };
        let status = ScenarioPlan::new(Fixture::ProductStatus(ProductStatus::TallyPrime));
        let simulator = SequenceSimulator::spawn(vec![
            plan(company.clone()),
            plan(xml.clone()),
            status.clone(),
            plan(xml),
            status,
            plan(company.clone()),
        ])
        .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let server = Server::new(Settings {
            endpoint: TallyEndpointConfig {
                host: simulator.address().ip().to_string(),
                port: simulator.address().port(),
            },
            data_dir: directory.path().to_path_buf(),
            max_rows: 10,
            max_bytes: 200_000,
            redaction: Redaction::None,
            import_enabled: false,
        });
        let (page, _) = server
            .read_movement_vouchers(&identity, &observed.name, date.clone(), date)
            .await
            .unwrap();
        assert_eq!(page.observed_rows, 3);
        assert!(
            page.rows.is_empty(),
            "excluded vouchers must not affect math"
        );
        assert_eq!(simulator.finish().unwrap().len(), 6);
    }
}
