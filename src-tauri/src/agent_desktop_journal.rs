//! Desktop Journal service: persisted-record admission and shared runtime operations.
use super::super::{
    default_data_dir, ensure_private_directory, DirectoryAdmissionError, EvidenceStore, Redaction,
    Settings,
};
use super::desktop_journal_review::{
    DesktopJournalCompany, DesktopJournalOperation, DesktopJournalReview,
};
use super::*;
use crate::tally::{TallyConfig, TallyRuntime};
use bridge_tally_transport::canonical_loopback_origin;
use std::env;
use std::io::Read;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

const MAX_SELECTED_JOURNAL_BYTES: usize = 5_000_000;

/// A command-scoped facade. Its cloned runtime retains the desktop application's
/// endpoint session and queue; it never constructs a second runtime.
pub(crate) struct DesktopJournalService {
    server: Server,
}

impl DesktopJournalService {
    pub(crate) fn new(config: TallyConfig, runtime: TallyRuntime) -> Result<Self, String> {
        Ok(Self {
            server: Server::with_runtime(Settings::for_desktop_journal(config)?, runtime),
        })
    }

    /// Accepts only a byte-for-byte copy of a locally persisted Bridge build.
    /// No XML from the selected file is parsed or later sent to Tally.
    pub(super) fn review_selected_xml(
        &self,
        selected: &[u8],
    ) -> Result<DesktopJournalReview, String> {
        if selected.len() > MAX_SELECTED_JOURNAL_BYTES {
            return Err("import_selected_file_too_large".into());
        }
        let selected_sha256 = sha256_hex(selected);
        let snapshot = self
            .snapshot_for_sha256(&selected_sha256)?
            .ok_or_else(|| "import_selected_file_not_found".to_string())?;
        let review = self.review_snapshot(snapshot)?;
        let persisted = self.read_persisted_xml(&review.batch_id)?;
        if persisted != selected {
            return Err("import_selected_file_not_exact".into());
        }
        Ok(review)
    }

    pub(super) async fn post(
        &self,
        batch_id: &str,
        sha256: &str,
        company_guid: &str,
    ) -> Result<DesktopJournalOperation, String> {
        let args = json!({"batch_id":batch_id,"company_guid":company_guid});
        match self.server.post_import_checked(&args, Some(sha256)).await {
            Ok(outcome) => Ok(DesktopJournalOperation::from_outcome(outcome)),
            Err(failure) => Ok(DesktopJournalOperation::from_failure(
                batch_id,
                failure,
                "The original Journal remains available for reconciliation; do not rebuild or resend it.",
            )),
        }
    }

    pub(super) async fn reconcile(
        &self,
        batch_id: &str,
        sha256: &str,
        company_guid: &str,
    ) -> Result<DesktopJournalOperation, String> {
        let args = json!({"batch_id":batch_id,"company_guid":company_guid});
        // This method checks the expected digest and durable dispatch intent in
        // the same server entrypoint. It cannot fall through to approval/POST.
        match self.server.reconcile_import(&args, sha256).await {
            Ok(outcome) => Ok(DesktopJournalOperation::from_outcome(outcome)),
            Err(failure) => {
                let message = if failure.code == "import_not_dispatched" {
                    "No posting attempt is recorded for this original Journal."
                } else {
                    "Bridge could not confirm this original Journal. Reconcile it again after the underlying condition changes."
                };
                Ok(DesktopJournalOperation::from_failure(
                    batch_id, failure, message,
                ))
            }
        }
    }

    fn snapshot_for_sha256(&self, sha256: &str) -> Result<Option<ledger::BatchSnapshot>, String> {
        let _lock = self.server.lock_import_admission_shared()?;
        let batch_id = match self.server.import_journal_while_admitted()? {
            Some(reader) => ledger::find_batch_id_by_sha256(reader, sha256)?,
            None => None,
        };
        match batch_id {
            Some(batch_id) => self.server.import_snapshot_while_admitted(Some(&batch_id)),
            None => Ok(None),
        }
    }

    fn review_snapshot(
        &self,
        snapshot: ledger::BatchSnapshot,
    ) -> Result<DesktopJournalReview, String> {
        let (_, preview) = admit_saved_journal(&snapshot.batch, &self.server.settings.endpoint)?;
        let company = snapshot
            .batch
            .company
            .ok_or_else(|| "import_post_company_missing".to_string())?;
        Ok(DesktopJournalReview {
            batch_id: snapshot.batch.batch_id,
            sha256: snapshot.batch.sha256,
            company: DesktopJournalCompany {
                name: company.name,
                guid: company.guid,
                company_number: company.company_number,
                books_from: company.books_from,
            },
            built_at: snapshot.batch.built_at,
            dispatched: snapshot.dispatched,
            response_recorded: snapshot.response.is_some(),
            preview,
        })
    }

    fn read_persisted_xml(&self, batch_id: &str) -> Result<Vec<u8>, String> {
        let uuid = batch_id
            .strip_prefix("bridge-")
            .and_then(|value| uuid::Uuid::parse_str(value).ok())
            .ok_or_else(|| "import_batch_identifier_invalid".to_string())?;
        let path = self
            .server
            .imports_dir()?
            .join(format!("bridge-{uuid}.xml"));
        let mut file = super::super::local_file::open_local_file(&path, false)
            .map_err(|_| "import_persisted_file_unavailable".to_string())?;
        let length = file
            .metadata()
            .map_err(|_| "import_persisted_file_unavailable".to_string())?
            .len();
        if length > MAX_SELECTED_JOURNAL_BYTES as u64 {
            return Err("import_persisted_file_too_large".into());
        }
        let mut bytes = Vec::with_capacity(length as usize);
        file.by_ref()
            .take((MAX_SELECTED_JOURNAL_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| "import_persisted_file_unavailable".to_string())?;
        if bytes.len() > MAX_SELECTED_JOURNAL_BYTES {
            return Err("import_persisted_file_too_large".into());
        }
        Ok(bytes)
    }
}

pub(super) struct DesktopJournalOperation {
    pub(super) result: Value,
    pub(super) evidence: Option<Value>,
}

impl DesktopJournalOperation {
    fn from_outcome(outcome: ToolOutcome) -> Self {
        Self {
            result: outcome.payload,
            evidence: serde_json::to_value(outcome.evidence).ok(),
        }
    }

    fn from_failure(batch_id: &str, failure: ToolFailure, message: &'static str) -> Self {
        Self {
            result: json!({"result":{"batch_id":batch_id,"error":{"code":failure.code,"message":message}}}),
            evidence: failure
                .evidence
                .and_then(|evidence| serde_json::to_value(*evidence).ok()),
        }
    }
}

impl Settings {
    fn for_desktop_journal(endpoint: TallyConfig) -> Result<Self, String> {
        canonical_loopback_origin(&endpoint).map_err(|_| "host_setting_invalid".to_string())?;
        let data_dir = env::var_os("BRIDGE_AGENT_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(default_data_dir);
        if data_dir.to_str().is_none() {
            return Err("agent_data_dir_encoding_invalid".into());
        }
        ensure_private_directory(&data_dir).map_err(|error| match error {
            DirectoryAdmissionError::Unavailable => "agent_data_dir_unavailable".to_string(),
            #[cfg(unix)]
            DirectoryAdmissionError::Permissions => "agent_data_dir_permissions_failed".to_string(),
        })?;
        Ok(Self {
            endpoint,
            data_dir,
            max_rows: 500,
            max_bytes: MAX_SELECTED_JOURNAL_BYTES,
            redaction: Redaction::None,
            import_enabled: true,
            writes_enabled: true,
        })
    }
}

impl Server {
    fn with_runtime(settings: Settings, runtime: TallyRuntime) -> Self {
        Self {
            settings,
            runtime,
            evidence: Arc::new(Mutex::new(EvidenceStore::default())),
        }
    }
}
