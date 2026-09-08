//! Cross-process native-dispatch admission for one local Tally listener.
//!
//! This lease is deliberately outside `BRIDGE_AGENT_DATA_DIR`: two connector
//! processes may keep distinct recoverable journals while still talking to one
//! loopback Tally endpoint. The primary port lock is exclusive for native
//! dispatch, shared for snapshots and short durable observations. Snapshots
//! additionally take a secondary exclusive lock so they remain serialized.
//! A crash drops the kernel leases; the caller's durable local intent and
//! reconciliation remain the recovery boundary.
use crate::local_files::{
    directory::ensure_private_directory,
    file::{lock_error, open_local_file},
    paths::default_dispatch_coordination_dir,
};
use bridge_tally_transport::{canonical_loopback_origin, TallyEndpointConfig};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::path::{Path, PathBuf};

pub(crate) struct EndpointDispatchLease {
    _file: File,
}

pub(crate) struct EndpointSnapshotLease {
    _primary: EndpointObservationLease,
    _snapshot: File,
}

pub(crate) struct EndpointObservationLease {
    _primary: File,
}

/// Acquire the existing nonblocking, exclusive per-user/per-port dispatch lease.
///
/// The endpoint is validated first. Every admitted loopback alias on the same
/// port shares one lease because IPv4, IPv6 and localhost can address the same
/// Tally listener. This is intentionally more conservative than the transport
/// endpoint identity.
pub(crate) fn acquire(endpoint: &TallyEndpointConfig) -> Result<EndpointDispatchLease, String> {
    let root = default_dispatch_coordination_dir()
        .ok_or_else(|| "import_admission_lock_unavailable".to_string())?;
    acquire_at(&root, endpoint)
}

/// Acquire a short shared observation lease. An active native dispatch excludes
/// it; a read-only snapshot does not.
pub(crate) fn acquire_observation(
    endpoint: &TallyEndpointConfig,
) -> Result<EndpointObservationLease, String> {
    let root = default_dispatch_coordination_dir()
        .ok_or_else(|| "import_admission_lock_unavailable".to_string())?;
    acquire_observation_at(&root, endpoint)
}

/// Acquire a serialized read-only snapshot lease: shared on the primary
/// dispatch lane and exclusive on its snapshot-only mutex.
pub(crate) fn acquire_snapshot(
    endpoint: &TallyEndpointConfig,
) -> Result<EndpointSnapshotLease, String> {
    let root = default_dispatch_coordination_dir()
        .ok_or_else(|| "import_admission_lock_unavailable".to_string())?;
    acquire_snapshot_at(&root, endpoint)
}

fn acquire_at(
    coordination_root: &Path,
    endpoint: &TallyEndpointConfig,
) -> Result<EndpointDispatchLease, String> {
    let path = lease_path(coordination_root, endpoint)?;
    let file = open_local_file(&path, true)
        .map_err(|_| "import_admission_lock_unavailable".to_string())?;
    file.try_lock().map_err(lock_error)?;
    Ok(EndpointDispatchLease { _file: file })
}

fn acquire_observation_at(
    coordination_root: &Path,
    endpoint: &TallyEndpointConfig,
) -> Result<EndpointObservationLease, String> {
    let path = lease_path(coordination_root, endpoint)?;
    let file = open_local_file(&path, true)
        .map_err(|_| "import_admission_lock_unavailable".to_string())?;
    file.try_lock_shared().map_err(lock_error)?;
    Ok(EndpointObservationLease { _primary: file })
}

fn acquire_snapshot_at(
    coordination_root: &Path,
    endpoint: &TallyEndpointConfig,
) -> Result<EndpointSnapshotLease, String> {
    let primary = acquire_observation_at(coordination_root, endpoint)?;
    let snapshot_path = lease_path(coordination_root, endpoint)?.with_extension("snapshot.lock");
    let snapshot = open_local_file(&snapshot_path, true)
        .map_err(|_| "import_admission_lock_unavailable".to_string())?;
    snapshot.try_lock().map_err(lock_error)?;
    Ok(EndpointSnapshotLease {
        _primary: primary,
        _snapshot: snapshot,
    })
}

fn lease_path(coordination_root: &Path, endpoint: &TallyEndpointConfig) -> Result<PathBuf, String> {
    // Validate the configured host before intentionally collapsing all loopback
    // aliases for this port into one conservative dispatch lane.
    canonical_loopback_origin(endpoint)
        .map_err(|_| "import_admission_lock_unavailable".to_string())?;
    let directory = coordination_root.join("native-dispatch-leases");
    ensure_private_directory(&directory)
        .map_err(|_| "import_admission_lock_unavailable".to_string())?;
    let mut key = String::with_capacity(64);
    for byte in Sha256::digest(format!("native-dispatch-port:{}", endpoint.port).as_bytes()) {
        use std::fmt::Write as _;
        let _ = write!(key, "{byte:02x}");
    }
    Ok(directory.join(format!("{key}.lock")))
}

#[cfg(test)]
#[path = "endpoint_coordination_tests.rs"]
mod tests;

#[cfg(all(test, target_os = "macos"))]
#[path = "endpoint_coordination_macos_tests.rs"]
mod macos_tests;
