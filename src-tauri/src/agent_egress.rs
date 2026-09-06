//! Locked append-only egress receipts and bounded tail reads.
use fs2::FileExt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

pub(super) const EGRESS_TAIL_CHUNK_BYTES: usize = 64 * 1024;
const MAX_EGRESS_TAIL_BYTES: usize = 256 * 1024;

pub(super) fn append_egress_line(path: &Path, line: &str) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        // Windows LockFileEx requires GENERIC_READ or GENERIC_WRITE. Rust's
        // append-only handle has neither; read access supplies GENERIC_READ
        // while preserving append-only writes. See Microsoft's LockFileEx docs:
        // https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-lockfileex
        .read(true)
        .open(path)
        .map_err(|_| "egress_record_write_failed".to_string())?;
    file.lock_exclusive()
        .map_err(|_| "egress_record_write_failed".to_string())?;
    let write_result = (|| {
        file.write_all(line.as_bytes())
            .map_err(|_| "egress_record_write_failed".to_string())?;
        file.write_all(b"\n")
            .map_err(|_| "egress_record_write_failed".to_string())?;
        file.sync_data()
            .map_err(|_| "egress_record_write_failed".to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))
                .map_err(|_| "egress_record_write_failed".to_string())?;
        }
        Ok(())
    })();
    let unlock_result = file
        .unlock()
        .map_err(|_| "egress_record_write_failed".to_string());
    write_result.and(unlock_result)
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
mod tests {
    use super::*;

    #[test]
    fn tail_discards_a_partial_unicode_line_before_decoding() {
        let directory = tempfile::tempdir().expect("temporary egress directory");
        let path = directory.path().join("agent-egress.jsonl");
        let body = format!(
            "{{\"tool\":\"{}\"}}\n{{\"tool\":\"new\"}}\n",
            "古".repeat(EGRESS_TAIL_CHUNK_BYTES)
        );
        assert!(!body.is_char_boundary(body.len() - EGRESS_TAIL_CHUNK_BYTES));
        fs::write(&path, body).expect("Unicode receipt followed by recent receipt");
        assert_eq!(
            read_egress_tail(&path, 1).expect("latest complete receipt"),
            [r#"{"tool":"new"}"#]
        );
    }

    #[test]
    fn locked_append_preserves_existing_receipts_and_adds_a_complete_line() {
        let directory = tempfile::tempdir().expect("temporary egress directory");
        let path = directory.path().join("agent-egress.jsonl");
        fs::write(&path, b"{\"tool\":\"existing\"}\n").expect("existing receipt");
        append_egress_line(&path, r#"{"tool":"new"}"#).expect("locked append");
        assert_eq!(
            fs::read(&path).expect("receipt bytes"),
            b"{\"tool\":\"existing\"}\n{\"tool\":\"new\"}\n"
        );
        assert_eq!(
            read_egress_tail(&path, 2).expect("locked tail read"),
            [r#"{"tool":"new"}"#, r#"{"tool":"existing"}"#]
        );
    }
}
