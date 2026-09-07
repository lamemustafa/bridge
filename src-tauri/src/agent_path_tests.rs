use super::*;
#[cfg(windows)]
use std::process::Command;

#[test]
fn native_dispatch_coordination_dir_requires_an_absolute_os_root() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().to_path_buf();
    assert_eq!(
        stable_coordination_dir(Some(root.clone())),
        Some(root.join("Bridge").join("agent"))
    );
    assert_eq!(stable_coordination_dir(None), None);
    assert_eq!(stable_coordination_dir(Some(PathBuf::from("Bridge"))), None);
}

#[cfg(target_os = "macos")]
#[test]
fn macos_coordination_dir_uses_the_historical_application_support_root() {
    let directory = tempfile::tempdir().unwrap();
    let home = directory.path().join("account-home");
    assert_eq!(
        macos_coordination_dir(Some(home.clone())),
        Some(
            home.join("Library")
                .join("Application Support")
                .join("Bridge")
        )
    );
    assert_eq!(macos_coordination_dir(None), None);
    assert_eq!(
        macos_coordination_dir(Some(PathBuf::from("account-home"))),
        None
    );
}

#[cfg(target_os = "macos")]
#[test]
fn macos_default_coordination_dir_matches_the_historical_default_data_root() {
    let expected = macos_account_home_dir().map(|home| {
        home.join("Library")
            .join("Application Support")
            .join("Bridge")
    });
    assert_eq!(default_dispatch_coordination_dir(), expected);
}

#[cfg(not(any(windows, target_os = "macos")))]
#[test]
fn native_dispatch_coordination_dir_matches_the_existing_default_data_dir() {
    assert_eq!(
        default_dispatch_coordination_dir(),
        Some(default_data_dir())
    );
}

#[cfg(windows)]
const COORDINATION_TEST_EXPECTED_ROOT: &str = "BRIDGE_COORDINATION_TEST_EXPECTED_ROOT";
#[cfg(windows)]
const COORDINATION_TEST_CUSTOM_DATA_ROOT: &str = "BRIDGE_COORDINATION_TEST_CUSTOM_DATA_ROOT";

#[cfg(windows)]
#[test]
fn windows_native_dispatch_coordination_dir_ignores_filtered_environment_and_custom_data_dir() {
    let expected_root = default_dispatch_coordination_dir()
        .expect("Windows local app-data known folder")
        .into_os_string();
    let directory = tempfile::tempdir().unwrap();
    let custom_data_root = directory.path().join("separate-agent-data");
    let module = module_path!()
        .strip_prefix(concat!(env!("CARGO_CRATE_NAME"), "::"))
        .expect("crate module prefix");
    let status = Command::new(std::env::current_exe().expect("test executable"))
        .arg("--exact")
        .arg(format!(
            "{module}::windows_native_dispatch_coordination_dir_child"
        ))
        .arg("--nocapture")
        .env_remove("LOCALAPPDATA")
        .env_remove("APPDATA")
        .env("BRIDGE_AGENT_DATA_DIR", &custom_data_root)
        .env(COORDINATION_TEST_EXPECTED_ROOT, expected_root)
        .env(COORDINATION_TEST_CUSTOM_DATA_ROOT, &custom_data_root)
        .status()
        .expect("coordination child");
    assert!(status.success());
    assert!(custom_data_root.with_extension("observed").is_file());
}

#[cfg(windows)]
#[test]
fn windows_native_dispatch_coordination_dir_child() {
    let Some(expected_root) = std::env::var_os(COORDINATION_TEST_EXPECTED_ROOT) else {
        return;
    };
    let custom_data_root = PathBuf::from(
        std::env::var_os(COORDINATION_TEST_CUSTOM_DATA_ROOT).expect("custom data root"),
    );
    assert!(std::env::var_os("LOCALAPPDATA").is_none());
    assert!(std::env::var_os("APPDATA").is_none());
    assert_ne!(PathBuf::from(&expected_root), custom_data_root);
    assert_eq!(
        default_dispatch_coordination_dir(),
        Some(PathBuf::from(expected_root))
    );
    std::fs::write(custom_data_root.with_extension("observed"), b"checked").unwrap();
}
