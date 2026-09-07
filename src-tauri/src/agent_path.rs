//! Local artifact and native-dispatch coordination path resolution.
use std::env;
use std::path::PathBuf;

pub(super) fn default_data_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        if let Some(local_app_data) = env::var_os("LOCALAPPDATA") {
            return PathBuf::from(local_app_data).join("Bridge").join("agent");
        }
        if let Some(app_data) = env::var_os("APPDATA") {
            return PathBuf::from(app_data).join("Bridge").join("agent");
        }
        PathBuf::from("Bridge").join("agent")
    }
    #[cfg(not(target_os = "windows"))]
    {
        #[cfg(target_os = "macos")]
        if let Some(home) = env::var_os("HOME") {
            return PathBuf::from(home)
                .join("Library")
                .join("Application Support")
                .join("Bridge");
        }
        env::temp_dir().join("bridge")
    }
}

/// Resolve this version's shared per-user native-dispatch coordination root.
///
/// This deliberately ignores `BRIDGE_AGENT_DATA_DIR`: callers may choose
/// separate recoverable journals, but they must still serialize a dispatch to
/// the same local Tally listener. Windows uses the shell's per-user known
/// folder directly so launcher environment filtering cannot move the lease.
pub(super) fn default_dispatch_coordination_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        stable_coordination_dir(windows_local_app_data_dir())
    }
    #[cfg(not(windows))]
    {
        let root = default_data_dir();
        root.is_absolute().then_some(root)
    }
}

#[cfg(any(windows, test))]
pub(super) fn stable_coordination_dir(os_data_dir: Option<PathBuf>) -> Option<PathBuf> {
    os_data_dir
        .filter(|path| path.is_absolute())
        .map(|path| path.join("Bridge").join("agent"))
}

#[cfg(windows)]
pub(super) fn windows_local_app_data_dir() -> Option<PathBuf> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use std::ptr;
    use std::slice;
    use windows_sys::Win32::Foundation::S_OK;
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::{
        FOLDERID_LocalAppData, SHGetKnownFolderPath, KF_FLAG_DONT_VERIFY,
    };

    unsafe extern "C" {
        fn wcslen(value: *const u16) -> usize;
    }

    let mut raw_path = ptr::null_mut();
    // SAFETY: SHGetKnownFolderPath initializes `raw_path` to a CoTaskMem-allocated,
    // nul-terminated UTF-16 string on success. It accepts a null current-user token.
    let result = unsafe {
        SHGetKnownFolderPath(
            &FOLDERID_LocalAppData,
            KF_FLAG_DONT_VERIFY as u32,
            ptr::null_mut(),
            &mut raw_path,
        )
    };
    if result != S_OK || raw_path.is_null() {
        // SAFETY: CoTaskMemFree accepts null and releases any buffer supplied on error.
        unsafe { CoTaskMemFree(raw_path.cast()) };
        return None;
    }
    // SAFETY: the successful API result is a nul-terminated UTF-16 string.
    let path = unsafe {
        PathBuf::from(OsString::from_wide(slice::from_raw_parts(
            raw_path,
            wcslen(raw_path),
        )))
    };
    // SAFETY: `raw_path` was allocated by SHGetKnownFolderPath above.
    unsafe { CoTaskMemFree(raw_path.cast()) };
    Some(path)
}

#[cfg(test)]
mod tests {
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

    #[cfg(not(windows))]
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
}
