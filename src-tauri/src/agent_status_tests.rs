//! Status-page faults against the captured gateway product observation.
use super::*;
use tally_protocol_simulator::{
    encode, Fixture, ProductStatus, ScenarioPlan, SequenceSimulator, WireEncoding,
};

#[tokio::test]
async fn tally_status_uses_observed_gateway_product_and_preserves_wire_evidence() {
    let raw = include_bytes!("../crates/bridge-tally-protocol/tests/fixtures/agent/native-licensed-release-companies.utf16le.xml");
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
        "release_missing",
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
        } else if fault == "release_missing" {
            captured.replace("<BRIDGERELEASE TYPE=\"String\">7.1</BRIDGERELEASE>", "")
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
            writes_enabled: false,
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
        assert_eq!(
            result["release"],
            if fault == "release_missing" {
                Value::Null
            } else {
                json!("7.1")
            },
            "{fault}"
        );
        assert_eq!(
            result["license_tier"],
            if matches!(fault, "education" | "unobserved") {
                Value::Null
            } else {
                json!("silver")
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

#[tokio::test]
async fn tally_status_failure_retains_completed_sources_in_response_and_history() {
    let bytes = include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-ageing-receivable.utf16le.xml"
    );
    let wrong_shape = String::from_utf16(
        &bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>(),
    )
    .unwrap();
    for fallback in [false, true] {
        let mut plans = vec![ScenarioPlan::new(Fixture::ProductStatus(
            ProductStatus::TallyPrime,
        ))];
        if fallback {
            // The untouched captured bills source is a wrong-shape discovery
            // response, so the subsequent legacy discovery request must fail.
            plans.push(
                ScenarioPlan::new(Fixture::SyntheticXml(wrong_shape.clone()))
                    .with_encoding(WireEncoding::Utf16Le),
            );
        }
        plans.push(
            ScenarioPlan::new(Fixture::SyntheticXml(wrong_shape.clone()))
                .with_encoding(WireEncoding::Utf16Le)
                .with_http_status(503),
        );
        let response_bodies = plans
            .iter()
            .map(ScenarioPlan::response_bytes)
            .collect::<Vec<_>>();
        let simulator = SequenceSimulator::spawn(plans).unwrap();
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
            writes_enabled: false,
        });
        let response = server.call_tool("tally_status", json!({})).await;
        assert_eq!(response["isError"], true);
        let structured = &response["structuredContent"];
        let text: Value =
            serde_json::from_str(response["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(&text, structured);
        assert_eq!(
            structured["result"]["error"]["code"],
            "status_probe_unavailable"
        );
        let evidence = &structured["evidence"];
        assert_eq!(evidence["state"], "partial");
        assert_eq!(evidence["reason_code"], "status_probe_unavailable");
        let requests = simulator.finish().unwrap();
        assert_eq!(requests.len(), if fallback { 3 } else { 2 });
        let mut expected_request = requests[0].request_body_sha256.clone();
        let mut expected_response = sha256_hex(&response_bodies[0]);
        let mut expected_bytes = response_bodies[0].len();
        if fallback {
            expected_request = sha256_hex(
                format!("{}:{}", expected_request, requests[1].request_body_sha256).as_bytes(),
            );
            expected_response = sha256_hex(
                format!("{}:{}", expected_response, sha256_hex(&response_bodies[1])).as_bytes(),
            );
            expected_bytes += response_bodies[1].len();
        }
        // The final 503 attempt is not a successfully completed source body.
        assert_eq!(evidence["request_sha256"], expected_request);
        assert_eq!(evidence["response_sha256"], expected_response);
        assert_eq!(evidence["bytes"], expected_bytes);
        let history = server.call_tool("read_evidence", json!({"limit":1})).await;
        assert_eq!(history["isError"], false);
        assert_eq!(
            history["structuredContent"]["result"]["records"][0],
            *evidence
        );
    }
}

#[test]
fn runtime_failure_conversion_distinguishes_absent_evidence_from_zero_byte_sources() {
    use crate::tally::runtime::{with_read_evidence, RuntimeReadEvidence};
    let absent = ToolFailure::from_runtime(
        "status_probe_unavailable",
        with_read_evidence(
            anyhow::anyhow!("test refusal"),
            RuntimeReadEvidence::empty(),
        ),
    );
    assert!(absent.evidence.is_none());
    let empty_body_hash = sha256_hex(b"");
    let observed = ToolFailure::from_runtime(
        "status_probe_unavailable",
        with_read_evidence(
            anyhow::anyhow!("test refusal"),
            RuntimeReadEvidence {
                request_sha256: empty_body_hash.clone(),
                response_sha256: empty_body_hash.clone(),
                bytes: 0,
            },
        ),
    );
    let evidence = observed
        .evidence
        .expect("a completed zero-byte source has hashes");
    assert_eq!(evidence.bytes, 0);
    assert_eq!(evidence.request_sha256, empty_body_hash);
    assert_eq!(evidence.response_sha256, empty_body_hash);
}

#[test]
fn party_master_period_refusals_use_the_operation_specific_period_code() {
    use crate::tally::connection::PartyLedgerMasterSourceValidationError;

    for refusal in [
        PartyLedgerMasterSourceValidationError::MasterPeriod,
        PartyLedgerMasterSourceValidationError::BalancePeriod,
    ] {
        let failure = ToolFailure::from_runtime(
            "party_ledger_master_read_failed",
            anyhow::Error::new(refusal),
        );
        assert_eq!(failure.code, "opening_period_not_honoured");
    }
}
