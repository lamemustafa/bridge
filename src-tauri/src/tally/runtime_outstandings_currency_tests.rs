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
