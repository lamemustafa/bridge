//! Completed native source bodies survive a later paired-read refusal.
use super::*;
use bridge_tally_transport::TallyTransportError;
use tally_protocol_simulator::{
    Fixture, ProductStatus, ResponseFraming, ScenarioPlan, SequenceSimulator, WireEncoding,
};

fn captured(bytes: &[u8]) -> String {
    String::from_utf16(
        &bytes
            .chunks_exact(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect::<Vec<_>>(),
    )
    .unwrap()
}

#[tokio::test]
async fn agent_pair_failures_retain_each_completed_source_body_and_transport_cause() {
    let companies_xml = captured(include_bytes!("../../crates/bridge-tally-protocol/tests/fixtures/agent/native-licensed-companies.utf16le.xml"));
    let body = include_bytes!("../../crates/bridge-tally-protocol/tests/fixtures/agent/native-ledger-catalogue.utf16le.xml");
    let companies = parse_companies_from_collection(&companies_xml).unwrap();
    let company = companies
        .iter()
        .find(|company| company.name == "WR2 Unicode Lab")
        .unwrap();
    let identity = VerifiedCompanyIdentity::from_observed_companies(
        company.name.clone(),
        company.guid.clone().unwrap(),
        company.company_number.clone().unwrap(),
        company.books_from.clone().unwrap(),
        &companies,
    )
    .unwrap();
    let request = ReadOnlyProfile::StandardLedgerCatalogV1 {
        company: &bridge_tally_protocol::xml_read_profiles::ValidatedCompanyName::new(
            company.name.clone(),
        )
        .unwrap(),
    }
    .render();
    for framing in [
        ResponseFraming::ContentLength,
        ResponseFraming::Chunked { chunk_bytes: 37 },
    ] {
        for (fail_at, fault) in (0..6)
            .map(|index| (index, "http"))
            .chain([(5, "identity"), (4, "drift")])
        {
            let plan = |xml| {
                ScenarioPlan::new(Fixture::SyntheticXml(xml))
                    .with_encoding(WireEncoding::Utf16Le)
                    .with_framing(framing)
            };
            let company_plan = plan(companies_xml.clone());
            let report = plan(captured(body));
            let health = ScenarioPlan::new(Fixture::ProductStatus(ProductStatus::TallyPrime));
            let mut plans = vec![
                company_plan.clone(),
                report.clone(),
                health.clone(),
                report,
                health,
                company_plan,
            ];
            if fault == "identity" {
                let changed = companies_xml.replace(
                    identity.company_guid(),
                    "00000000-0000-4000-8000-000000000099",
                );
                assert_ne!(changed, companies_xml);
                plans[fail_at].fixture = Fixture::SyntheticXml(changed);
            } else if fault == "drift" {
                let first = captured(body);
                let second = first.replacen("Cash", "Changed Cash", 1);
                assert_ne!(first, second);
                plans[3].fixture = Fixture::SyntheticXml(second);
            } else {
                plans[fail_at].http_status = 503;
            }
            let expected_first = plans[1].response_bytes();
            let expected_second = plans[3].response_bytes();
            plans.truncate(fail_at + 1);
            let legacy_drift_plans = (fault == "drift").then(|| plans[1..].to_vec());
            let simulator = SequenceSimulator::spawn(plans).unwrap();
            let runtime = TallyRuntime::default();
            let result = runtime
                .fetch_agent_read(
                    TallyConfig {
                        host: "127.0.0.1".into(),
                        port: simulator.address().port(),
                    },
                    &identity,
                    super::super::agent_read_request::AgentReadRequest::parse(request.clone())
                        .unwrap(),
                )
                .await;
            let error = result.expect_err("injected read failure must refuse");
            if fault == "identity" {
                assert!(error.chain().any(|cause| matches!(
                    cause.downcast_ref::<CompanyIdentityBracketError>(),
                    Some(CompanyIdentityBracketError::AbsentOrAmbiguous)
                )));
            } else if fault == "drift" {
                assert!(error
                    .chain()
                    .any(|cause| cause.is::<crate::tally::connection::NativeReportPairDrift>()));
            } else {
                assert!(
                    error.chain().any(|cause| matches!(
                        cause.downcast_ref::<TallyTransportError>(),
                        Some(TallyTransportError::HttpStatus { status: 503 })
                    )),
                    "typed cause lost at {fail_at}: {error:?}"
                );
            }
            let observed = simulator.finish().unwrap();
            assert_eq!(observed.len(), fail_at + 1);
            if let Some(plans) = legacy_drift_plans {
                let simulator = SequenceSimulator::spawn(plans).unwrap();
                let client = TallyClient::new(TallyConfig {
                    host: "127.0.0.1".into(),
                    port: simulator.address().port(),
                })
                .unwrap();
                // Financial consumers retain their established Partial branch.
                assert!(matches!(
                    client
                        .fetch_native_report_paired(request.clone())
                        .await
                        .unwrap(),
                    NativePairedRead::Drifted
                ));
                assert_eq!(simulator.finish().unwrap().len(), 4);
            }
            let evidence = error
                .chain()
                .find_map(|cause| cause.downcast_ref::<RuntimeReadFailure>());
            if fail_at < 2 {
                assert!(evidence.is_none(), "no completed source at {fail_at}");
            } else {
                let evidence = &evidence
                    .expect("completed source evidence must survive")
                    .evidence;
                if fault == "drift" {
                    let join = |a: &str, b: &str| sha256_hex(format!("{a}:{b}").as_bytes());
                    assert_eq!(
                        evidence.request_sha256,
                        join(
                            &observed[1].request_body_sha256,
                            &observed[3].request_body_sha256
                        )
                    );
                    assert_eq!(
                        evidence.response_sha256,
                        join(&sha256_hex(&expected_first), &sha256_hex(&expected_second))
                    );
                    assert_eq!(evidence.bytes, expected_first.len() + expected_second.len());
                } else {
                    assert_eq!(evidence.request_sha256, observed[1].request_body_sha256);
                    assert_eq!(evidence.response_sha256, sha256_hex(&expected_first));
                    assert_eq!(
                        evidence.bytes,
                        expected_first.len() * if fail_at < 4 { 1 } else { 2 }
                    );
                }
            }
        }
    }
}
