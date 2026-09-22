use super::*;
use tally_protocol_simulator::{Fixture, ScenarioPlan, Simulator, WireEncoding};

#[tokio::test]
async fn sends_only_the_rendered_sealed_read_profile() {
    let profile = ReadOnlyProfile::CompanyListV1;
    let expected = profile.render();
    for attempt in 1..=5 {
        let simulator = Simulator::spawn(
            ScenarioPlan::new(Fixture::EmptyExport).with_encoding(WireEncoding::Utf16Le),
        )
        .unwrap();
        let transport =
            ReadOnlyTransport::new(ReadLoopback::Ipv4, simulator.address().port()).unwrap();
        match transport.send(profile).await {
            Ok(response) => {
                assert_eq!(response.http_status(), 200);
                let observed = simulator.finish().unwrap();
                assert_eq!(observed.method, "POST");
                assert_eq!(observed.path, "/");
                assert!(observed.request_processed);
                assert!(observed.bytes_received > expected.len());
                return;
            }
            Err(error) if attempt < 5 && error.safe_code() == "request_failed" => {
                // Windows endpoint-security software can abort a newly opened synthetic
                // loopback flow before the request reaches the listener. Recreate only the
                // test fixture; the production read transport remains single-attempt.
                drop(simulator);
            }
            Err(error) => panic!("sealed read fixture failed: {error:?}"),
        }
    }
    unreachable!("bounded fixture attempts always return")
}

#[tokio::test]
async fn an_education_transport_refuses_the_report_formula_profiles_before_sending() {
    use bridge_tally_protocol::xml_read_profiles::{
        ValidatedCanaryLedgerName, ValidatedCompanyName, ValidatedDateRange,
        ValidatedIdentityQuerySha256,
    };
    use tally_protocol_simulator::SequenceSimulator;
    let company = ValidatedCompanyName::new("Synthetic Co").unwrap();
    let range = ValidatedDateRange::new("20260401", "20260430").unwrap();
    let canary = ValidatedCanaryLedgerName::new("BRIDGE-CANARY-0").unwrap();
    let identity = ValidatedIdentityQuerySha256::new("a".repeat(64)).unwrap();
    let refused = [
        ReadOnlyProfile::LedgersV1 { company: &company },
        ReadOnlyProfile::LedgerCanaryReadbackV1 {
            company: &company,
            ledger_name: &canary,
            identity_query_sha256: &identity,
        },
        ReadOnlyProfile::VouchersV2 {
            company: &company,
            range: &range,
        },
        ReadOnlyProfile::VouchersV3 {
            company: &company,
            range: &range,
        },
    ];
    let simulator = SequenceSimulator::spawn(vec![
        ScenarioPlan::new(Fixture::EmptyExport).with_encoding(WireEncoding::Utf16Le),
        ScenarioPlan::new(Fixture::EmptyExport).with_encoding(WireEncoding::Utf16Le),
    ])
    .unwrap();
    let port = simulator.address().port();
    let education = ReadOnlyTransport::new(ReadLoopback::Ipv4, port)
        .unwrap()
        .education_restricted();
    for profile in refused {
        let error = education.send(profile).await.unwrap_err();
        assert_eq!(error.safe_code(), "education_report_family_unsupported");
        assert_eq!(error.http_status(), None);
        assert_eq!(simulator.received(), 0, "{}", profile.id().as_str());
    }
    // A profile Education can parse still goes out on the restricted transport,
    // and the unrestricted transport still sends a refused profile.
    let allowed = education.send(ReadOnlyProfile::CompanyListV2).await;
    let licensed = ReadOnlyTransport::new(ReadLoopback::Ipv4, port)
        .unwrap()
        .send(ReadOnlyProfile::LedgersV1 { company: &company })
        .await;
    let observed = simulator.finish().unwrap();
    assert!(
        allowed.is_ok() && licensed.is_ok(),
        "{allowed:?} {licensed:?}"
    );
    assert_eq!(
        observed
            .iter()
            .map(|request| request.request_body_sha256.clone())
            .collect::<Vec<_>>(),
        [
            ReadOnlyProfile::CompanyListV2.render(),
            ReadOnlyProfile::LedgersV1 { company: &company }.render()
        ]
        .map(|xml| sha256_of_wire(&xml))
    );
}

fn sha256_of_wire(xml: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bridge_tally_protocol::encode_tally_xml_request_utf16le(xml))
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
