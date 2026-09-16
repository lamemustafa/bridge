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
            license_tier: None,
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
    let job_count = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM tally_import_outbox_jobs")
        .fetch_one(&repository.pool)
        .await
        .unwrap();
    assert_eq!(job_count, 1, "stale preparation must not persist a job");
}
