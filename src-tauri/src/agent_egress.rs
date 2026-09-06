//! Locked append-only egress receipts and bounded tail reads.
use fs2::FileExt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

pub(super) const EGRESS_TAIL_CHUNK_BYTES: usize = 64 * 1024;
const MAX_EGRESS_TAIL_BYTES: usize = 256 * 1024;

pub(super) fn canonical_company_guid(value: &str) -> Option<String> {
    uuid::Uuid::parse_str(value)
        .ok()
        .map(|guid| guid.hyphenated().to_string())
}

pub(super) fn append_egress_line(path: &Path, line: &str) -> Result<(), String> {
    // Leave room for the preceding newline in a bounded reverse tail read.
    if line.len().saturating_add(1) >= MAX_EGRESS_TAIL_BYTES {
        return Err("egress_record_too_large".to_string());
    }
    let mut options = OpenOptions::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(path)
        .map_err(|_| "egress_record_write_failed".to_string())?;
    // Read/write access permits LockFileEx and rollback truncation on Windows.
    // Every append seeks to EOF while holding this exclusive lock.
    file.lock_exclusive()
        .map_err(|_| "egress_record_write_failed".to_string())?;
    let write_result = (|| {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))
                .map_err(|_| "egress_record_write_failed".to_string())?;
        }
        append_locked(&mut file, line, |file, bytes| {
            file.write_all(bytes)?;
            file.sync_data()
        })
    })();
    let unlock_result = file
        .unlock()
        .map_err(|_| "egress_record_write_failed".to_string());
    write_result.and(unlock_result)
}

fn append_locked(
    file: &mut File,
    line: &str,
    append_and_sync: impl FnOnce(&mut File, &[u8]) -> std::io::Result<()>,
) -> Result<(), String> {
    let original_length = file
        .metadata()
        .map_err(|_| "egress_record_write_failed".to_string())?
        .len();
    if original_length > 0 {
        file.seek(SeekFrom::End(-1))
            .map_err(|_| "egress_record_write_failed".to_string())?;
        let mut final_byte = [0];
        file.read_exact(&mut final_byte)
            .map_err(|_| "egress_record_write_failed".to_string())?;
        if final_byte != [b'\n'] {
            return Err("egress_log_incomplete".to_string());
        }
    }
    file.seek(SeekFrom::Start(original_length))
        .map_err(|_| "egress_record_write_failed".to_string())?;
    if append_and_sync(file, format!("{line}\n").as_bytes()).is_err() {
        file.set_len(original_length)
            .and_then(|_| file.sync_data())
            .map_err(|_| "egress_record_rollback_failed".to_string())?;
        return Err("egress_record_write_failed".to_string());
    }
    Ok(())
}

pub(super) fn read_egress_tail(path: &Path, take: usize) -> Result<Vec<String>, String> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err("egress_log_unreadable".to_string()),
    };
    file.lock_shared()
        .map_err(|_| "egress_log_unreadable".to_string())?;
    let file_len = file
        .metadata()
        .map_err(|_| "egress_log_unreadable".to_string())?
        .len();
    let mut position = file_len;
    let mut scanned = 0_usize;
    let mut newlines = 0_usize;
    let mut chunks = Vec::new();
    while position > 0 && scanned < MAX_EGRESS_TAIL_BYTES && newlines < take.saturating_add(1) {
        let chunk_size = EGRESS_TAIL_CHUNK_BYTES
            .min(MAX_EGRESS_TAIL_BYTES - scanned)
            .min(position as usize);
        position -= chunk_size as u64;
        file.seek(SeekFrom::Start(position))
            .map_err(|_| "egress_log_unreadable".to_string())?;
        let mut chunk = vec![0_u8; chunk_size];
        file.read_exact(&mut chunk)
            .map_err(|_| "egress_log_unreadable".to_string())?;
        newlines += chunk.iter().filter(|byte| **byte == b'\n').count();
        scanned += chunk_size;
        chunks.push(chunk);
    }
    chunks.reverse();
    let bytes = chunks.concat();
    let complete_lines = if position > 0 {
        // The discarded leading row can begin inside a UTF-8 code point.
        bytes.splitn(2, |byte| *byte == b'\n').nth(1).unwrap_or(&[])
    } else {
        &bytes
    };
    let text =
        std::str::from_utf8(complete_lines).map_err(|_| "egress_log_unreadable".to_string())?;
    let lines = text.lines().rev().take(take).map(str::to_string).collect();
    file.unlock()
        .map_err(|_| "egress_log_unreadable".to_string())?;
    Ok(lines)
}

#[cfg(test)]
#[path = "agent_egress_tests.rs"]
mod tests;
