//! Discovery faults remain distinct from an observed empty company collection.
use super::*;
use tally_protocol_simulator::{
    Fixture, ProductStatus, ScenarioPlan, SequenceSimulator, WireEncoding,
};

#[tokio::test]
async fn status_retains_invalid_discovery_reason_and_completed_sources() {
    let raw = include_bytes!("../crates/bridge-tally-protocol/tests/fixtures/agent/native-licensed-release-companies.utf16le.xml");
    let captured = String::from_utf16(
        &raw.chunks_exact(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let mut empty = captured.clone();
    let start = empty.find("<COMPANY ").unwrap();
    let end = empty.rfind("</COMPANY>").unwrap() + "</COMPANY>".len();
    empty.replace_range(start..end, "");
    // In-memory mutations of one retained capture, not new live fixtures.
    for (name, source, invalid) in [
        ("valid", captured.clone(), false),
        ("empty", empty, false),
        (
            "guid",
            captured.replacen("bb8ad19e-6aef-4239-a917-87fec0c6215e", "non-ascii-é", 1),
            true,
        ),
        (
            "number",
            captured.replacen("> 100000</COMPANYNUMBER>", ">invalid</COMPANYNUMBER>", 1),
            true,
        ),
        (
            "date",
            captured.replacen(">20240401</BOOKSFROM>", ">20240230</BOOKSFROM>", 1),
            true,
        ),
    ] {
        if invalid {
            assert_ne!(source, captured, "{name}");
        }
        let plans = vec![
            ScenarioPlan::new(Fixture::ProductStatus(ProductStatus::TallyPrime)),
            ScenarioPlan::new(Fixture::SyntheticXml(source)).with_encoding(WireEncoding::Utf16Le),
        ];
        let bodies = plans
            .iter()
            .map(ScenarioPlan::response_bytes)
            .collect::<Vec<_>>();
        let simulator = SequenceSimulator::spawn(plans).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let server = Server::new(Settings {
            endpoint: TallyEndpointConfig {
                host: "127.0.0.1".into(),
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
        assert_eq!(
            response["isError"], false,
            "status retains diagnostic facts: {name}"
        );
        let content = &response["structuredContent"];
        let reason = if invalid {
            json!("company_identity_invalid")
        } else {
            Value::Null
        };
        assert_eq!(content["result"]["refusal_reason"], reason, "{name}");
        assert_eq!(
            content["result"]["loaded_companies"]
                .as_array()
                .unwrap()
                .is_empty(),
            name != "valid"
        );
        let evidence = &content["evidence"];
        assert_eq!(
            evidence["state"],
            if invalid { "partial" } else { "complete" }
        );
        assert_eq!(evidence["reason_code"], reason);
        let requests = simulator.finish().unwrap();
        assert_eq!(requests.len(), 2);
        let join = |left: &str, right: &str| sha256_hex(format!("{left}:{right}").as_bytes());
        assert_eq!(
            evidence["request_sha256"],
            join(
                &requests[0].request_body_sha256,
                &requests[1].request_body_sha256
            )
        );
        assert_eq!(
            evidence["response_sha256"],
            join(&sha256_hex(&bodies[0]), &sha256_hex(&bodies[1]))
        );
        assert_eq!(
            evidence["bytes"],
            bodies.iter().map(Vec::len).sum::<usize>()
        );
        assert_eq!(
            serde_json::from_str::<Value>(response["content"][0]["text"].as_str().unwrap())
                .unwrap(),
            *content
        );
        let history = server.call_tool("read_evidence", json!({})).await;
        assert_eq!(
            history["structuredContent"]["result"]["records"][0],
            *evidence
        );
    }
}
