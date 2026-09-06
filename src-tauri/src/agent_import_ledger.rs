//! Legacy full batch records plus compact, hash-bound verification status updates.
use super::*;

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

pub(super) fn parse_records(text: &str) -> Result<Vec<ImportLedgerLine>, String> {
    let mut batches: Vec<ImportLedgerLine> = Vec::new();
    let mut latest: BTreeMap<String, usize> = BTreeMap::new();
    for text in text.lines() {
        let value: Value =
            serde_json::from_str(text).map_err(|_| "import_ledger_invalid".to_string())?;
        if value.get("record_type").is_some() {
            let update: StatusRecord =
                serde_json::from_value(value).map_err(|_| "import_ledger_invalid".to_string())?;
            let index = latest
                .get(&update.batch_id)
                .ok_or_else(|| "import_ledger_invalid".to_string())?;
            let batch = &mut batches[*index];
            if batch.sha256 != update.batch_sha256 {
                return Err("import_ledger_invalid".into());
            }
            batch.status = update.status;
        } else {
            let batch: ImportLedgerLine =
                serde_json::from_value(value).map_err(|_| "import_ledger_invalid".to_string())?;
            // Keep legacy full-record history readable without rewriting it.
            latest.insert(batch.batch_id.clone(), batches.len());
            batches.push(batch);
        }
    }
    Ok(batches)
}
