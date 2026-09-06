use super::*;

#[test]
fn large_pages_use_logarithmically_bounded_serialization_probes() {
    let mut response = json!({"result":{"offset":0,"items":
        (0..10_000).map(|id| json!({"id":id,"padding":"x".repeat(64)})).collect::<Vec<_>>()}});
    let mut probes = 0;
    assert!(fit_response(&mut response, "", 512, |value| {
        probes += 1;
        value.to_string().len()
    })
    .unwrap());
    assert!(probes <= 15, "serialization probes: {probes}");
    assert!(response.to_string().len() <= 512);
    let kept = response["result"]["items"].as_array().unwrap();
    assert!(!kept.is_empty());
    assert_eq!(response["result"]["next_offset"], kept.len());
    assert_eq!(kept[0]["id"], 0);
    assert_eq!(kept.last().unwrap()["id"], kept.len() - 1);
}

#[test]
fn standalone_master_and_status_rows_are_counted_in_final_receipts() {
    for (tool, axis) in [
        ("validate_masters", "masters"),
        ("tally_status", "loaded_companies"),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let server = Server::new(Settings {
            endpoint: TallyEndpointConfig {
                host: "127.0.0.1".into(),
                port: 9,
            },
            data_dir: directory.path().to_path_buf(),
            max_rows: 10,
            max_bytes: 200_000,
            redaction: Redaction::None,
            import_enabled: false,
        });
        let mut structured = json!({"result": {}});
        structured["result"][axis] = json!([{"name":"A"}, {"name":"B"}]);
        let mut response = json!({"jsonrpc":"2.0", "id":1, "result":{
            "content":[{"type":"text","text":""}], "structuredContent":structured,
            "isError":false}});
        set_mcp_content_json(&mut response["result"]);
        let wire = format!("{response}\n");
        server
            .append_framed_egress(
                EgressContext {
                    tool: tool.into(),
                    args_sha256: sha256_json(&json!({})),
                    company_guid: None,
                },
                &response,
                &wire,
            )
            .unwrap();
        let receipt: Value = serde_json::from_str(
            &fs::read_to_string(directory.path().join("agent-egress.jsonl")).unwrap(),
        )
        .unwrap();
        assert_eq!(receipt["rows_prepared"], 2, "{tool}");
        assert_eq!(receipt["response_sha256"], sha256_hex(wire.as_bytes()));
    }
}

fn outstandings_page(offset: usize) -> Value {
    let rows = |count| {
        (0..count)
            .map(|id| json!({"id":id,"padding":"x".repeat(120)}))
            .collect()
    };
    let (bills, _, bill_next) = paginate_open_bills(rows(3), offset, 3);
    let (parties, _, party_next) = paginate_open_bills(rows(5), offset, 3);
    json!({"truncated":bill_next.is_some() || party_next.is_some(), "result":{
        "offset":offset,"open_bills":bills,"next_offset":bill_next,
        "unallocated":{"count":5,"parties":parties,"next_offset":party_next,
        "truncated":party_next.is_some()}}})
}

#[test]
fn final_framing_caps_advance_both_outstandings_axes_without_losing_rows() {
    let mut offset = 0;
    let mut bills = Vec::new();
    let mut parties = Vec::new();
    loop {
        let mut response = json!({"jsonrpc":"2.0","id":1,"result":{
            "content":[{"type":"text","text":""}], "isError":false,
            "structuredContent":outstandings_page(offset)}});
        set_mcp_content_json(&mut response["result"]);
        enforce_jsonrpc_response_byte_cap(&mut response, 1200).unwrap();
        assert!(response.to_string().len() < 1200);
        let payload = &response["result"]["structuredContent"];
        assert_eq!(
            serde_json::from_str::<Value>(
                response["result"]["content"][0]["text"].as_str().unwrap()
            )
            .unwrap(),
            *payload
        );
        bills.extend(
            payload["result"]["open_bills"]
                .as_array()
                .unwrap()
                .iter()
                .map(|row| row["id"].as_u64().unwrap()),
        );
        parties.extend(
            payload["result"]["unallocated"]["parties"]
                .as_array()
                .unwrap()
                .iter()
                .map(|row| row["id"].as_u64().unwrap()),
        );
        assert_eq!(payload["result"]["unallocated"]["count"], 5);
        let cursors = [
            payload["result"]["next_offset"].as_u64(),
            payload["result"]["unallocated"]["next_offset"].as_u64(),
        ];
        if offset >= 3 {
            assert!(cursors[0].is_none());
        }
        let continuing = cursors.into_iter().flatten().collect::<Vec<_>>();
        let Some(next) = continuing.first() else {
            break;
        };
        assert!(continuing.iter().all(|cursor| cursor == next));
        assert!(*next > offset as u64);
        offset = *next as usize;
        assert!(offset <= 5);
    }
    assert_eq!(bills, [0, 1, 2]);
    assert_eq!(parties, [0, 1, 2, 3, 4]);
}

#[test]
fn every_cap_layer_refuses_a_page_whose_first_rows_cannot_fit() {
    let mut structured = outstandings_page(0);
    structured["result"]["open_bills"][0]["padding"] = json!("x".repeat(2000));
    assert_eq!(
        enforce_response_byte_cap(structured.clone(), 500).unwrap_err(),
        "agent_response_too_large"
    );
    let mcp = json!({"content":[{"type":"text","text":""}], "isError":false,"structuredContent":structured});
    assert_eq!(
        enforce_mcp_result_byte_cap(&mut mcp.clone(), 500, "outstandings", 6).unwrap_err(),
        "agent_response_too_large"
    );
    let mut framed = json!({"jsonrpc":"2.0","id":1,"result":mcp});
    set_mcp_content_json(&mut framed["result"]);
    assert_eq!(
        enforce_jsonrpc_response_byte_cap(&mut framed, 500).unwrap_err(),
        "agent_response_too_large"
    );
    for axis in ["items", "ledgers"] {
        let mut page = json!({"result":{"offset":7}});
        page["result"][axis] = json!([{"padding":"x".repeat(2000)}]);
        assert_eq!(
            enforce_response_byte_cap(page, 500).unwrap_err(),
            "agent_response_too_large"
        );
    }
}
