//! Captured read replay with mode/date fault injection; no live Education calls.
use super::*;
use tally_protocol_simulator::{
    encode, Fixture, ProductStatus, ResponseFraming, ScenarioPlan, SequenceSimulator, WireEncoding,
};

const GUID: &str = "61c6de69-1748-461c-ad3f-162cb949df9f";
fn decode(bytes: &[u8]) -> String {
    String::from_utf16(
        &bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>(),
    )
    .unwrap()
}
fn companies() -> String {
    decode(include_bytes!(
    "../../crates/bridge-tally-protocol/tests/fixtures/agent/native-licensed-release-companies.utf16le.xml"))
}
fn extents() -> String {
    include_str!(
        "../../crates/bridge-tally-protocol/tests/fixtures/agent/native-company-book-extents-with-number.utf8.xml"
    ).to_owned()
}
fn company_identity(xml: &str) -> VerifiedCompanyIdentity {
    identity_for_guid(xml, GUID)
}
fn identity_for_guid(xml: &str, guid: &str) -> VerifiedCompanyIdentity {
    let companies = parse_companies_from_collection(xml).unwrap();
    let row = companies
        .iter()
        .find(|row| row.guid.as_deref() == Some(guid))
        .unwrap();
    VerifiedCompanyIdentity::from_observed_companies(
        row.name.clone(),
        guid.into(),
        row.company_number.clone().unwrap(),
        row.books_from.clone().unwrap(),
        &companies,
    )
    .unwrap()
}
fn xml(text: String) -> ScenarioPlan {
    ScenarioPlan::new(Fixture::SyntheticXml(text))
        .with_encoding(WireEncoding::Utf16Le)
        .with_framing(ResponseFraming::ContentLength)
}
fn status() -> ScenarioPlan {
    ScenarioPlan::new(Fixture::ProductStatus(ProductStatus::TallyPrime))
}
fn education(text: &str) -> String {
    text.replace(
        "<EDUMODE TYPE=\"Logical\">No</EDUMODE>",
        "<EDUMODE TYPE=\"Logical\">Yes</EDUMODE>",
    )
}
fn assertion(
    extent: &str,
    identity: &VerifiedCompanyIdentity,
) -> PartyLedgerMasterCurrencyAssertion {
    PartyLedgerMasterCurrencyAssertion {
        assertion: OutstandingsCurrencyAssertion::Inr,
        decimal_places: 2,
        currency_read_extent:
            bridge_tally_protocol::outstandings_shared::parse_company_book_extent_v2(
                extent,
                &identity.company_book_extent_expectation().unwrap(),
            )
            .unwrap(),
    }
}
fn stale_cache(runtime: &TallyRuntime, config: &TallyConfig) {
    let companies = parse_companies_from_collection(&companies()).unwrap();
    *runtime
        .session(config.clone())
        .unwrap()
        .cached_probe
        .write()
        .unwrap() = Some(CachedProbe {
        review_id: "stale-licensed".into(),
        observed_at_unix_ms: 1,
        freshness_origin_unix_ms: 1,
        reserved: false,
        result: TallyProbeResult {
            connection: ConnectionStatus {
                reachable: true,
                compatible: true,
                server_text: String::new(),
                product: super::super::TallyProduct::Unknown,
                error: None,
            },
            companies,
            profile: bridge_tally_core::CapabilityProfile {
                profile_version: 3,
                product: "TallyPrime".into(),
                release: None,
                license_tier: None,
                mode: Some("Licensed".into()),
                transports: Default::default(),
                features: Default::default(),
                packs: Default::default(),
            },
            selected_read_scope: None,
            passport_snapshot_id: None,
        },
    });
}
fn join(left: &str, right: &str) -> String {
    sha256_hex(format!("{left}:{right}").as_bytes())
}

// These mutations alter profile metadata in captured bodies. They exercise
// admission behavior only; they are not live evidence for another product,
// release, or licence tier.
const REFUSAL_PROFILE_FAULTS: [&str; 2] = ["mode_unknown", "product_unknown"];
const ADMITTED_PROFILE_VARIANTS: [&str; 5] = [
    "none",
    "release_old",
    "release_missing",
    "release_unknown",
    "gold",
];
fn fault_company(fault: &str) -> String {
    let source = companies();
    let release = "<BRIDGERELEASE TYPE=\"String\">7.1</BRIDGERELEASE>";
    let silver = "<SILVER TYPE=\"Logical\">Yes</SILVER>";
    let gold = "<GOLD TYPE=\"Logical\">No</GOLD>";
    let product = "<PRODUCTNAME TYPE=\"String\">TallyPrime</PRODUCTNAME>";
    let changed = match fault {
        "education" => education(&source),
        "erp9" => source.replace(
            product,
            "<PRODUCTNAME TYPE=\"String\">Tally ERP 9</PRODUCTNAME>",
        ),
        "editlog" => source.replace(
            product,
            "<PRODUCTNAME TYPE=\"String\">TallyPrime Edit Log</PRODUCTNAME>",
        ),
        "product_unknown" => source.replace(
            product,
            "<PRODUCTNAME TYPE=\"String\">UnknownProduct</PRODUCTNAME>",
        ),
        "release_old" => source.replace(
            release,
            "<BRIDGERELEASE TYPE=\"String\">7.0</BRIDGERELEASE>",
        ),
        "release_missing" => source.replace(release, ""),
        "release_unknown" => source.replace(
            release,
            "<BRIDGERELEASE TYPE=\"String\">unknown</BRIDGERELEASE>",
        ),
        "gold" => source
            .replace(silver, "<SILVER TYPE=\"Logical\">No</SILVER>")
            .replace(gold, "<GOLD TYPE=\"Logical\">Yes</GOLD>"),
        "tier_ambiguous" => source.replace(gold, "<GOLD TYPE=\"Logical\">Yes</GOLD>"),
        "mode_unknown" => source.replace(silver, "<SILVER TYPE=\"Logical\">No</SILVER>"),
        "none" => return source,
        _ => panic!("unknown profile fault: {fault}"),
    };
    assert_ne!(changed, source, "{fault}");
    changed
}
fn assert_profile_refusal(error: &anyhow::Error, fault: &str) {
    assert_eq!(
        error
            .chain()
            .find_map(|cause| cause.downcast_ref::<OpeningBoundaryObservationError>()),
        Some(&OpeningBoundaryObservationError::Unobserved),
        "{fault}: {error:?}"
    );
}

#[tokio::test]
async fn financial_reads_refuse_unqualified_profiles_before_reports_despite_stale_cache() {
    for party in [false, true] {
        for cached in [false, true] {
            for fault in REFUSAL_PROFILE_FAULTS {
                let plans = vec![status(), xml(fault_company(fault))];
                let response_bytes = plans
                    .iter()
                    .map(ScenarioPlan::response_bytes)
                    .collect::<Vec<_>>();
                let simulator = SequenceSimulator::spawn(plans).unwrap();
                let config = TallyConfig {
                    host: simulator.address().ip().to_string(),
                    port: simulator.address().port(),
                };
                let identity = company_identity(&companies());
                let runtime = TallyRuntime::default();
                if cached {
                    stale_cache(&runtime, &config);
                }
                let error = if party {
                    runtime
                        .fetch_party_ledger_master_source_with_evidence(
                            config,
                            &identity,
                            assertion(&extents(), &identity),
                        )
                        .await
                        .unwrap_err()
                } else {
                    runtime
                        .fetch_outstandings_native(
                            config,
                            &identity,
                            TallyDate::parse("20260902").unwrap(),
                            OutstandingsCurrencyAssertion::Inr,
                            OutstandingsAgeingAnchor::DueDate,
                        )
                        .await
                        .unwrap_err()
                };
                assert_profile_refusal(&error, fault);
                let evidence = &error.downcast_ref::<RuntimeReadFailure>().unwrap().evidence;
                let observed = simulator.finish().unwrap();
                assert_eq!(
                    observed.len(),
                    2,
                    "{fault}: refusal must precede financial reports"
                );
                assert_eq!(
                    evidence.request_sha256,
                    join(
                        &observed[0].request_body_sha256,
                        &observed[1].request_body_sha256
                    )
                );
                assert_eq!(
                    evidence.response_sha256,
                    join(
                        &sha256_hex(&response_bytes[0]),
                        &sha256_hex(&response_bytes[1])
                    )
                );
                assert_eq!(
                    evidence.bytes,
                    response_bytes[0].len() + response_bytes[1].len()
                );
            }
        }
    }
}

fn pair(plans: &mut Vec<ScenarioPlan>, response: ScenarioPlan) {
    plans.extend([response.clone(), status(), response, status()]);
}
fn compliance_plans(profile_fault: &str) -> Vec<ScenarioPlan> {
    let company = xml(fault_company(profile_fault));
    let extent = xml(extents());
    let mut plans = vec![status(), company.clone(), company.clone()];
    pair(&mut plans, extent.clone());
    for bytes in [
        include_bytes!("../../crates/bridge-tally-protocol/tests/fixtures/agent/native-party-masters.utf16le.xml").as_slice(),
        include_bytes!("../../crates/bridge-tally-protocol/tests/fixtures/agent/native-party-balances.utf16le.xml").as_slice(),
        include_bytes!("../../crates/bridge-tally-protocol/tests/fixtures/agent/native-party-groups.utf16le.xml").as_slice(),
    ] { pair(&mut plans,xml(decode(bytes))); }
    pair(&mut plans, extent);
    plans.extend([company.clone(), status(), company]);
    plans
}

#[tokio::test]
async fn compliance_source_requires_closing_mode_and_preserves_source_commitments() {
    for fault in ADMITTED_PROFILE_VARIANTS {
        let plans = compliance_plans(fault);
        let responses = plans
            .iter()
            .map(|plan| encode(&plan.fixture.body(), plan.encoding))
            .collect::<Vec<_>>();
        let simulator = SequenceSimulator::spawn(plans).unwrap();
        let identity = company_identity(&companies());
        let result = TallyRuntime::default()
            .fetch_party_ledger_master_source_with_evidence(
                TallyConfig {
                    host: simulator.address().ip().to_string(),
                    port: simulator.address().port(),
                },
                &identity,
                assertion(&extents(), &identity),
            )
            .await;
        let (source, evidence) = result.unwrap();
        assert_eq!(source.rows.len(), 9);
        let observed = simulator
            .finish()
            .unwrap_or_else(|error| panic!("{fault}: {error}"));
        assert_eq!(observed.len(), 26);
        let req = |i: usize| observed[i].request_body_sha256.clone();
        let res = |i: usize| sha256_hex(&responses[i]);
        let triple =
            |a: String, b: String, c: String| sha256_hex(format!("{a}:{b}:{c}").as_bytes());
        assert_eq!(
            evidence.request_sha256,
            join(
                &join(&join(&req(0), &req(1)), &triple(req(7), req(11), req(15))),
                &join(&req(24), &req(25))
            )
        );
        assert_eq!(
            evidence.response_sha256,
            join(
                &join(&join(&res(0), &res(1)), &triple(res(7), res(11), res(15))),
                &join(&res(24), &res(25))
            )
        );
        assert_eq!(
            evidence.bytes,
            [0, 1, 7, 9, 11, 13, 15, 17, 24, 25]
                .iter()
                .map(|i| responses[*i].len())
                .sum::<usize>()
        );
    }
}

#[tokio::test]
async fn outstandings_requires_closing_mode_and_retains_sources_on_refusal() {
    for fault in std::iter::once("amount").chain(ADMITTED_PROFILE_VARIANTS) {
        let profile_fault = if fault == "amount" { "none" } else { fault };
        let company = xml(fault_company(profile_fault));
        let extent = xml(extents());
        let mut plans = vec![status(), company.clone(), company.clone()];
        pair(&mut plans, extent.clone());
        for (index, bytes) in [
            include_bytes!("../../crates/bridge-tally-protocol/tests/fixtures/agent/native-ageing-receivable.utf16le.xml").as_slice(),
            include_bytes!("../../crates/bridge-tally-protocol/tests/fixtures/agent/native-ageing-groups.utf16le.xml").as_slice(),
            include_bytes!("../../crates/bridge-tally-protocol/tests/fixtures/agent/native-ageing-payable.utf16le.xml").as_slice(),
            include_bytes!("../../crates/bridge-tally-protocol/tests/fixtures/agent/native-ageing-ledgers.utf16le.xml").as_slice(),
        ].into_iter().enumerate() {
            let mut source = decode(bytes);
            if fault == "amount" && index == 0 {
                let start = source.find("<BILLCL>").unwrap() + "<BILLCL>".len();
                let end = start + source[start..].find("</BILLCL>").unwrap();
                source.replace_range(start..end, "not-a-number");
            }
            pair(&mut plans, xml(source));
        }
        pair(&mut plans, extent);
        plans.extend([company, status(), xml(fault_company(profile_fault))]);
        let responses = plans
            .iter()
            .map(|plan| encode(&plan.fixture.body(), plan.encoding))
            .collect::<Vec<_>>();
        let simulator = SequenceSimulator::spawn(plans).unwrap();
        let identity = identity_for_guid(&companies(), "eebb9a9f-1679-4468-9e8f-814c729674cb");
        let result = TallyRuntime::default()
            .fetch_outstandings_native(
                TallyConfig {
                    host: simulator.address().ip().to_string(),
                    port: simulator.address().port(),
                },
                &identity,
                TallyDate::parse("20260801").unwrap(),
                OutstandingsCurrencyAssertion::Inr,
                OutstandingsAgeingAnchor::DueDate,
            )
            .await;
        let evidence = if ADMITTED_PROFILE_VARIANTS.contains(&fault) {
            let (result, evidence) = result.unwrap();
            assert!(matches!(result, OutstandingsLoadResult::Complete { .. }));
            evidence
        } else {
            let error = result.unwrap_err();
            if fault != "amount" {
                assert_profile_refusal(&error, fault);
            } else {
                assert!(matches!(
                    error
                        .chain()
                        .find_map(|cause| cause.downcast_ref::<NativeOutstandingsError>()),
                    Some(NativeOutstandingsError::InvalidAmount)
                ));
            }
            error
                .downcast_ref::<RuntimeReadFailure>()
                .unwrap()
                .evidence
                .clone()
        };
        let observed = simulator.finish().unwrap();
        assert_eq!(observed.len(), 30);
        let req = |i: usize| observed[i].request_body_sha256.clone();
        let res = |i: usize| sha256_hex(&responses[i]);
        let request = [7, 11, 15, 19]
            .iter()
            .fold(join(&req(0), &req(1)), |prior, i| join(&prior, &req(*i)));
        let response = [7, 11, 15, 19]
            .iter()
            .fold(join(&res(0), &res(1)), |prior, i| join(&prior, &res(*i)));
        assert_eq!(
            evidence.request_sha256,
            join(&request, &join(&req(28), &req(29)))
        );
        assert_eq!(
            evidence.response_sha256,
            join(&response, &join(&res(28), &res(29)))
        );
        assert_eq!(
            evidence.bytes,
            [0, 1, 7, 9, 11, 13, 15, 17, 19, 21, 28, 29]
                .iter()
                .map(|i| responses[*i].len())
                .sum::<usize>()
        );
    }
}

#[test]
fn retained_evidence_does_not_change_runtime_error_classification() {
    let error = with_read_evidence(
        TallyRuntimeReadError::ApplicationResponseRejected.into(),
        RuntimeReadEvidence::empty(),
    );
    assert_eq!(classify_failure(&error), ReadFailureClass::Application);
    let error = with_read_evidence(
        TallyTransportError::ConnectionFailed.into(),
        RuntimeReadEvidence::empty(),
    );
    assert_eq!(classify_failure(&error), ReadFailureClass::Connection);
}

#[tokio::test]
async fn rejected_currency_retains_its_completed_pair_before_any_master_read() {
    let captured = decode(include_bytes!(
        "../../crates/bridge-tally-protocol/tests/fixtures/currency_inr_modern_live.utf16le.xml"
    ));
    for malformed in [false, true] {
        let faulty = if malformed {
            captured.replace(
                "<DECIMALPLACES TYPE=\"Number\"> 2</DECIMALPLACES>",
                "<DECIMALPLACES TYPE=\"Number\">invalid</DECIMALPLACES>",
            )
        } else {
            captured.replace(
                "<MAILINGNAME TYPE=\"String\">INR</MAILINGNAME>",
                "<MAILINGNAME TYPE=\"String\">USD</MAILINGNAME>",
            )
        };
        assert_ne!(faulty, captured);
        let encoded = encode(&faulty, WireEncoding::Utf16Le);
        let company = xml(companies());
        let extent = xml(extents());
        let mut plans = vec![company.clone()];
        pair(&mut plans, extent.clone());
        pair(&mut plans, xml(faulty));
        pair(&mut plans, extent);
        plans.push(company);
        let simulator = SequenceSimulator::spawn(plans).unwrap();
        let error = TallyRuntime::default()
            .fetch_agent_party_ledger_masters_with_evidence(
                TallyConfig {
                    host: simulator.address().ip().to_string(),
                    port: simulator.address().port(),
                },
                &company_identity(&companies()),
            )
            .await
            .unwrap_err();
        if malformed {
            assert!(matches!(
                error
                    .chain()
                    .find_map(|cause| cause.downcast_ref::<NativeOutstandingsError>()),
                Some(NativeOutstandingsError::InvalidResponse(
                    "currency_decimal_places_invalid"
                ))
            ));
        } else {
            assert_eq!(error.to_string(), "company_base_currency_not_inr");
        }
        let evidence = &error.downcast_ref::<RuntimeReadFailure>().unwrap().evidence;
        let observations = simulator.finish().unwrap();
        assert_eq!(observations.len(), 14);
        assert_eq!(evidence.request_sha256, observations[5].request_body_sha256);
        assert_eq!(evidence.response_sha256, sha256_hex(&encoded));
        assert_eq!(evidence.bytes, 2 * encoded.len());
    }
}

#[path = "runtime_outstandings_currency_tests.rs"]
mod outstandings_currency_tests;

#[path = "runtime_failure_evidence_inventory_tests.rs"]
mod failure_evidence_inventory_tests;
