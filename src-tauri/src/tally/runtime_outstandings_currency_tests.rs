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
    guid: &str,
) -> (anyhow::Result<OutstandingsLoadResult>, usize) {
    let simulator = SequenceSimulator::spawn(plans).unwrap();
    let result = TallyRuntime::default()
        .fetch_operator_outstandings(
            TallyConfig {
                host: simulator.address().ip().to_string(),
                port: simulator.address().port(),
            },
            &identity_for_guid(&companies(), guid),
            TallyDate::parse("20260801").unwrap(),
            Some(OutstandingsCurrencyAssertion::Inr),
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

const AGEING_GUID: &str = "eebb9a9f-1679-4468-9e8f-814c729674cb";

/// Labelled edits of the captured Company collection: its FOREX row's
/// `CURRENCYNAME` replaced by `value`.
fn forex_company_naming(value: &str) -> String {
    let company = forex_company_currency();
    let rupee = "<CURRENCYNAME TYPE=\"String\">\u{20b9}</CURRENCYNAME>";
    assert!(company.find(rupee).unwrap() < company.find("BRIDGE SHAPE LAB").unwrap());
    company.replacen(
        rupee,
        &format!("<CURRENCYNAME TYPE=\"String\">{value}</CURRENCYNAME>"),
        1,
    )
}

/// FOREX's classified currency read with its company naming `value`, then
/// FOREX's native read, queued so that a read past a refusal would be served
/// and counted.
fn forex_plans_naming(value: &str) -> Vec<ScenarioPlan> {
    let mut plans = classified_currency_plans(
        forex_originalname_currency(),
        forex_company_naming(value),
        forex_company_naming(value),
    );
    plans.extend(forex_native_plans());
    plans
}

/// bridge#551, on the desktop read: a book whose currency read admits no INR
/// base refuses before any bill. Several masters, from labelled edits of the
/// FOREX captures: the company names its `$` master (not INR) or no master
/// (undetermined). And a read with no master at all.
#[tokio::test]
async fn operator_outstandings_refuse_a_book_without_an_inr_base_before_any_bill() {
    let captured = currency_source();
    let start = captured.find("<CURRENCY ").unwrap();
    let end = start + captured[start..].find("</CURRENCY>").unwrap() + "</CURRENCY>".len();
    let mut none = captured.clone();
    none.replace_range(start..end, "");
    for (fault, plans, guid, reason, requests) in [
        (
            "dollar base",
            forex_plans_naming("$"),
            FOREX_GUID,
            "company_base_currency_not_inr",
            22,
        ),
        (
            "no base",
            forex_plans_naming("\u{20ac}"),
            FOREX_GUID,
            "company_base_currency_undetermined",
            22,
        ),
        (
            "empty",
            currency_then_native_plans(none),
            AGEING_GUID,
            "company_currency_probe_failed",
            14,
        ),
    ] {
        let (result, sent) = operator_outstandings(plans, guid).await;
        let result = result.unwrap();
        assert!(
            matches!(&result, OutstandingsLoadResult::Partial { reason: got, .. } if *got == reason.into()),
            "{fault}: {result:?}"
        );
        // The currency read's requests (the plain read, and with several
        // masters the ORIGINALNAME re-read and the Company read), no more.
        assert_eq!(sent, requests, "{fault}");
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
        let (result, requests) =
            operator_outstandings(currency_then_native_plans(currency), AGEING_GUID).await;
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
    let (result, requests) = operator_outstandings(plans, AGEING_GUID).await;
    let result = result.unwrap();
    assert!(
        matches!(&result, OutstandingsLoadResult::Partial { reason, .. }
            if *reason == "book_changed_during_read".into()),
        "{result:?}"
    );
    assert_eq!(requests, 21);
}

/// bridge#551, through the desktop command's own body on FOREX's captures:
/// a book with an INR base and a `$` master reads through, with its dollar
/// ledgers left out, as the base-currency-ledgers-only partial. No working
/// paper, statement source or unavailable-reason is issued for it.
#[tokio::test]
async fn the_desktop_command_reads_forex_as_base_currency_ledgers_only() {
    let mut plans = vec![xml(companies())];
    plans.extend(forex_classified_currency_plans());
    plans.extend(forex_native_plans());
    let plan_count = plans.len();
    let simulator = SequenceSimulator::spawn(plans).unwrap();
    let rows = parse_companies_from_collection(&companies()).unwrap();
    let row = rows
        .iter()
        .find(|row| row.guid.as_deref() == Some(FOREX_GUID))
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
        "as_of_yyyymmdd": "20250930",
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
    let OutstandingsLoadResult::BaseCurrencyLedgersOnly {
        foreign_currency_ledgers_excluded,
        ..
    } = &response.result
    else {
        panic!("{:?}", response.result);
    };
    assert_eq!(foreign_currency_ledgers_excluded.len(), 3);
    assert!(response.working_paper_export_id.is_none());
    assert!(response.party_statement_source_id.is_none());
    assert_eq!(response.working_paper_unavailable_reason_code, None);
    assert_eq!(requests_sent(simulator), plan_count);
}

/// bridge#551: on a book with one Currency master, every ledger's own
/// currency must be that master's NAME. One that is not refuses the read
/// in-band, naming the ledger, instead of any figure; one that is reads
/// through.
#[tokio::test]
async fn a_ledger_in_another_currency_on_a_one_master_book_refuses_the_read() {
    let (result, requests) = operator_outstandings(
        currency_then_native_plans_with_ledgers(
            currency_source(),
            ageing_ledgers_with_currency("$"),
        ),
        AGEING_GUID,
    )
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

    let (result, requests) = operator_outstandings(
        currency_then_native_plans_with_ledgers(
            currency_source(),
            ageing_ledgers_with_currency("I\u{20b9}"),
        ),
        AGEING_GUID,
    )
    .await;
    assert!(
        matches!(result.unwrap(), OutstandingsLoadResult::Complete { .. }),
        "the same book with every ledger in its master reads through"
    );
    assert_eq!(requests, 44);
}

const FOREX_GUID: &str = "b14e9b2d-8a63-4779-804d-25d59eb787eb";

fn fixture(bytes: &[u8]) -> ScenarioPlan {
    xml(decode(bytes))
}

fn forex_originalname_currency() -> String {
    decode(include_bytes!(
        "../../crates/bridge-tally-protocol/tests/fixtures/currency_originalname_forex_live.utf16le.xml"
    ))
}

fn forex_company_currency() -> String {
    decode(include_bytes!(
        "../../crates/bridge-tally-protocol/tests/fixtures/company_currencyname_live.utf16le.xml"
    ))
}

/// The classified currency read of a two-master book: the plain Currency
/// collection, the same with `ORIGINALNAME`, then the Company collection as a
/// pair (`company` twice, then `company_again` on the second send), inside the
/// identity and extent bracket.
fn classified_currency_plans(
    with_original_names: String,
    company: String,
    company_again: String,
) -> Vec<ScenarioPlan> {
    let identity = xml(companies());
    let extent = xml(extents());
    let mut plans = vec![identity.clone()];
    pair(&mut plans, extent.clone());
    pair(&mut plans, xml(multi_currency_source()));
    pair(&mut plans, xml(with_original_names));
    plans.extend([xml(company), status(), xml(company_again), status()]);
    pair(&mut plans, extent);
    plans.push(identity);
    plans
}

/// FOREX's classified currency read, from captures (the plain two-master read
/// of 2026-08-23, the `ORIGINALNAME` read and the edited Company collection of
/// 2026-09-23).
fn forex_classified_currency_plans() -> Vec<ScenarioPlan> {
    classified_currency_plans(
        forex_originalname_currency(),
        forex_company_currency(),
        forex_company_currency(),
    )
}

fn request_sha256(request: &str) -> String {
    sha256_hex(&bridge_tally_protocol::encode_tally_xml_request_utf16le(
        request,
    ))
}

async fn forex_classified_read(
    plans: Vec<ScenarioPlan>,
) -> anyhow::Result<ClassifiedCompanyCurrencyRead> {
    let simulator = SequenceSimulator::spawn(plans).unwrap();
    let read = TallyRuntime::default()
        .detect_classified_base_currency_with_extent(
            TallyConfig {
                host: simulator.address().ip().to_string(),
                port: simulator.address().port(),
            },
            &identity_for_guid(&companies(), FOREX_GUID),
        )
        .await;
    simulator.cancel();
    read
}

/// bridge#551, labelled edits of the FOREX captures: the company's
/// `CURRENCYNAME` naming the `$` master identifies a base that is not INR,
/// and one naming no master identifies none, although the book holds an INR
/// master either way. A Company collection that changes between the pair's
/// two sends, or an `ORIGINALNAME` re-read whose masters differ from the plain
/// read's, refuses as drift.
#[tokio::test]
async fn a_book_whose_company_names_no_inr_base_is_refused() {
    let company = forex_company_currency();
    let forex_row = company.find("BRIDGE CORPUS FOREX").unwrap();
    let rupee = "<CURRENCYNAME TYPE=\"String\">\u{20b9}</CURRENCYNAME>";
    assert!(company.find(rupee).unwrap() > forex_row);
    let first = company.find(rupee).unwrap();
    assert!(company.find("BRIDGE CORPUS FOREX").unwrap() < first);
    assert!(first < company.find("BRIDGE SHAPE LAB").unwrap());
    let naming = |value: &str| {
        company.replacen(
            rupee,
            &format!("<CURRENCYNAME TYPE=\"String\">{value}</CURRENCYNAME>"),
            1,
        )
    };
    for (value, code) in [
        ("$", "company_base_currency_not_inr"),
        ("\u{20ac}", "company_base_currency_undetermined"),
    ] {
        let read = forex_classified_read(classified_currency_plans(
            forex_originalname_currency(),
            naming(value),
            naming(value),
        ))
        .await
        .unwrap();
        assert_eq!(read.admit_inr_classified().err(), Some(code), "{value}");
    }

    let drift = |plans| async {
        forex_classified_read(plans)
            .await
            .unwrap_err()
            .chain()
            .find_map(|cause| {
                cause
                    .downcast_ref::<PairedReadValidationError>()
                    .map(PairedReadValidationError::safe_code)
            })
    };
    assert!(matches!(
        drift(classified_currency_plans(
            forex_originalname_currency(),
            company.clone(),
            naming("$"),
        ))
        .await,
        Some("company_currency_name_changed")
    ));
    let changed = forex_originalname_currency().replace(
        "<MAILINGNAME TYPE=\"String\">USD</MAILINGNAME>",
        "<MAILINGNAME TYPE=\"String\">US Dollar</MAILINGNAME>",
    );
    assert_ne!(changed, forex_originalname_currency());
    assert!(matches!(
        drift(classified_currency_plans(changed, company.clone(), company)).await,
        Some("currency_master_changed")
    ));
}

/// FOREX's native outstandings read in the order the read sends it: its
/// bills, groups and ledgers captured 2026-09-22/23, with the shared company
/// and extent fixtures from earlier sessions.
fn forex_native_plans() -> Vec<ScenarioPlan> {
    let extent = xml(extents());
    let mut plans = vec![status(), xml(companies()), xml(companies())];
    pair(&mut plans, extent.clone());
    for plan in [
        fixture(include_bytes!(
            "../../crates/bridge-tally-protocol/tests/fixtures/bills_receivable_forex_live.utf16le.xml"
        )),
        fixture(include_bytes!(
            "../../crates/bridge-tally-protocol/tests/fixtures/groups_forex_live.utf16le.xml"
        )),
        fixture(include_bytes!(
            "../../crates/bridge-tally-protocol/tests/fixtures/bills_payable_forex_live.utf16le.xml"
        )),
        fixture(include_bytes!(
            "../../crates/bridge-tally-protocol/tests/fixtures/ledgers_currency_forex_live.utf16le.xml"
        )),
    ] {
        pair(&mut plans, plan);
    }
    pair(&mut plans, extent);
    plans.extend([xml(companies()), status(), xml(companies())]);
    plans
}

/// bridge#551, end to end from captures: FOREX has an `I₹` and a `$` master.
/// The classified read identifies the rupee master as the base through the
/// company's `CURRENCYNAME`, and admits it as INR. The outstandings read
/// then leaves the three `$` ledgers and their four bills out of every
/// figure, and says so: a partial result whose figures describe the rupee
/// ledgers only (TALLY_PROTOCOL_REFERENCE §8.2d, §9.10a.2).
#[tokio::test]
async fn forex_outstandings_leave_the_dollar_ledgers_out_and_say_so() {
    let mut plans = forex_classified_currency_plans();
    plans.extend(forex_native_plans());
    let plan_count = plans.len();
    let simulator = SequenceSimulator::spawn(plans).unwrap();
    let config = TallyConfig {
        host: simulator.address().ip().to_string(),
        port: simulator.address().port(),
    };
    let identity = identity_for_guid(&companies(), FOREX_GUID);
    let runtime = TallyRuntime::default();
    let witness = runtime
        .detect_classified_base_currency_with_extent(config.clone(), &identity)
        .await
        .unwrap()
        .admit_inr_classified()
        .unwrap();
    let (result, _) = runtime
        .fetch_agent_outstandings_with_evidence(
            config,
            &identity,
            TallyDate::parse("20250930").unwrap(),
            witness,
            OutstandingsAgeingAnchor::DueDate,
        )
        .await
        .unwrap();
    let requests = simulator.finish().unwrap();
    let OutstandingsLoadResult::BaseCurrencyLedgersOnly {
        reason,
        foreign_currency_ledgers_excluded,
        base_currency_ledgers,
        ..
    } = result
    else {
        panic!("{result:?}");
    };
    assert_eq!(reason.reason_code, "foreign_currency_ledgers_excluded");
    let excluded = foreign_currency_ledgers_excluded
        .iter()
        .map(|ledger| (ledger.ledger.as_str(), ledger.currency.as_str()))
        .collect::<Vec<_>>();
    let dollars = ["BRIDGE FX DEBTOR A", "FX USD Debtor 01", "FX USD Debtor 02"];
    assert_eq!(excluded, dollars.map(|ledger| (ledger, "$")));
    // The 14 rupee bills only, 34,500; the four dollar bills (350,100 read as
    // rupees) are in no figure and no statement row.
    assert_eq!(
        base_currency_ledgers.report.receivable_total.as_str(),
        "34500"
    );
    assert_eq!(base_currency_ledgers.statement_open_bills.len(), 14);
    for row in &base_currency_ledgers.statement_open_bills {
        assert!(!dollars.contains(&row.party.as_str()), "{row:?}");
    }
    for party in &base_currency_ledgers.statement_unallocated_by_party {
        assert!(!dollars.contains(&party.party.as_str()), "{party:?}");
    }
    // Every scripted response was served, and each read sent its own request:
    // the plain currency read, the ORIGINALNAME re-read, the Company read.
    assert_eq!(requests.len(), plan_count);
    for (indexes, request) in [
        (
            [5, 7],
            render_company_currency_request("BRIDGE CORPUS FOREX"),
        ),
        (
            [9, 11],
            render_company_currency_request_with_originalname("BRIDGE CORPUS FOREX"),
        ),
        (
            [13, 15],
            render_company_base_currency_request("BRIDGE CORPUS FOREX"),
        ),
    ] {
        for index in indexes {
            assert_eq!(
                requests[index].request_body_sha256,
                request_sha256(&request)
            );
        }
    }
}

/// bridge#551: on a book with one master the classified read sends exactly
/// the plain currency read, no `ORIGINALNAME` re-read and no Company read,
/// and admits as `admit_inr` does: the captured `INR` master, and not the
/// same master with its mailing name changed.
#[tokio::test]
async fn a_one_master_classified_read_sends_only_the_plain_currency_read() {
    let captured = currency_source();
    let rupees = captured.replace(
        "<MAILINGNAME TYPE=\"String\">INR</MAILINGNAME>",
        "<MAILINGNAME TYPE=\"String\">Rupees</MAILINGNAME>",
    );
    assert_ne!(rupees, captured);
    for (currency, admitted) in [(captured, true), (rupees, false)] {
        let plans = currency_plans(currency);
        let plan_count = plans.len();
        let simulator = SequenceSimulator::spawn(plans).unwrap();
        let read = TallyRuntime::default()
            .detect_classified_base_currency_with_extent(
                TallyConfig {
                    host: simulator.address().ip().to_string(),
                    port: simulator.address().port(),
                },
                &identity_for_guid(&companies(), "eebb9a9f-1679-4468-9e8f-814c729674cb"),
            )
            .await
            .unwrap();
        let requests = simulator.finish().unwrap();
        assert_eq!(requests.len(), plan_count);
        let plain = request_sha256(&render_company_currency_request("Bridge Ageing Lab"));
        assert_eq!(requests[5].request_body_sha256, plain);
        assert_eq!(requests[7].request_body_sha256, plain);
        match read.admit_inr_classified() {
            Ok(_) => assert!(admitted),
            Err(code) => {
                assert!(!admitted, "{code}");
                assert_eq!(code, "company_base_currency_not_inr");
            }
        }
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

/// bridge#551: the sweep admits a book whose base is INR, the only master or
/// the one the company names, and refuses any other after its currency read,
/// before any bill: several masters whose company names the `$` master, or no
/// master (labelled edits of the FOREX captures), and one master named USD.
/// FOREX itself reads through as the base-currency-ledgers-only partial.
#[tokio::test]
async fn the_sweep_admits_only_an_inr_base_and_reads_forex_as_base_currency_ledgers_only() {
    let foreign = currency_source().replace(
        "<MAILINGNAME TYPE=\"String\">INR</MAILINGNAME>",
        "<MAILINGNAME TYPE=\"String\">USD</MAILINGNAME>",
    );
    assert_ne!(foreign, currency_source());
    let captured = currency_source();
    let start = captured.find("<CURRENCY ").unwrap();
    let end = start + captured[start..].find("</CURRENCY>").unwrap() + "</CURRENCY>".len();
    let mut no_master = captured.clone();
    no_master.replace_range(start..end, "");
    let mut forex = forex_classified_currency_plans();
    forex.extend(forex_native_plans());
    let forex_count = forex.len();
    for (plans, guid, expected, requests) in [
        (
            forex_plans_naming("$"),
            FOREX_GUID,
            Some("company_base_currency_not_inr"),
            22,
        ),
        (
            forex_plans_naming("\u{20ac}"),
            FOREX_GUID,
            Some("company_base_currency_undetermined"),
            22,
        ),
        (
            currency_then_native_plans(foreign),
            AGEING_GUID,
            Some("company_base_currency_not_inr"),
            14,
        ),
        (
            currency_then_native_plans(no_master.clone()),
            AGEING_GUID,
            Some("company_currency_probe_failed"),
            14,
        ),
        (forex, FOREX_GUID, None, forex_count),
    ] {
        let simulator = SequenceSimulator::spawn(plans).unwrap();
        let result = crate::commands::sweep_company_outstandings(
            &TallyRuntime::default(),
            &TallyConfig {
                host: simulator.address().ip().to_string(),
                port: simulator.address().port(),
            },
            &identity_for_guid(&companies(), guid),
            &TallyDate::parse(if expected.is_none() {
                "20250930"
            } else {
                "20260801"
            })
            .unwrap(),
            OutstandingsAgeingAnchor::DueDate,
        )
        .await;
        simulator.cancel();
        match (result, expected) {
            (Err(crate::commands::CompanySweepFailure::ReasonCode(code)), Some(expected)) => {
                assert_eq!(code, expected);
            }
            (
                Ok(OutstandingsLoadResult::BaseCurrencyLedgersOnly {
                    foreign_currency_ledgers_excluded,
                    ..
                }),
                None,
            ) => assert_eq!(foreign_currency_ledgers_excluded.len(), 3),
            (Ok(other), _) => panic!("{expected:?}: {other:?}"),
            (Err(crate::commands::CompanySweepFailure::ReasonCode(code)), _) => {
                panic!("{expected:?}: refused with {code}")
            }
            (Err(_), _) => panic!("{expected:?}: the read failed"),
        }
        assert_eq!(requests_sent(simulator), requests, "{expected:?}");
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

/// bridge#551: a rupee symbol alone does not admit. The captured
/// single-master read (`I₹`, mailing name `INR`) is admitted through
/// `admit_inr`; with its mailing name changed to `Rupees` it is refused as
/// not INR, whether or not an `ORIGINALNAME` `₹` is injected alongside
/// (TALLY_PROTOCOL_REFERENCE §9.10a.2: whether `ORIGINALNAME` survives a
/// base-currency rename is unmeasured).
#[tokio::test]
async fn a_single_rupee_master_is_not_admitted_by_its_symbol_alone() {
    let captured = currency_source();
    let rupees = captured.replace(
        "<MAILINGNAME TYPE=\"String\">INR</MAILINGNAME>",
        "<MAILINGNAME TYPE=\"String\">Rupees</MAILINGNAME>",
    );
    assert_ne!(rupees, captured);
    assert_eq!(rupees.matches("<DECIMALPLACES").count(), 1);
    let symbol = rupees.replace(
        "<DECIMALPLACES",
        "<ORIGINALNAME TYPE=\"String\">\u{20b9}</ORIGINALNAME>\r\n     <DECIMALPLACES",
    );
    assert_ne!(symbol, rupees);
    for (currency, admitted) in [(captured, true), (symbol, false), (rupees, false)] {
        let simulator = SequenceSimulator::spawn(currency_plans(currency)).unwrap();
        let read = TallyRuntime::default()
            .detect_base_currency_with_extent(
                TallyConfig {
                    host: simulator.address().ip().to_string(),
                    port: simulator.address().port(),
                },
                &identity_for_guid(&companies(), "eebb9a9f-1679-4468-9e8f-814c729674cb"),
            )
            .await
            .unwrap();
        simulator.cancel();
        match read.admit_inr() {
            Ok(_) => assert!(
                admitted,
                "a master without an Indian mailing name was admitted"
            ),
            Err(code) => {
                assert!(!admitted, "the captured INR master was refused: {code}");
                assert_eq!(code, "company_base_currency_not_inr");
            }
        }
    }
}

/// bridge#551: a two-master response carrying `ORIGINALNAME` (the captured
/// FOREX read) is still refused as undetermined through `admit_inr`. The
/// runtime takes no company `CURRENCYNAME`, so nothing identifies the rupee
/// master as the base, even though the response now says which master's
/// `ORIGINALNAME` is `₹`.
#[tokio::test]
async fn a_two_master_read_with_originalname_stays_undetermined() {
    let currency = decode(include_bytes!(
        "../../crates/bridge-tally-protocol/tests/fixtures/currency_originalname_forex_live.utf16le.xml"
    ));
    assert_eq!(currency.matches("<ORIGINALNAME").count(), 2);
    let simulator = SequenceSimulator::spawn(currency_plans(currency)).unwrap();
    let read = TallyRuntime::default()
        .detect_base_currency_with_extent(
            TallyConfig {
                host: simulator.address().ip().to_string(),
                port: simulator.address().port(),
            },
            &identity_for_guid(&companies(), "eebb9a9f-1679-4468-9e8f-814c729674cb"),
        )
        .await
        .unwrap();
    simulator.cancel();
    assert_eq!(read.currency_count(), 2);
    assert_eq!(
        read.admit_inr().err(),
        Some("company_base_currency_undetermined")
    );
}

const SHAPE_GUID: &str = "3a6bd6e1-b835-4bff-89dd-8a6af138c346";

/// A captured company list and book extents with only synthetic companies
/// loaded (2026-09-24), which carry SHAPE LAB.
fn synthetic_companies() -> String {
    decode(include_bytes!(
        "../../crates/bridge-tally-protocol/tests/fixtures/company_list_synthetic_live.utf16le.xml"
    ))
}

/// SHAPE LAB's classified currency read and native read, from captures: two
/// masters (`I₹`, `UUSD`), the company naming `₹`, and 44 ledgers all in
/// `I₹`.
fn shape_plans(leading_company_list: bool) -> Vec<ScenarioPlan> {
    let companies = xml(synthetic_companies());
    let extent = fixture(include_bytes!(
        "../../crates/bridge-tally-protocol/tests/fixtures/company_extents_synthetic_live.utf16le.xml"
    ));
    let mut plans = Vec::new();
    if leading_company_list {
        plans.push(companies.clone());
    }
    plans.push(companies.clone());
    pair(&mut plans, extent.clone());
    for plan in [
        fixture(include_bytes!(
            "../../crates/bridge-tally-protocol/tests/fixtures/currency_shape_live.utf16le.xml"
        )),
        fixture(include_bytes!(
            "../../crates/bridge-tally-protocol/tests/fixtures/currency_originalname_shape_live.utf16le.xml"
        )),
        fixture(include_bytes!(
            "../../crates/bridge-tally-protocol/tests/fixtures/company_currencyname_live.utf16le.xml"
        )),
    ] {
        pair(&mut plans, plan);
    }
    pair(&mut plans, extent.clone());
    plans.push(companies.clone());
    plans.extend([status(), companies.clone(), companies.clone()]);
    pair(&mut plans, extent.clone());
    for plan in [
        fixture(include_bytes!(
            "../../crates/bridge-tally-protocol/tests/fixtures/bills_receivable_shape_live.utf16le.xml"
        )),
        fixture(include_bytes!(
            "../../crates/bridge-tally-protocol/tests/fixtures/groups_shape_live.utf16le.xml"
        )),
        fixture(include_bytes!(
            "../../crates/bridge-tally-protocol/tests/fixtures/bills_payable_shape_live.utf16le.xml"
        )),
        fixture(include_bytes!(
            "../../crates/bridge-tally-protocol/tests/fixtures/ledgers_currency_shape_live.utf16le.xml"
        )),
    ] {
        pair(&mut plans, plan);
    }
    pair(&mut plans, extent);
    plans.extend([companies.clone(), status(), companies]);
    plans
}

/// bridge#551, from captures: SHAPE LAB defines a second master (`UUSD`) but
/// keeps every ledger in its INR base, so nothing is left out and the sweep
/// and the desktop read it as a complete report, with the desktop issuing
/// its working paper and statement source.
#[tokio::test]
async fn several_masters_with_every_ledger_in_the_base_read_as_complete() {
    let plans = shape_plans(false);
    let plan_count = plans.len();
    let simulator = SequenceSimulator::spawn(plans).unwrap();
    let result = crate::commands::sweep_company_outstandings(
        &TallyRuntime::default(),
        &TallyConfig {
            host: simulator.address().ip().to_string(),
            port: simulator.address().port(),
        },
        &identity_for_guid(&synthetic_companies(), SHAPE_GUID),
        &TallyDate::parse("20260923").unwrap(),
        OutstandingsAgeingAnchor::DueDate,
    )
    .await;
    simulator.cancel();
    assert!(
        matches!(result, Ok(OutstandingsLoadResult::Complete { .. })),
        "sweep: {:?}",
        result.map(|_| ()).map_err(|_| "refused")
    );
    assert_eq!(requests_sent(simulator), plan_count, "sweep");

    let plans = shape_plans(true);
    let plan_count = plans.len();
    let simulator = SequenceSimulator::spawn(plans).unwrap();
    let rows = parse_companies_from_collection(&synthetic_companies()).unwrap();
    let row = rows
        .iter()
        .find(|row| row.guid.as_deref() == Some(SHAPE_GUID))
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
        "as_of_yyyymmdd": "20260923",
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
    let OutstandingsLoadResult::Complete { report, .. } = &response.result else {
        panic!("desktop: {:?}", response.result);
    };
    assert_eq!(report.receivable_total.as_str(), "5000");
    assert!(response.working_paper_export_id.is_some());
    assert!(response.party_statement_source_id.is_some());
    assert_eq!(requests_sent(simulator), plan_count, "desktop");
}

/// bridge#551, the stale-assertion case: the screen reads a book with several
/// masters without asserting INR, so its request carries no assertion. If
/// the book later reads with one master Tally does not name INR (a user
/// deleted the unused INR master), nothing admits it: the one-master arm
/// binds only an assertion the screen actually sent. Refused before any bill.
#[tokio::test]
async fn a_desktop_read_without_an_assertion_admits_one_master_only_by_its_mailing_name() {
    let dollar = currency_source().replace(
        "<MAILINGNAME TYPE=\"String\">INR</MAILINGNAME>",
        "<MAILINGNAME TYPE=\"String\">USD</MAILINGNAME>",
    );
    assert_ne!(dollar, currency_source());
    for (currency, admitted) in [(dollar, false), (currency_source(), true)] {
        let mut plans = vec![xml(companies())];
        plans.extend(currency_then_native_plans(currency));
        let simulator = SequenceSimulator::spawn(plans).unwrap();
        let rows = parse_companies_from_collection(&companies()).unwrap();
        let row = rows
            .iter()
            .find(|row| row.guid.as_deref() == Some(AGEING_GUID))
            .unwrap();
        let request: crate::commands::OutstandingsRequest =
            serde_json::from_value(serde_json::json!({
                "config": {"host": simulator.address().ip().to_string(), "port": simulator.address().port()},
                "selected_company": {
                    "display_name": row.name,
                    "company_guid": row.guid,
                    "company_number": row.company_number,
                    "books_from_yyyymmdd": row.books_from,
                },
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
        simulator.cancel();
        if admitted {
            assert!(
                matches!(response.result, OutstandingsLoadResult::Complete { .. }),
                "INR by its mailing name: {:?}",
                response.result
            );
        } else {
            assert!(
                matches!(&response.result, OutstandingsLoadResult::Partial { reason, .. }
                    if *reason == "company_base_currency_not_inr".into()),
                "{:?}",
                response.result
            );
            // The company list, then the currency read's 14 requests, no more.
            assert_eq!(requests_sent(simulator), 15);
        }
    }
}
