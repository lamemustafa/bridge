use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::{Row, Sqlite, Transaction};
use std::collections::BTreeMap;
use uuid::Uuid;

use super::tally_mirror::TallyMirrorRepository;

const MAX_IDENTIFIER_BYTES: usize = 200;

#[derive(Debug, thiserror::Error)]
pub enum SafeWriteStoreError {
    #[error("safe-write database operation failed")]
    Database(#[from] sqlx::Error),
    #[error("invalid safe-write input ({0})")]
    InvalidInput(&'static str),
    #[error("safe-write record was not found")]
    NotFound,
    #[error("safe-write state transition is not allowed")]
    InvalidTransition,
    #[error("open conflicts must be resolved before approval")]
    OpenConflicts,
    #[error("mapping version does not match the requested scope")]
    MappingScopeMismatch,
    #[error("legacy caller-attested verification evidence cannot promote a write outcome")]
    LegacyVerificationEvidence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteOperation {
    Create,
    Alter,
    Delete,
}

impl WriteOperation {
    fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Alter => "alter",
            Self::Delete => "delete",
        }
    }

    fn parse(value: &str) -> Result<Self, SafeWriteStoreError> {
        match value {
            "create" => Ok(Self::Create),
            "alter" => Ok(Self::Alter),
            "delete" => Ok(Self::Delete),
            _ => Err(SafeWriteStoreError::InvalidInput("stored_write_operation")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteJobState {
    Prepared,
    Approved,
    ReadyToSend,
    SendStarted,
    ConfirmedSuccess,
    ConfirmedFailure,
    OutcomeUnknown,
    RecoveredSuccess,
    RecoveredNotApplied,
    /// A pre-PR11A terminal row whose evidence authority was caller-attested.
    LegacyUntrusted,
    FailedPreSend,
    Cancelled,
}

impl WriteJobState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::Approved => "approved",
            Self::ReadyToSend => "ready_to_send",
            Self::SendStarted => "send_started",
            Self::ConfirmedSuccess => "confirmed_success",
            Self::ConfirmedFailure => "confirmed_failure",
            Self::OutcomeUnknown => "outcome_unknown",
            Self::RecoveredSuccess => "recovered_success",
            Self::RecoveredNotApplied => "recovered_not_applied",
            Self::LegacyUntrusted => "legacy_untrusted",
            Self::FailedPreSend => "failed_pre_send",
            Self::Cancelled => "cancelled",
        }
    }

    fn parse(value: &str) -> Result<Self, SafeWriteStoreError> {
        match value {
            "prepared" => Ok(Self::Prepared),
            "approved" => Ok(Self::Approved),
            "ready_to_send" => Ok(Self::ReadyToSend),
            "send_started" => Ok(Self::SendStarted),
            "confirmed_success" => Ok(Self::LegacyUntrusted),
            "confirmed_failure" => Ok(Self::ConfirmedFailure),
            "outcome_unknown" => Ok(Self::OutcomeUnknown),
            "recovered_success" | "recovered_not_applied" => Ok(Self::LegacyUntrusted),
            "failed_pre_send" => Ok(Self::FailedPreSend),
            "cancelled" => Ok(Self::Cancelled),
            _ => Err(SafeWriteStoreError::InvalidInput("stored_job_state")),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CreateMappingVersionInput {
    pub company_id: String,
    pub object_type: String,
    pub mapping_key: String,
    pub version: u32,
    pub mapping_sha256: String,
    pub supersedes_id: Option<String>,
    pub created_at_unix_ms: i64,
}

#[derive(Debug, Clone)]
pub struct PrepareImportItemInput {
    pub object_type: String,
    pub operation: WriteOperation,
    pub source_identity_sha256: String,
    pub payload_sha256: String,
    pub diff_sha256: String,
    pub expected_before_sha256: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PrepareImportJobInput {
    pub company_id: String,
    pub mapping_version_id: String,
    pub request_id: String,
    pub payload_sha256: String,
    pub diff_sha256: String,
    pub idempotency_key_sha256: String,
    pub preparation_evidence_sha256: String,
    pub created_at_unix_ms: i64,
    pub items: Vec<PrepareImportItemInput>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictResolution {
    Resolved,
    Rejected,
}

impl ConflictResolution {
    fn as_str(self) -> &'static str {
        match self {
            Self::Resolved => "resolved",
            Self::Rejected => "rejected",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ImportCounters {
    pub created: u64,
    pub altered: u64,
    pub deleted: u64,
    pub ignored: u64,
    pub errors: u64,
    pub cancelled: u64,
    pub exceptions: u64,
    pub line_errors: u64,
}

impl ImportCounters {
    fn as_database_values(&self) -> Result<[i64; 8], SafeWriteStoreError> {
        let convert = |value| {
            i64::try_from(value).map_err(|_| SafeWriteStoreError::InvalidInput("import_counter"))
        };
        Ok([
            convert(self.created)?,
            convert(self.altered)?,
            convert(self.deleted)?,
            convert(self.ignored)?,
            convert(self.errors)?,
            convert(self.cancelled)?,
            convert(self.exceptions)?,
            convert(self.line_errors)?,
        ])
    }
}

#[derive(Debug, Clone)]
pub struct ImportResultEvidenceInput {
    pub job_id: String,
    pub verification_id: String,
    pub result_sha256: String,
    pub safe_result_code: String,
    pub counters: Option<ImportCounters>,
    pub observed_at_unix_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitialImportOutcome {
    ConfirmedSuccess,
    ConfirmedFailure,
    OutcomeUnknown,
}

impl InitialImportOutcome {
    fn state(self) -> WriteJobState {
        match self {
            Self::ConfirmedSuccess => WriteJobState::ConfirmedSuccess,
            Self::ConfirmedFailure => WriteJobState::ConfirmedFailure,
            Self::OutcomeUnknown => WriteJobState::OutcomeUnknown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryOutcome {
    RecoveredSuccess,
    RecoveredNotApplied,
    Inconclusive,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryObservedState {
    Present { payload_sha256: String },
    Absent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryIdentityReadback {
    pub source_identity_sha256: String,
    pub observed: RecoveryObservedState,
}

/// Read-back evidence for an ambiguous post-send result. The repository, not
/// the caller, derives the recovery outcome from this shape and the immutable
/// outbox items.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryReadbackEvidence {
    pub intended_payload_sha256: String,
    pub observed_payload_sha256: String,
    /// Opaque digest of the observed Tally release/version evidence.
    pub observed_version_digest: String,
    pub identities: Vec<RecoveryIdentityReadback>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteJobSnapshot {
    pub id: String,
    pub request_id: String,
    pub state: WriteJobState,
    pub dispatch_attempts: u8,
    pub approval_digest: Option<String>,
    pub payload_sha256: String,
}

#[derive(Debug)]
struct RecoveryBinding {
    intended_payload_sha256: String,
    observed_payload_sha256: String,
    identity_coverage_sha256: String,
    observed_version_digest: String,
}

#[derive(Debug)]
struct StoredRecoveryItem {
    operation: WriteOperation,
    payload_sha256: String,
    expected_before_sha256: Option<String>,
}

#[derive(Serialize)]
struct RecoveryCoveragePreimage<'a> {
    contract: &'static str,
    identities: Vec<RecoveryCoverageIdentity<'a>>,
}

#[derive(Serialize)]
struct RecoveryCoverageIdentity<'a> {
    source_identity_sha256: &'a str,
    state: &'static str,
    payload_sha256: Option<&'a str>,
}

impl TallyMirrorRepository {
    pub async fn create_write_mapping_version(
        &self,
        input: CreateMappingVersionInput,
    ) -> Result<String, SafeWriteStoreError> {
        validate_id(&input.company_id, "company_id")?;
        validate_safe_code(&input.object_type, "object_type")?;
        validate_safe_code(&input.mapping_key, "mapping_key")?;
        validate_hash(&input.mapping_sha256, "mapping_sha256")?;
        validate_timestamp(input.created_at_unix_ms)?;
        if input.version == 0 {
            return Err(SafeWriteStoreError::InvalidInput("mapping_version"));
        }
        if let Some(id) = input.supersedes_id.as_deref() {
            validate_id(id, "supersedes_id")?;
        }

        let mut transaction = self.pool.begin().await?;
        let existing_count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_write_mapping_versions \
             WHERE company_id = ?1 AND object_type = ?2 AND mapping_key = ?3",
        )
        .bind(&input.company_id)
        .bind(&input.object_type)
        .bind(&input.mapping_key)
        .fetch_one(&mut *transaction)
        .await?;

        match input.supersedes_id.as_deref() {
            None if existing_count != 0 || input.version != 1 => {
                return Err(SafeWriteStoreError::MappingScopeMismatch);
            }
            Some(supersedes_id) => {
                let previous = sqlx::query(
                    "SELECT company_id, object_type, mapping_key, version \
                     FROM tally_write_mapping_versions WHERE id = ?1",
                )
                .bind(supersedes_id)
                .fetch_optional(&mut *transaction)
                .await?
                .ok_or(SafeWriteStoreError::NotFound)?;
                let previous_version: i64 = previous.try_get("version")?;
                if previous.try_get::<String, _>("company_id")? != input.company_id
                    || previous.try_get::<String, _>("object_type")? != input.object_type
                    || previous.try_get::<String, _>("mapping_key")? != input.mapping_key
                    || i64::from(input.version) != previous_version + 1
                {
                    return Err(SafeWriteStoreError::MappingScopeMismatch);
                }
            }
            _ => {}
        }

        let id = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO tally_write_mapping_versions(\
               id, company_id, object_type, mapping_key, version, mapping_sha256, \
               supersedes_id, created_at_unix_ms\
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )
        .bind(&id)
        .bind(input.company_id)
        .bind(input.object_type)
        .bind(input.mapping_key)
        .bind(i64::from(input.version))
        .bind(input.mapping_sha256)
        .bind(input.supersedes_id)
        .bind(input.created_at_unix_ms)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(id)
    }

    pub async fn activate_write_mapping_version(
        &self,
        mapping_version_id: &str,
        activated_at_unix_ms: i64,
    ) -> Result<(), SafeWriteStoreError> {
        validate_id(mapping_version_id, "mapping_version_id")?;
        validate_timestamp(activated_at_unix_ms)?;
        let mapping = sqlx::query(
            "SELECT company_id, object_type, mapping_key \
             FROM tally_write_mapping_versions WHERE id = ?1",
        )
        .bind(mapping_version_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(SafeWriteStoreError::NotFound)?;
        sqlx::query(
            "INSERT INTO tally_write_mapping_heads(\
               company_id, object_type, mapping_key, mapping_version_id, activated_at_unix_ms\
             ) VALUES (?1, ?2, ?3, ?4, ?5) \
             ON CONFLICT(company_id, object_type, mapping_key) DO UPDATE SET \
               mapping_version_id = excluded.mapping_version_id, \
               activated_at_unix_ms = excluded.activated_at_unix_ms",
        )
        .bind(mapping.try_get::<String, _>("company_id")?)
        .bind(mapping.try_get::<String, _>("object_type")?)
        .bind(mapping.try_get::<String, _>("mapping_key")?)
        .bind(mapping_version_id)
        .bind(activated_at_unix_ms)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn prepare_import_job(
        &self,
        input: PrepareImportJobInput,
    ) -> Result<String, SafeWriteStoreError> {
        validate_id(&input.company_id, "company_id")?;
        validate_id(&input.mapping_version_id, "mapping_version_id")?;
        validate_id(&input.request_id, "request_id")?;
        validate_hash(&input.payload_sha256, "payload_sha256")?;
        validate_hash(&input.diff_sha256, "diff_sha256")?;
        validate_hash(&input.idempotency_key_sha256, "idempotency_key_sha256")?;
        validate_hash(
            &input.preparation_evidence_sha256,
            "preparation_evidence_sha256",
        )?;
        validate_timestamp(input.created_at_unix_ms)?;
        if input.items.is_empty() {
            return Err(SafeWriteStoreError::InvalidInput("items"));
        }
        for item in &input.items {
            validate_import_item(item)?;
        }

        let mut transaction = self.pool.begin().await?;
        let mapping_company = sqlx::query_scalar::<_, String>(
            "SELECT version.company_id \
             FROM tally_write_mapping_versions AS version \
             INNER JOIN tally_write_mapping_heads AS head ON \
               head.mapping_version_id = version.id AND \
               head.company_id = version.company_id AND \
               head.object_type = version.object_type AND \
               head.mapping_key = version.mapping_key \
             WHERE version.id = ?1",
        )
        .bind(&input.mapping_version_id)
        .fetch_optional(&mut *transaction)
        .await?;
        let mapping_company = match mapping_company {
            Some(company_id) => company_id,
            None => {
                let exists = sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(*) FROM tally_write_mapping_versions WHERE id = ?1",
                )
                .bind(&input.mapping_version_id)
                .fetch_one(&mut *transaction)
                .await?;
                return Err(if exists == 0 {
                    SafeWriteStoreError::NotFound
                } else {
                    SafeWriteStoreError::MappingScopeMismatch
                });
            }
        };
        if mapping_company != input.company_id {
            return Err(SafeWriteStoreError::MappingScopeMismatch);
        }

        let job_id = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO tally_import_outbox_jobs(\
               id, company_id, mapping_version_id, request_id, payload_sha256, diff_sha256, \
               state, dispatch_attempts, created_at_unix_ms\
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'prepared', 0, ?7)",
        )
        .bind(&job_id)
        .bind(input.company_id)
        .bind(input.mapping_version_id)
        .bind(&input.request_id)
        .bind(input.payload_sha256)
        .bind(input.diff_sha256)
        .bind(input.created_at_unix_ms)
        .execute(&mut *transaction)
        .await?;

        sqlx::query(
            "INSERT INTO tally_import_idempotency_state(\
               idempotency_key_sha256, job_id, state, reserved_at_unix_ms\
             ) VALUES (?1, ?2, 'reserved', ?3)",
        )
        .bind(input.idempotency_key_sha256)
        .bind(&job_id)
        .bind(input.created_at_unix_ms)
        .execute(&mut *transaction)
        .await?;

        for (ordinal, item) in input.items.into_iter().enumerate() {
            let ordinal = i64::try_from(ordinal)
                .map_err(|_| SafeWriteStoreError::InvalidInput("item_ordinal"))?;
            sqlx::query(
                "INSERT INTO tally_import_outbox_items(\
                   id, job_id, ordinal, object_type, operation, source_identity_sha256, \
                   payload_sha256, diff_sha256, expected_before_sha256\
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )
            .bind(Uuid::new_v4().to_string())
            .bind(&job_id)
            .bind(ordinal)
            .bind(item.object_type)
            .bind(item.operation.as_str())
            .bind(item.source_identity_sha256)
            .bind(item.payload_sha256)
            .bind(item.diff_sha256)
            .bind(item.expected_before_sha256)
            .execute(&mut *transaction)
            .await?;
        }

        insert_event(
            &mut transaction,
            &job_id,
            None,
            WriteJobState::Prepared,
            &input.request_id,
            None,
            Some("write_job_prepared"),
            &input.preparation_evidence_sha256,
            input.created_at_unix_ms,
        )
        .await?;
        transaction.commit().await?;
        Ok(job_id)
    }

    pub async fn record_import_conflict(
        &self,
        job_id: &str,
        source_identity_sha256: &str,
        diff_sha256: &str,
        conflict_code: &str,
        created_at_unix_ms: i64,
    ) -> Result<String, SafeWriteStoreError> {
        validate_id(job_id, "job_id")?;
        validate_hash(source_identity_sha256, "source_identity_sha256")?;
        validate_hash(diff_sha256, "diff_sha256")?;
        validate_safe_code(conflict_code, "conflict_code")?;
        validate_timestamp(created_at_unix_ms)?;
        let state = self.import_job(job_id).await?.state;
        if state != WriteJobState::Prepared {
            return Err(SafeWriteStoreError::InvalidTransition);
        }
        let id = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO tally_import_conflicts(\
               id, job_id, source_identity_sha256, diff_sha256, conflict_code, state, \
               created_at_unix_ms\
             ) VALUES (?1, ?2, ?3, ?4, ?5, 'open', ?6)",
        )
        .bind(&id)
        .bind(job_id)
        .bind(source_identity_sha256)
        .bind(diff_sha256)
        .bind(conflict_code)
        .bind(created_at_unix_ms)
        .execute(&self.pool)
        .await?;
        Ok(id)
    }

    pub async fn resolve_import_conflict(
        &self,
        conflict_id: &str,
        resolution: ConflictResolution,
        resolution_code: &str,
        resolution_digest: &str,
        resolved_at_unix_ms: i64,
    ) -> Result<(), SafeWriteStoreError> {
        validate_id(conflict_id, "conflict_id")?;
        validate_safe_code(resolution_code, "resolution_code")?;
        validate_hash(resolution_digest, "resolution_digest")?;
        validate_timestamp(resolved_at_unix_ms)?;
        let result = sqlx::query(
            "UPDATE tally_import_conflicts SET state = ?1, resolution_code = ?2, \
               resolution_digest = ?3, resolved_at_unix_ms = ?4 \
             WHERE id = ?5 AND state = 'open'",
        )
        .bind(resolution.as_str())
        .bind(resolution_code)
        .bind(resolution_digest)
        .bind(resolved_at_unix_ms)
        .bind(conflict_id)
        .execute(&self.pool)
        .await?;
        require_changed(result.rows_affected())
    }

    pub async fn approve_import_job(
        &self,
        job_id: &str,
        approval_digest: &str,
        evidence_sha256: &str,
        approved_at_unix_ms: i64,
    ) -> Result<(), SafeWriteStoreError> {
        validate_hash(approval_digest, "approval_digest")?;
        self.transition_with_conflict_gate(
            job_id,
            WriteJobState::Prepared,
            WriteJobState::Approved,
            evidence_sha256,
            approved_at_unix_ms,
            Some(approval_digest),
        )
        .await
    }

    pub async fn mark_import_job_ready(
        &self,
        job_id: &str,
        evidence_sha256: &str,
        observed_at_unix_ms: i64,
    ) -> Result<(), SafeWriteStoreError> {
        self.transition_with_conflict_gate(
            job_id,
            WriteJobState::Approved,
            WriteJobState::ReadyToSend,
            evidence_sha256,
            observed_at_unix_ms,
            None,
        )
        .await
    }

    pub async fn mark_import_send_started(
        &self,
        job_id: &str,
        evidence_sha256: &str,
        send_started_at_unix_ms: i64,
    ) -> Result<(), SafeWriteStoreError> {
        validate_transition_input(job_id, evidence_sha256, send_started_at_unix_ms)?;
        let mut transaction = self.pool.begin().await?;
        let job = load_job(&mut transaction, job_id).await?;
        if job.state != WriteJobState::ReadyToSend || job.dispatch_attempts != 0 {
            return Err(SafeWriteStoreError::InvalidTransition);
        }
        let changed = sqlx::query(
            "UPDATE tally_import_outbox_jobs SET state = 'send_started', dispatch_attempts = 1, \
               send_started_at_unix_ms = ?1 \
             WHERE id = ?2 AND state = 'ready_to_send' AND dispatch_attempts = 0",
        )
        .bind(send_started_at_unix_ms)
        .bind(job_id)
        .execute(&mut *transaction)
        .await?;
        require_changed(changed.rows_affected())?;
        let changed = sqlx::query(
            "UPDATE tally_import_idempotency_state SET state = 'send_started', \
               send_started_at_unix_ms = ?1 \
             WHERE job_id = ?2 AND state = 'reserved'",
        )
        .bind(send_started_at_unix_ms)
        .bind(job_id)
        .execute(&mut *transaction)
        .await?;
        require_changed(changed.rows_affected())?;
        insert_event(
            &mut transaction,
            job_id,
            Some(WriteJobState::ReadyToSend),
            WriteJobState::SendStarted,
            &job.request_id,
            None,
            Some("write_send_started"),
            evidence_sha256,
            send_started_at_unix_ms,
        )
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn record_initial_import_result(
        &self,
        input: ImportResultEvidenceInput,
        outcome: InitialImportOutcome,
    ) -> Result<(), SafeWriteStoreError> {
        validate_result_input(&input)?;
        // This legacy input shape accepts caller-provided hashes and counters.
        // It may retain a failure or ambiguous result, but it must never promote
        // a job to success. A future migration must accept the opaque portable
        // derived-verdict contract and persist its distinct commitments.
        if outcome == InitialImportOutcome::ConfirmedSuccess {
            return Err(SafeWriteStoreError::LegacyVerificationEvidence);
        }
        let target = outcome.state();
        let mut transaction = self.pool.begin().await?;
        let job = load_job(&mut transaction, &input.job_id).await?;
        if job.state != WriteJobState::SendStarted {
            return Err(SafeWriteStoreError::InvalidTransition);
        }
        insert_result(
            &mut transaction,
            &input,
            "initial",
            target.as_str(),
            &input.safe_result_code,
            None,
        )
        .await?;
        let completed_at =
            (target != WriteJobState::OutcomeUnknown).then_some(input.observed_at_unix_ms);
        let changed = sqlx::query(
            "UPDATE tally_import_outbox_jobs SET state = ?1, completed_at_unix_ms = ?2 \
             WHERE id = ?3 AND state = 'send_started'",
        )
        .bind(target.as_str())
        .bind(completed_at)
        .bind(&input.job_id)
        .execute(&mut *transaction)
        .await?;
        require_changed(changed.rows_affected())?;
        let idempotency_state = if target == WriteJobState::OutcomeUnknown {
            "outcome_unknown"
        } else {
            "terminal"
        };
        let changed = sqlx::query(
            "UPDATE tally_import_idempotency_state SET state = ?1, terminal_at_unix_ms = ?2 \
             WHERE job_id = ?3 AND state = 'send_started'",
        )
        .bind(idempotency_state)
        .bind(completed_at)
        .bind(&input.job_id)
        .execute(&mut *transaction)
        .await?;
        require_changed(changed.rows_affected())?;
        insert_event(
            &mut transaction,
            &input.job_id,
            Some(WriteJobState::SendStarted),
            target,
            &job.request_id,
            Some(&input.verification_id),
            Some(&input.safe_result_code),
            &input.result_sha256,
            input.observed_at_unix_ms,
        )
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn record_outcome_unknown_recovery(
        &self,
        _input: ImportResultEvidenceInput,
        _evidence: RecoveryReadbackEvidence,
    ) -> Result<RecoveryOutcome, SafeWriteStoreError> {
        // RecoveryReadbackEvidence predates parser-derived, company-bound
        // readback commitments. Keep existing rows readable, but never resume
        // or promote them under this caller-attested contract.
        Err(SafeWriteStoreError::LegacyVerificationEvidence)
    }

    pub async fn cancel_import_job_before_send(
        &self,
        job_id: &str,
        safe_reason_code: &str,
        evidence_sha256: &str,
        cancelled_at_unix_ms: i64,
    ) -> Result<(), SafeWriteStoreError> {
        self.terminate_before_send(
            job_id,
            WriteJobState::Cancelled,
            safe_reason_code,
            evidence_sha256,
            cancelled_at_unix_ms,
        )
        .await
    }

    pub async fn fail_import_job_before_send(
        &self,
        job_id: &str,
        safe_reason_code: &str,
        evidence_sha256: &str,
        failed_at_unix_ms: i64,
    ) -> Result<(), SafeWriteStoreError> {
        self.terminate_before_send(
            job_id,
            WriteJobState::FailedPreSend,
            safe_reason_code,
            evidence_sha256,
            failed_at_unix_ms,
        )
        .await
    }

    pub async fn import_job(&self, job_id: &str) -> Result<WriteJobSnapshot, SafeWriteStoreError> {
        validate_id(job_id, "job_id")?;
        let row = sqlx::query(
            "SELECT id, request_id, state, dispatch_attempts, approval_digest, payload_sha256 \
             FROM tally_import_outbox_jobs WHERE id = ?1",
        )
        .bind(job_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(SafeWriteStoreError::NotFound)?;
        job_snapshot(row)
    }

    async fn transition_with_conflict_gate(
        &self,
        job_id: &str,
        expected: WriteJobState,
        target: WriteJobState,
        evidence_sha256: &str,
        observed_at_unix_ms: i64,
        approval_digest: Option<&str>,
    ) -> Result<(), SafeWriteStoreError> {
        validate_transition_input(job_id, evidence_sha256, observed_at_unix_ms)?;
        let mut transaction = self.pool.begin().await?;
        let job = load_job(&mut transaction, job_id).await?;
        if job.state != expected {
            return Err(SafeWriteStoreError::InvalidTransition);
        }
        let open_conflicts = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_import_conflicts WHERE job_id = ?1 AND state = 'open'",
        )
        .bind(job_id)
        .fetch_one(&mut *transaction)
        .await?;
        if open_conflicts != 0 {
            return Err(SafeWriteStoreError::OpenConflicts);
        }
        let changed = if target == WriteJobState::Approved {
            sqlx::query(
                "UPDATE tally_import_outbox_jobs SET state = 'approved', approval_digest = ?1, \
                   approved_at_unix_ms = ?2 WHERE id = ?3 AND state = 'prepared'",
            )
            .bind(approval_digest)
            .bind(observed_at_unix_ms)
            .bind(job_id)
            .execute(&mut *transaction)
            .await?
        } else {
            sqlx::query(
                "UPDATE tally_import_outbox_jobs SET state = ?1 WHERE id = ?2 AND state = ?3",
            )
            .bind(target.as_str())
            .bind(job_id)
            .bind(expected.as_str())
            .execute(&mut *transaction)
            .await?
        };
        require_changed(changed.rows_affected())?;
        insert_event(
            &mut transaction,
            job_id,
            Some(expected),
            target,
            &job.request_id,
            None,
            Some(if target == WriteJobState::Approved {
                "write_job_approved"
            } else {
                "write_job_ready"
            }),
            evidence_sha256,
            observed_at_unix_ms,
        )
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    async fn terminate_before_send(
        &self,
        job_id: &str,
        target: WriteJobState,
        safe_reason_code: &str,
        evidence_sha256: &str,
        observed_at_unix_ms: i64,
    ) -> Result<(), SafeWriteStoreError> {
        validate_transition_input(job_id, evidence_sha256, observed_at_unix_ms)?;
        validate_safe_code(safe_reason_code, "safe_reason_code")?;
        let mut transaction = self.pool.begin().await?;
        let job = load_job(&mut transaction, job_id).await?;
        let allowed = match target {
            WriteJobState::Cancelled => matches!(
                job.state,
                WriteJobState::Prepared | WriteJobState::Approved | WriteJobState::ReadyToSend
            ),
            WriteJobState::FailedPreSend => job.state == WriteJobState::ReadyToSend,
            _ => false,
        };
        if !allowed {
            return Err(SafeWriteStoreError::InvalidTransition);
        }
        let changed = sqlx::query(
            "UPDATE tally_import_outbox_jobs SET state = ?1, completed_at_unix_ms = ?2 \
             WHERE id = ?3 AND state = ?4 AND dispatch_attempts = 0",
        )
        .bind(target.as_str())
        .bind(observed_at_unix_ms)
        .bind(job_id)
        .bind(job.state.as_str())
        .execute(&mut *transaction)
        .await?;
        require_changed(changed.rows_affected())?;
        let changed = sqlx::query(
            "UPDATE tally_import_idempotency_state SET state = 'abandoned_before_send', \
               terminal_at_unix_ms = ?1 \
             WHERE job_id = ?2 AND state = 'reserved' AND send_started_at_unix_ms IS NULL",
        )
        .bind(observed_at_unix_ms)
        .bind(job_id)
        .execute(&mut *transaction)
        .await?;
        require_changed(changed.rows_affected())?;
        insert_event(
            &mut transaction,
            job_id,
            Some(job.state),
            target,
            &job.request_id,
            None,
            Some(safe_reason_code),
            evidence_sha256,
            observed_at_unix_ms,
        )
        .await?;
        transaction.commit().await?;
        Ok(())
    }
}

async fn load_job(
    transaction: &mut Transaction<'_, Sqlite>,
    job_id: &str,
) -> Result<WriteJobSnapshot, SafeWriteStoreError> {
    let row = sqlx::query(
        "SELECT id, request_id, state, dispatch_attempts, approval_digest, payload_sha256 \
         FROM tally_import_outbox_jobs WHERE id = ?1",
    )
    .bind(job_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(SafeWriteStoreError::NotFound)?;
    job_snapshot(row)
}

fn job_snapshot(row: sqlx::sqlite::SqliteRow) -> Result<WriteJobSnapshot, SafeWriteStoreError> {
    let dispatch_attempts = row.try_get::<i64, _>("dispatch_attempts")?;
    Ok(WriteJobSnapshot {
        id: row.try_get("id")?,
        request_id: row.try_get("request_id")?,
        state: WriteJobState::parse(&row.try_get::<String, _>("state")?)?,
        dispatch_attempts: u8::try_from(dispatch_attempts)
            .map_err(|_| SafeWriteStoreError::InvalidInput("dispatch_attempts"))?,
        approval_digest: row.try_get("approval_digest")?,
        payload_sha256: row.try_get("payload_sha256")?,
    })
}

#[allow(dead_code)] // retained only to decode/audit legacy rows; never promotes new evidence
async fn derive_recovery_outcome(
    transaction: &mut Transaction<'_, Sqlite>,
    job: &WriteJobSnapshot,
    evidence: &RecoveryReadbackEvidence,
) -> Result<(RecoveryOutcome, RecoveryBinding), SafeWriteStoreError> {
    validate_hash(
        &evidence.intended_payload_sha256,
        "recovery_intended_payload_sha256",
    )?;
    validate_hash(
        &evidence.observed_payload_sha256,
        "recovery_observed_payload_sha256",
    )?;
    validate_hash(
        &evidence.observed_version_digest,
        "recovery_observed_version_digest",
    )?;
    if evidence.intended_payload_sha256 != job.payload_sha256 {
        return Err(SafeWriteStoreError::InvalidInput(
            "recovery_intended_payload_mismatch",
        ));
    }

    let mut observed = BTreeMap::new();
    for identity in &evidence.identities {
        validate_hash(
            &identity.source_identity_sha256,
            "recovery_source_identity_sha256",
        )?;
        if let RecoveryObservedState::Present { payload_sha256 } = &identity.observed {
            validate_hash(payload_sha256, "recovery_identity_payload_sha256")?;
        }
        if observed
            .insert(
                identity.source_identity_sha256.clone(),
                identity.observed.clone(),
            )
            .is_some()
        {
            return Err(SafeWriteStoreError::InvalidInput(
                "duplicate_recovery_identity",
            ));
        }
    }
    let identity_coverage_sha256 = recovery_coverage_sha256(&observed)?;

    let rows = sqlx::query(
        "SELECT source_identity_sha256, operation, payload_sha256, expected_before_sha256 \
         FROM tally_import_outbox_items WHERE job_id = ?1 ORDER BY source_identity_sha256",
    )
    .bind(&job.id)
    .fetch_all(&mut **transaction)
    .await?;
    let mut intended = BTreeMap::new();
    for row in rows {
        intended.insert(
            row.try_get::<String, _>("source_identity_sha256")?,
            StoredRecoveryItem {
                operation: WriteOperation::parse(&row.try_get::<String, _>("operation")?)?,
                payload_sha256: row.try_get("payload_sha256")?,
                expected_before_sha256: row.try_get("expected_before_sha256")?,
            },
        );
    }

    let exact_coverage = observed.len() == intended.len() && observed.keys().eq(intended.keys());
    let success_items = exact_coverage
        && intended.iter().all(|(identity, item)| {
            let actual = observed.get(identity);
            match item.operation {
                WriteOperation::Create | WriteOperation::Alter => matches!(
                    actual,
                    Some(RecoveryObservedState::Present { payload_sha256 })
                        if payload_sha256 == &item.payload_sha256
                ),
                WriteOperation::Delete => matches!(actual, Some(RecoveryObservedState::Absent)),
            }
        });

    let expected_before = intended
        .iter()
        .map(|(identity, item)| {
            let state = match item.operation {
                WriteOperation::Create => RecoveryObservedState::Absent,
                WriteOperation::Alter | WriteOperation::Delete => RecoveryObservedState::Present {
                    payload_sha256: item
                        .expected_before_sha256
                        .clone()
                        .expect("alter/delete preparation requires a before hash"),
                },
            };
            (identity.clone(), state)
        })
        .collect::<BTreeMap<_, _>>();
    let expected_before_sha256 = recovery_coverage_sha256(&expected_before)?;
    let not_applied = exact_coverage
        && observed == expected_before
        && evidence.observed_payload_sha256 == expected_before_sha256;
    let success =
        success_items && evidence.observed_payload_sha256 == evidence.intended_payload_sha256;
    let outcome = if success {
        RecoveryOutcome::RecoveredSuccess
    } else if not_applied {
        RecoveryOutcome::RecoveredNotApplied
    } else {
        RecoveryOutcome::Inconclusive
    };
    Ok((
        outcome,
        RecoveryBinding {
            intended_payload_sha256: evidence.intended_payload_sha256.clone(),
            observed_payload_sha256: evidence.observed_payload_sha256.clone(),
            identity_coverage_sha256,
            observed_version_digest: evidence.observed_version_digest.clone(),
        },
    ))
}

fn recovery_coverage_sha256(
    identities: &BTreeMap<String, RecoveryObservedState>,
) -> Result<String, SafeWriteStoreError> {
    let identities = identities
        .iter()
        .map(|(identity, state)| RecoveryCoverageIdentity {
            source_identity_sha256: identity,
            state: match state {
                RecoveryObservedState::Present { .. } => "present",
                RecoveryObservedState::Absent => "absent",
            },
            payload_sha256: match state {
                RecoveryObservedState::Present { payload_sha256 } => Some(payload_sha256),
                RecoveryObservedState::Absent => None,
            },
        })
        .collect();
    let canonical = serde_json::to_vec(&RecoveryCoveragePreimage {
        contract: "bridge_tally_recovery_identity_coverage_v1",
        identities,
    })
    .map_err(|_| SafeWriteStoreError::InvalidInput("recovery_coverage_serialization"))?;
    Ok(Sha256::digest(canonical)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

#[allow(clippy::too_many_arguments)]
async fn insert_event(
    transaction: &mut Transaction<'_, Sqlite>,
    job_id: &str,
    from_state: Option<WriteJobState>,
    to_state: WriteJobState,
    request_id: &str,
    verification_id: Option<&str>,
    safe_reason_code: Option<&str>,
    evidence_sha256: &str,
    observed_at_unix_ms: i64,
) -> Result<(), SafeWriteStoreError> {
    sqlx::query(
        "INSERT INTO tally_import_job_events(\
           id, job_id, from_state, to_state, request_id, verification_id, safe_reason_code, \
           evidence_sha256, observed_at_unix_ms\
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(job_id)
    .bind(from_state.map(WriteJobState::as_str))
    .bind(to_state.as_str())
    .bind(request_id)
    .bind(verification_id)
    .bind(safe_reason_code)
    .bind(evidence_sha256)
    .bind(observed_at_unix_ms)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn insert_result(
    transaction: &mut Transaction<'_, Sqlite>,
    input: &ImportResultEvidenceInput,
    phase: &str,
    outcome: &str,
    safe_result_code: &str,
    recovery: Option<&RecoveryBinding>,
) -> Result<(), SafeWriteStoreError> {
    let counts = input
        .counters
        .as_ref()
        .map(ImportCounters::as_database_values)
        .transpose()?;
    let observed = counts.is_some();
    let values = counts.unwrap_or([0; 8]);
    let value = |index: usize| observed.then_some(values[index]);
    sqlx::query(
        "INSERT INTO tally_import_results(\
           id, job_id, phase, verification_id, outcome, result_sha256, safe_result_code, \
           intended_payload_sha256, observed_payload_sha256, identity_coverage_sha256, \
           observed_version_digest, \
           counters_observed, created_count, altered_count, deleted_count, ignored_count, \
           error_count, cancelled_count, exception_count, line_error_count, observed_at_unix_ms\
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, \
           ?15, ?16, ?17, ?18, ?19, ?20, ?21)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(&input.job_id)
    .bind(phase)
    .bind(&input.verification_id)
    .bind(outcome)
    .bind(&input.result_sha256)
    .bind(safe_result_code)
    .bind(recovery.map(|value| &value.intended_payload_sha256))
    .bind(recovery.map(|value| &value.observed_payload_sha256))
    .bind(recovery.map(|value| &value.identity_coverage_sha256))
    .bind(recovery.map(|value| &value.observed_version_digest))
    .bind(i64::from(observed))
    .bind(value(0))
    .bind(value(1))
    .bind(value(2))
    .bind(value(3))
    .bind(value(4))
    .bind(value(5))
    .bind(value(6))
    .bind(value(7))
    .bind(input.observed_at_unix_ms)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

fn validate_import_item(item: &PrepareImportItemInput) -> Result<(), SafeWriteStoreError> {
    validate_safe_code(&item.object_type, "item_object_type")?;
    validate_hash(&item.source_identity_sha256, "source_identity_sha256")?;
    validate_hash(&item.payload_sha256, "item_payload_sha256")?;
    validate_hash(&item.diff_sha256, "item_diff_sha256")?;
    if let Some(hash) = item.expected_before_sha256.as_deref() {
        validate_hash(hash, "expected_before_sha256")?;
    }
    if item.operation == WriteOperation::Alter
        && item.expected_before_sha256.as_deref() == Some(item.payload_sha256.as_str())
    {
        return Err(SafeWriteStoreError::InvalidInput("alter_payload_unchanged"));
    }
    match (item.operation, item.expected_before_sha256.is_some()) {
        (WriteOperation::Create, false)
        | (WriteOperation::Alter | WriteOperation::Delete, true) => Ok(()),
        _ => Err(SafeWriteStoreError::InvalidInput("expected_before_sha256")),
    }
}

fn validate_result_input(input: &ImportResultEvidenceInput) -> Result<(), SafeWriteStoreError> {
    validate_id(&input.job_id, "job_id")?;
    validate_id(&input.verification_id, "verification_id")?;
    validate_hash(&input.result_sha256, "result_sha256")?;
    validate_safe_code(&input.safe_result_code, "safe_result_code")?;
    validate_timestamp(input.observed_at_unix_ms)?;
    if let Some(counters) = &input.counters {
        counters.as_database_values()?;
    }
    Ok(())
}

fn validate_transition_input(
    job_id: &str,
    evidence_sha256: &str,
    observed_at_unix_ms: i64,
) -> Result<(), SafeWriteStoreError> {
    validate_id(job_id, "job_id")?;
    validate_hash(evidence_sha256, "evidence_sha256")?;
    validate_timestamp(observed_at_unix_ms)
}

fn validate_hash(value: &str, field: &'static str) -> Result<(), SafeWriteStoreError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(SafeWriteStoreError::InvalidInput(field));
    }
    Ok(())
}

fn validate_id(value: &str, field: &'static str) -> Result<(), SafeWriteStoreError> {
    if value.is_empty()
        || value.len() > MAX_IDENTIFIER_BYTES
        || value.chars().any(|character| {
            character.is_control()
                || !(character.is_ascii_alphanumeric()
                    || matches!(character, '-' | '_' | ':' | '.'))
        })
    {
        return Err(SafeWriteStoreError::InvalidInput(field));
    }
    Ok(())
}

fn validate_safe_code(value: &str, field: &'static str) -> Result<(), SafeWriteStoreError> {
    validate_id(value, field)
}

fn validate_timestamp(value: i64) -> Result<(), SafeWriteStoreError> {
    if value < 0 {
        return Err(SafeWriteStoreError::InvalidInput("timestamp"));
    }
    Ok(())
}

fn require_changed(rows: u64) -> Result<(), SafeWriteStoreError> {
    if rows == 1 {
        Ok(())
    } else {
        Err(SafeWriteStoreError::InvalidTransition)
    }
}

#[cfg(test)]
mod tests {
    use sqlx::sqlite::SqlitePoolOptions;

    use super::*;

    #[test]
    fn legacy_success_states_are_never_exposed_as_authoritative() {
        assert_eq!(
            WriteJobState::parse("confirmed_success").unwrap(),
            WriteJobState::LegacyUntrusted
        );
        assert_eq!(
            WriteJobState::parse("recovered_success").unwrap(),
            WriteJobState::LegacyUntrusted
        );
        assert_eq!(
            WriteJobState::parse("recovered_not_applied").unwrap(),
            WriteJobState::LegacyUntrusted
        );
    }
    use crate::db::tally_mirror::{
        CapabilitySnapshotInput, CompanyInput, Confidence, SourceIdentityInput,
    };

    const HASH_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const HASH_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const HASH_C: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
    const HASH_D: &str = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";

    async fn repository_and_company() -> (TallyMirrorRepository, String) {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .after_connect(|connection, _| {
                Box::pin(async move {
                    sqlx::query("PRAGMA foreign_keys = ON")
                        .execute(connection)
                        .await?;
                    Ok(())
                })
            })
            .connect("sqlite::memory:")
            .await
            .expect("connect in-memory SQLite");
        let repository = TallyMirrorRepository::new(pool);
        repository.migrate().await.expect("migrate mirror");
        let snapshot = repository
            .save_capability_snapshot(CapabilitySnapshotInput {
                canonical_origin: "http://127.0.0.1:9000".to_string(),
                observed_at_unix_ms: 1,
                profile_version: 1,
                product: "TallyPrime".to_string(),
                release: Some("synthetic".to_string()),
                mode: Some("Education".to_string()),
                mode_confidence: Confidence::Observed,
                items: Vec::new(),
            })
            .await
            .expect("save capability snapshot");
        let company = repository
            .upsert_company(CompanyInput {
                endpoint_id: snapshot.endpoint_id,
                display_name: "Synthetic Company".to_string(),
                identity: SourceIdentityInput {
                    guid: Some("synthetic-company-guid".to_string()),
                    confidence: Some(Confidence::Observed),
                    ..Default::default()
                },
                observed_at_unix_ms: 2,
            })
            .await
            .expect("save company");
        (repository, company.id)
    }

    async fn mapping(repository: &TallyMirrorRepository, company_id: &str) -> String {
        let id = repository
            .create_write_mapping_version(CreateMappingVersionInput {
                company_id: company_id.to_string(),
                object_type: "voucher".to_string(),
                mapping_key: "voucher_import".to_string(),
                version: 1,
                mapping_sha256: HASH_A.to_string(),
                supersedes_id: None,
                created_at_unix_ms: 3,
            })
            .await
            .expect("create mapping version");
        repository
            .activate_write_mapping_version(&id, 4)
            .await
            .expect("activate mapping version");
        id
    }

    fn job_input(
        company_id: &str,
        mapping_version_id: &str,
        request_id: &str,
        idempotency_hash: &str,
    ) -> PrepareImportJobInput {
        PrepareImportJobInput {
            company_id: company_id.to_string(),
            mapping_version_id: mapping_version_id.to_string(),
            request_id: request_id.to_string(),
            payload_sha256: HASH_B.to_string(),
            diff_sha256: HASH_C.to_string(),
            idempotency_key_sha256: idempotency_hash.to_string(),
            preparation_evidence_sha256: HASH_D.to_string(),
            created_at_unix_ms: 5,
            items: vec![PrepareImportItemInput {
                object_type: "voucher".to_string(),
                operation: WriteOperation::Alter,
                source_identity_sha256: HASH_A.to_string(),
                payload_sha256: HASH_B.to_string(),
                diff_sha256: HASH_C.to_string(),
                expected_before_sha256: Some(HASH_D.to_string()),
            }],
        }
    }

    async fn send_started_job(
        repository: &TallyMirrorRepository,
        company_id: &str,
        mapping_id: &str,
        request_id: &str,
    ) -> String {
        let job = repository
            .prepare_import_job(job_input(company_id, mapping_id, request_id, HASH_A))
            .await
            .expect("prepare job");
        repository
            .approve_import_job(&job, HASH_B, HASH_C, 6)
            .await
            .expect("approve job");
        repository
            .mark_import_job_ready(&job, HASH_D, 7)
            .await
            .expect("ready job");
        repository
            .mark_import_send_started(&job, HASH_A, 8)
            .await
            .expect("persist send started");
        job
    }

    #[tokio::test]
    async fn migration_is_versioned_and_schema_has_no_raw_payload_or_error_columns() {
        let (repository, company_id) = repository_and_company().await;
        repository.migrate().await.expect("migration is idempotent");
        let marker = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_schema_migrations WHERE version = 3 AND \
             applied_at_unix_ms > 0",
        )
        .fetch_one(&repository.pool)
        .await
        .expect("read migration marker");
        assert_eq!(marker, 1);
        let mapping_id = mapping(&repository, &company_id).await;
        let job = repository
            .prepare_import_job(job_input(&company_id, &mapping_id, "request:1", HASH_A))
            .await
            .expect("prepare hash-only job");
        assert_eq!(
            repository.import_job(&job).await.unwrap().state,
            WriteJobState::Prepared
        );

        for columns_sql in [
            "PRAGMA table_info(tally_import_outbox_jobs)",
            "PRAGMA table_info(tally_import_outbox_items)",
            "PRAGMA table_info(tally_import_results)",
            "PRAGMA table_info(tally_import_conflicts)",
        ] {
            let columns = sqlx::query(columns_sql)
                .fetch_all(&repository.pool)
                .await
                .expect("inspect safe-write table");
            for column in columns {
                let name: String = column.try_get("name").unwrap();
                assert!(!name.contains("payload_json"));
                assert!(!name.contains("raw"));
                assert!(!name.contains("error_message"));
            }
        }
    }

    #[tokio::test]
    async fn conflicts_gate_approval_and_outcome_unknown_never_reenters_send() {
        let (repository, company_id) = repository_and_company().await;
        let mapping_id = mapping(&repository, &company_id).await;
        let job = repository
            .prepare_import_job(job_input(&company_id, &mapping_id, "request:2", HASH_A))
            .await
            .unwrap();
        let conflict = repository
            .record_import_conflict(&job, HASH_B, HASH_C, "mapping_conflict", 6)
            .await
            .unwrap();
        assert!(matches!(
            repository.approve_import_job(&job, HASH_B, HASH_C, 7).await,
            Err(SafeWriteStoreError::OpenConflicts)
        ));
        repository
            .resolve_import_conflict(
                &conflict,
                ConflictResolution::Resolved,
                "approved_resolution",
                HASH_D,
                8,
            )
            .await
            .unwrap();
        repository
            .approve_import_job(&job, HASH_B, HASH_C, 9)
            .await
            .unwrap();
        repository
            .mark_import_job_ready(&job, HASH_D, 10)
            .await
            .unwrap();
        repository
            .mark_import_send_started(&job, HASH_A, 11)
            .await
            .unwrap();
        repository
            .record_initial_import_result(
                ImportResultEvidenceInput {
                    job_id: job.clone(),
                    verification_id: "verification:unknown".to_string(),
                    result_sha256: HASH_B.to_string(),
                    safe_result_code: "response_not_observed".to_string(),
                    counters: None,
                    observed_at_unix_ms: 12,
                },
                InitialImportOutcome::OutcomeUnknown,
            )
            .await
            .unwrap();
        assert_eq!(
            repository.import_job(&job).await.unwrap(),
            WriteJobSnapshot {
                id: job.clone(),
                request_id: "request:2".to_string(),
                state: WriteJobState::OutcomeUnknown,
                dispatch_attempts: 1,
                approval_digest: Some(HASH_B.to_string()),
                payload_sha256: HASH_B.to_string(),
            }
        );
        assert!(matches!(
            repository.mark_import_send_started(&job, HASH_C, 13).await,
            Err(SafeWriteStoreError::InvalidTransition)
        ));

        let forged_success = repository
            .record_outcome_unknown_recovery(
                ImportResultEvidenceInput {
                    job_id: job.clone(),
                    verification_id: "verification:inconclusive".to_string(),
                    result_sha256: HASH_C.to_string(),
                    safe_result_code: "readback_inconclusive".to_string(),
                    counters: None,
                    observed_at_unix_ms: 14,
                },
                RecoveryReadbackEvidence {
                    intended_payload_sha256: HASH_B.to_string(),
                    observed_payload_sha256: HASH_B.to_string(),
                    observed_version_digest: HASH_A.to_string(),
                    identities: vec![RecoveryIdentityReadback {
                        source_identity_sha256: HASH_A.to_string(),
                        observed: RecoveryObservedState::Present {
                            payload_sha256: HASH_C.to_string(),
                        },
                    }],
                },
            )
            .await;
        assert!(matches!(
            forged_success,
            Err(SafeWriteStoreError::LegacyVerificationEvidence)
        ));
        assert_eq!(
            repository.import_job(&job).await.unwrap().state,
            WriteJobState::OutcomeUnknown
        );

        let recovered = repository
            .record_outcome_unknown_recovery(
                ImportResultEvidenceInput {
                    job_id: job.clone(),
                    verification_id: "verification:not-applied".to_string(),
                    result_sha256: HASH_D.to_string(),
                    safe_result_code: "readback_proves_not_applied".to_string(),
                    counters: None,
                    observed_at_unix_ms: 15,
                },
                RecoveryReadbackEvidence {
                    intended_payload_sha256: HASH_B.to_string(),
                    observed_payload_sha256: HASH_D.to_string(),
                    observed_version_digest: HASH_A.to_string(),
                    identities: vec![RecoveryIdentityReadback {
                        source_identity_sha256: HASH_A.to_string(),
                        observed: RecoveryObservedState::Present {
                            payload_sha256: HASH_D.to_string(),
                        },
                    }],
                },
            )
            .await;
        assert!(matches!(
            recovered,
            Err(SafeWriteStoreError::LegacyVerificationEvidence)
        ));
        assert_eq!(
            repository.import_job(&job).await.unwrap().state,
            WriteJobState::OutcomeUnknown
        );
        let idempotency_state = sqlx::query_scalar::<_, String>(
            "SELECT state FROM tally_import_idempotency_state WHERE job_id = ?1",
        )
        .bind(&job)
        .fetch_one(&repository.pool)
        .await
        .unwrap();
        assert_eq!(idempotency_state, "outcome_unknown");
    }

    #[tokio::test]
    async fn results_are_immutable_and_duplicate_idempotency_is_transactionally_rejected() {
        let (repository, company_id) = repository_and_company().await;
        let mapping_id = mapping(&repository, &company_id).await;
        let job = send_started_job(&repository, &company_id, &mapping_id, "request:3").await;
        repository
            .record_initial_import_result(
                ImportResultEvidenceInput {
                    job_id: job.clone(),
                    verification_id: "verification:success".to_string(),
                    result_sha256: HASH_B.to_string(),
                    safe_result_code: "synthetic_failure".to_string(),
                    counters: Some(ImportCounters {
                        created: 0,
                        altered: 1,
                        deleted: 0,
                        ignored: 0,
                        errors: 1,
                        cancelled: 0,
                        exceptions: 0,
                        line_errors: 0,
                    }),
                    observed_at_unix_ms: 9,
                },
                InitialImportOutcome::ConfirmedFailure,
            )
            .await
            .unwrap();
        assert_eq!(
            repository.import_job(&job).await.unwrap().state,
            WriteJobState::ConfirmedFailure
        );
        assert!(
            sqlx::query("UPDATE tally_import_results SET safe_result_code = 'changed'")
                .execute(&repository.pool)
                .await
                .is_err()
        );
        assert!(sqlx::query(
            "UPDATE tally_import_outbox_jobs SET state = 'send_started', \
             completed_at_unix_ms = NULL WHERE id = ?1"
        )
        .bind(&job)
        .execute(&repository.pool)
        .await
        .is_err());

        let before = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM tally_import_outbox_jobs")
            .fetch_one(&repository.pool)
            .await
            .unwrap();
        let duplicate = repository
            .prepare_import_job(job_input(
                &company_id,
                &mapping_id,
                "request:duplicate",
                HASH_A,
            ))
            .await;
        assert!(matches!(duplicate, Err(SafeWriteStoreError::Database(_))));
        let after = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM tally_import_outbox_jobs")
            .fetch_one(&repository.pool)
            .await
            .unwrap();
        assert_eq!(after, before, "failed reservation must roll back the job");
    }

    #[tokio::test]
    async fn cancellation_and_pre_send_failure_are_terminal_without_dispatch() {
        let (repository, company_id) = repository_and_company().await;
        let mapping_id = mapping(&repository, &company_id).await;
        let cancelled = repository
            .prepare_import_job(job_input(
                &company_id,
                &mapping_id,
                "request:cancelled",
                HASH_A,
            ))
            .await
            .unwrap();
        repository
            .cancel_import_job_before_send(&cancelled, "operator_cancelled_before_send", HASH_B, 6)
            .await
            .unwrap();
        let cancelled = repository.import_job(&cancelled).await.unwrap();
        assert_eq!(cancelled.state, WriteJobState::Cancelled);
        assert_eq!(cancelled.dispatch_attempts, 0);
        let cancelled_idempotency = sqlx::query(
            "SELECT state, send_started_at_unix_ms, terminal_at_unix_ms \
             FROM tally_import_idempotency_state WHERE job_id = ?1",
        )
        .bind(&cancelled.id)
        .fetch_one(&repository.pool)
        .await
        .unwrap();
        assert_eq!(
            cancelled_idempotency.get::<String, _>("state"),
            "abandoned_before_send"
        );
        assert_eq!(
            cancelled_idempotency.get::<Option<i64>, _>("send_started_at_unix_ms"),
            None
        );
        assert_eq!(
            cancelled_idempotency.get::<Option<i64>, _>("terminal_at_unix_ms"),
            Some(6)
        );

        let failed = repository
            .prepare_import_job(job_input(
                &company_id,
                &mapping_id,
                "request:failed",
                HASH_C,
            ))
            .await
            .unwrap();
        repository
            .approve_import_job(&failed, HASH_B, HASH_C, 7)
            .await
            .unwrap();
        repository
            .mark_import_job_ready(&failed, HASH_D, 8)
            .await
            .unwrap();
        repository
            .fail_import_job_before_send(&failed, "pre_send_validation_failed", HASH_A, 9)
            .await
            .unwrap();
        let failed = repository.import_job(&failed).await.unwrap();
        assert_eq!(failed.state, WriteJobState::FailedPreSend);
        assert_eq!(failed.dispatch_attempts, 0);
        let failed_idempotency = sqlx::query(
            "SELECT state, send_started_at_unix_ms, terminal_at_unix_ms \
             FROM tally_import_idempotency_state WHERE job_id = ?1",
        )
        .bind(&failed.id)
        .fetch_one(&repository.pool)
        .await
        .unwrap();
        assert_eq!(
            failed_idempotency.get::<String, _>("state"),
            "abandoned_before_send"
        );
        assert_eq!(
            failed_idempotency.get::<Option<i64>, _>("send_started_at_unix_ms"),
            None
        );
        assert_eq!(
            failed_idempotency.get::<Option<i64>, _>("terminal_at_unix_ms"),
            Some(9)
        );
    }

    #[tokio::test]
    async fn superseded_inactive_mapping_cannot_prepare_a_job() {
        let (repository, company_id) = repository_and_company().await;
        let version_one = mapping(&repository, &company_id).await;
        let version_two = repository
            .create_write_mapping_version(CreateMappingVersionInput {
                company_id: company_id.clone(),
                object_type: "voucher".to_string(),
                mapping_key: "voucher_import".to_string(),
                version: 2,
                mapping_sha256: HASH_B.to_string(),
                supersedes_id: Some(version_one.clone()),
                created_at_unix_ms: 5,
            })
            .await
            .expect("create superseding mapping");
        repository
            .activate_write_mapping_version(&version_two, 6)
            .await
            .expect("activate superseding mapping");

        let stale = repository
            .prepare_import_job(job_input(
                &company_id,
                &version_one,
                "request:stale-mapping",
                HASH_C,
            ))
            .await;
        assert!(matches!(
            stale,
            Err(SafeWriteStoreError::MappingScopeMismatch)
        ));

        let active = repository
            .prepare_import_job(job_input(
                &company_id,
                &version_two,
                "request:active-mapping",
                HASH_D,
            ))
            .await
            .expect("active mapping can prepare");
        assert_eq!(
            repository.import_job(&active).await.unwrap().state,
            WriteJobState::Prepared
        );
        let job_count =
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM tally_import_outbox_jobs")
                .fetch_one(&repository.pool)
                .await
                .unwrap();
        assert_eq!(job_count, 1, "stale preparation must not persist a job");
    }
}
