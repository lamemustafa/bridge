use super::*;

#[test]
fn output_path_is_optional_and_requires_its_flag() {
    let mut no_output = Vec::<String>::new().into_iter();
    assert_eq!(optional_output_path(&mut no_output).unwrap(), None);

    let mut output = vec!["--output".to_string(), "next.json".to_string()].into_iter();
    assert_eq!(
        optional_output_path(&mut output).unwrap(),
        Some(PathBuf::from("next.json"))
    );

    let mut missing_path = vec!["--output".to_string()].into_iter();
    assert_eq!(
        optional_output_path(&mut missing_path),
        Err("missing_output_path")
    );
}

#[test]
fn output_file_is_utf8_without_bom_and_replaces_existing_file() {
    let directory = tempfile::tempdir().unwrap();
    let output_path = directory.path().join("surface.json");
    fs::write(&output_path, b"previous").unwrap();

    emit_output(CommandOutput::output_file(
        "{\"a\":1}".to_string(),
        Some(output_path.clone()),
    ))
    .unwrap();

    assert_eq!(fs::read(&output_path).unwrap(), br#"{"a":1}"#);
}

#[cfg(unix)]
fn assert_new_destination_mode(expected_mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    let output_path = directory.path().join("new-surface.json");
    emit_output(CommandOutput::output_file(
        "{\"a\":1}".to_string(),
        Some(output_path.clone()),
    ))
    .unwrap();
    assert_eq!(
        fs::metadata(output_path).unwrap().permissions().mode() & 0o777,
        expected_mode
    );
}

#[cfg(unix)]
fn assert_existing_destination_mode() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().unwrap();
    let output_path = directory.path().join("surface.json");
    fs::write(&output_path, b"previous").unwrap();
    fs::set_permissions(&output_path, std::fs::Permissions::from_mode(0o644)).unwrap();

    emit_output(CommandOutput::output_file(
        "{\"a\":1}".to_string(),
        Some(output_path.clone()),
    ))
    .unwrap();

    assert_eq!(
        fs::metadata(output_path).unwrap().permissions().mode() & 0o777,
        0o644
    );
}

#[cfg(unix)]
fn run_output_mode_child(test_name: &str, umask: &str, marker: &str) {
    let status = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("umask {umask}; exec \"$@\""))
        .arg("sh")
        .arg(std::env::current_exe().unwrap())
        .arg(test_name)
        .arg("--exact")
        .env("BRIDGE_COMPAT_UMASK_CHILD", marker)
        .status()
        .expect("spawn controlled-umask child");
    assert!(status.success());
}

#[cfg(unix)]
#[test]
fn output_file_preserves_existing_destination_mode() {
    if std::env::var_os("BRIDGE_COMPAT_UMASK_CHILD").as_deref() == Some(std::ffi::OsStr::new("022"))
    {
        assert_existing_destination_mode();
        assert_new_destination_mode(0o644);
        return;
    }
    run_output_mode_child(
        "tests::output_file_preserves_existing_destination_mode",
        "022",
        "022",
    );
}

#[cfg(unix)]
#[test]
fn output_file_preserves_existing_mode_under_restrictive_umask() {
    if std::env::var_os("BRIDGE_COMPAT_UMASK_CHILD").as_deref() == Some(std::ffi::OsStr::new("077"))
    {
        assert_existing_destination_mode();
        assert_new_destination_mode(0o600);
        return;
    }
    run_output_mode_child(
        "tests::output_file_preserves_existing_mode_under_restrictive_umask",
        "077",
        "077",
    );
}

#[test]
fn failed_command_does_not_create_requested_output_file() {
    let directory = tempfile::tempdir().unwrap();
    let output_path = directory.path().join("surface.json");
    let missing_surface = directory.path().join("missing-surface.json");
    let error = run_from_args(
        vec![
            "rehash-surface".to_string(),
            missing_surface.display().to_string(),
            directory.path().display().to_string(),
            "--output".to_string(),
            output_path.display().to_string(),
        ]
        .into_iter(),
    )
    .unwrap_err();

    assert_eq!(error, "artifact_unavailable");
    assert!(!output_path.exists());
}
