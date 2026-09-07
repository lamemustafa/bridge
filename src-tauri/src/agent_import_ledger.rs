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
    DispatchIntent,
    DispatchResponse,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StatusRecord {
    record_type: StatusKind,
    batch_id: String,
    batch_sha256: String,
    status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    response: Option<DispatchResponse>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DispatchResponse {
    pub(super) request_sha256: String,
    pub(super) response_sha256: String,
    pub(super) bytes: usize,
    pub(super) outcome: Option<bridge_tally_protocol::TallyImportOutcome>,
}

impl StatusRecord {
    pub(super) fn dispatch(batch: &ImportLedgerLine) -> Self {
        Self {
            record_type: StatusKind::DispatchIntent,
            batch_id: batch.batch_id.clone(),
            batch_sha256: batch.sha256.clone(),
            status: "dispatch_started".into(),
            response: None,
        }
    }
    pub(super) fn response(batch: &ImportLedgerLine, response: DispatchResponse) -> Self {
        Self {
            record_type: StatusKind::DispatchResponse,
            batch_id: batch.batch_id.clone(),
            batch_sha256: batch.sha256.clone(),
            status: "response_received".into(),
            response: Some(response),
        }
    }
}

impl From<&ImportLedgerLine> for StatusRecord {
    fn from(batch: &ImportLedgerLine) -> Self {
        Self {
            record_type: StatusKind::VerificationStatus,
            batch_id: batch.batch_id.clone(),
            batch_sha256: batch.sha256.clone(),
            status: batch.status.clone(),
            response: None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct VerificationGeneration(usize);

pub(super) struct BatchSnapshot {
    pub(super) batch: ImportLedgerLine,
    pub(super) dispatched: bool,
    pub(super) response: Option<DispatchResponse>,
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
                response: selected
                    .as_ref()
                    .and_then(|snapshot| snapshot.response.clone()),
                dispatched: selected
                    .as_ref()
                    .is_some_and(|snapshot| snapshot.dispatched),
                batch: *batch,
                generation,
            });
        }
        Record::Status(update) if batch_id == Some(update.batch_id.as_str()) => {
            // Whole-journal admission already established the preceding batch.
            let snapshot = selected
                .as_mut()
                .expect("status refers to an admitted batch");
            if let Some(response) = update.response {
                snapshot.response = Some(response);
            }
            snapshot.dispatched |= matches!(update.record_type, StatusKind::DispatchIntent);
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
    let mut dispatched = BTreeSet::new();
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
            if matches!(update.record_type, StatusKind::DispatchIntent)
                && !dispatched.insert(update.batch_id.clone())
            {
                return Err("import_ledger_duplicate_dispatch".into());
            }
            if matches!(update.record_type, StatusKind::DispatchResponse) {
                if !dispatched.contains(&update.batch_id) || update.response.is_none() {
                    return Err("import_ledger_invalid".into());
                }
            } else if update.response.is_some() {
                return Err("import_ledger_invalid".into());
            }
            Record::Status(update)
        } else {
            let batch: ImportLedgerLine =
                serde_json::from_value(value).map_err(|_| "import_ledger_invalid".to_string())?;
            if dispatched.contains(&batch.batch_id)
                && latest.get(&batch.batch_id) != Some(&batch.sha256)
            {
                return Err("import_ledger_invalid".into());
            }
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
            return if line.is_empty() {
                Ok(false)
            } else {
                // Even a complete JSON value needs its record delimiter;
                // otherwise the next append would concatenate two objects.
                Err("import_ledger_invalid".into())
            };
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
            let dispatched = latest
                .get(&batch.batch_id)
                .is_some_and(|index| batches[*index].dispatched);
            let response = latest
                .get(&batch.batch_id)
                .and_then(|index| batches.get(*index))
                .and_then(|snapshot| snapshot.response.clone());
            latest.insert(batch.batch_id.clone(), batches.len());
            batches.push(BatchSnapshot {
                response,
                dispatched,
                batch: *batch,
                generation,
            });
        }
        Record::Status(update) => {
            let snapshot = &mut batches[latest[&update.batch_id]];
            if let Some(response) = update.response {
                snapshot.response = Some(response);
            }
            snapshot.dispatched |= matches!(update.record_type, StatusKind::DispatchIntent);
            snapshot.batch.status = update.status;
            snapshot.generation = generation;
        }
    })?;
    Ok(batches)
}

#[cfg(test)]
#[path = "agent_import_ledger_stream_tests.rs"]
mod stream_tests;
