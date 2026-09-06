use super::*;

#[tokio::test]
async fn unqualified_change_feed_is_hidden_and_direct_calls_refuse_before_tally() {
    for import_enabled in [false, true] {
        assert!(!tool_definitions(import_enabled)
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["name"] == "changed_since"));
    }
    let directory = tempfile::tempdir().unwrap();
    let server = Server::new(Settings {
        endpoint: TallyEndpointConfig {
            host: "127.0.0.1".into(),
            port: 9,
        },
        data_dir: directory.path().to_path_buf(),
        max_rows: 500,
        max_bytes: 200_000,
        redaction: Redaction::None,
        import_enabled: true,
    });
    // A dispatched read against this unavailable endpoint would return a
    // transport/company failure, not the explicit admission refusal.
    for arguments in [json!({}), json!({"company_guid":"synthetic-company"})] {
        let result = server.call_tool_response("changed_since", arguments).await;
        assert_eq!(result.value["isError"], true);
        assert_eq!(
            result.value["structuredContent"]["result"]["error"]["code"],
            "changed_since_unqualified"
        );
        assert_eq!(result.value["structuredContent"]["evidence"]["bytes"], 0);
    }
}
