use std::process::{Command, Stdio};

fn startup_command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_bridge_mcp"));
    command
        .env("BRIDGE_TALLY_HOST", "127.0.0.1")
        .env("BRIDGE_TALLY_PORT", "9")
        .env("BRIDGE_AGENT_REDACTION", "none")
        .env("BRIDGE_AGENT_MAX_ROWS", "500")
        .env("BRIDGE_AGENT_MAX_BYTES", "200000")
        .stdin(Stdio::null());
    command
}

#[cfg(any(unix, windows))]
fn non_unicode_component() -> std::ffi::OsString {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        std::ffi::OsString::from_vec(vec![b'a', 0xff])
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStringExt;
        std::ffi::OsString::from_wide(&[b'a' as u16, 0xd800])
    }
}

#[cfg(any(unix, windows))]
#[test]
fn non_unicode_data_directory_fails_before_creating_state() {
    let directory = tempfile::tempdir().unwrap();
    let data = directory.path().join(non_unicode_component());
    assert!(data.to_str().is_none());
    assert!(serde_json::to_value(&data).is_err());
    let output = startup_command()
        .env("BRIDGE_AGENT_DATA_DIR", &data)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr).unwrap().trim(),
        "bridge-mcp: agent_data_dir_encoding_invalid"
    );
    assert!(!data.exists());
}

#[cfg(any(target_os = "macos", windows))]
#[test]
fn non_unicode_default_data_directory_fails_before_creating_state() {
    let directory = tempfile::tempdir().unwrap();
    let base = directory.path().join(non_unicode_component());
    let mut command = startup_command();
    command.env_remove("BRIDGE_AGENT_DATA_DIR");
    #[cfg(target_os = "macos")]
    command.env("HOME", &base);
    #[cfg(windows)]
    command.env("LOCALAPPDATA", &base);
    let output = command.output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr).unwrap().trim(),
        "bridge-mcp: agent_data_dir_encoding_invalid"
    );
    assert!(!base.exists());
}

#[test]
fn unicode_data_directory_is_preserved_and_admitted() {
    let directory = tempfile::tempdir().unwrap();
    let data = directory.path().join("पुस्तक-账簿-📚");
    let output = startup_command()
        .env("BRIDGE_AGENT_DATA_DIR", &data)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
    assert!(data.is_dir());
    assert_eq!(serde_json::to_value(&data).unwrap().as_str(), data.to_str());
}

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
