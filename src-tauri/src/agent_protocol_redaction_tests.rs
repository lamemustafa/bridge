use super::*;

#[test]
fn schema_metadata_exception_never_exempts_accounting_narration() {
    let bytes = include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-three-vouchers.utf16le.xml"
    );
    let xml = String::from_utf16(
        &bytes
            .chunks_exact(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let rows = parse_agent_rows(&xml, "61c6de69-1748-461c-ad3f-162cb949df9f").unwrap();
    assert!(rows.iter().any(|row| row["narration"]
        .as_str()
        .is_some_and(|text| !text.is_empty())));
    let value = json!({"result": {
        "schema":{"properties":{"narration":{"type":"string"}}},
        "items":rows,
        "other":{"schema":{"narration":"Accounting text under an arbitrary schema key"}}
    }});
    for tool in ["voucher_schema", "vouchers"] {
        let redacted = redact_tool_response(tool, value.clone(), Redaction::DropNarration);
        for (before, after) in rows
            .iter()
            .zip(redacted["result"]["items"].as_array().unwrap())
        {
            assert!(after.get("narration").is_none());
            assert_eq!(after["amounts"], before["amounts"]);
        }
        assert!(redacted["result"]["other"]["schema"]
            .get("narration")
            .is_none());
        assert_eq!(
            redacted["result"]["schema"]["properties"]
                .get("narration")
                .is_some(),
            tool == "voucher_schema"
        );
    }
}

#[tokio::test]
async fn narration_redaction_preserves_the_closed_server_voucher_schema() {
    let directory = tempfile::tempdir().unwrap();
    let mut server = server(directory.path());
    server.settings.redaction = Redaction::DropNarration;
    server.settings.import_enabled = true;
    let responses = session(server, &[
        initialize("2025-06-18"),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"voucher_schema","arguments":{}}}),
    ]).await;
    let result = &responses[2]["result"];
    assert_eq!(result["isError"], false);
    let schema = &result["structuredContent"]["result"]["schema"];
    let voucher = &schema["properties"]["vouchers"]["items"];
    assert_eq!(voucher["additionalProperties"], false);
    assert_eq!(voucher["properties"]["narration"], json!({"type":"string"}));
    let build_schema = &responses[1]["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "build_import_xml")
        .unwrap()["inputSchema"];
    assert_eq!(
        schema, build_schema,
        "schema inspection must describe the admitted payload"
    );
    let text: Value = serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(text, result["structuredContent"]);
}
