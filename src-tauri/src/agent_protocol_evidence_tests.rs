//! Final-frame refusal must agree with the next evidence-history response.
use super::*;
use tally_protocol_simulator::{
    Fixture, ProductStatus, ScenarioPlan, SequenceSimulator, WireEncoding,
};

#[tokio::test]
async fn admitted_long_id_cap_refusal_finalizes_retained_source_evidence() {
    let bytes = include_bytes!("../crates/bridge-tally-protocol/tests/fixtures/agent/native-licensed-companies.utf16le.xml");
    let xml = String::from_utf16(
        &bytes
            .chunks_exact(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let company =
        ScenarioPlan::new(Fixture::SyntheticXml(xml)).with_encoding(WireEncoding::Utf16Le);
    let status = ScenarioPlan::new(Fixture::ProductStatus(ProductStatus::TallyPrime));
    let simulator =
        SequenceSimulator::spawn(vec![status.clone(), company.clone(), status, company]).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let mut server = server(directory.path());
    server.settings.endpoint.port = simulator.address().port();
    let cap = server.settings.max_bytes;
    let long_id = "i".repeat(cap - 500);
    let requests = [
        initialize("2025-06-18"),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"tally_status","arguments":{}}}),
        json!({"jsonrpc":"2.0","id":long_id,"method":"tools/call","params":{"name":"tally_status","arguments":{}}}),
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"read_evidence","arguments":{"limit":2}}}),
    ];
    let frames = session(server, &requests).await;
    assert_eq!(frames.len(), 4);
    assert_eq!(frames[1]["result"]["isError"], false);
    assert!(frames[1]["result"].to_string().len() < cap);
    // Exact echoed ID proves the real pre-dispatch reservation admitted it.
    assert!(frames[2]["id"].as_str() == Some(long_id.as_str()));
    assert_eq!(frames[2]["error"]["message"], "agent_response_too_large");
    assert!(frames.iter().all(|frame| frame.to_string().len() < cap));
    let original = &frames[1]["result"]["structuredContent"]["evidence"];
    let records = frames[3]["result"]["structuredContent"]["result"]["records"]
        .as_array()
        .unwrap();
    assert_eq!(records.len(), 2, "one record per completed frame outcome");
    let withheld = &records[0];
    for field in ["request_sha256", "response_sha256", "bytes"] {
        assert_eq!(withheld[field], original[field], "{field}");
    }
    assert!(withheld["bytes"].as_u64().unwrap() > 0);
    assert_eq!(withheld["state"], "partial");
    assert_eq!(withheld["reason_code"], "agent_response_too_large");
    assert_eq!(records[1]["state"], "complete");
    let receipts = fs::read_to_string(directory.path().join("agent-egress.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(receipts[2]["record_type"], "response_prepared");
    assert_eq!(receipts[2]["rows_prepared"], 0);
    assert_eq!(receipts[2]["fields_prepared"], json!([]));
    assert_eq!(receipts[3]["record_type"], "stdio_write_completed");
    assert_eq!(receipts[3]["receipt_id"], receipts[2]["receipt_id"]);
    assert_eq!(simulator.finish().unwrap().len(), 4);
}
