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
            // Only EDUMODE flips, leaving SILVER=Yes and GOLD=No: the
            // combination a live Education instance reported on 22 Sep 2026
            // (bridge#581). It exercises the flag mapping, not a qualification
            // of an Education endpoint.
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

// -- #629: a refusal that parsed no Tally answer names the endpoint ----------
//
// A wrong or unreachable endpoint used to read as a Tally data problem: the
// operation code (`status_probe_unavailable`, `company_collection_invalid`)
// was the whole refusal. These drive the tool call against a closed port, a
// responder that is not Tally, and a Tally-shaped answer that fails parsing,
// and assert the typed cause and whether the configured endpoint is named.

fn server_at(port: u16, max_bytes: usize) -> (Server, tempfile::TempDir) {
    let directory = tempfile::tempdir().unwrap();
    let server = Server::new(Settings {
        endpoint: TallyEndpointConfig {
            host: "127.0.0.1".into(),
            port,
        },
        data_dir: directory.path().into(),
        max_rows: 500,
        max_bytes,
        redaction: Redaction::None,
        import_enabled: false,
        writes_enabled: false,
    });
    (server, directory)
}

fn refusal(response: &Value) -> &Value {
    assert_eq!(response["isError"], true, "{response}");
    &response["structuredContent"]["result"]["error"]
}

/// A port nothing listens on, as after a reinstall reset the configured port
/// to the default. The endpoint appears in the refusal only: the evidence is
/// the ordinary no-source placeholder, hashed from the code alone.
#[tokio::test]
async fn a_closed_port_is_refused_as_unreachable_and_names_the_endpoint() {
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    for (tool, code) in [
        ("tally_status", "status_probe_unavailable"),
        ("list_companies", "company_collection_invalid"),
    ] {
        let (server, _directory) = server_at(port, 200_000);
        let response = server.call_tool(tool, json!({})).await;
        let error = refusal(&response);
        assert_eq!(error["code"], code, "{tool}");
        assert_eq!(error["cause"], "endpoint_unreachable", "{tool}");
        assert_eq!(
            error["endpoint"],
            format!("http://127.0.0.1:{port}"),
            "{tool}"
        );
        let evidence = &response["structuredContent"]["evidence"];
        assert_eq!(
            evidence["response_sha256"],
            sha256_hex(code.as_bytes()),
            "{tool}"
        );
        assert_eq!(evidence["bytes"], 0, "{tool}");
    }
}

/// Something answers on the port, but not as a Tally gateway. First an HTML
/// page, as a web server on Tally's default port would send (a raw responder,
/// because the gateway double only speaks XML); then an HTTP error status.
#[tokio::test]
async fn a_responder_that_is_not_tally_is_named_with_the_endpoint() {
    use tokio::io::AsyncWriteExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    // The status GET and the company POST, each on its own connection, each
    // read to its terminator and declared length before it is answered.
    let serve = tokio::spawn(async move {
        let mut methods = Vec::new();
        for _ in 0..2 {
            let (mut socket, _) = listener.accept().await.unwrap();
            let request =
                crate::tally::connection::tests::read_complete_http_request(&mut socket).await;
            methods.push(
                String::from_utf8_lossy(&request)
                    .split_ascii_whitespace()
                    .next()
                    .unwrap()
                    .to_string(),
            );
            let body = b"<!doctype html><html><body>Not Tally</body></html>";
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            socket.write_all(head.as_bytes()).await.unwrap();
            socket.write_all(body).await.unwrap();
            socket.shutdown().await.unwrap();
        }
        methods
    });
    let (server, _directory) = server_at(port, 200_000);
    let response = server.call_tool("tally_status", json!({})).await;
    let error = refusal(&response);
    assert_eq!(error["code"], "status_probe_unavailable");
    assert_eq!(error["cause"], "response_content_type_unsupported");
    assert_eq!(error["endpoint"], format!("http://127.0.0.1:{port}"));
    assert_eq!(serve.await.unwrap(), ["GET", "POST"]);

    let plans = vec![
        ScenarioPlan::new(Fixture::ProductStatus(ProductStatus::TallyPrime)),
        ScenarioPlan::new(Fixture::SyntheticXml("<ENVELOPE/>".into())).with_http_status(404),
    ];
    let simulator = SequenceSimulator::spawn(plans).unwrap();
    let port = simulator.address().port();
    let (server, _directory) = server_at(port, 200_000);
    let response = server.call_tool("tally_status", json!({})).await;
    let error = refusal(&response);
    assert_eq!(error["code"], "status_probe_unavailable");
    assert_eq!(error["cause"], "http_status_failure");
    assert_eq!(error["endpoint"], format!("http://127.0.0.1:{port}"));
    assert_eq!(simulator.finish().unwrap().len(), 2);
}

/// Control: a real gateway answered and its body failed parsing. That is the
/// protocol failure the operation code already names, so the refusal carries
/// no endpoint cause and no endpoint. The captured company list is served
/// with only its root element renamed.
#[tokio::test]
async fn a_tally_answer_that_fails_parsing_names_no_endpoint() {
    let raw = include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-licensed-release-companies.utf16le.xml"
    );
    let captured = String::from_utf16(
        &raw.chunks_exact(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect::<Vec<_>>(),
    )
    .unwrap();
    assert_eq!(captured.matches("ENVELOPE>").count(), 2);
    let renamed = captured.replace("ENVELOPE>", "NOTENVELOPE>");
    let body =
        ScenarioPlan::new(Fixture::SyntheticXml(renamed)).with_encoding(WireEncoding::Utf16Le);
    let status = ScenarioPlan::new(Fixture::ProductStatus(ProductStatus::TallyPrime));
    let simulator =
        SequenceSimulator::spawn(vec![body.clone(), status.clone(), body, status]).unwrap();
    let (server, _directory) = server_at(simulator.address().port(), 200_000);
    let response = server.call_tool("list_companies", json!({})).await;
    let error = refusal(&response);
    assert_eq!(error["code"], "company_collection_invalid");
    assert!(error.get("cause").is_none(), "{error}");
    assert!(error.get("endpoint").is_none(), "{error}");
    assert_eq!(simulator.finish().unwrap().len(), 4);
}

/// The endpoint follows the cause's byte-budget rule: at a deliberately small
/// response cap the refusal keeps its code and gives up both.
#[tokio::test]
async fn a_small_response_budget_keeps_the_code_and_drops_the_endpoint() {
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    // Just under the guidance threshold, and still roomy enough for the refusal
    // envelope, so this exercises the budget rule, not the too-large path.
    let (server, _directory) = server_at(port, REMEDIATION_MIN_RESPONSE_BUDGET - 1);
    let response = server.call_tool("tally_status", json!({})).await;
    let error = refusal(&response);
    assert_eq!(error["code"], "status_probe_unavailable");
    assert!(error.get("cause").is_none(), "{error}");
    assert!(error.get("endpoint").is_none(), "{error}");
}

/// After repeated connection failures the runtime holds requests back and
/// sends nothing. That refusal also names the endpoint, so "no cause" is never
/// what a caller sees when Bridge did not reach Tally at all.
#[tokio::test]
async fn a_request_held_back_after_repeated_failures_names_the_endpoint() {
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let (server, _directory) = server_at(port, 200_000);
    let mut causes = Vec::new();
    for _ in 0..3 {
        let response = server.call_tool("list_companies", json!({})).await;
        let error = refusal(&response);
        assert_eq!(error["code"], "company_collection_invalid");
        assert_eq!(error["endpoint"], format!("http://127.0.0.1:{port}"));
        causes.push(error["cause"].as_str().unwrap().to_string());
    }
    // The first call's retries open the circuit; the next two are held back.
    assert_eq!(
        causes,
        [
            "endpoint_unreachable",
            "endpoint_circuit_cooldown",
            "endpoint_circuit_cooldown"
        ]
    );
}

/// When the operation code already is the unanswered reason (a generic read
/// whose code the deadline replaced), the cause is not repeated, but the
/// endpoint is still named. A typed validation cause, when present, wins.
#[test]
fn an_unanswered_code_is_not_repeated_as_its_cause_and_keeps_the_endpoint() {
    let (server, _directory) = server_at(9, 200_000);
    let error_of = |failure: ToolFailure| {
        let response =
            server.finish_tool_response("vouchers", &json!({}), Utc::now(), Err(failure));
        response.value["structuredContent"]["result"]["error"].clone()
    };
    let mut repeated = ToolFailure::from("request_deadline_exceeded".to_string());
    repeated.unanswered = Some(Unanswered("request_deadline_exceeded"));
    let error = error_of(repeated);
    assert!(error.get("cause").is_none(), "{error}");
    assert_eq!(error["endpoint"], "http://127.0.0.1:9");

    let mut both = ToolFailure::from("agent_runtime_read_failed".to_string());
    both.cause = Some("native_report_pair_changed");
    both.unanswered = Some(Unanswered("endpoint_unreachable"));
    let error = error_of(both);
    assert_eq!(error["cause"], "native_report_pair_changed");
    assert_eq!(error["endpoint"], "http://127.0.0.1:9");
}
