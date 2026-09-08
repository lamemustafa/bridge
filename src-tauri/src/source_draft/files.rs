use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{
    local_files::file::open_local_file,
    source_draft_xml::{parse_source_xml, ParsedSource, MAX_SOURCE_BYTES},
};

use super::types::{
    command_error, error, CommandResult, SourceDraftProposal, MAX_DRAFT_BYTES, MAX_TEXT_BYTES,
};

pub(super) async fn pick_file(
    title: &'static str,
    extensions: &'static [&'static str],
    max_bytes: usize,
) -> CommandResult<Option<(String, Vec<u8>)>> {
    tokio::task::spawn_blocking(move || {
        let Some(path) = rfd::FileDialog::new()
            .set_title(title)
            .add_filter("Bridge source draft", extensions)
            .pick_file()
        else {
            return Ok(None);
        };
        let filename = path
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty() && name.len() <= MAX_TEXT_BYTES)
            .map(str::to_owned)
            .ok_or_else(|| error("source_draft_filename_invalid"))?;
        Ok(Some((filename, read_regular_file(&path, max_bytes)?)))
    })
    .await
    .map_err(|_| error("source_draft_file_picker_failed"))?
}

pub(super) async fn save_path() -> CommandResult<Option<PathBuf>> {
    tokio::task::spawn_blocking(|| {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Save Bridge source draft")
            .add_filter("Bridge source draft", &["json"])
            .set_file_name("source.bridge-draft.json")
            .save_file()
        else {
            return Ok(None);
        };
        validate_save_destination(&path)?;
        Ok(Some(path))
    })
    .await
    .map_err(|_| error("source_draft_file_picker_failed"))?
}

pub(super) fn serialize_draft(
    source: &ParsedSource,
    proposals: &[SourceDraftProposal],
) -> CommandResult<Vec<u8>> {
    let bytes = serde_json::to_vec(&StoredDraft {
        version: 1,
        source_filename: source.filename.clone(),
        source_sha256: source.sha256.clone(),
        original_source_utf8: source.utf8.clone(),
        proposals: proposals.to_vec(),
    })
    .map_err(|_| error("source_draft_serialization_failed"))?;
    if bytes.len() > MAX_DRAFT_BYTES {
        Err(error("source_draft_file_too_large"))
    } else {
        Ok(bytes)
    }
}

pub(super) fn open_saved_draft(
    bytes: Vec<u8>,
) -> CommandResult<(ParsedSource, Vec<SourceDraftProposal>)> {
    let saved: StoredDraft =
        serde_json::from_slice(&bytes).map_err(|_| error("source_draft_saved_json_invalid"))?;
    if saved.version != 1
        || saved.original_source_utf8.len() > MAX_SOURCE_BYTES
        || saved.source_filename.len() > MAX_TEXT_BYTES
    {
        return Err(error("source_draft_saved_json_invalid"));
    }
    let source = parse_source_xml(saved.original_source_utf8.as_bytes(), saved.source_filename)
        .map_err(command_error)?;
    if source.sha256 != saved.source_sha256 {
        return Err(error("source_draft_saved_source_hash_mismatch"));
    }
    Ok((source, saved.proposals))
}

pub(super) fn write_private_file(path: &Path, bytes: &[u8]) -> CommandResult<()> {
    let parent = path
        .parent()
        .ok_or_else(|| error("source_draft_destination_invalid"))?;
    if fs::symlink_metadata(path).is_ok() {
        // Existing save targets are allowed only after the local-file helper has
        // refused aliases, reparse points, directories, and hard links.
        open_local_file(path, true).map_err(|_| error("source_draft_destination_unavailable"))?;
    }
    let mut temp = tempfile::NamedTempFile::new_in(parent)
        .map_err(|_| error("source_draft_destination_unavailable"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temp.as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|_| error("source_draft_destination_unavailable"))?;
    }
    temp.write_all(bytes)
        .and_then(|_| temp.as_file().sync_all())
        .map_err(|_| error("source_draft_destination_unavailable"))?;
    if fs::symlink_metadata(path).is_ok() {
        temp.persist(path)
            .map_err(|_| error("source_draft_destination_unavailable"))?;
    } else {
        temp.persist_noclobber(path)
            .map_err(|_| error("source_draft_destination_unavailable"))?;
    }
    Ok(())
}

pub(super) fn validate_save_destination(path: &Path) -> CommandResult<()> {
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| error("source_draft_filename_invalid"))?;
    if filename.ends_with(".bridge-draft.json") {
        Ok(())
    } else {
        Err(error("source_draft_extension_invalid"))
    }
}

fn read_regular_file(path: &Path, max_bytes: usize) -> CommandResult<Vec<u8>> {
    let mut file =
        open_local_file(path, false).map_err(|_| error("source_draft_local_file_unavailable"))?;
    let length = file
        .metadata()
        .map_err(|_| error("source_draft_local_file_unavailable"))?
        .len();
    if length > max_bytes as u64 {
        return Err(error("source_draft_file_too_large"));
    }
    let mut bytes = Vec::with_capacity(length as usize);
    Read::by_ref(&mut file)
        .take((max_bytes + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| error("source_draft_local_file_unavailable"))?;
    if bytes.len() > max_bytes {
        return Err(error("source_draft_file_too_large"));
    }
    Ok(bytes)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StoredDraft {
    version: u8,
    source_filename: String,
    source_sha256: String,
    original_source_utf8: String,
    proposals: Vec<SourceDraftProposal>,
}
