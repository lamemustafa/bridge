use super::*;
use std::ffi::OsString;
use std::fs;
use std::process::{Child, Command};
use std::thread;
use std::time::{Duration, Instant};

const CHILD_MODE: &str = "BRIDGE_DISPATCH_LEASE_TEST_MODE";
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
fn snapshots_share_observation_lane_but_exclude_writers_and_each_other() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let endpoint = endpoint("127.0.0.1", 9001);
    let snapshot = acquire_snapshot_at(root, &endpoint).unwrap();
    let observation = acquire_observation_at(root, &endpoint).unwrap();
    assert_eq!(
        acquire_at(root, &endpoint).err().as_deref(),
        Some("import_admission_busy")
    );
    assert_eq!(
        acquire_snapshot_at(root, &endpoint).err().as_deref(),
        Some("import_admission_busy")
    );
    drop(observation);
    drop(snapshot);

    let writer = acquire_at(root, &endpoint).unwrap();
    assert_eq!(
        acquire_observation_at(root, &endpoint).err().as_deref(),
        Some("import_admission_busy")
    );
    assert_eq!(
        acquire_snapshot_at(root, &endpoint).err().as_deref(),
        Some("import_admission_busy")
    );
    drop(writer);
    let _reacquired = acquire_snapshot_at(root, &endpoint).unwrap();
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
    let endpoint = endpoint("127.0.0.1", 9001);
    for mode in ["dispatch", "snapshot"] {
        let ready = directory.path().join(format!("{mode}-ready"));
        let release = directory.path().join(format!("{mode}-release"));
        let child = ChildLease::new(
            spawn_child(
                &coordination_root,
                endpoint.port,
                &ready,
                &release,
                second_data_root.clone().into_os_string(),
                mode,
            ),
            release.clone(),
        );
        wait_for(&ready).unwrap();
        assert_eq!(
            acquire_at(&coordination_root, &endpoint).err().as_deref(),
            Some("import_admission_busy")
        );
        assert_eq!(
            acquire_snapshot_at(&coordination_root, &endpoint)
                .err()
                .as_deref(),
            Some("import_admission_busy")
        );
        let observation = acquire_observation_at(&coordination_root, &endpoint);
        if mode == "snapshot" {
            drop(observation.expect("snapshot permits durable observation"));
        } else {
            assert_eq!(observation.err().as_deref(), Some("import_admission_busy"));
        }
        assert!(child.release().unwrap().success());
        drop(acquire_at(&coordination_root, &endpoint).unwrap());
    }
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
    let mode = std::env::var(CHILD_MODE).expect("child lease mode");
    assert!(matches!(mode.as_str(), "dispatch" | "snapshot"));
    let root = PathBuf::from(root);
    let endpoint = endpoint("::1", port);
    let _dispatch =
        (mode == "dispatch").then(|| acquire_at(&root, &endpoint).expect("child dispatch lease"));
    let _snapshot = (mode == "snapshot")
        .then(|| acquire_snapshot_at(&root, &endpoint).expect("child snapshot lease"));
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
    mode: &str,
) -> Child {
    let module = module_path!()
        .strip_prefix(concat!(env!("CARGO_CRATE_NAME"), "::"))
        .expect("crate module prefix");
    Command::new(std::env::current_exe().expect("test executable"))
        .arg("--exact")
        .arg(format!("{module}::dispatch_lease_child"))
        .arg("--nocapture")
        .env(CHILD_MODE, mode)
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
