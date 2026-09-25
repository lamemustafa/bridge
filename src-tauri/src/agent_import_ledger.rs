//! Legacy full batch records plus compact, hash-bound verification status updates.
use super::*;
use std::io::BufRead;

// The MCP frame is at most 5,000,000 bytes. This also leaves room for worst-case
// JSON escaping, repeated transaction labels and batch metadata in old records.
// Bound individual records, not append-only journal history.
pub(super) const MAX_RECORD_BYTES: usize = 32 * 1024 * 1024;

/// The most vouchers one native post may send, and so the most REMOTEIDs one
/// dispatch intent may record: the writer refuses more and the reader admits
/// no more. Imports of 50 were measured on the raw gateway (protocol reference
/// §11c.5, PARTIAL); through Bridge's own post path it is not yet measured.
pub(super) const MAX_BATCH_POST_VOUCHERS: usize = 50;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StatusKind {
    VerificationStatus,
    DispatchIntent,
    DispatchResponse,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::agent) struct StatusRecord {
    record_type: StatusKind,
    batch_id: String,
    batch_sha256: String,
    status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    response: Option<DispatchResponse>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    native_request_sha256: Option<String>,
    /// The REMOTEID the native post sends. Tally deletes a voucher only by the
    /// client REMOTEID it was created with, and exports its own GUID in that
    /// attribute instead, so this record is the only place it survives
    /// (bridge#579). Written with the dispatch intent, before the POST.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    native_remote_id: Option<String>,
    /// The REMOTEIDs a native batch post sends, one per voucher, in order.
    /// Written with a batch's dispatch intent, before the POST, and never
    /// beside `native_remote_id`. A binary older than this field refuses a
    /// journal holding one (`deny_unknown_fields`): loudly, never by
    /// skipping a record of what was sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    native_remote_ids: Option<Vec<String>>,
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
    #[cfg(test)]
    pub(in crate::agent) fn dispatch(batch: &ImportLedgerLine) -> Self {
        let mut record = Self::dispatch_native(batch, String::new(), Uuid::nil());
        record.native_request_sha256 = None;
        record.native_remote_id = None;
        record
    }

    pub(super) fn dispatch_native(
        batch: &ImportLedgerLine,
        request_sha256: String,
        remote_id: Uuid,
    ) -> Self {
        Self {
            record_type: StatusKind::DispatchIntent,
            batch_id: batch.batch_id.clone(),
            batch_sha256: batch.sha256.clone(),
            status: "dispatch_started".into(),
            response: None,
            native_request_sha256: Some(request_sha256),
            native_remote_id: Some(remote_id.hyphenated().to_string()),
            native_remote_ids: None,
        }
    }
    /// The dispatch intent of one native post, bound to the request it sends:
    /// its wire digest and the REMOTEID it carries come from the same value,
    /// so the record cannot name a different request (bridge#579).
    pub(super) fn dispatch_for(
        batch: &ImportLedgerLine,
        request: &super::post::NativePostRequest,
    ) -> Self {
        match request.remote_ids.as_slice() {
            // One voucher keeps the single-id shape, so a journal written by
            // a one-voucher post stays readable by an older binary.
            [remote_id] => {
                Self::dispatch_native(batch, request.request_sha256.clone(), *remote_id)
            }
            remote_ids => Self {
                native_remote_id: None,
                native_remote_ids: Some(
                    remote_ids
                        .iter()
                        .map(|remote_id| remote_id.hyphenated().to_string())
                        .collect(),
                ),
                ..Self::dispatch_native(batch, request.request_sha256.clone(), Uuid::nil())
            },
        }
    }

    pub(super) fn response(batch: &ImportLedgerLine, response: DispatchResponse) -> Self {
        Self {
            record_type: StatusKind::DispatchResponse,
            batch_id: batch.batch_id.clone(),
            batch_sha256: batch.sha256.clone(),
            status: "response_received".into(),
            response: Some(response),
            native_request_sha256: None,
            native_remote_id: None,
            native_remote_ids: None,
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
            native_request_sha256: None,
            native_remote_id: None,
            native_remote_ids: None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct VerificationGeneration(usize);

pub(super) struct BatchSnapshot {
    pub(super) batch: ImportLedgerLine,
    pub(super) dispatched: bool,
    pub(super) response: Option<DispatchResponse>,
    /// The REMOTEID recorded with the native dispatch intent, if any.
    pub(super) native_remote_id: Option<String>,
    // Last matching physical journal record, including identical status appends.
    pub(super) generation: VerificationGeneration,
}

enum Record {
    Batch(Box<ImportLedgerLine>),
    Status(Box<StatusRecord>),
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
                native_remote_id: selected
                    .as_ref()
                    .and_then(|snapshot| snapshot.native_remote_id.clone()),
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
            if update.native_remote_id.is_some() {
                snapshot.native_remote_id = update.native_remote_id.clone();
            }
            snapshot.batch.status = update.status;
            snapshot.generation = generation;
        }
        _ => {}
    })?;
    Ok(selected)
}

/// Validate the entire journal, retaining every build that carries one wire
/// identity: the original batch and each amendment of it, in journal order.
pub(super) fn read_lineage(
    reader: impl BufRead,
    identity_batch_id: &str,
) -> Result<Vec<BatchSnapshot>, String> {
    let mut builds: Vec<BatchSnapshot> = Vec::new();
    let mut latest: BTreeMap<String, usize> = BTreeMap::new();
    scan_records(reader, |record, generation| match record {
        Record::Batch(batch) if batch.identity_batch_id() == identity_batch_id => {
            // A repeated full record replaces its earlier snapshot, keeping
            // any dispatch already recorded against that batch.
            let prior = latest.get(&batch.batch_id).map(|index| &builds[*index]);
            let snapshot = BatchSnapshot {
                response: prior.and_then(|snapshot| snapshot.response.clone()),
                native_remote_id: prior.and_then(|snapshot| snapshot.native_remote_id.clone()),
                dispatched: prior.is_some_and(|snapshot| snapshot.dispatched),
                batch: *batch,
                generation,
            };
            match latest.get(&snapshot.batch.batch_id) {
                Some(index) => builds[*index] = snapshot,
                None => {
                    latest.insert(snapshot.batch.batch_id.clone(), builds.len());
                    builds.push(snapshot);
                }
            }
        }
        Record::Status(update) => {
            if let Some(index) = latest.get(&update.batch_id) {
                let snapshot = &mut builds[*index];
                if let Some(response) = update.response {
                    snapshot.response = Some(response);
                }
                snapshot.dispatched |= matches!(update.record_type, StatusKind::DispatchIntent);
                if update.native_remote_id.is_some() {
                    snapshot.native_remote_id = update.native_remote_id.clone();
                }
                snapshot.batch.status = update.status;
                snapshot.generation = generation;
            }
        }
        _ => {}
    })?;
    Ok(builds)
}

/// Finds the sole batch identity bound to a persisted XML digest.  The caller
/// still reads that batch's snapshot afterwards, so status updates remain part
/// of the normal snapshot admission path.
pub(super) fn find_batch_id_by_sha256(
    reader: impl BufRead,
    wanted_sha256: &str,
) -> Result<Option<String>, String> {
    let mut batch_id = None;
    scan_records(reader, |record, _| {
        if let Record::Batch(batch) = record {
            if batch.sha256 == wanted_sha256 {
                match &batch_id {
                    Some(existing) if existing != &batch.batch_id => {
                        // A digest is an identity only while it identifies one
                        // local batch. Do not choose between two history entries.
                        batch_id = Some(String::new());
                    }
                    Some(_) => {}
                    None => batch_id = Some(batch.batch_id),
                }
            }
        }
    })?;
    match batch_id.as_deref() {
        Some("") => Err("import_batch_digest_ambiguous".into()),
        Some(id) => Ok(Some(id.to_string())),
        None => Ok(None),
    }
}

/// A REMOTEID as the journal records it: a canonical, lower-case, hyphenated
/// UUID that is not nil.
fn is_canonical_remote_id(remote_id: &str) -> bool {
    Uuid::parse_str(remote_id)
        .is_ok_and(|id| !id.is_nil() && id.hyphenated().to_string() == remote_id)
}

/// Whether any dispatch intent in the journal already records any of
/// `remote_ids`, as a single post's REMOTEID or as one of a batch's.
/// The whole journal is admitted on the way, as for every other read.
pub(super) fn remote_ids_recorded(
    reader: impl BufRead,
    remote_ids: &[Uuid],
) -> Result<bool, String> {
    let wanted = remote_ids
        .iter()
        .map(|remote_id| remote_id.hyphenated().to_string())
        .collect::<std::collections::BTreeSet<_>>();
    let mut recorded = false;
    scan_records(reader, |record, _| {
        if let Record::Status(update) = record {
            recorded |= update
                .native_remote_id
                .iter()
                .chain(update.native_remote_ids.iter().flatten())
                .any(|recorded_id| wanted.contains(recorded_id));
        }
    })?;
    Ok(recorded)
}

fn scan_records(
    mut reader: impl BufRead,
    mut visit: impl FnMut(Record, VerificationGeneration),
) -> Result<(), String> {
    // Compact records must be checked even for unrelated batches. Retain only
    // the latest hash per distinct ID, not every historical voucher payload.
    // Memory therefore still grows with distinct IDs, not with status history.
    // Each batch's latest hash, with its voucher count for its REMOTEIDs.
    let mut latest: BTreeMap<String, (String, usize)> = BTreeMap::new();
    let mut dispatched: BTreeMap<String, Option<String>> = BTreeMap::new();
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
            let Some((batch_sha256, voucher_count)) = latest.get(&update.batch_id) else {
                return Err("import_ledger_invalid".into());
            };
            if *batch_sha256 != update.batch_sha256 {
                return Err("import_ledger_invalid".into());
            }
            if let Some(hash) = &update.native_request_sha256 {
                if !matches!(update.record_type, StatusKind::DispatchIntent)
                    || hash.len() != 64
                    || !hash.bytes().all(|byte| byte.is_ascii_hexdigit())
                {
                    return Err("import_ledger_invalid".into());
                }
            }
            // A recorded REMOTEID belongs only to a native dispatch intent,
            // beside its request hash, and must be a canonical UUID.
            if let Some(remote_id) = &update.native_remote_id {
                if update.native_request_sha256.is_none() || !is_canonical_remote_id(remote_id) {
                    return Err("import_ledger_invalid".into());
                }
            }
            // A batch's REMOTEIDs follow the same rule, and are distinct, one
            // per voucher of the batch, at least two (one is `native_remote_id`)
            // and at most the batch cap.
            if let Some(remote_ids) = &update.native_remote_ids {
                let distinct = remote_ids.iter().collect::<std::collections::BTreeSet<_>>();
                if update.native_request_sha256.is_none()
                    || update.native_remote_id.is_some()
                    || remote_ids.len() != *voucher_count
                    || !(2..=MAX_BATCH_POST_VOUCHERS).contains(&remote_ids.len())
                    || distinct.len() != remote_ids.len()
                    || !remote_ids.iter().all(|remote_id| is_canonical_remote_id(remote_id))
                {
                    return Err("import_ledger_invalid".into());
                }
            }
            if matches!(update.record_type, StatusKind::DispatchIntent)
                && dispatched
                    .insert(
                        update.batch_id.clone(),
                        update.native_request_sha256.clone(),
                    )
                    .is_some()
            {
                return Err("import_ledger_duplicate_dispatch".into());
            }
            if matches!(update.record_type, StatusKind::DispatchResponse) {
                let request_hash = dispatched
                    .get(&update.batch_id)
                    .ok_or("import_ledger_invalid")?;
                let response = update.response.as_ref().ok_or("import_ledger_invalid")?;
                if request_hash
                    .as_ref()
                    .is_some_and(|hash| hash != &response.request_sha256)
                {
                    return Err("import_ledger_invalid".into());
                }
            } else if update.response.is_some() {
                return Err("import_ledger_invalid".into());
            }
            Record::Status(Box::new(update))
        } else {
            let batch: ImportLedgerLine =
                serde_json::from_value(value).map_err(|_| "import_ledger_invalid".to_string())?;
            if dispatched.contains_key(&batch.batch_id)
                && latest.get(&batch.batch_id).map(|(sha256, _)| sha256) != Some(&batch.sha256)
            {
                return Err("import_ledger_invalid".into());
            }
            latest.insert(
                batch.batch_id.clone(),
                (batch.sha256.clone(), batch.vouchers.len()),
            );
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
            let native_remote_id = latest
                .get(&batch.batch_id)
                .and_then(|index| batches.get(*index))
                .and_then(|snapshot| snapshot.native_remote_id.clone());
            latest.insert(batch.batch_id.clone(), batches.len());
            batches.push(BatchSnapshot {
                response,
                native_remote_id,
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
            if update.native_remote_id.is_some() {
                snapshot.native_remote_id = update.native_remote_id.clone();
            }
            snapshot.batch.status = update.status;
            snapshot.generation = generation;
        }
    })?;
    Ok(batches)
}

#[cfg(test)]
#[path = "agent_import_ledger_stream_tests.rs"]
mod stream_tests;
