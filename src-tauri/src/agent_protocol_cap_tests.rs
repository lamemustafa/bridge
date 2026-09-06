use super::*;

fn bounded_frames(output: &[u8], cap: usize) -> Vec<Value> {
    assert_eq!(output.last(), Some(&b'\n'));
    output
        .split_inclusive(|byte| *byte == b'\n')
        .map(|frame| {
            assert!(
                frame.len() <= cap,
                "frame exceeds {cap} bytes: {}",
                frame.len()
            );
            serde_json::from_slice(frame).expect("complete JSON-RPC frame")
        })
        .collect()
}

#[tokio::test]
async fn control_frames_obey_minimum_cap_and_refusals_keep_session_usable() {
    for import_enabled in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let mut server = server(directory.path());
        server.settings.max_bytes = 256;
        server.settings.import_enabled = import_enabled;
        // Use the largest escaped, multibyte ID admitted by the same recovery
        // reservation as a persisted tool call, including the wire newline.
        let mut id = String::new();
        while recovery_error(
            json!(format!("{id}é\"")),
            Some("bridge-00000000-0000-0000-0000-000000000000"),
            "import_publication_recovery_required",
        )
        .to_string()
        .len()
            < 256
        {
            id.push_str("é\"");
        }
        let mut init = initialize("2025-06-18");
        init["id"] = json!(id);
        let requests = [
            init,
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            json!({"jsonrpc":"2.0","id":id,"method":"tools/list"}),
            json!({"jsonrpc":"2.0","id":id,"method":"unknown"}),
            json!({"jsonrpc":"2.0","id":"x".repeat(256),"method":"tools/list"}),
            json!({"jsonrpc":"2.0","id":id,"method":"ping"}),
        ];
        let input = format!(
            "{{\n{}",
            requests
                .iter()
                .map(|value| format!("{value}\n"))
                .collect::<String>()
        );
        let mut output = Vec::new();
        serve_stdio(server, BufReader::new(input.as_bytes()), &mut output)
            .await
            .unwrap();
        let frames = bounded_frames(&output, 256);
        assert_eq!(frames.len(), 6);
        assert_eq!(frames[0]["error"]["code"], -32700);
        assert_eq!(frames[1]["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(frames[2]["error"]["code"], -32000);
        assert_eq!(frames[2]["error"]["message"], "agent_response_too_large");
        assert!(frames[2].get("result").is_none());
        assert_eq!(frames[3]["error"]["code"], -32601);
        assert_eq!(frames[4]["error"]["message"], "request_id_too_large");
        assert!(frames[4]["id"].is_null());
        assert_eq!(frames[5]["result"], json!({}));
        for index in [1, 2, 3, 5] {
            assert_eq!(frames[index]["id"], id);
        }
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
    }
}

#[tokio::test]
async fn control_final_frame_counts_newline_and_bounds_error_replacements() {
    let directory = tempfile::tempdir().unwrap();
    let mut server = server(directory.path());
    server.settings.max_bytes = 256;
    let id = json!("quoted\"é");
    let envelope = json!({"jsonrpc":"2.0","id":id,"result":{"padding":""}});
    let padding = "x".repeat(256 - envelope.to_string().len());
    let oversized_result = json!({"padding":padding});
    assert_eq!(
        json!({"jsonrpc":"2.0","id":id,"result":oversized_result})
            .to_string()
            .len(),
        256,
        "JSON fits but its mandatory newline does not"
    );
    let mut output = Vec::new();
    for result in [Ok(oversized_result), Err("large error".repeat(100))] {
        finish_response(&server, &mut output, id.clone(), result, None, None, false)
            .await
            .unwrap();
    }
    for frame in bounded_frames(&output, 256) {
        assert_eq!(frame["id"], id);
        assert_eq!(frame["error"]["code"], -32000);
        assert_eq!(frame["error"]["message"], "agent_response_too_large");
    }
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
}
