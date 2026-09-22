//! Captured currency/native sources with one extent or currency fault at a time.
use super::*;

fn currency_source() -> String {
    decode(include_bytes!(
        "../../crates/bridge-tally-protocol/tests/fixtures/currency_inr_modern_live.utf16le.xml"
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
    let extent = extents();
    let mut plans = currency_plans(currency);
    plans.extend([status(), xml(companies()), xml(companies())]);
    pair(&mut plans, xml(extent.clone()));
    for bytes in [
        include_bytes!("../../crates/bridge-tally-protocol/tests/fixtures/agent/native-ageing-receivable.utf16le.xml").as_slice(),
        include_bytes!("../../crates/bridge-tally-protocol/tests/fixtures/agent/native-ageing-groups.utf16le.xml").as_slice(),
        include_bytes!("../../crates/bridge-tally-protocol/tests/fixtures/agent/native-ageing-payable.utf16le.xml").as_slice(),
        include_bytes!("../../crates/bridge-tally-protocol/tests/fixtures/agent/native-ageing-ledgers.utf16le.xml").as_slice(),
    ] {
        pair(&mut plans, xml(decode(bytes)));
    }
    pair(&mut plans, xml(extent));
    plans.extend([xml(companies()), status(), xml(companies())]);
    plans
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
    (result, simulator.finish().unwrap().len())
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
    for (fault, reason) in [
        ("multiple", "company_base_currency_undetermined"),
        ("empty", "company_currency_probe_failed"),
    ] {
        let mut changed = captured.clone();
        if fault == "multiple" {
            changed.insert_str(end, &captured[start..end]);
        } else {
            changed.replace_range(start..end, "");
        }
        let (result, requests) = operator_outstandings(currency_then_native_plans(changed)).await;
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
/// the two reads is refused before any bill is read.
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
    let error = result.unwrap_err();
    assert!(matches!(
        error
            .chain()
            .find_map(|cause| cause.downcast_ref::<PairedReadValidationError>()),
        Some(PairedReadValidationError::CurrencyToMasterExtent)
    ));
    assert_eq!(requests, 21);
}

/// bridge#604, through the desktop command's own body: an operator's INR
/// assertion for a book with several Currency masters comes back as a partial
/// result, with no working paper, and no bill is read.
#[tokio::test]
async fn the_desktop_command_refuses_several_currency_masters_before_any_bill() {
    let captured = currency_source();
    let start = captured.find("<CURRENCY ").unwrap();
    let end = start + captured[start..].find("</CURRENCY>").unwrap() + "</CURRENCY>".len();
    let mut several = captured.clone();
    several.insert_str(end, &captured[start..end]);
    let mut plans = vec![xml(companies())];
    plans.extend(currency_then_native_plans(several));
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
    simulator.cancel();
    // The company list, then the currency read's 14 requests, and no more.
    assert_eq!(simulator.finish().unwrap().len(), 15);
}
