//! Status-page faults against the captured gateway product observation.
use super::*;
use tally_protocol_simulator::{
    encode, Fixture, ProductStatus, ScenarioPlan, SequenceSimulator, WireEncoding,
};

#[tokio::test]
async fn tally_status_uses_observed_gateway_product_and_preserves_wire_evidence() {
    let raw = include_bytes!("../crates/bridge-tally-protocol/tests/fixtures/agent/native-licensed-companies.utf16le.xml");
    let captured = String::from_utf16(
        &raw.chunks_exact(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect::<Vec<_>>(),
    )
    .unwrap();
    for fault in [
        "unrecognized",
        "conflicting",
        "unavailable",
        "unobserved",
        "education",
    ] {
        let status = ScenarioPlan::new(Fixture::ProductStatus(match fault {
            "conflicting" => ProductStatus::TallyErp9,
            "unrecognized" => ProductStatus::Unknown,
            _ => ProductStatus::TallyPrime,
        }))
        .with_http_status(if fault == "unavailable" { 404 } else { 200 });
        let gateway = if fault == "unobserved" {
            // Product text is still TallyPrime, but no license mode is observed.
            let altered = captured.replace(
                "<SILVER TYPE=\"Logical\">Yes</SILVER>",
                "<SILVER TYPE=\"Logical\">No</SILVER>",
            );
            assert_ne!(altered, captured);
            altered
        } else if fault == "education" {
            // A single metadata fault exercises the flag mapping, not live
            // qualification of an Education-mode Tally endpoint.
            let altered = captured.replace(
                "<EDUMODE TYPE=\"Logical\">No</EDUMODE>",
                "<EDUMODE TYPE=\"Logical\">Yes</EDUMODE>",
            );
            assert_ne!(altered, captured);
            altered
        } else {
            captured.clone()
        };
        let company =
            ScenarioPlan::new(Fixture::SyntheticXml(gateway)).with_encoding(WireEncoding::Utf16Le);
        let company_bytes = encode(&company.fixture.body(), company.encoding);
        let status_bytes = encode(&status.fixture.body(), status.encoding);
        let simulator = SequenceSimulator::spawn(vec![status, company]).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let server = Server::new(Settings {
            endpoint: TallyEndpointConfig {
                host: simulator.address().ip().to_string(),
                port: simulator.address().port(),
            },
            data_dir: directory.path().into(),
            max_rows: 500,
            max_bytes: 200_000,
            redaction: Redaction::None,
            import_enabled: false,
        });
        let response = server.call_tool("tally_status", json!({})).await;
        assert_eq!(response["isError"], false, "{fault}");
        let result = &response["structuredContent"]["result"];
        assert_eq!(
            result["product"],
            if fault == "unobserved" {
                "not_observed"
            } else {
                "TallyPrime"
            },
            "{fault}"
        );
        assert_eq!(
            result["education_mode"],
            match fault {
                "education" => json!(true),
                "unobserved" => Value::Null,
                _ => json!(false),
            },
            "{fault}"
        );
        let evidence = &response["structuredContent"]["evidence"];
        assert_eq!(evidence["state"], "complete");
        let observed = simulator.finish().unwrap();
        assert_eq!(observed.len(), 2);
        let joined = |left: &str, right: &str| sha256_hex(format!("{left}:{right}").as_bytes());
        if fault == "unavailable" {
            // The rejected status request supplies no retained source body;
            // gateway evidence remains exactly the successful POST observation.
            assert_eq!(evidence["request_sha256"], observed[1].request_body_sha256);
            assert_eq!(evidence["response_sha256"], sha256_hex(&company_bytes));
            assert_eq!(evidence["bytes"], company_bytes.len());
        } else {
            assert_eq!(
                evidence["request_sha256"],
                joined(
                    &observed[0].request_body_sha256,
                    &observed[1].request_body_sha256
                )
            );
            assert_eq!(
                evidence["response_sha256"],
                joined(&sha256_hex(&status_bytes), &sha256_hex(&company_bytes))
            );
            assert_eq!(evidence["bytes"], status_bytes.len() + company_bytes.len());
        }
    }
}
