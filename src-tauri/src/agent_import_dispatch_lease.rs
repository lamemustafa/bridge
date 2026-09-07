//! Cross-process native-dispatch admission for one local Tally listener.
//!
//! This lease is deliberately outside `BRIDGE_AGENT_DATA_DIR`: two connector
//! processes may keep distinct recoverable journals while still talking to one
//! loopback Tally endpoint. It serializes active dispatches only. A crash drops
//! the kernel lease; the caller's durable local intent and reconciliation remain
//! the recovery boundary.
use bridge_tally_transport::{canonical_loopback_origin, TallyEndpointConfig};
use std::fs::File;
use std::path::{Path, PathBuf};

pub(super) struct EndpointDispatchLease {
    _file: File,
}

/// Acquire a nonblocking, per-user/per-port lease for a native dispatch.
///
/// The endpoint is validated first. Every admitted loopback alias on the same
/// port shares one lease because IPv4, IPv6 and localhost can address the same
/// Tally listener. This is intentionally more conservative than the transport
/// endpoint identity.
pub(super) fn acquire(endpoint: &TallyEndpointConfig) -> Result<EndpointDispatchLease, String> {
    let root = super::super::default_dispatch_coordination_dir()
        .ok_or_else(|| "import_admission_lock_unavailable".to_string())?;
    acquire_at(&root, endpoint)
}

fn acquire_at(
    coordination_root: &Path,
    endpoint: &TallyEndpointConfig,
) -> Result<EndpointDispatchLease, String> {
    let path = lease_path(coordination_root, endpoint)?;
    let file = super::super::local_file::open_local_file(&path, true)
        .map_err(|_| "import_admission_lock_unavailable".to_string())?;
    file.try_lock()
        .map_err(super::import_admission_lock_error)?;
    Ok(EndpointDispatchLease { _file: file })
}

fn lease_path(coordination_root: &Path, endpoint: &TallyEndpointConfig) -> Result<PathBuf, String> {
    // Validate the configured host before intentionally collapsing all loopback
    // aliases for this port into one conservative dispatch lane.
    canonical_loopback_origin(endpoint)
        .map_err(|_| "import_admission_lock_unavailable".to_string())?;
    let directory = coordination_root.join("native-dispatch-leases");
    super::super::ensure_private_directory(&directory)
        .map_err(|_| "import_admission_lock_unavailable".to_string())?;
    let key =
        super::super::sha256_hex(format!("native-dispatch-port:{}", endpoint.port).as_bytes());
    Ok(directory.join(format!("{key}.lock")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::fs;
    use std::process::{Child, Command};
    use std::thread;
    use std::time::{Duration, Instant};

    const CHILD_ROOT: &str = "BRIDGE_DISPATCH_LEASE_TEST_ROOT";
    const CHILD_PORT: &str = "BRIDGE_DISPATCH_LEASE_TEST_PORT";
    const CHILD_READY: &str = "BRIDGE_DISPATCH_LEASE_TEST_READY";
    const CHILD_RELEASE: &str = "BRIDGE_DISPATCH_LEASE_TEST_RELEASE";

    fn endpoint(host: &str, port: u16) -> TallyEndpointConfig {
        TallyEndpointConfig {
            host: host.into(),
            port,
        }
    }

    #[test]
    fn loopback_aliases_share_a_port_lease_and_ports_remain_independent() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let local = lease_path(root, &endpoint("localhost", 9001)).unwrap();
        for host in ["127.0.0.1", "127.0.0.2", "::1"] {
            assert_eq!(lease_path(root, &endpoint(host, 9001)).unwrap(), local);
        }
        let other_port = lease_path(root, &endpoint("127.0.0.1", 9002)).unwrap();
        assert_ne!(local, other_port);
        let _first = acquire_at(root, &endpoint("localhost", 9001)).unwrap();
        let _second = acquire_at(root, &endpoint("::1", 9002)).unwrap();
    }

    #[test]
    fn unsafe_lease_leaf_is_refused() {
        let directory = tempfile::tempdir().unwrap();
        let endpoint = endpoint("127.0.0.1", 9001);
        let path = lease_path(directory.path(), &endpoint).unwrap();
        fs::create_dir(&path).unwrap();
        assert_eq!(
            acquire_at(directory.path(), &endpoint).err().as_deref(),
            Some("import_admission_lock_unavailable")
        );
    }

    #[test]
    fn separate_processes_with_distinct_data_roots_contend_then_reacquire() {
        let directory = tempfile::tempdir().unwrap();
        let coordination_root = directory.path().join("coordination");
        let first_data_root = directory.path().join("data-a");
        let second_data_root = directory.path().join("data-b");
        fs::create_dir(&first_data_root).unwrap();
        fs::create_dir(&second_data_root).unwrap();
        let ready = directory.path().join("ready");
        let release = directory.path().join("release");
        let endpoint = endpoint("127.0.0.1", 9001);
        let child = ChildLease::new(
            spawn_child(
                &coordination_root,
                endpoint.port,
                &ready,
                &release,
                second_data_root.into_os_string(),
            ),
            release.clone(),
        );
        wait_for(&ready).unwrap();
        assert_eq!(
            acquire_at(&coordination_root, &endpoint).err().as_deref(),
            Some("import_admission_busy")
        );
        assert!(child.release().unwrap().success());
        let _reacquired = acquire_at(&coordination_root, &endpoint).unwrap();
        assert!(first_data_root.is_dir());
    }

    #[test]
    fn dispatch_lease_child() {
        let Some(root) = std::env::var_os(CHILD_ROOT) else {
            return;
        };
        let port = std::env::var(CHILD_PORT)
            .ok()
            .and_then(|value| value.parse().ok())
            .expect("child port");
        let ready = PathBuf::from(std::env::var_os(CHILD_READY).expect("child ready path"));
        let release = PathBuf::from(std::env::var_os(CHILD_RELEASE).expect("child release path"));
        let _lease =
            acquire_at(&PathBuf::from(root), &endpoint("::1", port)).expect("child dispatch lease");
        fs::write(ready, b"ready").expect("child ready");
        let deadline = Instant::now() + Duration::from_secs(15);
        while !release.exists() {
            assert!(Instant::now() < deadline, "child release timeout");
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn spawn_child(
        coordination_root: &Path,
        port: u16,
        ready: &Path,
        release: &Path,
        data_root: OsString,
    ) -> Child {
        let module = module_path!()
            .strip_prefix(concat!(env!("CARGO_CRATE_NAME"), "::"))
            .expect("crate module prefix");
        Command::new(std::env::current_exe().expect("test executable"))
            .arg("--exact")
            .arg(format!("{module}::dispatch_lease_child"))
            .arg("--nocapture")
            .env(CHILD_ROOT, coordination_root)
            .env(CHILD_PORT, port.to_string())
            .env(CHILD_READY, ready)
            .env(CHILD_RELEASE, release)
            // This process deliberately has a different recoverable data root;
            // acquire_at uses only the shared coordination root.
            .env("BRIDGE_AGENT_DATA_DIR", data_root)
            .spawn()
            .expect("dispatch lease child")
    }

    struct ChildLease {
        child: Child,
        release: PathBuf,
    }

    impl ChildLease {
        fn new(child: Child, release: PathBuf) -> Self {
            Self { child, release }
        }

        fn release(mut self) -> std::io::Result<std::process::ExitStatus> {
            fs::write(&self.release, b"release")?;
            self.child.wait()
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

    fn wait_for(path: &Path) -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !path.exists() {
            if Instant::now() >= deadline {
                return Err("child did not acquire dispatch lease".into());
            }
            thread::sleep(Duration::from_millis(10));
        }
        Ok(())
    }
}
