//! Captured currency/native sources with one extent or currency fault at a time.
use super::*;

fn currency_source() -> String {
    decode(include_bytes!(
        "../../crates/bridge-tally-protocol/tests/fixtures/currency_inr_modern_live.utf16le.xml"
    ))
}

fn multi_currency_source() -> String {
    decode(include_bytes!(
        "../../crates/bridge-tally-protocol/tests/fixtures/currency_multi_live.utf16le.xml"
    ))
}

fn currency_plans(currency: String) -> Vec<ScenarioPlan> {
    let company = xml(companies());
    let extent = xml(extents());
    let mut plans = vec![company.clone()];
    pair(&mut plans, extent.clone());
    pair(&mut plans, xml(currency));
    pair(&mut plans, extent);
    plans.push(company);
    plans
}

#[tokio::test]
async fn agent_outstandings_currency_witness_brackets_native_source() {
    for fault in ["none", "opening", "closing"] {
        let extent = extents();
        let changed = extent.replace(
            "<ALTMSTID TYPE=\"Number\"> 224</ALTMSTID>",
            "<ALTMSTID TYPE=\"Number\"> 225</ALTMSTID>",
        );
        assert_ne!(changed, extent);
        let mut plans = currency_plans(currency_source());
        plans.extend([status(), xml(companies()), xml(companies())]);
        pair(
            &mut plans,
            xml(if fault == "opening" {
                changed.clone()
            } else {
                extent.clone()
            }),
        );
        if fault != "opening" {
            for bytes in [
                include_bytes!("../../crates/bridge-tally-protocol/tests/fixtures/agent/native-ageing-receivable.utf16le.xml").as_slice(),
                include_bytes!("../../crates/bridge-tally-protocol/tests/fixtures/agent/native-ageing-groups.utf16le.xml").as_slice(),
                include_bytes!("../../crates/bridge-tally-protocol/tests/fixtures/agent/native-ageing-payable.utf16le.xml").as_slice(),
                include_bytes!("../../crates/bridge-tally-protocol/tests/fixtures/agent/native-ageing-ledgers.utf16le.xml").as_slice(),
            ] { pair(&mut plans, xml(decode(bytes))); }
            pair(
                &mut plans,
                xml(if fault == "closing" { changed } else { extent }),
            );
            if fault == "none" {
                plans.extend([xml(companies()), status(), xml(companies())]);
            }
        }
        let responses = plans
            .iter()
            .map(|plan| encode(&plan.fixture.body(), plan.encoding))
            .collect::<Vec<_>>();
        let simulator = SequenceSimulator::spawn(plans).unwrap();
        let config = TallyConfig {
            host: simulator.address().ip().to_string(),
            port: simulator.address().port(),
        };
        let runtime = TallyRuntime::default();
        let identity = identity_for_guid(&companies(), "eebb9a9f-1679-4468-9e8f-814c729674cb");
        let currency = runtime
            .detect_base_currency_with_extent(config.clone(), &identity)
            .await
            .unwrap();
        let currency_evidence = currency.evidence();
        let witness = currency.admit_inr().unwrap();
        let result = runtime
            .fetch_agent_outstandings_with_evidence(
                config,
                &identity,
                TallyDate::parse("20260801").unwrap(),
                witness,
                OutstandingsAgeingAnchor::DueDate,
            )
            .await;
        let native_evidence = if fault == "opening" {
            let error = result.unwrap_err();
            assert!(matches!(
                error
                    .chain()
                    .find_map(|cause| cause.downcast_ref::<PairedReadValidationError>()),
                Some(PairedReadValidationError::CurrencyToMasterExtent)
            ));
            error
                .downcast_ref::<RuntimeReadFailure>()
                .unwrap()
                .evidence
                .clone()
        } else {
            let (load, evidence) = result.unwrap();
            if fault == "none" {
                assert!(matches!(
                    load,
                    OutstandingsLoadResult::Complete {
                        currency_assertion: OutstandingsCurrencyAssertion::Inr,
                        ..
                    }
                ));
            } else {
                assert!(
                    matches!(load, OutstandingsLoadResult::Partial { reason, .. } if reason == "book_changed_during_read".into())
                );
            }
            evidence
        };
        let requests = simulator.finish().unwrap();
        assert_eq!(
            requests.len(),
            match fault {
                "opening" => 21,
                "closing" => 41,
                _ => 44,
            }
        );
        assert_eq!(
            currency_evidence.request_sha256,
            requests[5].request_body_sha256
        );
        assert_eq!(currency_evidence.response_sha256, sha256_hex(&responses[5]));
        assert_eq!(currency_evidence.bytes, responses[5].len() * 2);
        let mut expected_request = join(
            &requests[14].request_body_sha256,
            &requests[15].request_body_sha256,
        );
        let mut expected_response = join(&sha256_hex(&responses[14]), &sha256_hex(&responses[15]));
        let mut expected_bytes = responses[14].len() + responses[15].len();
        if fault != "opening" {
            for i in [21, 25, 29, 33] {
                expected_request = join(&expected_request, &requests[i].request_body_sha256);
                expected_response = join(&expected_response, &sha256_hex(&responses[i]));
                expected_bytes += responses[i].len() * 2;
            }
        }
        if fault == "none" {
            expected_request = join(
                &expected_request,
                &join(
                    &requests[42].request_body_sha256,
                    &requests[43].request_body_sha256,
                ),
            );
            expected_response = join(
                &expected_response,
                &join(&sha256_hex(&responses[42]), &sha256_hex(&responses[43])),
            );
            expected_bytes += responses[42].len() + responses[43].len();
        }
        assert_eq!(native_evidence.request_sha256, expected_request);
        assert_eq!(native_evidence.response_sha256, expected_response);
        assert_eq!(native_evidence.bytes, expected_bytes);
    }
}

#[tokio::test]
async fn agent_outstandings_rejects_unadmitted_currency_before_native_read() {
    let captured = currency_source();
    for fault in ["foreign", "multiple", "empty", "malformed"] {
        let faulty = match fault {
            "foreign" => captured.replace(
                "<MAILINGNAME TYPE=\"String\">INR</MAILINGNAME>",
                "<MAILINGNAME TYPE=\"String\">USD</MAILINGNAME>",
            ),
            "malformed" => captured.replace(
                "<DECIMALPLACES TYPE=\"Number\"> 2</DECIMALPLACES>",
                "<DECIMALPLACES TYPE=\"Number\">invalid</DECIMALPLACES>",
            ),
            other => {
                let start = captured.find("<CURRENCY ").unwrap();
                let end =
                    start + captured[start..].find("</CURRENCY>").unwrap() + "</CURRENCY>".len();
                let mut changed = captured.clone();
                if other == "multiple" {
                    changed.insert_str(end, &captured[start..end]);
                } else {
                    changed.replace_range(start..end, "");
                }
                changed
            }
        };
        assert_ne!(faulty, captured);
        let response = encode(&faulty, WireEncoding::Utf16Le);
        let simulator = SequenceSimulator::spawn(currency_plans(faulty)).unwrap();
        let result = TallyRuntime::default()
            .detect_base_currency_with_extent(
                TallyConfig {
                    host: simulator.address().ip().to_string(),
                    port: simulator.address().port(),
                },
                &identity_for_guid(&companies(), "eebb9a9f-1679-4468-9e8f-814c729674cb"),
            )
            .await;
        let evidence = if fault == "malformed" {
            let error = result.unwrap_err();
            assert!(matches!(
                error
                    .chain()
                    .find_map(|cause| cause.downcast_ref::<NativeOutstandingsError>()),
                Some(NativeOutstandingsError::InvalidResponse(
                    "currency_decimal_places_invalid"
                ))
            ));
            error
                .downcast_ref::<RuntimeReadFailure>()
                .unwrap()
                .evidence
                .clone()
        } else {
            let read = result.unwrap();
            let evidence = read.evidence();
            assert_eq!(
                read.admit_inr().unwrap_err(),
                match fault {
                    "foreign" => "company_base_currency_not_inr",
                    "multiple" => "company_base_currency_undetermined",
                    _ => "company_currency_probe_failed",
                }
            );
            evidence
        };
        let requests = simulator.finish().unwrap();
        assert_eq!(requests.len(), 14);
        assert_eq!(evidence.request_sha256, requests[5].request_body_sha256);
        assert_eq!(evidence.response_sha256, sha256_hex(&response));
        assert_eq!(evidence.bytes, response.len() * 2);
    }
}

/// The currency read, then a complete native outstandings read on one
/// unchanged extent, as `agent_outstandings_currency_witness_brackets_native_source`
/// scripts it with no fault.
fn currency_then_native_plans(currency: String) -> Vec<ScenarioPlan> {
    currency_then_native_plans_with_ledgers(currency, ageing_ledgers())
}

fn ageing_ledgers() -> String {
    decode(include_bytes!(
        "../../crates/bridge-tally-protocol/tests/fixtures/agent/native-ageing-ledgers.utf16le.xml"
    ))
}

fn currency_then_native_plans_with_ledgers(currency: String, ledgers: String) -> Vec<ScenarioPlan> {
    let mut plans = currency_plans(currency);
    plans.extend(native_plans_with_ledgers(ledgers));
    plans
}

fn native_plans_with_ledgers(ledgers: String) -> Vec<ScenarioPlan> {
    let extent = extents();
    let mut plans = vec![status(), xml(companies()), xml(companies())];
    pair(&mut plans, xml(extent.clone()));
    for bytes in [
        include_bytes!("../../crates/bridge-tally-protocol/tests/fixtures/agent/native-ageing-receivable.utf16le.xml").as_slice(),
        include_bytes!("../../crates/bridge-tally-protocol/tests/fixtures/agent/native-ageing-groups.utf16le.xml").as_slice(),
        include_bytes!("../../crates/bridge-tally-protocol/tests/fixtures/agent/native-ageing-payable.utf16le.xml").as_slice(),
    ] {
        pair(&mut plans, xml(decode(bytes)));
    }
    pair(&mut plans, xml(ledgers));
    pair(&mut plans, xml(extent));
    plans.extend([xml(companies()), status(), xml(companies())]);
    plans
}

/// bridge#551: the captured ageing ledgers predate `CURRENCYNAME`. Each row
/// gets the field in the captured position (`ledgers_currency_forex_live`):
/// the book's single master `I₹` on every row but `Ageing Customer A`.
fn ageing_ledgers_with_currency(customer_a: &str) -> String {
    let captured = ageing_ledgers();
    let mut pieces = captured.split("<LEDGER ");
    let mut ledgers = pieces.next().unwrap().to_string();
    let mut rows = 0;
    for piece in pieces {
        let tag_end = piece.find('>').unwrap() + 1;
        let currency = if piece.starts_with("NAME=\"Ageing Customer A\"") {
            customer_a
        } else {
            "I\u{20b9}"
        };
        ledgers.push_str("<LEDGER ");
        ledgers.push_str(&piece[..tag_end]);
        ledgers.push_str(&format!(
            "\r\n     <CURRENCYNAME TYPE=\"String\">{currency}</CURRENCYNAME>"
        ));
        ledgers.push_str(&piece[tag_end..]);
        rows += 1;
    }
    assert_eq!(rows, 6);
    assert!(ledgers.contains(&format!(
        "<LEDGER NAME=\"Ageing Customer A\" RESERVEDNAME=\"\">\r\n     <CURRENCYNAME TYPE=\"String\">{customer_a}</CURRENCYNAME>"
    )));
    ledgers
}

async fn operator_outstandings(
    plans: Vec<ScenarioPlan>,
) -> (anyhow::Result<OutstandingsLoadResult>, usize) {
    let simulator = SequenceSimulator::spawn(plans).unwrap();
    let result = TallyRuntime::default()
        .fetch_operator_outstandings(
            TallyConfig {
                host: simulator.address().ip().to_string(),
                port: simulator.address().port(),
            },
            &identity_for_guid(&companies(), "eebb9a9f-1679-4468-9e8f-814c729674cb"),
            TallyDate::parse("20260801").unwrap(),
            OutstandingsCurrencyAssertion::Inr,
            OutstandingsAgeingAnchor::DueDate,
        )
        .await;
    simulator.cancel();
    (result, requests_sent(simulator))
}

/// The requests the client sent. `cancel` wakes the simulator with a
/// connection of its own, which the simulator records as a cancelled,
/// unprocessed entry when it was already waiting for the next request; that
/// entry is the harness's, not a request, and whether it appears depends on
/// scheduling (it did under load, 15 against 14).
fn requests_sent(simulator: SequenceSimulator) -> usize {
    simulator
        .finish()
        .unwrap()
        .iter()
        .filter(|request| !request.cancelled)
        .count()
}

/// bridge#604: the desktop read of an operator's INR assertion reads the
/// currency masters itself, and refuses a book with several (which can hold a
/// foreign-currency ledger whose bills Tally returns as plain amounts) or with
/// none (several not ruled out) before any bill is read. Spare native
/// responses are queued so that a read past the refusal would be served and
/// counted.
#[tokio::test]
async fn operator_outstandings_refuse_several_or_no_currency_masters_before_any_bill() {
    let captured = currency_source();
    let start = captured.find("<CURRENCY ").unwrap();
    let end = start + captured[start..].find("</CURRENCY>").unwrap() + "</CURRENCY>".len();
    let mut none = captured.clone();
    none.replace_range(start..end, "");
    for (fault, currency, reason) in [
        // A live capture of a book with `I₹` and `$` masters.
        (
            "multiple",
            multi_currency_source(),
            "company_base_currency_undetermined",
        ),
        ("empty", none, "company_currency_probe_failed"),
    ] {
        let (result, requests) = operator_outstandings(currency_then_native_plans(currency)).await;
        let result = result.unwrap();
        assert!(
            matches!(&result, OutstandingsLoadResult::Partial { reason: got, .. } if *got == reason.into()),
            "{fault}: {result:?}"
        );
        // The currency read's 14 requests, and not one more.
        assert_eq!(requests, 14, "{fault}");
    }
}

/// bridge#604: with one Currency master the operator's assertion stands, as
/// before: a master Tally names INR, and one it does not (the case the
/// screen's confirmation exists for), both read through to a complete report.
#[tokio::test]
async fn operator_outstandings_with_one_currency_master_read_through() {
    let captured = currency_source();
    let foreign = captured.replace(
        "<MAILINGNAME TYPE=\"String\">INR</MAILINGNAME>",
        "<MAILINGNAME TYPE=\"String\">USD</MAILINGNAME>",
    );
    assert_ne!(foreign, captured);
    for currency in [captured, foreign] {
        let (result, requests) = operator_outstandings(currency_then_native_plans(currency)).await;
        assert!(matches!(
            result.unwrap(),
            OutstandingsLoadResult::Complete {
                currency_assertion: OutstandingsCurrencyAssertion::Inr,
                ..
            }
        ),);
        assert_eq!(requests, 44);
    }
}

/// bridge#604: the operator's assertion is bound to the extent the currency
/// read observed, as the agent read's witness is. A book that changed between
/// the two reads is the retryable partial a change during the read gives,
/// before any bill is read.
#[tokio::test]
async fn operator_outstandings_refuse_a_book_changed_since_the_currency_read() {
    let extent = extents();
    let changed = extent.replace(
        "<ALTMSTID TYPE=\"Number\"> 224</ALTMSTID>",
        "<ALTMSTID TYPE=\"Number\"> 225</ALTMSTID>",
    );
    assert_ne!(changed, extent);
    let mut plans = currency_plans(currency_source());
    plans.extend([status(), xml(companies()), xml(companies())]);
    pair(&mut plans, xml(changed));
    // Served only if the read went on past the changed extent.
    pair(&mut plans, xml(decode(include_bytes!(
        "../../crates/bridge-tally-protocol/tests/fixtures/agent/native-ageing-receivable.utf16le.xml"
    ))));
    let (result, requests) = operator_outstandings(plans).await;
    let result = result.unwrap();
    assert!(
        matches!(&result, OutstandingsLoadResult::Partial { reason, .. }
            if *reason == "book_changed_during_read".into()),
        "{result:?}"
    );
    assert_eq!(requests, 21);
}

/// bridge#604, through the desktop command's own body: an operator's INR
/// assertion for a book with several Currency masters comes back as a partial
/// result, with no working paper, and no bill is read.
#[tokio::test]
async fn the_desktop_command_refuses_several_currency_masters_before_any_bill() {
    let mut plans = vec![xml(companies())];
    plans.extend(currency_then_native_plans(multi_currency_source()));
    let simulator = SequenceSimulator::spawn(plans).unwrap();
    let rows = parse_companies_from_collection(&companies()).unwrap();
    let row = rows
        .iter()
        .find(|row| row.guid.as_deref() == Some("eebb9a9f-1679-4468-9e8f-814c729674cb"))
        .unwrap();
    let request: crate::commands::OutstandingsRequest = serde_json::from_value(serde_json::json!({
        "config": {"host": simulator.address().ip().to_string(), "port": simulator.address().port()},
        "selected_company": {
            "display_name": row.name,
            "company_guid": row.guid,
            "company_number": row.company_number,
            "books_from_yyyymmdd": row.books_from,
        },
        "currency_assertion": "INR",
        "as_of_yyyymmdd": "20260801",
    }))
    .unwrap();
    let response = crate::commands::read_screen_outstandings(
        request,
        &TallyRuntime::default(),
        &crate::reports::outstandings_working_paper_store::WorkingPaperExportStore::default(),
        &crate::reports::outstandings_working_paper_store::PartyStatementSourceStore::default(),
    )
    .await
    .unwrap();
    assert!(
        matches!(&response.result, OutstandingsLoadResult::Partial { reason, .. }
            if *reason == "company_base_currency_undetermined".into()),
        "{:?}",
        response.result
    );
    assert!(response.working_paper_export_id.is_none());
    assert!(response.party_statement_source_id.is_none());
    simulator.cancel();
    // The company list, then the currency read's 14 requests, and no more.
    assert_eq!(requests_sent(simulator), 15);
}

/// bridge#551: on a book with one Currency master, every ledger's own
/// currency must be that master's NAME. One that is not refuses the read
/// in-band, naming the ledger, instead of any figure; one that is reads
/// through.
#[tokio::test]
async fn a_ledger_in_another_currency_on_a_one_master_book_refuses_the_read() {
    let (result, requests) = operator_outstandings(currency_then_native_plans_with_ledgers(
        currency_source(),
        ageing_ledgers_with_currency("$"),
    ))
    .await;
    match result.unwrap() {
        OutstandingsLoadResult::Partial { reason, .. } => {
            assert_eq!(reason.reason_code, "ledger_currency_base_unmatched");
            assert_eq!(
                reason.foreign_currency_ledger_name.as_deref(),
                Some("Ageing Customer A")
            );
        }
        other => panic!("{other:?}"),
    }
    // The snapshot is the last native read, so the read runs to its end.
    assert_eq!(requests, 44);

    let (result, requests) = operator_outstandings(currency_then_native_plans_with_ledgers(
        currency_source(),
        ageing_ledgers_with_currency("I\u{20b9}"),
    ))
    .await;
    assert!(
        matches!(result.unwrap(), OutstandingsLoadResult::Complete { .. }),
        "the same book with every ledger in its master reads through"
    );
    assert_eq!(requests, 44);
}

/// bridge#551: only a base among several masters can find a ledger in another
/// currency, and none is admitted before bridge#601. Where one is found, every
/// figure is withheld, naming the ledger, rather than reported for the whole
/// book or for part of it.
#[test]
fn a_foreign_ledger_withholds_every_figure() {
    let foreign =
        |ledger: &str| bridge_tally_protocol::native_outstandings::ForeignCurrencyLedger {
            ledger: ledger.to_string(),
            currency: "$".to_string(),
        };
    let snapshot = |foreign| ClassifiedLedgerSnapshot {
        base: Vec::new(),
        foreign,
        unobserved: 0,
    };
    assert_eq!(
        foreign_ledger_withholds_figures(&snapshot(Vec::new())),
        None
    );
    match foreign_ledger_withholds_figures(&snapshot(vec![
        foreign("FX Debtor A"),
        foreign("FX Debtor B"),
    ])) {
        Some(OutstandingsLoadResult::Partial { reason, .. }) => {
            assert_eq!(reason.reason_code, "foreign_currency_ledger_present");
            assert_eq!(
                reason.foreign_currency_ledger_name.as_deref(),
                Some("FX Debtor A")
            );
        }
        other => panic!("{other:?}"),
    }
}

/// bridge#551: a witness without a base has nothing to compare a ledger with,
/// so every ledger refuses as unmatched, naming none. No production witness
/// lacks one today (every path admits exactly one master, and the currency
/// parser refuses a blank NAME); the refusal keeps a future one fail-closed.
#[tokio::test]
async fn a_witness_without_a_base_refuses_every_ledger() {
    let simulator = SequenceSimulator::spawn(native_plans_with_ledgers(ageing_ledgers())).unwrap();
    let identity = identity_for_guid(&companies(), "eebb9a9f-1679-4468-9e8f-814c729674cb");
    let mut witness = inr_witness_for_tests(&extents(), &identity);
    witness.base = None;
    let (result, _) = TallyRuntime::default()
        .fetch_agent_outstandings_with_evidence(
            TallyConfig {
                host: simulator.address().ip().to_string(),
                port: simulator.address().port(),
            },
            &identity,
            TallyDate::parse("20260801").unwrap(),
            witness,
            OutstandingsAgeingAnchor::DueDate,
        )
        .await
        .unwrap();
    simulator.cancel();
    match result {
        OutstandingsLoadResult::Partial { reason, .. } => {
            assert_eq!(reason.reason_code, "ledger_currency_base_unmatched");
            assert_eq!(reason.foreign_currency_ledger_name, None);
        }
        other => panic!("{other:?}"),
    }
}

/// bridge#551: the all-companies sweep reads each company under its own
/// currency read, so a ledger in another currency on a one-master book
/// refuses there too, and the same book with every ledger in its master
/// reads through.
#[tokio::test]
async fn the_sweep_compares_each_ledgers_currency_with_its_own_read() {
    for (customer_a, refused) in [("$", true), ("I\u{20b9}", false)] {
        let simulator = SequenceSimulator::spawn(currency_then_native_plans_with_ledgers(
            currency_source(),
            ageing_ledgers_with_currency(customer_a),
        ))
        .unwrap();
        let result = crate::commands::sweep_company_outstandings(
            &TallyRuntime::default(),
            &TallyConfig {
                host: simulator.address().ip().to_string(),
                port: simulator.address().port(),
            },
            &identity_for_guid(&companies(), "eebb9a9f-1679-4468-9e8f-814c729674cb"),
            &TallyDate::parse("20260801").unwrap(),
            OutstandingsCurrencyAssertion::Inr,
            OutstandingsAgeingAnchor::DueDate,
        )
        .await;
        simulator.cancel();
        match result {
            Ok(OutstandingsLoadResult::Partial { reason, .. }) if refused => {
                assert_eq!(reason.reason_code, "ledger_currency_base_unmatched");
                assert_eq!(
                    reason.foreign_currency_ledger_name.as_deref(),
                    Some("Ageing Customer A")
                );
            }
            Ok(OutstandingsLoadResult::Complete { .. }) if !refused => {}
            Ok(other) => panic!("{customer_a}: {other:?}"),
            Err(_) => panic!("{customer_a}: the sweep failed"),
        }
    }
}

/// bridge#551: every currency refusal is an in-band partial naming the ledger
/// it concerns, when it concerns one.
#[test]
fn a_currency_refusal_is_a_partial_naming_its_ledger() {
    for (refusal, code, ledger) in [
        (
            LedgerCurrencyRefusal::Unobserved {
                ledger: "Synthetic Debtor".to_string(),
            },
            "ledger_currency_unobserved",
            Some("Synthetic Debtor"),
        ),
        (
            LedgerCurrencyRefusal::BaseUnmatched {
                ledger: Some("Synthetic FX Debtor".to_string()),
            },
            "ledger_currency_base_unmatched",
            Some("Synthetic FX Debtor"),
        ),
        (
            LedgerCurrencyRefusal::BaseUnmatched { ledger: None },
            "ledger_currency_base_unmatched",
            None,
        ),
    ] {
        match admit_native_ledger_snapshot(Err(anyhow::Error::from(
            NativeOutstandingsError::LedgerCurrency(refusal),
        ))) {
            Ok(NativeLedgerSnapshotAdmission::Partial(OutstandingsLoadResult::Partial {
                reason,
                ..
            })) => {
                assert_eq!(reason.reason_code, code);
                assert_eq!(reason.foreign_currency_ledger_name.as_deref(), ledger);
            }
            _ => panic!("{code}: not an in-band partial"),
        }
    }
}

/// The sweep admits only a book with one Currency master that Tally names INR,
/// and refuses any other after its currency read, before any bill: a book
/// with several masters (captured), and one master named something else.
#[tokio::test]
async fn the_sweep_refuses_a_book_that_is_not_single_currency_inr_before_any_bill() {
    let foreign = currency_source().replace(
        "<MAILINGNAME TYPE=\"String\">INR</MAILINGNAME>",
        "<MAILINGNAME TYPE=\"String\">USD</MAILINGNAME>",
    );
    assert_ne!(foreign, currency_source());
    for (currency, expected) in [
        (
            multi_currency_source(),
            "company_base_currency_undetermined",
        ),
        (foreign, "company_base_currency_not_inr"),
    ] {
        let simulator = SequenceSimulator::spawn(currency_then_native_plans(currency)).unwrap();
        let result = crate::commands::sweep_company_outstandings(
            &TallyRuntime::default(),
            &TallyConfig {
                host: simulator.address().ip().to_string(),
                port: simulator.address().port(),
            },
            &identity_for_guid(&companies(), "eebb9a9f-1679-4468-9e8f-814c729674cb"),
            &TallyDate::parse("20260801").unwrap(),
            OutstandingsCurrencyAssertion::Inr,
            OutstandingsAgeingAnchor::DueDate,
        )
        .await;
        simulator.cancel();
        match result {
            Err(crate::commands::CompanySweepFailure::ReasonCode(code)) => {
                assert_eq!(code, expected);
            }
            _ => panic!("{expected}: not refused"),
        }
        // The currency read's 14 requests, and not one more.
        assert_eq!(requests_sent(simulator), 14, "{expected}");
    }
}

/// bridge#551: a completed desktop read issues the working paper's one-use
/// handle and a separate statement handle over the same held source.
/// Exporting the working paper consumes only its own handle: statements
/// remain exportable, once per party, from the rows Bridge holds.
#[tokio::test]
async fn a_completed_read_holds_its_statement_source_apart_from_the_working_paper() {
    let mut plans = vec![xml(companies())];
    plans.extend(currency_then_native_plans(currency_source()));
    let simulator = SequenceSimulator::spawn(plans).unwrap();
    let rows = parse_companies_from_collection(&companies()).unwrap();
    let row = rows
        .iter()
        .find(|row| row.guid.as_deref() == Some("eebb9a9f-1679-4468-9e8f-814c729674cb"))
        .unwrap();
    let request: crate::commands::OutstandingsRequest = serde_json::from_value(serde_json::json!({
        "config": {"host": simulator.address().ip().to_string(), "port": simulator.address().port()},
        "selected_company": {
            "display_name": row.name,
            "company_guid": row.guid,
            "company_number": row.company_number,
            "books_from_yyyymmdd": row.books_from,
        },
        "currency_assertion": "INR",
        "as_of_yyyymmdd": "20260801",
    }))
    .unwrap();
    let working_papers =
        crate::reports::outstandings_working_paper_store::WorkingPaperExportStore::default();
    let statements =
        crate::reports::outstandings_working_paper_store::PartyStatementSourceStore::default();
    let response = crate::commands::read_screen_outstandings(
        request,
        &TallyRuntime::default(),
        &working_papers,
        &statements,
    )
    .await
    .unwrap();
    simulator.cancel();
    let OutstandingsLoadResult::Complete {
        statement_open_bills,
        ..
    } = &response.result
    else {
        panic!("{:?}", response.result);
    };
    assert!(!statement_open_bills.is_empty());
    let statement_id = response.party_statement_source_id.clone().unwrap();
    let working_paper_id = response.working_paper_export_id.clone().unwrap();
    assert_ne!(statement_id, working_paper_id);
    working_papers.take(&working_paper_id).unwrap();
    for _ in 0..2 {
        let source = crate::commands::party_statement_source(&statements, &statement_id).unwrap();
        assert_eq!(&source.open_bills, statement_open_bills);
    }
}
