use super::*;
use crate::agent::{agent_protocol::finish_response, Settings, ToolResponse};

// Reuse the existing build/readback regression's first sixteen observations.
// No new Tally behavior is inferred from these simulator fixtures.
async fn persisted_build(cap: usize) -> (tempfile::TempDir, Server, ToolResponse) {
    let simulator =
        SequenceSimulator::spawn(import_cycle_plans().into_iter().take(16).collect()).unwrap();
    let directory = tempfile::tempdir().unwrap();
    // A long but valid output path makes response caps independently reachable
    // after the bounded Tally reads have completed and the XML was persisted.
    let data_dir = directory
        .path()
        .join("a".repeat(150))
        .join("b".repeat(150))
        .join("c".repeat(150));
    fs::create_dir_all(&data_dir).unwrap();
    let server = Server::new(Settings {
        endpoint: TallyEndpointConfig {
            host: "127.0.0.1".into(),
            port: simulator.address().port(),
        },
        data_dir,
        max_rows: 10,
        max_bytes: cap,
        redaction: crate::agent::Redaction::None,
        import_enabled: true,
    });
    let tool = server
        .call_tool_response(
            "build_import_xml",
            serde_json::to_value(captured_catalogue_payload()).unwrap(),
        )
        .await;
    let batch_id = tool
        .recovery_batch_id
        .as_ref()
        .expect("successful persisted build has a recovery ID");
    let xml = fs::read(
        server
            .settings
            .data_dir
            .join("imports")
            .join(format!("{batch_id}.xml")),
    )
    .unwrap();
    assert!(!xml.is_empty());
    let ledger: Value = serde_json::from_str(
        fs::read_to_string(server.settings.data_dir.join("agent-import-ledger.jsonl"))
            .unwrap()
            .trim(),
    )
    .unwrap();
    assert_eq!(ledger["batch_id"], *batch_id);
    assert_eq!(ledger["status"], "built");
    assert_eq!(ledger["sha256"], sha256_hex(&xml));
    assert_eq!(simulator.finish().unwrap().len(), 16);
    (directory, server, tool)
}

async fn release(server: &Server, tool: ToolResponse, receipt_fails: bool) -> Value {
    let expected_batch = tool.recovery_batch_id.clone().unwrap();
    if receipt_fails {
        fs::create_dir(server.settings.data_dir.join("agent-egress.jsonl")).unwrap();
    }
    let mut wire = Vec::new();
    finish_response(
        server,
        &mut wire,
        json!(2),
        Ok(tool.value),
        Some(tool.egress),
        tool.recovery_batch_id,
        true,
    )
    .await
    .unwrap();
    assert!(wire.len() <= server.settings.max_bytes);
    let response: Value = serde_json::from_slice(&wire).unwrap();
    assert_eq!(response["error"]["data"]["batch_id"], expected_batch);
    assert_eq!(
        response["error"]["message"],
        if receipt_fails {
            "egress_record_write_failed"
        } else {
            "agent_response_too_large"
        }
    );
    if !receipt_fails {
        let receipt: Value = serde_json::from_str(
            fs::read_to_string(server.settings.data_dir.join("agent-egress.jsonl"))
                .unwrap()
                .trim(),
        )
        .unwrap();
        assert_eq!(receipt["response_sha256"], sha256_hex(&wire));
        assert_eq!(receipt["bytes_returned"], wire.len());
        assert_eq!(receipt["fields_returned"], json!([]));
    }
    response
}

#[tokio::test]
async fn persisted_batch_survives_structured_and_mcp_result_cap_replacements() {
    let (_directory, _server, baseline) = persisted_build(200_000).await;
    let structured_bytes = baseline.value["structuredContent"].to_string().len();
    let mcp_bytes = baseline.value.to_string().len();
    assert!(mcp_bytes > structured_bytes + 200);
    for cap in [structured_bytes - 100, (structured_bytes + mcp_bytes) / 2] {
        let (_directory, server, tool) = persisted_build(cap).await;
        assert_eq!(
            tool.value["isError"], true,
            "cap stage should replace full result"
        );
        release(&server, tool, false).await;
    }
}

#[tokio::test]
async fn persisted_batch_survives_final_256_byte_cap_and_receipt_failure() {
    for receipt_fails in [false, true] {
        let (_directory, mut server, tool) = persisted_build(200_000).await;
        assert_eq!(tool.value["isError"], false);
        // Exercise the production final framing path at the minimum configured
        // budget independently of the larger budget needed for prior Tally reads.
        server.settings.max_bytes = 256;
        release(&server, tool, receipt_fails).await;
    }
}

#[tokio::test]
async fn capped_reads_retain_source_commitments_for_read_evidence() {
    let (_directory, _server, baseline) = persisted_build(200_000).await;
    let expected = &baseline.value["structuredContent"]["evidence"];
    let structured_bytes = baseline.value["structuredContent"].to_string().len();
    let mcp_bytes = baseline.value.to_string().len();
    for cap in [structured_bytes - 100, (structured_bytes + mcp_bytes) / 2] {
        let (_directory, mut server, tool) = persisted_build(cap).await;
        assert_eq!(
            tool.value["structuredContent"]["error"]["code"],
            "agent_response_too_large"
        );
        server.settings.max_bytes = 200_000;
        let evidence = server.call_tool("read_evidence", json!({"limit": 1})).await;
        let records = evidence["structuredContent"]["result"]["records"]
            .as_array()
            .unwrap();
        assert_eq!(
            records.len(),
            1,
            "read evidence survives either cap replacement"
        );
        let record = &records[0];
        for field in ["request_sha256", "response_sha256", "bytes"] {
            assert_eq!(record[field], expected[field], "{field}");
        }
        assert!(record["bytes"].as_u64().unwrap() > 0);
        assert_eq!(record["state"], "partial");
        assert_eq!(record["reason_code"], "agent_response_too_large");
        assert!(record["read_at"].is_string());
        assert!(record["duration_ms"].is_number());
    }
}
