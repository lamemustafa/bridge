use super::*;
use bridge_tally_transport::TallyEndpointConfig;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::thread;
use std::time::{Duration, Instant};

const CHILD_EXPECTED_ROOT: &str = "BRIDGE_MACOS_DISPATCH_LEASE_EXPECTED_ROOT";
const CHILD_DATA_ROOT: &str = "BRIDGE_MACOS_DISPATCH_LEASE_DATA_ROOT";
const CHILD_HOME_MODE: &str = "BRIDGE_MACOS_DISPATCH_LEASE_HOME_MODE";
const CHILD_OVERRIDDEN_HOME: &str = "BRIDGE_MACOS_DISPATCH_LEASE_OVERRIDDEN_HOME";
const CHILD_PORT: &str = "BRIDGE_MACOS_DISPATCH_LEASE_PORT";
const CHILD_READY: &str = "BRIDGE_MACOS_DISPATCH_LEASE_READY";
const CHILD_RELEASE: &str = "BRIDGE_MACOS_DISPATCH_LEASE_RELEASE";

#[test]
fn macos_native_dispatch_lease_contends_across_filtered_and_overridden_home() {
    let directory = tempfile::tempdir().unwrap();
    let expected_default_root = crate::local_files::paths::default_dispatch_coordination_dir()
        .expect("macOS account coordination root");

    for home_mode in [HomeMode::Filtered, HomeMode::Overridden] {
        // Reserve the real endpoint identity for the entire parent/child run.
        // Both processes use the production root; a unique held port isolates
        // their lease file while still testing acquire() root selection.
        let (port_reservation, port) = reserve_loopback_port().expect("loopback test port");
        let endpoint = endpoint(port);
        let ready = directory.path().join(format!("{home_mode:?}-ready"));
        let release = directory.path().join(format!("{home_mode:?}-release"));
        let data_root = directory.path().join(format!("{home_mode:?}-agent-data"));
        let overridden_home = directory.path().join(format!("{home_mode:?}-home"));
        let mut child = ChildLease::new(
            spawn_child(
                &expected_default_root,
                &data_root,
                &overridden_home,
                home_mode,
                port,
                &ready,
                &release,
            ),
            release,
        );
        child.wait_for_ready(&ready).unwrap();
        assert_eq!(
            acquire(&endpoint).err().as_deref(),
            Some("import_admission_busy")
        );
        assert!(child.release().unwrap().success());
        drop(acquire(&endpoint).expect("lease reacquired after child release"));
        drop(port_reservation);
    }
}

#[test]
fn macos_native_dispatch_lease_child() {
    let Some(expected_root) = std::env::var_os(CHILD_EXPECTED_ROOT) else {
        return;
    };
    let data_root = PathBuf::from(std::env::var_os(CHILD_DATA_ROOT).expect("child data root"));
    let home_mode = std::env::var(CHILD_HOME_MODE).expect("child home mode");
    if home_mode == "filtered" {
        assert!(std::env::var_os("HOME").is_none());
    } else {
        assert_eq!(home_mode, "overridden");
        assert_eq!(
            std::env::var_os("HOME"),
            std::env::var_os(CHILD_OVERRIDDEN_HOME)
        );
    }
    assert_ne!(PathBuf::from(&expected_root), data_root);
    assert_eq!(
        std::env::var_os("BRIDGE_AGENT_DATA_DIR"),
        Some(data_root.into_os_string())
    );
    assert_eq!(
        crate::local_files::paths::default_dispatch_coordination_dir(),
        Some(PathBuf::from(expected_root))
    );
    let port = std::env::var(CHILD_PORT)
        .expect("child port")
        .parse()
        .expect("valid child port");
    let ready = PathBuf::from(std::env::var_os(CHILD_READY).expect("child ready path"));
    let release = PathBuf::from(std::env::var_os(CHILD_RELEASE).expect("child release path"));
    let _lease = acquire(&endpoint(port)).expect("child dispatch lease");
    fs::write(ready, b"acquired").expect("child ready marker");
    let deadline = Instant::now() + Duration::from_secs(15);
    while !release.exists() {
        assert!(Instant::now() < deadline, "child release timeout");
        thread::sleep(Duration::from_millis(10));
    }
}

#[derive(Clone, Copy, Debug)]
enum HomeMode {
    Filtered,
    Overridden,
}

fn endpoint(port: u16) -> TallyEndpointConfig {
    TallyEndpointConfig {
        host: "127.0.0.1".into(),
        port,
    }
}

fn reserve_loopback_port() -> std::io::Result<(std::net::TcpListener, u16)> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    Ok((listener, port))
}

fn spawn_child(
    expected_root: &Path,
    data_root: &Path,
    overridden_home: &Path,
    home_mode: HomeMode,
    port: u16,
    ready: &Path,
    release: &Path,
) -> Child {
    let module = module_path!()
        .strip_prefix(concat!(env!("CARGO_CRATE_NAME"), "::"))
        .expect("crate module prefix");
    let mut command = Command::new(std::env::current_exe().expect("test executable"));
    command
        .arg("--exact")
        .arg(format!("{module}::macos_native_dispatch_lease_child"))
        .arg("--nocapture")
        .env(CHILD_EXPECTED_ROOT, expected_root)
        .env(CHILD_DATA_ROOT, data_root)
        .env("BRIDGE_AGENT_DATA_DIR", data_root)
        .env(CHILD_HOME_MODE, home_mode_name(home_mode))
        .env(CHILD_OVERRIDDEN_HOME, overridden_home)
        .env(CHILD_PORT, port.to_string())
        .env(CHILD_READY, ready)
        .env(CHILD_RELEASE, release);
    match home_mode {
        HomeMode::Filtered => {
            command.env_remove("HOME");
        }
        HomeMode::Overridden => {
            command.env("HOME", overridden_home);
        }
    }
    command.spawn().expect("dispatch lease child")
}

fn home_mode_name(home_mode: HomeMode) -> &'static str {
    match home_mode {
        HomeMode::Filtered => "filtered",
        HomeMode::Overridden => "overridden",
    }
}

struct ChildLease {
    child: Child,
    release: PathBuf,
}

impl ChildLease {
    fn new(child: Child, release: PathBuf) -> Self {
        Self { child, release }
    }

    fn wait_for_ready(&mut self, ready: &Path) -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !ready.exists() {
            if let Some(status) = self.child.try_wait().map_err(|error| error.to_string())? {
                return Err(format!("child exited before acquiring the lease: {status}"));
            }
            if Instant::now() >= deadline {
                return Err("child did not acquire the dispatch lease".into());
            }
            thread::sleep(Duration::from_millis(10));
        }
        Ok(())
    }

    fn release(mut self) -> std::io::Result<std::process::ExitStatus> {
        fs::write(&self.release, b"release")?;
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = self.child.try_wait()? {
                return Ok(status);
            }
            if Instant::now() >= deadline {
                let _ = self.child.kill();
                let _ = self.child.wait();
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "child did not exit after release",
                ));
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for ChildLease {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = fs::write(&self.release, b"release");
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}
