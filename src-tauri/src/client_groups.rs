// SPDX-License-Identifier: Apache-2.0

//! Operator-owned company filing labels and all-client sort preference.
//!
//! These labels deliberately live in the ordinary application configuration
//! directory, not in the encrypted Tally mirror. They are an operator's filing
//! choice rather than accounting data. They contain neither accounting figures
//! nor mirror state, and reading them must never resolve the mirror key or
//! prompt the operating-system keychain.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

const FILE_NAME: &str = "client-group-labels-v1.json";
const SCHEMA_VERSION: u8 = 1;
const SORT_FILE_NAME: &str = "client-sort-preference-v1.json";
const SORT_SCHEMA_VERSION: u8 = 1;

pub type ClientGroupLabels = BTreeMap<String, String>;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClientSortKey {
    Client,
    Receivable,
    Overdue,
    Unallocated,
    Oldest,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ClientSortPreference {
    pub key: ClientSortKey,
    pub desc: bool,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ClientGroupLabelsFile {
    version: u8,
    labels: ClientGroupLabels,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ClientSortPreferenceFile {
    version: u8,
    sort: ClientSortPreference,
}

#[derive(Debug, thiserror::Error)]
pub enum ClientGroupLabelsError {
    #[error("client group label file could not be read")]
    Read(#[source] std::io::Error),
    #[error("client group label file is empty")]
    EmptyFile,
    #[error("client group label file is corrupt")]
    CorruptFile(#[source] serde_json::Error),
    #[error("client group label schema version {found} is unsupported; expected {supported}")]
    UnsupportedVersion { found: u8, supported: u8 },
    #[error("client group label file could not be written")]
    Write(#[source] std::io::Error),
}

pub fn load(directory: &Path) -> ClientGroupLabels {
    try_load(directory).unwrap_or_default()
}

pub fn load_sort_preference(directory: &Path) -> Option<ClientSortPreference> {
    let contents = std::fs::read_to_string(directory.join(SORT_FILE_NAME)).ok()?;
    if contents.trim().is_empty() {
        return None;
    }
    let file = serde_json::from_str::<ClientSortPreferenceFile>(&contents).ok()?;
    (file.version == SORT_SCHEMA_VERSION).then_some(file.sort)
}

fn try_load(directory: &Path) -> Result<ClientGroupLabels, ClientGroupLabelsError> {
    let contents = match std::fs::read_to_string(directory.join(FILE_NAME)) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ClientGroupLabels::new())
        }
        Err(error) => return Err(ClientGroupLabelsError::Read(error)),
    };
    if contents.trim().is_empty() {
        return Err(ClientGroupLabelsError::EmptyFile);
    }

    let file = serde_json::from_str::<ClientGroupLabelsFile>(&contents)
        .map_err(ClientGroupLabelsError::CorruptFile)?;
    if file.version != SCHEMA_VERSION {
        return Err(ClientGroupLabelsError::UnsupportedVersion {
            found: file.version,
            supported: SCHEMA_VERSION,
        });
    }

    Ok(normalize(file.labels))
}

pub fn save_label(
    directory: &Path,
    company_key: &str,
    label: &str,
) -> Result<(), ClientGroupLabelsError> {
    let mut labels = try_load(directory)?;
    let company_key = company_key.trim();
    let label = label.trim();
    if label.is_empty() {
        labels.remove(company_key);
    } else {
        labels.insert(company_key.to_string(), label.to_string());
    }

    replace_labels(directory, labels)
}

pub fn replace_labels(
    directory: &Path,
    labels: ClientGroupLabels,
) -> Result<(), ClientGroupLabelsError> {
    let contents = serde_json::to_vec_pretty(&ClientGroupLabelsFile {
        version: SCHEMA_VERSION,
        labels: normalize(labels),
    })
    .expect("client group label schema always serializes");
    save_bytes(directory, FILE_NAME, &contents).map_err(ClientGroupLabelsError::Write)
}

pub fn save_sort_preference(
    directory: &Path,
    sort: ClientSortPreference,
) -> Result<(), std::io::Error> {
    let contents = serde_json::to_vec_pretty(&ClientSortPreferenceFile {
        version: SORT_SCHEMA_VERSION,
        sort,
    })
    .expect("client sort preference schema always serializes");
    save_bytes(directory, SORT_FILE_NAME, &contents)
}

fn save_bytes(directory: &Path, file_name: &str, contents: &[u8]) -> Result<(), std::io::Error> {
    std::fs::create_dir_all(directory)?;
    let path = directory.join(file_name);
    write_file_atomically(&path, contents)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// Replacing the label file is a single rename after a fully written sibling
/// exists. A process interruption can therefore retain the previous labels or
/// the complete next labels, never a truncated JSON file that loads as none.
fn write_file_atomically(path: &Path, contents: &[u8]) -> Result<(), std::io::Error> {
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("missing_label_parent"))?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| std::io::Error::other("invalid_label_file_name"))?;
    for sequence in 1..=10_000_u32 {
        let temporary = temporary_label_path(parent, file_name, sequence);
        let mut file = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        };
        if let Err(error) = file.write_all(contents).and_then(|_| file.sync_all()) {
            drop(file);
            let _ = std::fs::remove_file(&temporary);
            return Err(error);
        }
        drop(file);
        if let Err(error) = replace_file(&temporary, path) {
            let _ = std::fs::remove_file(&temporary);
            return Err(error);
        }
        return Ok(());
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "label_temporary_name_exhausted",
    ))
}

#[cfg(not(windows))]
fn replace_file(temporary: &Path, destination: &Path) -> Result<(), std::io::Error> {
    std::fs::rename(temporary, destination)
}

#[cfg(windows)]
fn replace_file(temporary: &Path, destination: &Path) -> Result<(), std::io::Error> {
    use std::os::windows::ffi::OsStrExt as _;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let temporary = temporary
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    // SAFETY: both buffers are NUL-terminated and remain alive for the call.
    // The source is a newly created sibling owned by this process. The flags
    // provide Windows' replace-existing behavior and request durable metadata.
    let replaced = unsafe {
        MoveFileExW(
            temporary.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if replaced == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn temporary_label_path(parent: &Path, file_name: &str, sequence: u32) -> PathBuf {
    parent.join(format!(
        ".{file_name}.{}.{}.tmp",
        std::process::id(),
        sequence
    ))
}

fn normalize(labels: ClientGroupLabels) -> ClientGroupLabels {
    labels
        .into_iter()
        .filter_map(|(company_guid, label)| {
            let company_guid = company_guid.trim();
            let label = label.trim();
            (!company_guid.is_empty() && !label.is_empty())
                .then(|| (company_guid.to_string(), label.to_string()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_empty_and_corrupt_files_mean_no_groups() {
        let directory = tempfile::tempdir().expect("temporary config directory");
        assert!(load(directory.path()).is_empty());

        std::fs::write(directory.path().join(FILE_NAME), "  \n").expect("write empty file");
        assert!(load(directory.path()).is_empty());

        std::fs::write(directory.path().join(FILE_NAME), "not json").expect("write corrupt file");
        assert!(load(directory.path()).is_empty());
    }

    #[test]
    fn labels_survive_a_reload_and_blank_labels_remove_the_group() {
        let directory = tempfile::tempdir().expect("temporary config directory");
        save_label(directory.path(), "synthetic-company-guid", "North practice")
            .expect("save label");
        assert_eq!(
            load(directory.path()).get("synthetic-company-guid"),
            Some(&"North practice".to_string())
        );

        save_label(directory.path(), "synthetic-company-guid", "   ").expect("remove label");
        assert!(load(directory.path()).is_empty());
    }

    #[test]
    fn replacing_labels_atomically_retires_every_legacy_key() {
        let directory = tempfile::tempdir().expect("temporary config directory");
        save_label(directory.path(), "legacy-guid", "Legacy practice").expect("seed legacy");
        replace_labels(
            directory.path(),
            BTreeMap::from([(
                "[\"http://127.0.0.1:9000\",\"legacy-guid\",\"100001\",\"Client\",\"20260401\"]"
                    .to_string(),
                "Composite practice".to_string(),
            )]),
        )
        .expect("atomically replace labels");

        let labels = load(directory.path());
        assert!(!labels.contains_key("legacy-guid"));
        assert_eq!(labels.len(), 1);
    }

    #[test]
    fn replacement_writes_leave_only_a_complete_current_label_file() {
        let directory = tempfile::tempdir().expect("temporary config directory");
        save_label(directory.path(), "synthetic-company-guid", "North practice")
            .expect("first label");
        save_label(directory.path(), "synthetic-company-guid", "South practice")
            .expect("replacement label");

        assert_eq!(
            load(directory.path()).get("synthetic-company-guid"),
            Some(&"South practice".to_string())
        );
        assert!(directory
            .path()
            .read_dir()
            .expect("directory entries")
            .all(|entry| !entry
                .expect("directory entry")
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp")));
    }

    #[test]
    fn save_refuses_to_overwrite_a_corrupt_existing_label_file() {
        let directory = tempfile::tempdir().expect("temporary config directory");
        let path = directory.path().join(FILE_NAME);
        let original = b"{not valid json";
        std::fs::write(&path, original).expect("write corrupt label bytes");

        assert!(
            load(directory.path()).is_empty(),
            "display remains available"
        );
        assert!(matches!(
            save_label(directory.path(), "new-synthetic-guid", "New practice"),
            Err(ClientGroupLabelsError::CorruptFile(_))
        ));
        assert_eq!(
            std::fs::read(path).expect("read preserved label bytes"),
            original
        );
    }

    #[test]
    fn save_refuses_to_overwrite_an_unsupported_label_schema() {
        let directory = tempfile::tempdir().expect("temporary config directory");
        let path = directory.path().join(FILE_NAME);
        let original = br#"{"version":2,"labels":{"existing-guid":"Existing practice"}}"#;
        std::fs::write(&path, original).expect("write future label schema");

        assert!(
            load(directory.path()).is_empty(),
            "display remains available"
        );
        assert!(matches!(
            save_label(directory.path(), "new-synthetic-guid", "New practice"),
            Err(ClientGroupLabelsError::UnsupportedVersion {
                found: 2,
                supported: SCHEMA_VERSION
            })
        ));
        assert_eq!(
            std::fs::read(path).expect("read preserved label bytes"),
            original
        );
    }

    #[test]
    fn save_refuses_empty_or_unreadable_existing_label_paths() {
        let empty_directory = tempfile::tempdir().expect("temporary config directory");
        let empty_path = empty_directory.path().join(FILE_NAME);
        std::fs::write(&empty_path, b" \n").expect("write empty label file");
        assert!(load(empty_directory.path()).is_empty());
        assert!(matches!(
            save_label(empty_directory.path(), "new-synthetic-guid", "New practice"),
            Err(ClientGroupLabelsError::EmptyFile)
        ));
        assert_eq!(std::fs::read(empty_path).expect("read empty bytes"), b" \n");

        let unreadable_directory = tempfile::tempdir().expect("temporary config directory");
        let unreadable_path = unreadable_directory.path().join(FILE_NAME);
        std::fs::create_dir(&unreadable_path).expect("create unreadable label path");
        assert!(load(unreadable_directory.path()).is_empty());
        assert!(matches!(
            save_label(
                unreadable_directory.path(),
                "new-synthetic-guid",
                "New practice"
            ),
            Err(ClientGroupLabelsError::Read(_))
        ));
        assert!(unreadable_path.is_dir());
    }

    #[test]
    fn sort_preference_survives_a_reload_without_changing_group_labels() {
        let directory = tempfile::tempdir().expect("temporary config directory");
        save_label(directory.path(), "synthetic-company-guid", "North practice")
            .expect("save label");
        let sort = ClientSortPreference {
            key: ClientSortKey::Unallocated,
            desc: false,
        };

        save_sort_preference(directory.path(), sort.clone()).expect("save sort preference");

        assert_eq!(load_sort_preference(directory.path()), Some(sort));
        assert_eq!(
            load(directory.path()).get("synthetic-company-guid"),
            Some(&"North practice".to_string())
        );
    }

    #[test]
    fn replacement_sort_write_keeps_only_the_latest_complete_preference() {
        let directory = tempfile::tempdir().expect("temporary config directory");
        save_sort_preference(
            directory.path(),
            ClientSortPreference {
                key: ClientSortKey::Overdue,
                desc: true,
            },
        )
        .expect("first sort preference");
        let latest = ClientSortPreference {
            key: ClientSortKey::Client,
            desc: false,
        };
        save_sort_preference(directory.path(), latest.clone()).expect("replacement preference");

        assert_eq!(load_sort_preference(directory.path()), Some(latest));
        assert!(directory
            .path()
            .read_dir()
            .expect("directory entries")
            .all(|entry| !entry
                .expect("directory entry")
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp")));
    }

    #[test]
    fn sort_storage_remains_readable_by_the_old_v1_label_contract() {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct OldClientGroupLabelsFile {
            version: u8,
            labels: ClientGroupLabels,
        }

        let directory = tempfile::tempdir().expect("temporary config directory");
        save_label(directory.path(), "existing-guid", "Existing practice")
            .expect("save existing label");
        save_sort_preference(
            directory.path(),
            ClientSortPreference {
                key: ClientSortKey::Oldest,
                desc: true,
            },
        )
        .expect("save new sort preference");

        let raw_labels = std::fs::read(directory.path().join(FILE_NAME))
            .expect("read label bytes after new writer");
        let old_reader: OldClientGroupLabelsFile = serde_json::from_slice(&raw_labels)
            .expect("the previous release must still parse the label file after rollback");
        assert_eq!(old_reader.version, SCHEMA_VERSION);
        assert_eq!(
            old_reader.labels.get("existing-guid"),
            Some(&"Existing practice".to_string())
        );
    }
}
