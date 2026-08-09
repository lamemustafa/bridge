// SPDX-License-Identifier: Apache-2.0

//! Operator-owned company filing labels.
//!
//! These labels deliberately live in the ordinary application configuration
//! directory, not in the encrypted Tally mirror. They are an operator's filing
//! choice rather than accounting data, and reading them must never resolve the
//! mirror key or prompt the operating-system keychain.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

const FILE_NAME: &str = "client-group-labels-v1.json";
const SCHEMA_VERSION: u8 = 1;

pub type ClientGroupLabels = BTreeMap<String, String>;

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ClientGroupLabelsFile {
    version: u8,
    labels: ClientGroupLabels,
}

pub fn load(directory: &Path) -> ClientGroupLabels {
    let Ok(contents) = std::fs::read_to_string(directory.join(FILE_NAME)) else {
        return ClientGroupLabels::new();
    };
    if contents.trim().is_empty() {
        return ClientGroupLabels::new();
    }

    let Ok(file) = serde_json::from_str::<ClientGroupLabelsFile>(&contents) else {
        return ClientGroupLabels::new();
    };
    if file.version != SCHEMA_VERSION {
        return ClientGroupLabels::new();
    }

    normalize(file.labels)
}

pub fn save_label(directory: &Path, company_guid: &str, label: &str) -> Result<(), std::io::Error> {
    let mut labels = load(directory);
    let company_guid = company_guid.trim();
    let label = label.trim();
    if label.is_empty() {
        labels.remove(company_guid);
    } else {
        labels.insert(company_guid.to_string(), label.to_string());
    }

    std::fs::create_dir_all(directory)?;
    let path = directory.join(FILE_NAME);
    let contents = serde_json::to_vec_pretty(&ClientGroupLabelsFile {
        version: SCHEMA_VERSION,
        labels,
    })
    .expect("client group label schema always serializes");
    write_file_atomically(&path, &contents)?;
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
        if let Err(error) = std::fs::rename(&temporary, path) {
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
}
