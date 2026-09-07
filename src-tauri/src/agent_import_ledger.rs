//! Legacy full batch records plus compact, hash-bound verification status updates.
use super::*;
use std::io::BufRead;

// The MCP frame is at most 5,000,000 bytes. This also leaves room for worst-case
// JSON escaping, repeated transaction labels and batch metadata in old records.
// Bound individual records, not append-only journal history.
pub(super) const MAX_RECORD_BYTES: usize = 32 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StatusKind {
    VerificationStatus,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StatusRecord {
    record_type: StatusKind,
    batch_id: String,
    batch_sha256: String,
    status: String,
}

impl From<&ImportLedgerLine> for StatusRecord {
    fn from(batch: &ImportLedgerLine) -> Self {
        Self {
            record_type: StatusKind::VerificationStatus,
            batch_id: batch.batch_id.clone(),
            batch_sha256: batch.sha256.clone(),
            status: batch.status.clone(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct VerificationGeneration(usize);

pub(super) struct BatchSnapshot {
    pub(super) batch: ImportLedgerLine,
    // Last matching physical journal record, including identical status appends.
    pub(super) generation: VerificationGeneration,
}

enum Record {
    Batch(Box<ImportLedgerLine>),
    Status(StatusRecord),
}

/// Validate the entire journal, retaining only the requested batch payload.
/// `None` performs build admission without retaining any voucher payload.
pub(super) fn read_snapshot(
    reader: impl BufRead,
    batch_id: Option<&str>,
) -> Result<Option<BatchSnapshot>, String> {
    let mut selected: Option<BatchSnapshot> = None;
    scan_records(reader, |record, generation| match record {
        Record::Batch(batch) if batch_id == Some(batch.batch_id.as_str()) => {
            selected = Some(BatchSnapshot {
                batch: *batch,
                generation,
            });
        }
        Record::Status(update) if batch_id == Some(update.batch_id.as_str()) => {
            // Whole-journal admission already established the preceding batch.
            let snapshot = selected
                .as_mut()
                .expect("status refers to an admitted batch");
            snapshot.batch.status = update.status;
            snapshot.generation = generation;
        }
        _ => {}
    })?;
    Ok(selected)
}

fn scan_records(
    mut reader: impl BufRead,
    mut visit: impl FnMut(Record, VerificationGeneration),
) -> Result<(), String> {
    // Compact records must be checked even for unrelated batches. Retain only
    // the latest hash per distinct ID, not every historical voucher payload.
    // Memory therefore still grows with distinct IDs, not with status history.
    let mut latest: BTreeMap<String, String> = BTreeMap::new();
    let mut line = Vec::new();
    let mut ordinal = 0_usize;
    while read_record(&mut reader, &mut line)? {
        let text =
            std::str::from_utf8(&line).map_err(|_| "import_ledger_unavailable".to_string())?;
        let value: Value =
            serde_json::from_str(text).map_err(|_| "import_ledger_invalid".to_string())?;
        let record = if value.get("record_type").is_some() {
            let update: StatusRecord =
                serde_json::from_value(value).map_err(|_| "import_ledger_invalid".to_string())?;
            if latest.get(&update.batch_id) != Some(&update.batch_sha256) {
                return Err("import_ledger_invalid".into());
            }
            Record::Status(update)
        } else {
            let batch: ImportLedgerLine =
                serde_json::from_value(value).map_err(|_| "import_ledger_invalid".to_string())?;
            latest.insert(batch.batch_id.clone(), batch.sha256.clone());
            Record::Batch(Box::new(batch))
        };
        visit(record, VerificationGeneration(ordinal));
        ordinal = ordinal
            .checked_add(1)
            .ok_or_else(|| "import_ledger_invalid".to_string())?;
    }
    Ok(())
}

fn read_record(reader: &mut impl BufRead, line: &mut Vec<u8>) -> Result<bool, String> {
    line.clear();
    loop {
        let available = reader
            .fill_buf()
            .map_err(|_| "import_ledger_unavailable".to_string())?;
        if available.is_empty() {
            return Ok(!line.is_empty());
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let length = newline.map_or(available.len(), |index| index + 1);
        if length > MAX_RECORD_BYTES.saturating_sub(line.len()) {
            return Err("import_ledger_record_too_large".into());
        }
        line.extend_from_slice(&available[..length]);
        reader.consume(length);
        if newline.is_some() {
            return Ok(true);
        }
    }
}

#[cfg(test)]
pub(super) fn parse_snapshots(text: &str) -> Result<Vec<BatchSnapshot>, String> {
    read_history(std::io::Cursor::new(text.as_bytes()))
}

#[cfg(test)]
pub(super) fn read_history(reader: impl BufRead) -> Result<Vec<BatchSnapshot>, String> {
    let mut batches: Vec<BatchSnapshot> = Vec::new();
    let mut latest: BTreeMap<String, usize> = BTreeMap::new();
    scan_records(reader, |record, generation| match record {
        Record::Batch(batch) => {
            // Keep legacy full-record history readable without rewriting it.
            latest.insert(batch.batch_id.clone(), batches.len());
            batches.push(BatchSnapshot {
                batch: *batch,
                generation,
            });
        }
        Record::Status(update) => {
            let snapshot = &mut batches[latest[&update.batch_id]];
            snapshot.batch.status = update.status;
            snapshot.generation = generation;
        }
    })?;
    Ok(batches)
}

#[cfg(test)]
#[path = "agent_import_ledger_stream_tests.rs"]
mod stream_tests;
