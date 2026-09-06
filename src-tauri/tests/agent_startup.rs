use std::process::{Command, Stdio};

#[test]
fn invalid_endpoint_settings_fail_before_creating_state_or_advertising_tools() {
    for host in [
        "example.invalid",
        "192.0.2.1",
        "",
        "localhost/path",
        "localhost:9001",
        "[::1]",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let data = directory.path().join("not-created");
        let output = Command::new(env!("CARGO_BIN_EXE_bridge_mcp"))
            .env("BRIDGE_TALLY_HOST", host)
            .env("BRIDGE_TALLY_PORT", "9001")
            .env("BRIDGE_AGENT_DATA_DIR", &data)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        assert_eq!(
            String::from_utf8(output.stderr).unwrap().trim(),
            "bridge-mcp: host_setting_invalid"
        );
        assert!(!data.exists());
    }
}
