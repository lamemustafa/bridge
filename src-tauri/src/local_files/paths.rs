//! Local artifact and native-dispatch coordination path resolution.
use std::env;
use std::path::PathBuf;

pub(crate) fn default_data_dir() -> PathBuf {
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
/// the same local Tally listener. Supported platforms resolve an OS-owned
/// per-user root directly so launcher environment filtering cannot move the
/// lease.
pub(crate) fn default_dispatch_coordination_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        stable_coordination_dir(windows_local_app_data_dir())
    }
    #[cfg(target_os = "macos")]
    {
        macos_coordination_dir(macos_account_home_dir())
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let root = default_data_dir();
        root.is_absolute().then_some(root)
    }
}

#[cfg(target_os = "macos")]
fn macos_coordination_dir(account_home: Option<PathBuf>) -> Option<PathBuf> {
    account_home.filter(|path| path.is_absolute()).map(|home| {
        home.join("Library")
            .join("Application Support")
            .join("Bridge")
    })
}

#[cfg(target_os = "macos")]
fn macos_account_home_dir() -> Option<PathBuf> {
    use std::ffi::{CStr, OsStr};
    use std::mem::MaybeUninit;
    use std::os::unix::ffi::OsStrExt;
    use std::ptr;

    // macOS has no specified bound for a passwd entry; this covers normal
    // directory-service records while keeping an anomalous lookup fail-closed.
    const PASSWD_BUFFER_BYTES: usize = 16 * 1024;
    let mut passwd = MaybeUninit::<libc::passwd>::uninit();
    let mut result = ptr::null_mut();
    let mut buffer = vec![0u8; PASSWD_BUFFER_BYTES];
    // SAFETY: the output struct and buffer are live and writable for the call;
    // `geteuid` reads only the effective user identity.
    let status = unsafe {
        libc::getpwuid_r(
            libc::geteuid(),
            passwd.as_mut_ptr(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &mut result,
        )
    };
    if status != 0 || result.is_null() {
        return None;
    }
    // SAFETY: a successful getpwuid_r result points at the initialized passwd
    // structure and supplies a nul-terminated directory string in `pw_dir`.
    let home = unsafe {
        let directory = (*result).pw_dir;
        if directory.is_null() {
            return None;
        }
        PathBuf::from(OsStr::from_bytes(CStr::from_ptr(directory).to_bytes()))
    };
    home.is_absolute().then_some(home)
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
#[path = "paths_tests.rs"]
mod tests;
