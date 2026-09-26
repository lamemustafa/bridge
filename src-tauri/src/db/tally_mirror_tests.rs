use serde_json::json;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use std::path::Path;

use super::*;
use crate::sync::reconciliation::{proof_record_counts_sha256, CommitBatchParts};

const HASH_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HASH_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

pub(super) async fn repository() -> TallyMirrorRepository {
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
        .expect("connect to in-memory SQLite");
    let repository = TallyMirrorRepository::new(pool);
    repository.migrate().await.expect("run mirror migration");
    repository
}

async fn file_repository(path: &Path) -> TallyMirrorRepository {
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
        .connect_with(
            SqliteConnectOptions::new()
                .filename(path)
                .create_if_missing(true),
        )
        .await
        .expect("connect file-backed mirror");
    TallyMirrorRepository::new(pool)
}

async fn repository_through_v9() -> TallyMirrorRepository {
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
        .expect("connect to v9 in-memory SQLite");
    let mut transaction = pool.begin().await.expect("begin v9 migration");
    for migration in [
        MIRROR_MIGRATION_V2,
        MIRROR_MIGRATION_V3,
        MIRROR_MIGRATION_V4,
        MIRROR_MIGRATION_V5,
        MIRROR_MIGRATION_V6,
        MIRROR_MIGRATION_V7,
        MIRROR_MIGRATION_V8,
        MIRROR_MIGRATION_V9,
    ] {
        sqlx::raw_sql(migration)
            .execute(&mut *transaction)
            .await
            .expect("apply migration through v9");
    }
    transaction.commit().await.expect("commit v9 schema");
    TallyMirrorRepository::new(pool)
}

async fn repository_through_v13() -> TallyMirrorRepository {
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
        .expect("connect to v13 in-memory SQLite");
    let mut transaction = pool.begin().await.expect("begin v13 migration");
    for migration in [
        MIRROR_MIGRATION_V2,
        MIRROR_MIGRATION_V3,
        MIRROR_MIGRATION_V4,
        MIRROR_MIGRATION_V5,
        MIRROR_MIGRATION_V6,
        MIRROR_MIGRATION_V7,
        MIRROR_MIGRATION_V8,
        MIRROR_MIGRATION_V9,
        MIRROR_MIGRATION_V10,
        MIRROR_MIGRATION_V11,
        MIRROR_MIGRATION_V12,
        MIRROR_MIGRATION_V13,
    ] {
        sqlx::raw_sql(migration)
            .execute(&mut *transaction)
            .await
            .expect("apply migration through v13");
    }
    transaction.commit().await.expect("commit v13 schema");
    TallyMirrorRepository::new(pool)
}

async fn repository_through_v21() -> TallyMirrorRepository {
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
        .expect("connect to v21 in-memory SQLite");
    let mut transaction = pool.begin().await.expect("begin v21 migration");
    for migration in [
        MIRROR_MIGRATION_V2,
        MIRROR_MIGRATION_V3,
        MIRROR_MIGRATION_V4,
        MIRROR_MIGRATION_V5,
        MIRROR_MIGRATION_V6,
        MIRROR_MIGRATION_V7,
        MIRROR_MIGRATION_V8,
        MIRROR_MIGRATION_V9,
        MIRROR_MIGRATION_V10,
        MIRROR_MIGRATION_V11,
        MIRROR_MIGRATION_V12,
        MIRROR_MIGRATION_V13,
        MIRROR_MIGRATION_V14,
        MIRROR_MIGRATION_V15,
        MIRROR_MIGRATION_V16,
        MIRROR_MIGRATION_V17,
        MIRROR_MIGRATION_V18,
        MIRROR_MIGRATION_V19,
        MIRROR_MIGRATION_V20,
        MIRROR_MIGRATION_V21,
    ] {
        sqlx::raw_sql(migration)
            .execute(&mut *transaction)
            .await
            .expect("apply migration through v21");
    }
    transaction.commit().await.expect("commit v21 schema");
    TallyMirrorRepository::new(pool)
}

pub(super) async fn repository_through_v24() -> TallyMirrorRepository {
    let repository = repository_through_v21().await;
    let mut transaction = repository.pool.begin().await.expect("begin v24 migration");
    sqlx::raw_sql(MIRROR_MIGRATION_V22)
        .execute(&mut *transaction)
        .await
        .expect("apply v22 migration");
    sqlx::query("PRAGMA legacy_alter_table = ON")
        .execute(&mut *transaction)
        .await
        .expect("enable legacy alter table for v23");
    sqlx::raw_sql(MIRROR_MIGRATION_V23)
        .execute(&mut *transaction)
        .await
        .expect("apply v23 migration");
    sqlx::query("PRAGMA legacy_alter_table = OFF")
        .execute(&mut *transaction)
        .await
        .expect("restore legacy alter table after v23");
    sqlx::query(
        "INSERT INTO tally_schema_migrations(version, description, applied_at_unix_ms) \
         VALUES (23, 'test composite identity migration marker', 1)",
    )
    .execute(&mut *transaction)
    .await
    .expect("record v23 migration marker");
    sqlx::query("PRAGMA legacy_alter_table = ON")
        .execute(&mut *transaction)
        .await
        .expect("enable legacy alter table for v24");
    sqlx::raw_sql(MIRROR_MIGRATION_V24)
        .execute(&mut *transaction)
        .await
        .expect("apply v24 migration");
    sqlx::query("PRAGMA legacy_alter_table = OFF")
        .execute(&mut *transaction)
        .await
        .expect("restore legacy alter table after v24");
    transaction.commit().await.expect("commit v24 schema");
    repository
}

async fn seed_repository(
    repository: TallyMirrorRepository,
) -> (TallyMirrorRepository, CapabilitySnapshotRef, CompanyRef) {
    seed_repository_with_core_evidence(
        repository,
        CapabilityState::Supported,
        Confidence::Observed,
        None,
    )
    .await
}

async fn seed_repository_with_core_evidence(
    repository: TallyMirrorRepository,
    core_state: CapabilityState,
    core_confidence: Confidence,
    core_reason: Option<&str>,
) -> (TallyMirrorRepository, CapabilitySnapshotRef, CompanyRef) {
    let snapshot = repository
        .save_capability_snapshot(CapabilitySnapshotInput {
            canonical_origin: "http://127.0.0.1:9000".to_string(),
            observed_at_unix_ms: 1_000,
            profile_version: 1,
            product: "TallyPrime".to_string(),
            release: None,
            license_tier: None,
            mode: Some("Education".to_string()),
            mode_confidence: Confidence::Observed,
            items: vec![
                CapabilityItemInput {
                    kind: CapabilityKind::Transport,
                    key: "xml_http".to_string(),
                    state: CapabilityState::Supported,
                    confidence: Confidence::Observed,
                    safe_reason_code: None,
                },
                CapabilityItemInput {
                    kind: CapabilityKind::Pack,
                    key: "core_accounting".to_string(),
                    state: core_state,
                    confidence: core_confidence,
                    safe_reason_code: core_reason.map(str::to_string),
                },
            ],
        })
        .await
        .expect("save capability snapshot");
    let company = repository
        .upsert_company(CompanyInput {
            endpoint_id: snapshot.endpoint_id.clone(),
            display_name: "Synthetic Bridge Test".to_string(),
            identity: SourceIdentityInput {
                guid: Some("company-guid-1".to_string()),
                confidence: Some(Confidence::Observed),
                ..Default::default()
            },
            observed_at_unix_ms: 1_000,
        })
        .await
        .expect("save company");
    let composite_tuple_columns = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM pragma_table_info('tally_companies') \
         WHERE name IN ('company_number', 'books_from_yyyymmdd')",
    )
    .fetch_one(&repository.pool)
    .await
    .expect("inspect legacy company schema");
    if composite_tuple_columns == 0 {
        sqlx::query("UPDATE tally_companies SET identity_confidence = 'observed' WHERE id = ?1")
            .bind(&company.id)
            .execute(&repository.pool)
            .await
            .expect("seed legacy GUID-only observed company identity");
    }
    (repository, snapshot, company)
}

async fn seeded_repository() -> (TallyMirrorRepository, CapabilitySnapshotRef, CompanyRef) {
    seed_repository(repository().await).await
}

async fn seeded_repository_with_core_evidence(
    state: CapabilityState,
    confidence: Confidence,
    reason: Option<&str>,
) -> (TallyMirrorRepository, CapabilitySnapshotRef, CompanyRef) {
    seed_repository_with_core_evidence(repository().await, state, confidence, reason).await
}

pub(super) fn reviewed_setup_input(review_commitment_sha256: &str) -> ReviewedSetupInput {
    ReviewedSetupInput {
        review_commitment_sha256: review_commitment_sha256.to_string(),
        capability: CapabilitySnapshotInput {
            canonical_origin: "http://127.0.0.1:9000".to_string(),
            observed_at_unix_ms: 2_000,
            profile_version: 2,
            product: "TallyPrime".to_string(),
            release: Some("synthetic".to_string()),
            license_tier: None,
            mode: Some("Education".to_string()),
            mode_confidence: Confidence::Observed,
            items: vec![CapabilityItemInput {
                kind: CapabilityKind::Feature,
                key: "write".to_string(),
                state: CapabilityState::Unknown,
                confidence: Confidence::Unknown,
                safe_reason_code: Some("write_probe_not_run".to_string()),
            }],
        },
        company_display_name: "Synthetic Reviewed Company".to_string(),
        company_identity: SourceIdentityInput {
            guid: Some("reviewed-company-guid".to_string()),
            confidence: Some(Confidence::Observed),
            ..Default::default()
        },
        company_number: "100001".to_string(),
        books_from_yyyymmdd: "20260401".to_string(),
        selected_read_scope: None,
    }
}

#[tokio::test]
async fn reviewed_setup_keeps_guid_collision_as_separate_observed_company() {
    let (repository, snapshot, _) = seeded_repository().await;
    repository
        .upsert_company(CompanyInput {
            endpoint_id: snapshot.endpoint_id,
            display_name: "Synthetic Collision Peer".to_string(),
            identity: SourceIdentityInput {
                remote_id: Some("remote-peer-2".to_string()),
                confidence: Some(Confidence::Observed),
                ..Default::default()
            },
            observed_at_unix_ms: 1_000,
        })
        .await
        .expect("seed second identity");
    let company_count_before = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM tally_companies")
        .fetch_one(&repository.pool)
        .await
        .expect("count seeded company pins");

    let saved = repository
        .save_reviewed_setup(ReviewedSetupInput {
            review_commitment_sha256: HASH_A.to_string(),
            capability: CapabilitySnapshotInput {
                canonical_origin: "http://127.0.0.1:9000".to_string(),
                observed_at_unix_ms: 2_000,
                profile_version: 2,
                product: "Unknown".to_string(),
                release: None,
                license_tier: None,
                mode: None,
                mode_confidence: Confidence::Unknown,
                items: vec![CapabilityItemInput {
                    kind: CapabilityKind::Feature,
                    key: "write".to_string(),
                    state: CapabilityState::Unknown,
                    confidence: Confidence::Unknown,
                    safe_reason_code: Some("write_probe_not_run".to_string()),
                }],
            },
            company_display_name: "Synthetic Ambiguous".to_string(),
            company_identity: SourceIdentityInput {
                guid: Some("company-guid-1".to_string()),
                confidence: Some(Confidence::Observed),
                ..Default::default()
            },
            company_number: "100002".to_string(),
            books_from_yyyymmdd: "20260401".to_string(),
            selected_read_scope: None,
        })
        .await
        .expect("a distinct observed tuple must not collide on GUID alone");
    assert_eq!(saved.company.display_name, "Synthetic Ambiguous");
    assert_ne!(saved.company.id, "company-1");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM tally_companies")
            .fetch_one(&repository.pool)
            .await
            .expect("count separately observed company pins"),
        company_count_before + 1
    );
}

#[tokio::test]
async fn reviewed_setup_replay_after_lost_acknowledgement_is_idempotent() {
    let repository = repository().await;
    let input = reviewed_setup_input(HASH_A);
    let first = repository
        .save_reviewed_setup(input.clone())
        .await
        .expect("commit reviewed setup before acknowledgement is lost");
    let counts_after_commit = setup_row_counts(&repository).await;

    let replay = repository
        .save_reviewed_setup(input)
        .await
        .expect("replay the exact reviewed setup");

    assert_eq!(replay.snapshot.id, first.snapshot.id);
    assert_eq!(replay.snapshot.endpoint_id, first.snapshot.endpoint_id);
    assert_eq!(replay.company.id, first.company.id);
    assert_eq!(setup_row_counts(&repository).await, counts_after_commit);
    let consumptions = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM tally_reviewed_setup_consumptions \
         WHERE review_commitment_sha256 = ?1",
    )
    .bind(HASH_A)
    .fetch_one(&repository.pool)
    .await
    .expect("count durable review consumption");
    assert_eq!(consumptions, 1);
}

#[tokio::test]
async fn reviewed_setup_commitment_cannot_be_reused_for_changed_payload() {
    let repository = repository().await;
    let input = reviewed_setup_input(HASH_A);
    repository
        .save_reviewed_setup(input.clone())
        .await
        .expect("commit first reviewed setup");
    let counts_after_commit = setup_row_counts(&repository).await;
    let mut changed = input;
    changed.company_display_name = "Synthetic Changed Company".to_string();

    assert!(matches!(
        repository.save_reviewed_setup(changed).await,
        Err(MirrorError::InvalidInput("review_commitment_reused"))
    ));
    assert_eq!(setup_row_counts(&repository).await, counts_after_commit);
}

#[tokio::test]
async fn reviewed_setup_commitment_cannot_be_reused_for_a_different_composite_tuple() {
    let repository = repository().await;
    let input = reviewed_setup_input(HASH_A);
    repository
        .save_reviewed_setup(input.clone())
        .await
        .expect("commit first reviewed setup");
    let counts_after_commit = setup_row_counts(&repository).await;

    for changed in [
        ReviewedSetupInput {
            company_number: "100002".to_string(),
            ..input.clone()
        },
        ReviewedSetupInput {
            books_from_yyyymmdd: "20270401".to_string(),
            ..input.clone()
        },
    ] {
        assert!(matches!(
            repository.save_reviewed_setup(changed).await,
            Err(MirrorError::InvalidInput("review_commitment_reused"))
        ));
    }
    assert_eq!(setup_row_counts(&repository).await, counts_after_commit);
}

#[tokio::test]
async fn reviewed_setup_consumption_authority_is_immutable() {
    let repository = repository().await;
    repository
        .save_reviewed_setup(reviewed_setup_input(HASH_A))
        .await
        .expect("commit reviewed setup");

    assert!(sqlx::query(
        "UPDATE tally_reviewed_setup_consumptions SET consumed_at_unix_ms = 3 \
         WHERE review_commitment_sha256 = ?1",
    )
    .bind(HASH_A)
    .execute(&repository.pool)
    .await
    .is_err());
    assert!(sqlx::query(
        "DELETE FROM tally_reviewed_setup_consumptions WHERE review_commitment_sha256 = ?1",
    )
    .bind(HASH_A)
    .execute(&repository.pool)
    .await
    .is_err());
}

#[tokio::test]
async fn write_fixture_enrollment_is_idempotent_revocable_and_identity_safe() {
    let repository = repository().await;
    let saved = repository
        .save_reviewed_setup(reviewed_setup_input(HASH_A))
        .await
        .expect("persist observed company pin before local fixture enrollment");
    let input = WriteFixtureEnrollmentInput {
        company_id: saved.company.id.clone(),
        review_commitment_sha256: HASH_B.to_string(),
        disposable_company_attested: true,
        no_customer_data_attested: true,
        backup_guidance_acknowledged: true,
        enrolled_at_unix_ms: 3_000,
    };

    let first = repository
        .enroll_write_fixture(input.clone())
        .await
        .expect("locally enroll synthetic fixture");
    let replay = repository
        .enroll_write_fixture(input.clone())
        .await
        .expect("exact fixture enrollment replay is idempotent");
    assert_eq!(replay, first);

    let active = repository
        .write_fixture_enrollment_status(&saved.company.id)
        .await
        .expect("read safe local fixture status");
    assert_eq!(active.fixture_state, "active");
    assert_eq!(active.candidate_gate, "enrolled");
    assert_eq!(active.write_capability, "unknown");
    let serialized = serde_json::to_string(&active).expect("serialize safe status");
    assert!(!serialized.contains("Synthetic Reviewed Company"));
    assert!(!serialized.contains("reviewed-company-guid"));

    let mut competing = input;
    competing.review_commitment_sha256 = HASH_A.to_string();
    competing.enrolled_at_unix_ms = 4_000;
    assert!(repository.enroll_write_fixture(competing).await.is_err());

    let revoked = repository
        .revoke_write_fixture_enrollment(&saved.company.id, 5_000)
        .await
        .expect("append local revocation");
    assert_eq!(revoked.fixture_state, "revoked");
    assert_eq!(revoked.candidate_gate, "not_enrolled");
    assert_eq!(revoked.revoked_at_unix_ms, Some(5_000));
    assert_eq!(
        repository
            .revoke_write_fixture_enrollment(&saved.company.id, 6_000)
            .await
            .expect("repeat revocation is local and idempotent"),
        revoked
    );

    let renewed = repository
        .enroll_write_fixture(WriteFixtureEnrollmentInput {
            company_id: saved.company.id.clone(),
            review_commitment_sha256: HASH_A.to_string(),
            disposable_company_attested: true,
            no_customer_data_attested: true,
            backup_guidance_acknowledged: true,
            // Deliberately older than the revoked enrollment: wall clocks can roll back.
            enrolled_at_unix_ms: 2_000,
        })
        .await
        .expect("a freshly reviewed fixture may enroll after revocation");
    assert_ne!(renewed.id, first.id);
    assert_eq!(
        repository
            .write_fixture_enrollment_status(&saved.company.id)
            .await
            .expect("active enrollment wins over historical timestamp ordering")
            .fixture_state,
        "active"
    );
    let final_revocation = repository
        .revoke_write_fixture_enrollment(&saved.company.id, 1_000)
        .await
        .expect("revoke the active enrollment despite clock rollback");
    assert_eq!(final_revocation.fixture_state, "revoked");
    assert_eq!(final_revocation.revoked_at_unix_ms, Some(1_000));
    let latest_revoked = repository
        .write_fixture_enrollment_status(&saved.company.id)
        .await
        .expect("latest revoked evidence uses revocation ordering");
    assert_eq!(latest_revoked.fixture_state, "revoked");
    assert_eq!(latest_revoked.revoked_at_unix_ms, Some(1_000));
    assert_eq!(
        repository
            .revoke_write_fixture_enrollment(&saved.company.id, 500)
            .await
            .expect("repeat revocation reports the latest committed evidence"),
        latest_revoked
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM tally_write_fixture_revocations")
            .fetch_one(&repository.pool)
            .await
            .expect("count immutable revocations"),
        2
    );
    assert!(
        sqlx::query("UPDATE tally_write_fixture_enrollments SET enrolled_at_unix_ms = 1")
            .execute(&repository.pool)
            .await
            .is_err()
    );
    assert!(sqlx::query("DELETE FROM tally_write_fixture_revocations")
        .execute(&repository.pool)
        .await
        .is_err());
}

#[tokio::test]
async fn write_canary_reservation_is_fixture_bound_single_use_and_revocable() {
    let repository = repository().await;
    let saved = repository
        .save_reviewed_setup(reviewed_setup_input(HASH_A))
        .await
        .expect("persist observed company before reserving a canary");
    repository
        .enroll_write_fixture(WriteFixtureEnrollmentInput {
            company_id: saved.company.id.clone(),
            review_commitment_sha256: HASH_B.to_string(),
            disposable_company_attested: true,
            no_customer_data_attested: true,
            backup_guidance_acknowledged: true,
            enrolled_at_unix_ms: 3_000,
        })
        .await
        .expect("enroll the disposable fixture");

    let reservation_input = WriteCanaryReservationInput {
        company_id: saved.company.id.clone(),
        review_commitment_sha256: HASH_B.to_string(),
        reserved_at_unix_ms: 4_000,
    };
    let first = repository
        .reserve_write_canary(reservation_input.clone())
        .await
        .expect("reserve the single canary slot");
    let replay = repository
        .reserve_write_canary(reservation_input)
        .await
        .expect("replay the exact reservation safely");
    assert_eq!(replay, first);

    assert!(matches!(
        repository
            .reserve_write_canary(WriteCanaryReservationInput {
                company_id: saved.company.id.clone(),
                review_commitment_sha256: HASH_A.to_string(),
                reserved_at_unix_ms: 5_000,
            })
            .await,
        Err(MirrorError::InvalidInput("fixture_enrollment_not_active"))
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM tally_write_canary_reservations")
            .fetch_one(&repository.pool)
            .await
            .expect("count durable canary reservations"),
        1
    );

    repository
        .revoke_write_fixture_enrollment(&saved.company.id, 6_000)
        .await
        .expect("revoke the fixture before any dispatch");
    assert!(matches!(
        repository
            .reserve_write_canary(WriteCanaryReservationInput {
                company_id: saved.company.id,
                review_commitment_sha256: HASH_B.to_string(),
                reserved_at_unix_ms: 7_000,
            })
            .await,
        Err(MirrorError::InvalidInput("fixture_enrollment_not_active"))
    ));
    assert!(
        sqlx::query("UPDATE tally_write_canary_reservations SET reserved_at_unix_ms = 1")
            .execute(&repository.pool)
            .await
            .is_err()
    );
    assert!(sqlx::query("DELETE FROM tally_write_canary_reservations")
        .execute(&repository.pool)
        .await
        .is_err());
}

#[tokio::test]
async fn write_canary_payload_binding_is_exact_immutable_and_fixture_bound() {
    let repository = repository().await;
    let saved = repository
        .save_reviewed_setup(reviewed_setup_input(HASH_A))
        .await
        .expect("persist observed company before binding a canary payload");
    repository
        .enroll_write_fixture(WriteFixtureEnrollmentInput {
            company_id: saved.company.id.clone(),
            review_commitment_sha256: HASH_B.to_string(),
            disposable_company_attested: true,
            no_customer_data_attested: true,
            backup_guidance_acknowledged: true,
            enrolled_at_unix_ms: 3_000,
        })
        .await
        .expect("enroll the disposable fixture");
    let reservation = repository
        .reserve_write_canary(WriteCanaryReservationInput {
            company_id: saved.company.id.clone(),
            review_commitment_sha256: HASH_B.to_string(),
            reserved_at_unix_ms: 4_000,
        })
        .await
        .expect("reserve the only canary slot");
    let reservation_payload_sha256 = reservation.reservation_payload_sha256.clone();
    let persisted_reservation_payload_sha256 = sqlx::query_scalar::<_, String>(
        "SELECT reservation_payload_sha256 FROM tally_write_canary_reservations WHERE id = ?1",
    )
    .bind(&reservation.id)
    .fetch_one(&repository.pool)
    .await
    .expect("load immutable reservation payload commitment");
    assert_eq!(
        reservation_payload_sha256, persisted_reservation_payload_sha256,
        "private reservation reference must return the exact durable commitment"
    );
    let input = WriteCanaryPayloadBindingInput {
        company_id: saved.company.id.clone(),
        review_commitment_sha256: HASH_B.to_string(),
        reservation_id: reservation.id.clone(),
        reservation_payload_sha256: reservation_payload_sha256.clone(),
        wire_sha256: HASH_A.to_string(),
        intended_state_sha256: HASH_B.to_string(),
        identity_query_sha256: HASH_A.to_string(),
        bound_at_unix_ms: 5_000,
    };
    let mut mismatched_reservation = input.clone();
    mismatched_reservation.reservation_payload_sha256 = HASH_A.to_string();
    assert!(matches!(
        repository
            .bind_write_canary_payload(mismatched_reservation)
            .await,
        Err(MirrorError::InvalidInput("canary_reservation_not_active"))
    ));
    let first = repository
        .bind_write_canary_payload(input.clone())
        .await
        .expect("bind the exact canary commitments");
    assert_eq!(
        repository
            .bind_write_canary_payload(input.clone())
            .await
            .expect("replay the exact payload binding safely"),
        first
    );
    let active_binding = ActiveWriteCanaryPayloadBindingInput {
        company_id: input.company_id.clone(),
        review_commitment_sha256: input.review_commitment_sha256.clone(),
        reservation_id: input.reservation_id.clone(),
        reservation_payload_sha256: input.reservation_payload_sha256.clone(),
        wire_sha256: input.wire_sha256.clone(),
        intended_state_sha256: input.intended_state_sha256.clone(),
        identity_query_sha256: input.identity_query_sha256.clone(),
    };
    assert_eq!(
        repository
            .active_write_canary_payload_binding(active_binding.clone())
            .await
            .expect("load the exact active payload binding"),
        first
    );
    let mut mismatched_active_binding = active_binding.clone();
    mismatched_active_binding.wire_sha256 = HASH_B.to_string();
    assert!(matches!(
        repository
            .active_write_canary_payload_binding(mismatched_active_binding)
            .await,
        Err(MirrorError::InvalidInput(
            "canary_payload_binding_not_active"
        ))
    ));
    let preflight_input = BeginWriteCanaryPreflightInput {
        binding: active_binding.clone(),
        started_at_unix_ms: 5_500,
    };
    let preflight = repository
        .begin_write_canary_preflight(preflight_input.clone())
        .await
        .expect("claim the one sealed preflight attempt");
    assert_eq!(preflight.payload_binding_id, first.id);
    assert!(matches!(
        repository
            .begin_write_canary_preflight(preflight_input)
            .await,
        Err(MirrorError::InvalidInput(
            "canary_preflight_attempt_already_started"
        ))
    ));
    assert!(
        sqlx::query("UPDATE tally_write_canary_preflight_attempts SET started_at_unix_ms = 1")
            .execute(&repository.pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("DELETE FROM tally_write_canary_preflight_attempts")
            .execute(&repository.pool)
            .await
            .is_err()
    );
    let evidence_input = WriteCanaryPreflightEvidenceInput {
        attempt_id: preflight.id.clone(),
        readback_state_sha256: HASH_A.to_string(),
        identity_coverage_sha256: HASH_B.to_string(),
        canonical_endpoint_sha256: HASH_A.to_string(),
        company_identity_sha256: HASH_B.to_string(),
        verified_at_unix_ms: 5_750,
    };
    let mut early_evidence = evidence_input.clone();
    early_evidence.verified_at_unix_ms = 5_499;
    assert!(matches!(
        repository
            .record_write_canary_preflight_evidence(early_evidence)
            .await,
        Err(MirrorError::InvalidInput(
            "canary_preflight_evidence_before_attempt"
        ))
    ));
    let evidence = repository
        .record_write_canary_preflight_evidence(evidence_input.clone())
        .await
        .expect("persist digest-only sealed preflight evidence");
    let evidence_gate = ActiveWriteCanaryPreflightEvidenceInput {
        binding: active_binding.clone(),
        attempt_id: preflight.id.clone(),
        evidence_id: evidence.id.clone(),
        readback_state_sha256: HASH_A.to_string(),
        identity_coverage_sha256: HASH_B.to_string(),
        canonical_endpoint_sha256: HASH_A.to_string(),
        company_identity_sha256: HASH_B.to_string(),
    };
    assert_eq!(
        repository
            .active_write_canary_preflight_evidence(evidence_gate.clone())
            .await
            .expect("verify exact active preflight evidence without dispatch"),
        evidence
    );
    let mut changed_evidence_gate = evidence_gate.clone();
    changed_evidence_gate.identity_coverage_sha256 = HASH_A.to_string();
    assert!(matches!(
        repository
            .active_write_canary_preflight_evidence(changed_evidence_gate)
            .await,
        Err(MirrorError::InvalidInput(
            "canary_preflight_evidence_not_active"
        ))
    ));
    let mut changed_target_gate = evidence_gate.clone();
    changed_target_gate.canonical_endpoint_sha256 = HASH_B.to_string();
    assert!(matches!(
        repository
            .active_write_canary_preflight_evidence(changed_target_gate)
            .await,
        Err(MirrorError::InvalidInput(
            "canary_preflight_evidence_not_active"
        ))
    ));
    assert!(matches!(
        repository
            .begin_write_canary_dispatch_attempt(BeginWriteCanaryDispatchInput {
                evidence: evidence_gate.clone(),
                claimed_at_unix_ms: 5_749,
            })
            .await,
        Err(MirrorError::InvalidInput(
            "canary_dispatch_claim_before_evidence"
        ))
    ));
    let dispatch = repository
        .begin_write_canary_dispatch_attempt(BeginWriteCanaryDispatchInput {
            evidence: evidence_gate.clone(),
            claimed_at_unix_ms: 5_800,
        })
        .await
        .expect("claim one no-send canary dispatch attempt");
    assert_eq!(dispatch.evidence_id, evidence.id);
    let final_verdict_input = WriteCanaryFinalVerdictInput {
        dispatch: ActiveWriteCanaryDispatchAttemptInput {
            evidence: evidence_gate.clone(),
            dispatch_attempt_id: dispatch.id.clone(),
            claimed_at_unix_ms: dispatch.claimed_at_unix_ms,
        },
        import_response_sha256: HASH_A.to_string(),
        readback_state_sha256: HASH_B.to_string(),
        identity_coverage_sha256: HASH_A.to_string(),
        recorded_at_unix_ms: 5_850,
    };
    let mut early_final_verdict = final_verdict_input.clone();
    early_final_verdict.recorded_at_unix_ms = 5_799;
    assert!(matches!(
        repository
            .record_write_canary_final_verdict(early_final_verdict)
            .await,
        Err(MirrorError::InvalidInput(
            "canary_final_verdict_before_dispatch_claim"
        ))
    ));
    let final_verdict = repository
        .record_write_canary_final_verdict(final_verdict_input.clone())
        .await
        .expect("persist digest-only final canary verdict");
    assert_eq!(final_verdict.dispatch_attempt_id, dispatch.id);
    assert_eq!(
        repository
            .record_write_canary_final_verdict(final_verdict_input.clone())
            .await
            .expect("replay exact final verdict safely"),
        final_verdict
    );
    let mut changed_final_verdict = final_verdict_input;
    changed_final_verdict.import_response_sha256 = HASH_B.to_string();
    assert!(matches!(
        repository
            .record_write_canary_final_verdict(changed_final_verdict)
            .await,
        Err(MirrorError::InvalidInput(
            "canary_final_verdict_already_recorded"
        ))
    ));
    assert!(
        sqlx::query("UPDATE tally_write_canary_final_verdicts SET recorded_at_unix_ms = 1")
            .execute(&repository.pool)
            .await
            .is_err()
    );
    assert!(sqlx::query("DELETE FROM tally_write_canary_final_verdicts")
        .execute(&repository.pool)
        .await
        .is_err());
    assert!(matches!(
        repository
            .begin_write_canary_dispatch_attempt(BeginWriteCanaryDispatchInput {
                evidence: evidence_gate.clone(),
                claimed_at_unix_ms: 5_801,
            })
            .await,
        Err(MirrorError::InvalidInput(
            "canary_dispatch_attempt_already_claimed"
        ))
    ));
    assert!(
        sqlx::query("UPDATE tally_write_canary_dispatch_attempts SET claimed_at_unix_ms = 1")
            .execute(&repository.pool)
            .await
            .is_err()
    );
    assert_eq!(
        repository
            .record_write_canary_preflight_evidence(evidence_input.clone())
            .await
            .expect("replay the exact preflight evidence safely"),
        evidence
    );
    let mut changed_evidence = evidence_input.clone();
    changed_evidence.readback_state_sha256 = HASH_B.to_string();
    assert!(matches!(
        repository
            .record_write_canary_preflight_evidence(changed_evidence)
            .await,
        Err(MirrorError::InvalidInput(
            "canary_preflight_evidence_already_recorded"
        ))
    ));
    assert!(sqlx::query(
        "UPDATE tally_write_canary_preflight_evidence SET verified_at_unix_ms = 1"
    )
    .execute(&repository.pool)
    .await
    .is_err());
    assert!(
        sqlx::query("DELETE FROM tally_write_canary_preflight_evidence")
            .execute(&repository.pool)
            .await
            .is_err()
    );
    let mut changed = input;
    changed.identity_query_sha256 = HASH_B.to_string();
    assert!(matches!(
        repository.bind_write_canary_payload(changed).await,
        Err(MirrorError::InvalidInput("canary_payload_already_bound"))
    ));
    assert!(
        sqlx::query("UPDATE tally_write_canary_payload_bindings SET bound_at_unix_ms = 1")
            .execute(&repository.pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("DELETE FROM tally_write_canary_payload_bindings")
            .execute(&repository.pool)
            .await
            .is_err()
    );

    repository
        .revoke_write_fixture_enrollment(&saved.company.id, 6_000)
        .await
        .expect("revoke fixture before a second binding");
    assert!(matches!(
        repository
            .active_write_canary_payload_binding(active_binding)
            .await,
        Err(MirrorError::InvalidInput(
            "canary_payload_binding_not_active"
        ))
    ));
    assert!(matches!(
        repository
            .begin_write_canary_preflight(BeginWriteCanaryPreflightInput {
                binding: ActiveWriteCanaryPayloadBindingInput {
                    company_id: saved.company.id.clone(),
                    review_commitment_sha256: HASH_B.to_string(),
                    reservation_id: reservation.id.clone(),
                    reservation_payload_sha256: reservation_payload_sha256.clone(),
                    wire_sha256: HASH_A.to_string(),
                    intended_state_sha256: HASH_B.to_string(),
                    identity_query_sha256: HASH_A.to_string(),
                },
                started_at_unix_ms: 6_500,
            })
            .await,
        Err(MirrorError::InvalidInput(
            "canary_payload_binding_not_active"
        ))
    ));
    assert!(matches!(
        repository
            .record_write_canary_preflight_evidence(evidence_input)
            .await,
        Err(MirrorError::InvalidInput("canary_preflight_not_active"))
    ));
    assert!(matches!(
        repository
            .active_write_canary_preflight_evidence(evidence_gate)
            .await,
        Err(MirrorError::InvalidInput(
            "canary_preflight_evidence_not_active"
        ))
    ));
    assert!(matches!(
        repository
            .bind_write_canary_payload(WriteCanaryPayloadBindingInput {
                company_id: saved.company.id,
                review_commitment_sha256: HASH_B.to_string(),
                reservation_id: reservation.id,
                reservation_payload_sha256: HASH_A.to_string(),
                wire_sha256: HASH_A.to_string(),
                intended_state_sha256: HASH_B.to_string(),
                identity_query_sha256: HASH_A.to_string(),
                bound_at_unix_ms: 7_000,
            })
            .await,
        Err(MirrorError::InvalidInput("canary_reservation_not_active"))
    ));
}

#[tokio::test]
async fn v13_fixture_revocations_upgrade_to_durable_sequence() {
    let (repository, _, company) = seed_repository(repository_through_v13().await).await;
    sqlx::query(
        "INSERT INTO tally_write_fixture_enrollments(\
           id, company_id, review_commitment_sha256, enrollment_payload_sha256, \
           contract_version, disposable_company_attested, no_customer_data_attested, \
           backup_guidance_acknowledged, enrolled_at_unix_ms\
         ) VALUES ('legacy-enrollment', ?1, ?2, ?3, 1, 1, 1, 1, 3000)",
    )
    .bind(&company.id)
    .bind(HASH_B)
    .bind(HASH_A)
    .execute(&repository.pool)
    .await
    .expect("seed legacy enrollment");
    sqlx::query(
        "INSERT INTO tally_write_fixture_revocations(\
           id, enrollment_id, revocation_payload_sha256, safe_reason_code, revoked_at_unix_ms\
         ) VALUES ('legacy-revocation', 'legacy-enrollment', ?1, 'operator_revoked', 4000)",
    )
    .bind(HASH_B)
    .execute(&repository.pool)
    .await
    .expect("seed legacy revocation");

    repository
        .migrate()
        .await
        .expect("upgrade fixture revocation evidence from v13 to v14");
    let status = repository
        .write_fixture_enrollment_status(&company.id)
        .await
        .expect("read upgraded legacy fixture status");
    assert_eq!(status.fixture_state, "revoked");
    assert_eq!(status.revoked_at_unix_ms, Some(4_000));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT event_sequence FROM tally_write_fixture_revocations WHERE id = 'legacy-revocation'",
        )
        .fetch_one(&repository.pool)
        .await
        .expect("read durable backfilled sequence"),
        1
    );
    assert!(sqlx::query(
        "UPDATE tally_write_fixture_revocations SET event_sequence = 2 WHERE id = 'legacy-revocation'",
    )
    .execute(&repository.pool)
    .await
    .is_err());
}

#[tokio::test]
async fn already_sequenced_v13_fixture_revocations_upgrade_idempotently() {
    let (repository, _, _company) = seed_repository(repository_through_v13().await).await;
    // Emulate the pre-merge v13 schema that already carried this column.
    sqlx::query(
        "ALTER TABLE tally_write_fixture_revocations \
         ADD COLUMN event_sequence INTEGER NOT NULL DEFAULT 0",
    )
    .execute(&repository.pool)
    .await
    .expect("add pre-existing legacy event sequence");

    repository
        .migrate()
        .await
        .expect("upgrade already-sequenced v13 schema without duplicate column");
    let saved = repository
        .save_reviewed_setup(reviewed_setup_input(HASH_A))
        .await
        .expect("re-verify observed company after the composite-identity migration");
    repository
        .enroll_write_fixture(WriteFixtureEnrollmentInput {
            company_id: saved.company.id.clone(),
            review_commitment_sha256: HASH_B.to_string(),
            disposable_company_attested: true,
            no_customer_data_attested: true,
            backup_guidance_acknowledged: true,
            enrolled_at_unix_ms: 3_000,
        })
        .await
        .expect("enroll after already-sequenced upgrade");
    repository
        .revoke_write_fixture_enrollment(&saved.company.id, 4_000)
        .await
        .expect("revoke after already-sequenced upgrade");
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT event_sequence FROM tally_write_fixture_revocations LIMIT 1",
        )
        .fetch_one(&repository.pool)
        .await
        .expect("read durable sequence after alternate upgrade path"),
        1
    );
}

#[tokio::test]
async fn reviewed_setup_atomically_persists_scoped_selected_read_evidence() {
    let repository = repository().await;
    let feature = |key: &str, state, confidence, reason: &str| CapabilityItemInput {
        kind: CapabilityKind::Feature,
        key: key.to_string(),
        state,
        confidence,
        safe_reason_code: Some(reason.to_string()),
    };
    let observation = |key: &str, date_window_verified| SelectedReadObservationInput {
        capability_key: key.to_string(),
        state: CapabilityState::Supported,
        confidence: Confidence::Observed,
        safe_reason_code: if key == "selected_ledger_read" {
            "selected_ledger_read_non_empty_observed".to_string()
        } else {
            "selected_voucher_window_non_empty_observed".to_string()
        },
        result_bucket: "non_empty_observed".to_string(),
        request_sha256: Some(HASH_A.to_string()),
        decoded_response_sha256: Some(HASH_B.to_string()),
        response_encoding: Some("utf16le".to_string()),
        company_context_verified: true,
        schema_verified: true,
        record_count_verified: true,
        identity_evidence_state: "verified".to_string(),
        date_window_verified,
    };
    let observations = vec![
        observation("selected_ledger_read", false),
        observation("selected_voucher_window_read", true),
    ];
    let commitment_observations = observations
        .iter()
        .map(|observation| SelectedReadObservationCommitmentMaterial {
            capability_key: observation.capability_key.clone(),
            state: observation.state.as_str().to_string(),
            confidence: observation.confidence.as_str().to_string(),
            safe_reason_code: observation.safe_reason_code.clone(),
            result_bucket: observation.result_bucket.clone(),
            request_sha256: observation.request_sha256.clone(),
            decoded_response_sha256: observation.decoded_response_sha256.clone(),
            response_encoding: observation.response_encoding.clone(),
            company_context_verified: observation.company_context_verified,
            schema_verified: observation.schema_verified,
            record_count_verified: observation.record_count_verified,
            identity_evidence_state: observation.identity_evidence_state.clone(),
            date_window_verified: observation.date_window_verified,
        })
        .collect();
    let scope_commitment_sha256 =
        selected_read_scope_commitment_sha256(&SelectedReadScopeCommitmentMaterial {
            parent_review_commitment_sha256: HASH_B.to_string(),
            canonical_origin: "http://127.0.0.1:9000".to_string(),
            company_guid_ascii_casefolded: "qualified-company-guid".to_string(),
            company_name: "Synthetic Qualified Company".to_string(),
            company_number: "100003".to_string(),
            books_from_yyyymmdd: "20260701".to_string(),
            ledger_profile_id: "bridge.tally.ledgers/1".to_string(),
            voucher_profile_id: "bridge.tally.vouchers/3".to_string(),
            voucher_from_yyyymmdd: "20260701".to_string(),
            voucher_to_yyyymmdd: "20260731".to_string(),
            observed_at_unix_ms: 2_000,
            observations: commitment_observations,
        })
        .expect("compute selected-read commitment");
    let saved = repository
        .save_reviewed_setup(ReviewedSetupInput {
            review_commitment_sha256: HASH_A.to_string(),
            capability: CapabilitySnapshotInput {
                canonical_origin: "http://127.0.0.1:9000".to_string(),
                observed_at_unix_ms: 2_000,
                profile_version: 3,
                product: "Unknown".to_string(),
                release: None,
                license_tier: None,
                mode: None,
                mode_confidence: Confidence::Unknown,
                items: vec![
                    feature(
                        "ledger_read",
                        CapabilityState::Unknown,
                        Confidence::Unknown,
                        "selected_read_probe_not_run",
                    ),
                    feature(
                        "voucher_read",
                        CapabilityState::Unknown,
                        Confidence::Unknown,
                        "selected_read_probe_not_run",
                    ),
                    feature(
                        "selected_ledger_read",
                        CapabilityState::Supported,
                        Confidence::Observed,
                        "selected_ledger_read_non_empty_observed",
                    ),
                    feature(
                        "selected_voucher_window_read",
                        CapabilityState::Supported,
                        Confidence::Observed,
                        "selected_voucher_window_non_empty_observed",
                    ),
                ],
            },
            company_display_name: "Synthetic Qualified Company".to_string(),
            company_identity: SourceIdentityInput {
                guid: Some("qualified-company-guid".to_string()),
                confidence: Some(Confidence::Observed),
                ..Default::default()
            },
            company_number: "100003".to_string(),
            books_from_yyyymmdd: "20260701".to_string(),
            selected_read_scope: Some(SelectedReadScopeInput {
                scope_commitment_sha256,
                parent_review_sha256: HASH_B.to_string(),
                ledger_profile_id: "bridge.tally.ledgers/1".to_string(),
                voucher_profile_id: "bridge.tally.vouchers/3".to_string(),
                voucher_from_yyyymmdd: "20260701".to_string(),
                voucher_to_yyyymmdd: "20260731".to_string(),
                company_number: "100003".to_string(),
                books_from_yyyymmdd: "20260701".to_string(),
                observed_at_unix_ms: 2_000,
                observations,
            }),
        })
        .await
        .expect("atomically save selected-read evidence");

    let scope_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM tally_selected_read_scopes \
         WHERE capability_snapshot_id = ?1 AND company_id = ?2",
    )
    .bind(&saved.snapshot.id)
    .bind(&saved.company.id)
    .fetch_one(&repository.pool)
    .await
    .expect("count saved selected-read scope");
    let observation_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM tally_selected_read_observations \
         WHERE capability_snapshot_id = ?1",
    )
    .bind(&saved.snapshot.id)
    .fetch_one(&repository.pool)
    .await
    .expect("count saved selected-read observations");
    assert_eq!((scope_count, observation_count), (1, 2));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT scope_contract_version FROM tally_selected_read_scopes \
             WHERE capability_snapshot_id = ?1",
        )
        .bind(&saved.snapshot.id)
        .fetch_one(&repository.pool)
        .await
        .expect("read selected-read scope contract version"),
        2,
        "new composite selected-read material must be labelled v2"
    );
    assert!(sqlx::query(
        "UPDATE tally_selected_read_scopes SET completeness_state = 'not_claimed' WHERE capability_snapshot_id = ?1",
    )
    .bind(&saved.snapshot.id)
    .execute(&repository.pool)
    .await
    .is_err());
}

#[tokio::test]
async fn selected_read_observation_cannot_cross_wire_scope_and_snapshot() {
    let repository = repository().await;
    sqlx::raw_sql(
        "INSERT INTO tally_endpoints VALUES ('ep', 'http://127.0.0.1:9000', 1, 2);\
         INSERT INTO tally_capability_snapshots(id, endpoint_id, observed_at_unix_ms, profile_version, product, release, mode, mode_confidence) VALUES ('snap-a', 'ep', 1, 3, 'Unknown', NULL, NULL, 'unknown');\
         INSERT INTO tally_capability_snapshots(id, endpoint_id, observed_at_unix_ms, profile_version, product, release, mode, mode_confidence) VALUES ('snap-b', 'ep', 2, 3, 'Unknown', NULL, NULL, 'unknown');\
         INSERT INTO tally_capability_items VALUES ('snap-b', 'feature', 'selected_ledger_read', 'unknown', 'unknown', 'qualification_prerequisite_failed');\
         INSERT INTO tally_companies(\
           id, endpoint_id, display_name, company_guid, remote_id, master_id, fallback_fingerprint,\
           identity_confidence, first_observed_at_unix_ms, last_observed_at_unix_ms,\
           company_number, books_from_yyyymmdd\
         ) VALUES ('company', 'ep', 'Synthetic', 'company-guid', NULL, NULL, NULL, 'observed', 1, 2, '100001', '20260401');\
         INSERT INTO tally_selected_read_scopes VALUES (\
           'scope-a', 'snap-a', 'company', 1,\
           'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',\
           'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',\
           'bridge.tally.ledgers/1', 'bridge.tally.vouchers/3',\
           '20260701', '20260731', 2, 'not_claimed', 1, 0\
         );",
    )
    .execute(&repository.pool)
    .await
    .expect("seed two independent snapshot authorities");

    let error = sqlx::query(
        "INSERT INTO tally_selected_read_observations(\
           scope_id, capability_snapshot_id, capability_kind, capability_key,\
           capability_state, confidence, safe_reason_code, result_bucket,\
           request_sha256, decoded_response_sha256, response_encoding,\
           company_context_verified, schema_verified, record_count_verified,\
           identity_evidence_state, date_window_verified\
         ) VALUES (\
           'scope-a', 'snap-b', 'feature', 'selected_ledger_read',\
           'unknown', 'unknown', 'qualification_prerequisite_failed', 'skipped',\
           NULL, NULL, NULL, 0, 0, 0, 'unverified', 0\
         )",
    )
    .execute(&repository.pool)
    .await
    .expect_err("scope and observation snapshot must be the same authority");
    assert!(error
        .to_string()
        .to_ascii_lowercase()
        .contains("foreign key"));
}

async fn setup_row_counts(repository: &TallyMirrorRepository) -> (i64, i64, i64, i64) {
    let endpoints = sqlx::query_scalar("SELECT COUNT(*) FROM tally_endpoints")
        .fetch_one(&repository.pool)
        .await
        .unwrap();
    let snapshots = sqlx::query_scalar("SELECT COUNT(*) FROM tally_capability_snapshots")
        .fetch_one(&repository.pool)
        .await
        .unwrap();
    let items = sqlx::query_scalar("SELECT COUNT(*) FROM tally_capability_items")
        .fetch_one(&repository.pool)
        .await
        .unwrap();
    let companies = sqlx::query_scalar("SELECT COUNT(*) FROM tally_companies")
        .fetch_one(&repository.pool)
        .await
        .unwrap();
    (endpoints, snapshots, items, companies)
}

async fn begin_batch(
    repository: &TallyMirrorRepository,
    snapshot: &CapabilitySnapshotRef,
    company: &CompanyRef,
    run_id: &str,
) -> String {
    repository
        .begin_batch(BeginBatchInput {
            run_id: run_id.to_string(),
            capability_snapshot_id: snapshot.id.clone(),
            company_id: company.id.clone(),
            pack_id: "core_accounting".to_string(),
            pack_schema_major: 1,
            pack_schema_minor: 0,
            source_transport: "xml_http".to_string(),
            source_release: None,
            requested_from_yyyymmdd: Some("20260401".to_string()),
            requested_to_yyyymmdd: Some("20260401".to_string()),
            started_at_unix_ms: 2_000,
        })
        .await
        .expect("begin batch")
}

fn replayable_observation(batch_id: &str) -> ObservedRecordInput {
    ObservedRecordInput {
        batch_id: batch_id.to_string(),
        object_type: "ledger".to_string(),
        display_name: Some("Synthetic Replay Ledger".to_string()),
        identity: SourceIdentityInput {
            guid: Some("ledger-guid-replay".to_string()),
            confidence: Some(Confidence::Observed),
            ..Default::default()
        },
        observed_at_unix_ms: 2_100,
        raw_source_sha256: HASH_A.to_string(),
        canonical_sha256: Some(HASH_B.to_string()),
        canonical_payload: Some(json!({"amount": "1180.00", "name": "Synthetic Replay Ledger"})),
        exact_decimals: BTreeMap::from([("opening_balance".to_string(), "1180.00".to_string())]),
        observed_alter_id: Some("42".to_string()),
        status: ObservationStatus::Accepted,
        safe_rejection_code: None,
    }
}

#[tokio::test]
async fn identical_lost_ack_replay_returns_existing_observation_without_duplication() {
    let (repository, snapshot, company) = seeded_repository().await;
    let batch_id = begin_batch(&repository, &snapshot, &company, "replay-run").await;
    let input = replayable_observation(&batch_id);
    let inserted = repository
        .observe_record_idempotent(input.clone())
        .await
        .expect("insert first observation");
    let ObserveRecordOutcome::Inserted { observation_id } = inserted else {
        panic!("first observation must be inserted");
    };

    let mut replay = input.clone();
    replay.observed_at_unix_ms = 9_999;
    assert_eq!(
        repository
            .observe_record_idempotent(replay.clone())
            .await
            .expect("accept exact lost-ack replay"),
        ObserveRecordOutcome::AlreadyPresentIdentical {
            observation_id: observation_id.clone(),
        }
    );
    let observation_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM tally_record_observations WHERE batch_id = ?1",
    )
    .bind(&batch_id)
    .fetch_one(&repository.pool)
    .await
    .expect("count replay observations");
    assert_eq!(observation_count, 1);

    assert!(matches!(
        repository.observe_record(replay).await,
        Err(MirrorError::DuplicateObservation)
    ));
}

#[tokio::test]
async fn changed_replay_is_a_conflict_and_mutates_neither_record_nor_observation() {
    let (repository, snapshot, company) = seeded_repository().await;
    let batch_id = begin_batch(&repository, &snapshot, &company, "conflict-run").await;
    let input = replayable_observation(&batch_id);
    repository
        .observe_record_idempotent(input.clone())
        .await
        .expect("insert first observation");

    let source_before = sqlx::query_as::<_, (Option<String>, String, Option<i64>)>(
        "SELECT display_name, last_seen_batch_id, tombstoned_at_unix_ms \
         FROM tally_source_records \
         WHERE company_id = ?1 AND object_type = 'ledger' AND source_guid = ?2",
    )
    .bind(&company.id)
    .bind(input.identity.guid.as_deref())
    .fetch_one(&repository.pool)
    .await
    .expect("read source record before conflict");
    let observation_before = sqlx::query_as::<
        _,
        (
            i64,
            String,
            Option<String>,
            Option<String>,
            String,
            Option<String>,
            String,
            Option<String>,
        ),
    >(
        "SELECT observed_at_unix_ms, raw_source_sha256, canonical_sha256, \
           canonical_payload_json, exact_decimals_json, observed_alter_id, \
           validation_status, safe_rejection_code \
         FROM tally_record_observations WHERE batch_id = ?1",
    )
    .bind(&batch_id)
    .fetch_one(&repository.pool)
    .await
    .expect("read observation before conflict");

    let mut changed = input;
    changed.display_name = Some("Changed Replay Name".to_string());
    changed.observed_at_unix_ms = 8_888;
    changed.raw_source_sha256 = HASH_B.to_string();
    changed.canonical_sha256 = Some(HASH_A.to_string());
    changed.canonical_payload = Some(json!({"amount": "999.00", "name": "Changed"}));
    changed.exact_decimals =
        BTreeMap::from([("opening_balance".to_string(), "999.00".to_string())]);
    assert!(matches!(
        repository.observe_record_idempotent(changed).await,
        Err(MirrorError::ObservationConflict)
    ));
    let mut legacy_changed = replayable_observation(&batch_id);
    legacy_changed.raw_source_sha256 = HASH_B.to_string();
    assert!(matches!(
        repository.observe_record(legacy_changed).await,
        Err(MirrorError::DuplicateObservation)
    ));

    let source_after = sqlx::query_as::<_, (Option<String>, String, Option<i64>)>(
        "SELECT display_name, last_seen_batch_id, tombstoned_at_unix_ms \
         FROM tally_source_records \
         WHERE company_id = ?1 AND object_type = 'ledger' AND source_guid = ?2",
    )
    .bind(&company.id)
    .bind("ledger-guid-replay")
    .fetch_one(&repository.pool)
    .await
    .expect("read source record after conflict");
    let observation_after = sqlx::query_as::<
        _,
        (
            i64,
            String,
            Option<String>,
            Option<String>,
            String,
            Option<String>,
            String,
            Option<String>,
        ),
    >(
        "SELECT observed_at_unix_ms, raw_source_sha256, canonical_sha256, \
           canonical_payload_json, exact_decimals_json, observed_alter_id, \
           validation_status, safe_rejection_code \
         FROM tally_record_observations WHERE batch_id = ?1",
    )
    .bind(&batch_id)
    .fetch_one(&repository.pool)
    .await
    .expect("read observation after conflict");
    assert_eq!(source_after, source_before);
    assert_eq!(observation_after, observation_before);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_record_observations WHERE batch_id = ?1",
        )
        .bind(&batch_id)
        .fetch_one(&repository.pool)
        .await
        .expect("count observations after conflict"),
        1
    );
}

fn unavailable_membership(
    record_key: &str,
    canonical_sha256: &str,
    name: &str,
) -> SnapshotWindowMembershipInput {
    SnapshotWindowMembershipInput::ProvenanceUnavailable {
        record_key: record_key.to_string(),
        canonical_sha256: canonical_sha256.to_string(),
        canonical_payload: json!({"name": name}),
        exact_decimals: BTreeMap::new(),
        safe_reason_code: "record_provenance_unavailable".to_string(),
    }
}

async fn begin_window_attempt(
    repository: &TallyMirrorRepository,
    batch_id: &str,
    started_at_unix_ms: i64,
) -> SnapshotWindowAttemptRef {
    repository
        .begin_snapshot_window_attempt(BeginSnapshotWindowAttemptInput {
            batch_id: batch_id.to_string(),
            window_id: "voucher:20260401:20260401".to_string(),
            started_at_unix_ms,
        })
        .await
        .expect("begin window attempt")
        .attempt
}

#[tokio::test]
async fn normalized_window_staging_is_idempotent_and_loads_bounded_receipt_and_map() {
    let (repository, snapshot, company) = seeded_repository().await;
    let batch_id = begin_batch(&repository, &snapshot, &company, "window-stage-run").await;
    let attempt = begin_window_attempt(&repository, &batch_id, 3_000).await;
    let observed = SnapshotWindowMembershipInput::Observed {
        record_key: "ledger\0ledger-guid-replay".to_string(),
        observation: Box::new(replayable_observation(&batch_id)),
    };
    let unavailable = unavailable_membership(
        "voucher\0voucher-guid-unavailable",
        HASH_A,
        "Synthetic Unavailable Voucher",
    );
    let staged = repository
        .stage_snapshot_window_memberships(&attempt, vec![observed.clone(), unavailable.clone()])
        .await
        .expect("atomically stage mixed provenance chunk");
    assert_eq!(
        staged,
        StageSnapshotWindowMembershipsResult {
            inserted_memberships: 2,
            inserted_observations: 1,
            provenance_unavailable_memberships: 1,
            ..Default::default()
        }
    );
    let replayed = repository
        .stage_snapshot_window_memberships(&attempt, vec![observed, unavailable])
        .await
        .expect("replay exact mixed provenance chunk");
    assert_eq!(replayed.replayed_memberships, 2);
    assert_eq!(replayed.replayed_observations, 1);
    assert_eq!(replayed.provenance_unavailable_memberships, 1);

    let receipt = repository
        .complete_snapshot_window_attempt(&attempt, 4_000, json!({"response_bytes": 321}))
        .await
        .expect("complete immutable attempt");
    assert_eq!(receipt.member_count, 2);
    assert_eq!(
        repository
            .load_latest_completed_window_receipt(&batch_id, &attempt.window_id)
            .await
            .expect("load latest receipt"),
        Some(receipt.clone())
    );
    let map = repository
        .load_completed_window_canonical_record_map(&attempt)
        .await
        .expect("load ordered canonical map");
    assert_eq!(
        map.keys().cloned().collect::<Vec<_>>(),
        vec![
            "ledger\0ledger-guid-replay".to_string(),
            "voucher\0voucher-guid-unavailable".to_string()
        ]
    );
    assert!(matches!(
        repository
            .stage_snapshot_window_membership(
                &attempt,
                unavailable_membership("voucher\0later", HASH_B, "Later")
            )
            .await,
        Err(MirrorError::WindowAttemptClosed)
    ));
    assert!(sqlx::query(
        "UPDATE tally_snapshot_window_attempts SET receipt_sha256 = ?1 WHERE id = ?2",
    )
    .bind(HASH_B)
    .bind(&attempt.attempt_id)
    .execute(&repository.pool)
    .await
    .is_err());
    assert!(sqlx::query(
        "UPDATE tally_snapshot_window_memberships SET canonical_sha256 = ?1 \
         WHERE batch_id = ?2 AND window_id = ?3 AND record_key = ?4",
    )
    .bind(HASH_B)
    .bind(&batch_id)
    .bind(&attempt.window_id)
    .bind("voucher\0voucher-guid-unavailable")
    .execute(&repository.pool)
    .await
    .is_err());
    assert!(sqlx::query(
        "DELETE FROM tally_snapshot_window_memberships \
         WHERE batch_id = ?1 AND window_id = ?2",
    )
    .bind(&batch_id)
    .bind(&attempt.window_id)
    .execute(&repository.pool)
    .await
    .is_err());
}

#[tokio::test]
async fn membership_content_conflict_rolls_back_the_entire_chunk() {
    let (repository, snapshot, company) = seeded_repository().await;
    let batch_id = begin_batch(&repository, &snapshot, &company, "window-conflict-run").await;
    let first = begin_window_attempt(&repository, &batch_id, 3_000).await;
    repository
        .stage_snapshot_window_membership(
            &first,
            unavailable_membership("voucher\0stable", HASH_A, "Stable"),
        )
        .await
        .expect("seed immutable membership");
    let second = begin_window_attempt(&repository, &batch_id, 4_000).await;
    assert!(matches!(
        repository
            .stage_snapshot_window_memberships(
                &second,
                vec![
                    unavailable_membership("voucher\0new", HASH_B, "New"),
                    unavailable_membership("voucher\0stable", HASH_B, "Changed"),
                ],
            )
            .await,
        Err(MirrorError::WindowMembershipConflict)
    ));
    let new_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM tally_snapshot_window_memberships WHERE record_key = ?1",
    )
    .bind("voucher\0new")
    .fetch_one(&repository.pool)
    .await
    .expect("count rolled-back addition");
    assert_eq!(new_count, 0);
    let last_seen = sqlx::query_scalar::<_, String>(
        "SELECT last_seen_attempt_id FROM tally_snapshot_window_memberships \
         WHERE batch_id = ?1 AND window_id = ?2 AND record_key = ?3",
    )
    .bind(&batch_id)
    .bind(&first.window_id)
    .bind("voucher\0stable")
    .fetch_one(&repository.pool)
    .await
    .expect("read unchanged last seen attempt");
    assert_eq!(last_seen, first.attempt_id);
}

#[tokio::test]
async fn completion_detects_membership_from_abandoned_partial_attempt_and_allows_additions() {
    let (repository, snapshot, company) = seeded_repository().await;
    let batch_id = begin_batch(&repository, &snapshot, &company, "window-disappear-run").await;
    let crashed = begin_window_attempt(&repository, &batch_id, 3_000).await;
    let old = unavailable_membership("voucher\0old", HASH_A, "Old");
    repository
        .stage_snapshot_window_membership(&crashed, old.clone())
        .await
        .expect("stage before synthetic crash");
    let resumed = begin_window_attempt(&repository, &batch_id, 4_000).await;
    let addition = unavailable_membership("voucher\0addition", HASH_B, "Addition");
    repository
        .stage_snapshot_window_membership(&resumed, addition)
        .await
        .expect("stage addition on resumed attempt");
    assert!(matches!(
        repository
            .complete_snapshot_window_attempt(&resumed, 5_000, json!({}))
            .await,
        Err(MirrorError::WindowMembershipDisappeared)
    ));
    repository
        .stage_snapshot_window_membership(&resumed, old)
        .await
        .expect("re-observe pre-crash membership");
    let receipt = repository
        .complete_snapshot_window_attempt(&resumed, 5_001, json!({}))
        .await
        .expect("complete after exact full membership replay");
    assert_eq!(receipt.member_count, 2, "additions remain permitted");
    let crashed_state = sqlx::query_scalar::<_, String>(
        "SELECT state FROM tally_snapshot_window_attempts WHERE id = ?1",
    )
    .bind(crashed.attempt_id)
    .fetch_one(&repository.pool)
    .await
    .expect("read crashed attempt state");
    assert_eq!(crashed_state, "abandoned");
}

#[tokio::test]
async fn chunk_limit_and_owner_binding_fail_closed_without_mutation() {
    let (repository, snapshot, company) = seeded_repository().await;
    let batch_id = begin_batch(&repository, &snapshot, &company, "window-bounds-run").await;
    let attempt = begin_window_attempt(&repository, &batch_id, 3_000).await;
    let oversized = (0..=MAX_WINDOW_STAGE_CHUNK)
        .map(|index| unavailable_membership(&format!("voucher\0item-{index}"), HASH_A, "Synthetic"))
        .collect();
    assert!(matches!(
        repository
            .stage_snapshot_window_memberships(&attempt, oversized)
            .await,
        Err(MirrorError::InvalidInput("window_membership_chunk_size"))
    ));
    let forged = SnapshotWindowAttemptRef {
        window_id: "voucher:other".to_string(),
        ..attempt
    };
    assert!(matches!(
        repository
            .stage_snapshot_window_membership(
                &forged,
                unavailable_membership("voucher\0forged", HASH_A, "Forged")
            )
            .await,
        Err(MirrorError::NotFound)
    ));
    let membership_count =
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM tally_snapshot_window_memberships")
            .fetch_one(&repository.pool)
            .await
            .expect("count bounded memberships");
    assert_eq!(membership_count, 0);
}

#[tokio::test]
async fn explicit_window_abandon_clamps_clock_rollback_and_is_terminal_immutable() {
    let (repository, snapshot, company) = seeded_repository().await;
    let batch_id = begin_batch(&repository, &snapshot, &company, "window-abandon-run").await;
    let attempt = begin_window_attempt(&repository, &batch_id, 3_000).await;
    repository
        .stage_snapshot_window_membership(
            &attempt,
            unavailable_membership("voucher\0partial", HASH_A, "Partial"),
        )
        .await
        .expect("stage partial membership");
    assert!(matches!(
        repository
            .abandon_snapshot_window_attempt(&attempt, 0)
            .await,
        Err(MirrorError::InvalidInput("window_attempt_completed_at"))
    ));
    let forged = SnapshotWindowAttemptRef {
        window_id: "voucher:foreign".to_string(),
        ..attempt.clone()
    };
    assert!(matches!(
        repository
            .abandon_snapshot_window_attempt(&forged, 4_000)
            .await,
        Err(MirrorError::NotFound)
    ));
    let abandonment = repository
        .abandon_snapshot_window_attempt(&attempt, 2_999)
        .await
        .expect("clock rollback is clamped for terminal cleanup");
    assert_eq!(
        abandonment,
        AbandonSnapshotWindowAttemptResult {
            completed_at_unix_ms: 3_000,
            local_clock_moved_backwards: true,
        }
    );
    let stored = sqlx::query_as::<
        _,
        (
            String,
            Option<i64>,
            Option<String>,
            Option<String>,
            Option<String>,
        ),
    >(
        "SELECT state, completed_at_unix_ms, receipt_json, receipt_sha256, \
           terminal_safe_reason_code \
         FROM tally_snapshot_window_attempts WHERE id = ?1",
    )
    .bind(&attempt.attempt_id)
    .fetch_one(&repository.pool)
    .await
    .expect("read abandoned attempt");
    assert_eq!(
        stored,
        (
            "abandoned".to_string(),
            Some(3_000),
            None,
            None,
            Some("local_clock_moved_backwards".to_string()),
        )
    );
    let replayed = repository
        .abandon_snapshot_window_attempt(&attempt, 4_001)
        .await
        .expect("lost acknowledgement replays persisted abandonment evidence");
    assert_eq!(replayed, abandonment);
    assert!(matches!(
        repository
            .complete_snapshot_window_attempt(&attempt, 4_001, json!({}))
            .await,
        Err(MirrorError::WindowAttemptClosed)
    ));
    assert!(sqlx::query(
        "UPDATE tally_snapshot_window_attempts SET completed_at_unix_ms = 5000 WHERE id = ?1",
    )
    .bind(&attempt.attempt_id)
    .execute(&repository.pool)
    .await
    .is_err());
}

#[tokio::test]
async fn implicit_begin_abandonment_replays_cumulative_clock_evidence() {
    let (repository, snapshot, company) = seeded_repository().await;
    let batch_id = begin_batch(&repository, &snapshot, &company, "window-begin-clock-run").await;
    let first = repository
        .begin_snapshot_window_attempt(BeginSnapshotWindowAttemptInput {
            batch_id: batch_id.clone(),
            window_id: "voucher:clock-window".to_string(),
            started_at_unix_ms: 5_000,
        })
        .await
        .unwrap();
    assert!(first.prior_abandonment.is_none());
    let second = repository
        .begin_snapshot_window_attempt(BeginSnapshotWindowAttemptInput {
            batch_id: batch_id.clone(),
            window_id: "voucher:clock-window".to_string(),
            started_at_unix_ms: 4_000,
        })
        .await
        .unwrap();
    assert_eq!(
        second.prior_abandonment,
        Some(AbandonSnapshotWindowAttemptResult {
            completed_at_unix_ms: 5_000,
            local_clock_moved_backwards: true,
        })
    );
    let third = repository
        .begin_snapshot_window_attempt(BeginSnapshotWindowAttemptInput {
            batch_id,
            window_id: "voucher:clock-window".to_string(),
            started_at_unix_ms: 6_000,
        })
        .await
        .unwrap();
    assert_eq!(third.prior_abandonment, second.prior_abandonment);
    assert_eq!(third.attempt.attempt_ordinal, 3);
}

#[tokio::test]
async fn window_completion_clamps_and_reloads_clock_rollback_evidence() {
    let (repository, snapshot, company) = seeded_repository().await;
    let batch_id = begin_batch(
        &repository,
        &snapshot,
        &company,
        "window-complete-clock-run",
    )
    .await;
    let attempt = begin_window_attempt(&repository, &batch_id, 5_000).await;
    repository
        .stage_snapshot_window_membership(
            &attempt,
            unavailable_membership("voucher\0clock", HASH_A, "Clock"),
        )
        .await
        .unwrap();
    let completion = repository
        .complete_snapshot_window_attempt(&attempt, 4_999, json!({}))
        .await
        .expect("completion clamps rollback instead of stranding the run");
    assert!(completion.local_clock_moved_backwards);
    assert_eq!(completion.completed_at_unix_ms, 5_000);
    assert_eq!(
        repository
            .load_latest_completed_window_receipt(&batch_id, &attempt.window_id)
            .await
            .unwrap(),
        Some(completion)
    );
}

#[tokio::test]
async fn completed_window_attempt_cannot_be_abandoned() {
    let (repository, snapshot, company) = seeded_repository().await;
    let batch_id = begin_batch(&repository, &snapshot, &company, "window-complete-run").await;
    let attempt = begin_window_attempt(&repository, &batch_id, 3_000).await;
    repository
        .stage_snapshot_window_membership(
            &attempt,
            unavailable_membership("voucher\0complete", HASH_A, "Complete"),
        )
        .await
        .expect("stage complete membership");
    repository
        .complete_snapshot_window_attempt(&attempt, 4_000, json!({}))
        .await
        .expect("complete attempt");
    assert!(matches!(
        repository
            .abandon_snapshot_window_attempt(&attempt, 4_001)
            .await,
        Err(MirrorError::WindowAttemptClosed)
    ));
    let state = sqlx::query_scalar::<_, String>(
        "SELECT state FROM tally_snapshot_window_attempts WHERE id = ?1",
    )
    .bind(attempt.attempt_id)
    .fetch_one(&repository.pool)
    .await
    .expect("read complete state");
    assert_eq!(state, "complete");
}

#[tokio::test]
async fn commit_rejects_open_attempt_until_proof_bound_cleanup_completes() {
    let (repository, snapshot, company) = seeded_repository().await;
    let run_id = "orphan-window-terminal-run";
    let batch_id = begin_batch(&repository, &snapshot, &company, run_id).await;
    let attempt = begin_window_attempt(&repository, &batch_id, 3_000).await;
    repository
        .stage_snapshot_window_membership(
            &attempt,
            unavailable_membership("voucher\0orphan", HASH_A, "Orphan"),
        )
        .await
        .expect("stage unavailable orphan membership");

    let input = CommitBatchInput::test_only(CommitBatchParts {
        batch_id: batch_id.clone(),
        proof_contract_version: 2,
        outcome: RunOutcome::Failed,
        verification: VerificationState::Unverified,
        completed_at_unix_ms: 4_000,
        record_counts_sha256: None,
        snapshot_sha256: None,
        expected_checkpoint_before: None,
        checkpoint_after: None,
        freshness_target_seconds: 60,
        gap_codes: vec!["record_provenance_unavailable".to_string()],
        warning_codes: Vec::new(),
    });
    assert!(matches!(
        repository.commit_batch(input.clone()).await,
        Err(MirrorError::OpenWindowAttempts)
    ));
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT state FROM tally_snapshot_window_attempts WHERE id = ?1",
        )
        .bind(&attempt.attempt_id)
        .fetch_one(&repository.pool)
        .await
        .unwrap(),
        "open"
    );
    repository
        .abandon_open_snapshot_window_attempts_for_batch(&batch_id, 4_000)
        .await
        .expect("proof preparation closes attempts before commit");
    let receipt = repository
        .commit_batch(input)
        .await
        .expect("commit succeeds only after explicit evidence-bearing cleanup");
    assert_eq!(receipt.facts.provenance_unavailable_records, 1);
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT state FROM tally_snapshot_window_attempts WHERE id = ?1",
        )
        .bind(&attempt.attempt_id)
        .fetch_one(&repository.pool)
        .await
        .unwrap(),
        "abandoned"
    );
    assert!(matches!(
        repository
            .stage_snapshot_window_membership(
                &attempt,
                unavailable_membership("voucher\0late", HASH_B, "Late"),
            )
            .await,
        Err(MirrorError::WindowAttemptClosed)
    ));
    let recovered = repository
        .historical_commit_receipt_for_batch(&batch_id, run_id)
        .await
        .expect("v2 unavailable count is ledger-bound");
    assert_eq!(recovered.facts.provenance_unavailable_records, 1);
}

#[tokio::test]
async fn window_completion_clamps_pre_start_time_and_begin_handles_clock_rollback() {
    let (repository, snapshot, company) = seeded_repository().await;
    let batch_id = begin_batch(&repository, &snapshot, &company, "window-clock-run").await;
    let first = begin_window_attempt(&repository, &batch_id, 5_000).await;
    let completion = repository
        .complete_snapshot_window_attempt(&first, 4_999, json!({}))
        .await
        .expect("pre-start completion is clamped with durable rollback evidence");
    assert_eq!(completion.receipt.completed_at_unix_ms, 5_000);
    assert!(completion.local_clock_moved_backwards);
    let second = begin_window_attempt(&repository, &batch_id, 4_000).await;
    let first_terminal = sqlx::query_as::<_, (String, i64)>(
        "SELECT state, completed_at_unix_ms FROM tally_snapshot_window_attempts WHERE id = ?1",
    )
    .bind(&first.attempt_id)
    .fetch_one(&repository.pool)
    .await
    .expect("read clock-rollback abandonment");
    assert_eq!(first_terminal, ("complete".to_string(), 5_000));
    assert!(sqlx::query(
        "UPDATE tally_snapshot_window_attempts \
         SET state = 'abandoned', completed_at_unix_ms = 3999 WHERE id = ?1",
    )
    .bind(&second.attempt_id)
    .execute(&repository.pool)
    .await
    .is_err());
}

#[tokio::test]
async fn window_membership_receipt_digest_pages_without_changing_commitment() {
    let (repository, snapshot, company) = seeded_repository().await;
    let batch_id = begin_batch(&repository, &snapshot, &company, "window-paged-digest").await;
    let attempt = begin_window_attempt(&repository, &batch_id, 3_000).await;
    let memberships = (0..513)
        .map(|index| {
            unavailable_membership(
                &format!("voucher\0paged-{index:04}"),
                HASH_A,
                &format!("Paged {index:04}"),
            )
        })
        .collect::<Vec<_>>();
    for chunk in memberships.chunks(MAX_WINDOW_STAGE_CHUNK) {
        repository
            .stage_snapshot_window_memberships(&attempt, chunk.to_vec())
            .await
            .expect("stage bounded digest page fixture");
    }
    let expected_values = (0..513)
        .map(|index| {
            (
                format!("voucher\0paged-{index:04}"),
                HASH_A.to_string(),
                "unavailable".to_string(),
            )
        })
        .collect::<Vec<_>>();
    let expected_entries = expected_values
        .iter()
        .map(|(record_key, canonical_sha256, provenance_state)| {
            SnapshotWindowMembershipDigestEntry {
                record_key,
                canonical_sha256,
                provenance_state,
            }
        })
        .collect::<Vec<_>>();
    let expected_sha256 = sha256_json(&expected_entries).expect("hash legacy vector form");
    let receipt = repository
        .complete_snapshot_window_attempt(&attempt, 4_000, json!({}))
        .await
        .expect("complete paged digest attempt");
    assert_eq!(receipt.member_count, 513);
    assert_eq!(receipt.membership_sha256, expected_sha256);
    assert_eq!(
        repository
            .load_latest_completed_window_receipt(&batch_id, &attempt.window_id)
            .await
            .expect("revalidate paged receipt"),
        Some(receipt)
    );
}

#[tokio::test]
async fn earlier_v9_schema_upgrades_additively_and_preserves_v1_proof_hashes() {
    let repository = repository_through_v9().await;
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM pragma_table_info('tally_proof_ledger') \
             WHERE name = 'provenance_unavailable_records'",
        )
        .fetch_one(&repository.pool)
        .await
        .unwrap(),
        0
    );
    let (repository, snapshot, company) = seed_repository(repository).await;
    let run_id = "legacy-v1-proof-run";
    let batch_id = begin_batch(&repository, &snapshot, &company, run_id).await;
    let proof_id = Uuid::new_v4().to_string();
    let completed_at_unix_ms = 3_000;
    let created_at_unix_ms = 3_001;
    let empty_codes = Vec::<String>::new();
    let proof_sha256 = sha256_json(&ProofHashInput {
        proof_contract_version: 1,
        previous_entry_sha256: None,
        proof_id: &proof_id,
        run_id,
        batch_id: &batch_id,
        capability_snapshot_id: &snapshot.id,
        company_id: &company.id,
        pack_id: "core_accounting",
        outcome: RunOutcome::Failed,
        verification: VerificationState::Unverified,
        started_at_unix_ms: 2_000,
        completed_at_unix_ms,
        accepted_records: 0,
        rejected_records: 0,
        provenance_unavailable_records: None,
        record_counts_sha256: None,
        snapshot_sha256: None,
        checkpoint_before: None,
        checkpoint_after: None,
        gap_codes: &empty_codes,
        warning_codes: &empty_codes,
        created_at_unix_ms,
    })
    .unwrap();
    sqlx::query(
        "UPDATE tally_observation_batches SET state = 'failed', \
         completed_at_unix_ms = ?1 WHERE id = ?2",
    )
    .bind(completed_at_unix_ms)
    .bind(&batch_id)
    .execute(&repository.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO tally_proof_ledger(\
           id, proof_contract_version, previous_entry_sha256, entry_sha256, run_id, batch_id, \
           capability_snapshot_id, company_id, pack_id, outcome, verification_state, \
           started_at_unix_ms, completed_at_unix_ms, accepted_records, rejected_records, \
           snapshot_sha256, checkpoint_before, checkpoint_after, gap_codes_json, \
           warning_codes_json, created_at_unix_ms\
         ) VALUES (?1, 1, NULL, ?2, ?3, ?4, ?5, ?6, 'core_accounting', 'failed', \
           'unverified', 2000, ?7, 0, 0, NULL, NULL, NULL, '[]', '[]', ?8)",
    )
    .bind(&proof_id)
    .bind(&proof_sha256)
    .bind(run_id)
    .bind(&batch_id)
    .bind(&snapshot.id)
    .bind(&company.id)
    .bind(completed_at_unix_ms)
    .bind(created_at_unix_ms)
    .execute(&repository.pool)
    .await
    .unwrap();

    repository.migrate().await.expect("upgrade v9 through v12");
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_schema_migrations WHERE version = 10",
        )
        .fetch_one(&repository.pool)
        .await
        .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_schema_migrations WHERE version = 11",
        )
        .fetch_one(&repository.pool)
        .await
        .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_schema_migrations WHERE version = 12",
        )
        .fetch_one(&repository.pool)
        .await
        .unwrap(),
        1
    );
    let receipt = repository
        .historical_commit_receipt_for_batch(&batch_id, run_id)
        .await
        .expect("v1 receipt remains hash-valid after v10");
    assert_eq!(receipt.proof_sha256, proof_sha256);
    assert_eq!(receipt.facts.proof_contract_version, 1);
    assert_eq!(receipt.facts.provenance_unavailable_records, 0);
    assert_eq!(receipt.facts.record_counts_sha256, None);
}

#[tokio::test]
async fn migration_is_versioned_and_idempotent() {
    let repository = repository().await;
    repository.migrate().await.expect("reapply migration");
    let table_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name IN (\
         'tally_capability_snapshots', 'tally_companies', 'tally_observation_batches', \
         'tally_record_observations', 'tally_proof_ledger', 'tally_checkpoints')",
    )
    .fetch_one(&repository.pool)
    .await
    .expect("count mirror tables");
    let migration_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM tally_schema_migrations \
         WHERE version IN (2, 3, 4, 5, 6, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28)",
    )
    .fetch_one(&repository.pool)
    .await
    .expect("count migration marker");
    assert_eq!(table_count, 6);
    assert_eq!(migration_count, 25);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_schema_migrations WHERE version = 7",
        )
        .fetch_one(&repository.pool)
        .await
        .expect("read retired GUID-only migration marker"),
        0
    );
    for (version, expected) in [(24, 0_i64), (25, 1_i64)] {
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM tally_schema_migrations WHERE version = ?1",
            )
            .bind(version)
            .fetch_one(&repository.pool)
            .await
            .expect("read v24/v25 compatibility marker"),
            expected
        );
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'trigger' \
             AND name = 'tally_schema_migrations_reject_legacy_v24_after_observed_identity'",
        )
        .fetch_one(&repository.pool)
        .await
        .expect("read v24 compatibility trigger"),
        1
    );
    let target_binding_columns = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM pragma_table_info('tally_write_canary_preflight_evidence') \
         WHERE name IN ('canonical_endpoint_sha256', 'company_identity_sha256')",
    )
    .fetch_one(&repository.pool)
    .await
    .expect("count target-binding columns");
    assert_eq!(target_binding_columns, 2);
    let target_binding_trigger = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'trigger' \
         AND name = 'tally_write_canary_preflight_evidence_requires_target_binding'",
    )
    .fetch_one(&repository.pool)
    .await
    .expect("count target-binding trigger");
    assert_eq!(target_binding_trigger, 1);
    let snapshot_state_table = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM sqlite_master \
         WHERE type = 'table' AND name = 'tally_snapshot_run_states'",
    )
    .fetch_one(&repository.pool)
    .await
    .expect("count snapshot state table");
    assert_eq!(snapshot_state_table, 1);
    let incremental_table_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name IN (\
         'tally_incremental_capability_observations', \
         'tally_incremental_establishment_receipts', \
         'tally_incremental_checkpoint_heads')",
    )
    .fetch_one(&repository.pool)
    .await
    .expect("count incremental foundation tables");
    let incremental_trigger_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'trigger' AND name LIKE \
         'tally_incremental_%'",
    )
    .fetch_one(&repository.pool)
    .await
    .expect("count incremental immutability triggers");
    assert_eq!(incremental_table_count, 3);
    assert_eq!(incremental_trigger_count, 9);
    let selected_read_tables = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name IN (\
         'tally_selected_read_scopes', 'tally_selected_read_observations')",
    )
    .fetch_one(&repository.pool)
    .await
    .expect("count selected-read evidence tables");
    assert_eq!(selected_read_tables, 2);
    let reviewed_setup_consumption_tables = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' \
         AND name = 'tally_reviewed_setup_consumptions'",
    )
    .fetch_one(&repository.pool)
    .await
    .expect("count reviewed-setup consumption table");
    assert_eq!(reviewed_setup_consumption_tables, 1);
    let write_fixture_tables = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name IN (\
         'tally_write_fixture_enrollments', 'tally_write_fixture_revocations', \
         'tally_write_canary_reservations', 'tally_write_canary_payload_bindings')",
    )
    .fetch_one(&repository.pool)
    .await
    .expect("count durable write fixture tables");
    assert_eq!(write_fixture_tables, 4);
    let normalized_staging_tables = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name IN (\
         'tally_snapshot_window_attempts', 'tally_snapshot_window_memberships')",
    )
    .fetch_one(&repository.pool)
    .await
    .expect("count normalized staging tables");
    assert_eq!(normalized_staging_tables, 2);
    let identity_index_sql = sqlx::query_scalar::<_, String>(
        "SELECT sql FROM sqlite_master WHERE type = 'index' AND name = 'uq_tally_companies_observed_identity'",
    )
    .fetch_one(&repository.pool)
    .await
    .expect("read composite company identity index");
    assert!(identity_index_sql.contains("company_guid COLLATE NOCASE"));
    assert!(identity_index_sql.contains("company_number"));
    assert!(identity_index_sql.contains("books_from_yyyymmdd"));
    let applied_at = sqlx::query_scalar::<_, i64>(
        "SELECT MIN(applied_at_unix_ms) FROM tally_schema_migrations \
         WHERE version IN (2, 3, 4, 5, 6, 8, 9)",
    )
    .fetch_one(&repository.pool)
    .await
    .expect("read migration timestamp");
    assert!(applied_at > 0, "migration marker must contain real time");
}

#[tokio::test]
async fn bootstrap_markers_preserve_retired_objects_on_reopen() {
    let repository = repository().await;
    for (index, drop_index) in [
        (
            "idx_tally_capability_snapshots_endpoint_observed",
            "DROP INDEX idx_tally_capability_snapshots_endpoint_observed",
        ),
        (
            "idx_tally_import_jobs_company_state",
            "DROP INDEX idx_tally_import_jobs_company_state",
        ),
        (
            "idx_tally_snapshot_run_states_run",
            "DROP INDEX idx_tally_snapshot_run_states_run",
        ),
    ] {
        sqlx::query(drop_index)
            .execute(&repository.pool)
            .await
            .expect("retire bootstrap-created index");
        repository
            .migrate()
            .await
            .expect("marker-gated reopen must not recreate retired index");
        let present = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = ?1",
        )
        .bind(index)
        .fetch_one(&repository.pool)
        .await
        .expect("inspect retired index");
        assert_eq!(present, 0, "reopen recreated {index}");
    }
}

#[tokio::test]
async fn bootstrap_marker_upgrade_paths_install_remaining_schema() {
    for migrations in [
        vec![MIRROR_MIGRATION_V2],
        vec![MIRROR_MIGRATION_V2, MIRROR_MIGRATION_V3],
    ] {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect pre-bootstrap migration mirror");
        let mut transaction = pool.begin().await.expect("begin pre-bootstrap migration");
        for migration in migrations {
            sqlx::raw_sql(migration)
                .execute(&mut *transaction)
                .await
                .expect("apply historical bootstrap migration");
        }
        transaction
            .commit()
            .await
            .expect("commit historical bootstrap migration");

        let repository = TallyMirrorRepository::new(pool);
        repository
            .migrate()
            .await
            .expect("upgrade from historical bootstrap marker");
        for version in [2, 3, 4, 27] {
            assert_eq!(
                sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(*) FROM tally_schema_migrations WHERE version = ?1",
                )
                .bind(version)
                .fetch_one(&repository.pool)
                .await
                .expect("inspect installed migration marker"),
                1,
                "upgrade did not retain or install v{version}"
            );
        }
    }
}

#[tokio::test]
async fn legacy_reopened_v26_mirror_repairs_guid_index_before_shared_book_insert() {
    let directory = tempfile::tempdir().expect("temporary mirror directory");
    let path = directory.path().join("mirror.sqlite3");
    let old = file_repository(&path).await;
    old.migrate().await.expect("install current mirror schema");
    sqlx::query(
        "INSERT INTO tally_endpoints(id, canonical_origin, created_at_unix_ms, last_observed_at_unix_ms) \
         VALUES ('endpoint', 'http://127.0.0.1:9000', 1, 1)",
    )
    .execute(&old.pool)
    .await
    .expect("seed endpoint before repair");
    sqlx::query(
        "INSERT INTO tally_companies(\
           id, endpoint_id, display_name, company_guid, company_number, books_from_yyyymmdd, \
           identity_confidence, first_observed_at_unix_ms, last_observed_at_unix_ms\
         ) VALUES ('preserved', 'endpoint', 'Synthetic Preserved', 'preserved-guid', '0', \
                   '20230401', 'observed', 1, 1)",
    )
    .execute(&old.pool)
    .await
    .expect("seed company before repair");
    sqlx::query("DELETE FROM tally_schema_migrations WHERE version = 27")
        .execute(&old.pool)
        .await
        .expect("model pre-v27 mirror");
    sqlx::raw_sql(MIRROR_MIGRATION_V2)
        .execute(&old.pool)
        .await
        .expect("model legacy bootstrap reopen");
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = 'uq_tally_companies_guid'",
        )
        .fetch_one(&old.pool)
        .await
        .expect("inspect resurrected GUID-only index"),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_schema_migrations WHERE version = 26",
        )
        .fetch_one(&old.pool)
        .await
        .expect("inspect pre-fix v26 marker"),
        1
    );
    old.pool.close().await;

    let repaired = file_repository(&path).await;
    repaired
        .migrate()
        .await
        .expect("repair legacy-reopened v26 mirror");
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = 'uq_tally_companies_guid'",
        )
        .fetch_one(&repaired.pool)
        .await
        .expect("inspect retired GUID-only index"),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_companies WHERE id = 'preserved' AND company_guid = 'preserved-guid'",
        )
        .fetch_one(&repaired.pool)
        .await
        .expect("inspect preserved company"),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_schema_migrations WHERE version = 27",
        )
        .fetch_one(&repaired.pool)
        .await
        .expect("inspect initial v27 repair marker"),
        1
    );
    sqlx::raw_sql(MIRROR_MIGRATION_V2)
        .execute(&repaired.pool)
        .await
        .expect("model downgrade reopen after v27 repair");
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = 'uq_tally_companies_guid'",
        )
        .fetch_one(&repaired.pool)
        .await
        .expect("inspect post-v27 resurrected GUID-only index"),
        1
    );
    repaired.pool.close().await;

    let reopened = file_repository(&path).await;
    reopened
        .migrate()
        .await
        .expect("repair downgrade-reopened shared-GUID mirror");
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = 'uq_tally_companies_guid'",
        )
        .fetch_one(&reopened.pool)
        .await
        .expect("inspect repaired downgrade GUID-only index"),
        0
    );
    for (id, number, name, books_from) in [
        ("parent", "1", "Synthetic Parent", "20240401"),
        ("split", "2", "Synthetic Split", "20250401"),
    ] {
        sqlx::query(
            "INSERT INTO tally_companies(\
               id, endpoint_id, display_name, company_guid, company_number, books_from_yyyymmdd, \
               identity_confidence, first_observed_at_unix_ms, last_observed_at_unix_ms\
             ) VALUES (?1, 'endpoint', ?2, 'shared-guid', ?3, ?4, 'observed', 1, 1)",
        )
        .bind(id)
        .bind(name)
        .bind(number)
        .bind(books_from)
        .execute(&reopened.pool)
        .await
        .expect("insert distinct shared-GUID book");
    }
    let duplicate = sqlx::query(
        "INSERT INTO tally_companies(\
           id, endpoint_id, display_name, company_guid, company_number, books_from_yyyymmdd, \
           identity_confidence, first_observed_at_unix_ms, last_observed_at_unix_ms\
         ) VALUES ('duplicate', 'endpoint', 'Synthetic Parent', 'shared-guid', '1', \
                   '20240401', 'observed', 1, 1)",
    )
    .execute(&reopened.pool)
    .await
    .expect_err("composite identity must still reject an exact duplicate");
    let sqlx::Error::Database(error) = duplicate else {
        panic!("exact composite duplicate must return a SQLite database error");
    };
    assert!(error.is_unique_violation());
    reopened.pool.close().await;

    let final_reopen = file_repository(&path).await;
    final_reopen
        .migrate()
        .await
        .expect("reopen existing shared-GUID books");
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_companies WHERE endpoint_id = 'endpoint' AND company_guid = 'shared-guid'",
        )
        .fetch_one(&final_reopen.pool)
        .await
        .expect("count shared-GUID books"),
        2
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_schema_migrations WHERE version = 27",
        )
        .fetch_one(&final_reopen.pool)
        .await
        .expect("inspect repair marker"),
        1
    );
    final_reopen.pool.close().await;
}

fn assert_observed_identity_constraint(error: sqlx::Error) {
    let sqlx::Error::Database(database_error) = error else {
        panic!("incomplete observed tuple must fail with a database error");
    };
    assert_eq!(
        database_error.message(),
        "observed company identity requires complete tuple"
    );
}

fn assert_legacy_v24_rollback_barrier(error: sqlx::Error) {
    let sqlx::Error::Database(database_error) = error else {
        panic!("v24 rollback must fail with a database error");
    };
    assert_eq!(
        database_error.message(),
        "observed company identity requires a compatible Bridge binary"
    );
}

#[tokio::test]
async fn observed_company_identity_constraint_rejects_incomplete_tuples_and_preserves_reobservation(
) {
    let repository = repository_through_v24().await;
    let snapshot = repository
        .save_capability_snapshot(CapabilitySnapshotInput {
            canonical_origin: "http://127.0.0.1:9000".to_string(),
            observed_at_unix_ms: 1_000,
            profile_version: 1,
            product: "TallyPrime".to_string(),
            release: None,
            license_tier: None,
            mode: Some("Education".to_string()),
            mode_confidence: Confidence::Observed,
            items: vec![],
        })
        .await
        .expect("seed endpoint for v25 migration");
    for (id, guid, company_number, books_from_yyyymmdd, confidence) in [
        (
            "legacy-observed-incomplete",
            Some("legacy-observed-guid"),
            Some(" \t\r\n"),
            Some("20260401"),
            "observed",
        ),
        (
            "legacy-unknown-untouched",
            Some("legacy-unknown-guid"),
            None,
            None,
            "unknown",
        ),
    ] {
        sqlx::query(
            "INSERT INTO tally_companies(\
               id, endpoint_id, display_name, company_guid, remote_id, master_id, \
               fallback_fingerprint, identity_confidence, first_observed_at_unix_ms, \
               last_observed_at_unix_ms, company_number, books_from_yyyymmdd\
             ) VALUES (?1, ?2, ?3, ?4, NULL, NULL, NULL, ?5, 1, 1, ?6, ?7)",
        )
        .bind(id)
        .bind(&snapshot.endpoint_id)
        .bind(id)
        .bind(guid)
        .bind(confidence)
        .bind(company_number)
        .bind(books_from_yyyymmdd)
        .execute(&repository.pool)
        .await
        .expect("seed pre-v25 company row");
    }

    sqlx::raw_sql(MIRROR_MIGRATION_V25)
        .execute(&repository.pool)
        .await
        .expect("apply v25 migration to the v24 fixture");
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT identity_confidence FROM tally_companies WHERE id = 'legacy-observed-incomplete'",
        )
        .fetch_one(&repository.pool)
        .await
        .expect("read demoted legacy row"),
        "unknown"
    );
    assert_eq!(
        sqlx::query_scalar::<_, Option<String>>(
            "SELECT company_number FROM tally_companies WHERE id = 'legacy-unknown-untouched'",
        )
        .fetch_one(&repository.pool)
        .await
        .expect("read untouched v23 unknown row"),
        None,
        "v25 must not invent a tuple for the unknown row created by v23"
    );

    for (id, guid, company_number, books_from_yyyymmdd) in [
        ("null-guid", None, Some("100001"), Some("20260401")),
        ("empty-guid", Some(""), Some("100002"), Some("20260401")),
        (
            "blank-guid",
            Some(" \t\r\n"),
            Some("100003"),
            Some("20260401"),
        ),
        (
            "null-number",
            Some("null-number-guid"),
            None,
            Some("20260401"),
        ),
        (
            "empty-number",
            Some("empty-number-guid"),
            Some(""),
            Some("20260401"),
        ),
        (
            "blank-number",
            Some("blank-number-guid"),
            Some(" \t\r\n"),
            Some("20260401"),
        ),
        ("null-books", Some("null-books-guid"), Some("100007"), None),
        (
            "empty-books",
            Some("empty-books-guid"),
            Some("100008"),
            Some(""),
        ),
        (
            "blank-books",
            Some("blank-books-guid"),
            Some("100009"),
            Some(" \t\r\n"),
        ),
    ] {
        let error = sqlx::query(
            "INSERT INTO tally_companies(\
               id, endpoint_id, display_name, company_guid, remote_id, master_id, \
               fallback_fingerprint, identity_confidence, first_observed_at_unix_ms, \
               last_observed_at_unix_ms, company_number, books_from_yyyymmdd\
             ) VALUES (?1, ?2, ?3, ?4, NULL, NULL, NULL, 'observed', 1, 1, ?5, ?6)",
        )
        .bind(id)
        .bind(&snapshot.endpoint_id)
        .bind(id)
        .bind(guid)
        .bind(company_number)
        .bind(books_from_yyyymmdd)
        .execute(&repository.pool)
        .await
        .expect_err("incomplete observed tuple must be rejected");
        assert_observed_identity_constraint(error);
    }

    sqlx::query(
        "INSERT INTO tally_companies(\
           id, endpoint_id, display_name, company_guid, remote_id, master_id, \
           fallback_fingerprint, identity_confidence, first_observed_at_unix_ms, \
           last_observed_at_unix_ms, company_number, books_from_yyyymmdd\
         ) VALUES ('complete-observed', ?1, 'Complete observed', 'complete-guid', NULL, NULL, NULL, \
                   'observed', 1, 1, '100010', '20260401')",
    )
    .bind(&snapshot.endpoint_id)
    .execute(&repository.pool)
    .await
    .expect("complete observed tuple remains accepted");
    let update_error = sqlx::query(
        "UPDATE tally_companies SET books_from_yyyymmdd = ' \t\r\n' \
         WHERE id = 'complete-observed'",
    )
    .execute(&repository.pool)
    .await
    .expect_err("updates cannot weaken an observed tuple");
    assert_observed_identity_constraint(update_error);

    for (id, display_name) in [
        ("null-name", None),
        ("empty-name", Some("")),
        ("blank-name", Some(" \t\r\n")),
    ] {
        let error = sqlx::query(
            "INSERT INTO tally_companies(\
               id, endpoint_id, display_name, company_guid, remote_id, master_id, \
               fallback_fingerprint, identity_confidence, first_observed_at_unix_ms, \
               last_observed_at_unix_ms, company_number, books_from_yyyymmdd\
             ) VALUES (?1, ?2, ?3, 'complete-name-guid', NULL, NULL, NULL, \
                       'observed', 1, 1, '100011', '20260401')",
        )
        .bind(id)
        .bind(&snapshot.endpoint_id)
        .bind(display_name)
        .execute(&repository.pool)
        .await
        .expect_err("observed company name must be nonblank");
        assert_observed_identity_constraint(error);
    }
    let update_name_error = sqlx::query(
        "UPDATE tally_companies SET display_name = ' \t\r\n' \
         WHERE id = 'complete-observed'",
    )
    .execute(&repository.pool)
    .await
    .expect_err("updates cannot blank an observed company name");
    assert_observed_identity_constraint(update_name_error);

    let mut legacy_v24_transaction = repository
        .pool
        .begin()
        .await
        .expect("begin v24 rollback migration");
    sqlx::query("PRAGMA legacy_alter_table = ON")
        .execute(&mut *legacy_v24_transaction)
        .await
        .expect("enable legacy alter table for v24 rollback");
    let legacy_v24_error = sqlx::raw_sql(MIRROR_MIGRATION_V24)
        .execute(&mut *legacy_v24_transaction)
        .await
        .expect_err("v24 binary must fail before generic GUID-only writes");
    assert_legacy_v24_rollback_barrier(legacy_v24_error);
    legacy_v24_transaction
        .rollback()
        .await
        .expect("roll back rejected v24 migration");

    let mut reobservation = reviewed_setup_input(HASH_A);
    reobservation.company_display_name = "legacy-observed-incomplete".to_string();
    reobservation.company_identity.guid = Some("legacy-observed-guid".to_string());
    reobservation.company_number = "100011".to_string();
    let saved = repository
        .save_reviewed_setup(reobservation)
        .await
        .expect("complete re-observation creates a distinct observed company row");
    assert_ne!(saved.company.id, "legacy-observed-incomplete");
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT identity_confidence FROM tally_companies WHERE id = ?1",
        )
        .bind(&saved.company.id)
        .fetch_one(&repository.pool)
        .await
        .expect("read re-observed confidence"),
        "observed"
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT identity_confidence FROM tally_companies \
             WHERE id = 'legacy-observed-incomplete'",
        )
        .fetch_one(&repository.pool)
        .await
        .expect("read preserved legacy company confidence"),
        "unknown",
        "a later observed tuple must not reclassify the legacy pin"
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT company_number FROM tally_companies \
             WHERE id = 'legacy-observed-incomplete'",
        )
        .fetch_one(&repository.pool)
        .await
        .expect("read preserved legacy company tuple"),
        " \t\r\n",
        "a later observed tuple must not overwrite a legacy pin's identity"
    );

    let generic = repository
        .upsert_company(CompanyInput {
            endpoint_id: snapshot.endpoint_id.clone(),
            display_name: "Documented metadata only".to_string(),
            identity: SourceIdentityInput {
                guid: Some("documented-metadata-guid".to_string()),
                confidence: Some(Confidence::Observed),
                ..Default::default()
            },
            observed_at_unix_ms: 2_000,
        })
        .await
        .expect("generic metadata writer remains valid without an observed tuple");
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT identity_confidence FROM tally_companies WHERE id = ?1",
        )
        .bind(generic.id)
        .fetch_one(&repository.pool)
        .await
        .expect("read documented generic company confidence"),
        "documented"
    );

    for (label, confidence) in [
        ("unknown", Confidence::Unknown),
        ("inferred", Confidence::Inferred),
    ] {
        let identity_guid = format!("{label}-generic-metadata-guid");
        let initial = repository
            .upsert_company(CompanyInput {
                endpoint_id: snapshot.endpoint_id.clone(),
                display_name: format!("{label} generic metadata"),
                identity: SourceIdentityInput {
                    guid: Some(identity_guid.clone()),
                    confidence: Some(confidence),
                    ..Default::default()
                },
                observed_at_unix_ms: 2_100,
            })
            .await
            .expect("save explicitly non-observed generic company");
        let refreshed = repository
            .upsert_company(CompanyInput {
                endpoint_id: snapshot.endpoint_id.clone(),
                display_name: format!("{label} generic metadata refresh"),
                identity: SourceIdentityInput {
                    guid: Some(identity_guid),
                    confidence: Some(confidence),
                    ..Default::default()
                },
                observed_at_unix_ms: 2_200,
            })
            .await
            .expect("refresh explicitly non-observed generic company");
        assert_eq!(refreshed.id, initial.id);
        assert_eq!(
            sqlx::query_scalar::<_, String>(
                "SELECT identity_confidence FROM tally_companies WHERE id = ?1",
            )
            .bind(initial.id)
            .fetch_one(&repository.pool)
            .await
            .expect("read preserved generic confidence"),
            confidence.as_str(),
            "a generic {label} refresh must preserve its explicit confidence"
        );
    }

    let reviewed = repository
        .save_reviewed_setup(reviewed_setup_input(HASH_B))
        .await
        .expect("save independently reviewed observed company");
    let pin_before_refresh = repository
        .snapshot_source_pin(&reviewed.company.id)
        .await
        .expect("reviewed company supplies a durable source pin");
    let profile_before_refresh = repository
        .persisted_company_profiles()
        .await
        .expect("load reviewed company profile before generic refresh")
        .profiles
        .into_iter()
        .find(|profile| profile.mirror_company_id == reviewed.company.id)
        .expect("find reviewed company profile before generic refresh");
    let refreshed = repository
        .upsert_company(CompanyInput {
            endpoint_id: reviewed.snapshot.endpoint_id,
            display_name: "Synthetic reviewed metadata refresh".to_string(),
            identity: SourceIdentityInput {
                guid: Some("reviewed-company-guid".to_string()),
                confidence: Some(Confidence::Observed),
                ..Default::default()
            },
            observed_at_unix_ms: 3_000,
        })
        .await
        .expect("generic metadata refresh remains valid");
    assert_eq!(refreshed.id, reviewed.company.id);
    let pin_after_refresh = repository
        .snapshot_source_pin(&reviewed.company.id)
        .await
        .expect("generic metadata refresh preserves the observed source pin");
    assert_eq!(pin_after_refresh.company_id, pin_before_refresh.company_id);
    assert_eq!(
        pin_after_refresh.display_name,
        pin_before_refresh.display_name
    );
    assert_eq!(
        pin_after_refresh.endpoint_id,
        pin_before_refresh.endpoint_id
    );
    assert_eq!(
        pin_after_refresh.company_guid,
        pin_before_refresh.company_guid
    );
    assert_eq!(
        pin_after_refresh.company_number,
        pin_before_refresh.company_number
    );
    assert_eq!(
        pin_after_refresh.books_from_yyyymmdd,
        pin_before_refresh.books_from_yyyymmdd
    );
    let profile_after_refresh = repository
        .persisted_company_profiles()
        .await
        .expect("load reviewed company profile after generic refresh")
        .profiles
        .into_iter()
        .find(|profile| profile.mirror_company_id == reviewed.company.id)
        .expect("find reviewed company profile after generic refresh");
    assert_eq!(profile_after_refresh.name, profile_before_refresh.name);
    assert_eq!(
        profile_after_refresh.correlation_key,
        profile_before_refresh.correlation_key
    );
}

#[tokio::test]
async fn observed_identity_migration_revokes_demoted_fixture_before_preflight_gate() {
    let repository = repository_through_v24().await;
    let saved = repository
        .save_reviewed_setup(reviewed_setup_input(HASH_A))
        .await
        .expect("seed complete observed company before v25");
    repository
        .enroll_write_fixture(WriteFixtureEnrollmentInput {
            company_id: saved.company.id.clone(),
            review_commitment_sha256: HASH_B.to_string(),
            disposable_company_attested: true,
            no_customer_data_attested: true,
            backup_guidance_acknowledged: true,
            enrolled_at_unix_ms: 3_000,
        })
        .await
        .expect("enroll active fixture before v25");
    let reservation = repository
        .reserve_write_canary(WriteCanaryReservationInput {
            company_id: saved.company.id.clone(),
            review_commitment_sha256: HASH_B.to_string(),
            reserved_at_unix_ms: 4_000,
        })
        .await
        .expect("reserve active fixture before v25");
    let binding_input = WriteCanaryPayloadBindingInput {
        company_id: saved.company.id.clone(),
        review_commitment_sha256: HASH_B.to_string(),
        reservation_id: reservation.id.clone(),
        reservation_payload_sha256: reservation.reservation_payload_sha256.clone(),
        wire_sha256: HASH_A.to_string(),
        intended_state_sha256: HASH_B.to_string(),
        identity_query_sha256: HASH_A.to_string(),
        bound_at_unix_ms: 5_000,
    };
    let binding = repository
        .bind_write_canary_payload(binding_input.clone())
        .await
        .expect("bind active fixture before v25");
    let active_binding = ActiveWriteCanaryPayloadBindingInput {
        company_id: binding_input.company_id.clone(),
        review_commitment_sha256: binding_input.review_commitment_sha256.clone(),
        reservation_id: binding_input.reservation_id.clone(),
        reservation_payload_sha256: binding_input.reservation_payload_sha256.clone(),
        wire_sha256: binding_input.wire_sha256.clone(),
        intended_state_sha256: binding_input.intended_state_sha256.clone(),
        identity_query_sha256: binding_input.identity_query_sha256.clone(),
    };
    let preflight = repository
        .begin_write_canary_preflight(BeginWriteCanaryPreflightInput {
            binding: active_binding.clone(),
            started_at_unix_ms: 5_500,
        })
        .await
        .expect("start active preflight before v25");
    assert_eq!(preflight.payload_binding_id, binding.id);
    let evidence = repository
        .record_write_canary_preflight_evidence(WriteCanaryPreflightEvidenceInput {
            attempt_id: preflight.id.clone(),
            readback_state_sha256: HASH_A.to_string(),
            identity_coverage_sha256: HASH_B.to_string(),
            canonical_endpoint_sha256: HASH_A.to_string(),
            company_identity_sha256: HASH_B.to_string(),
            verified_at_unix_ms: 5_750,
        })
        .await
        .expect("record active preflight before v25");
    let preflight_gate = ActiveWriteCanaryPreflightEvidenceInput {
        binding: active_binding,
        attempt_id: preflight.id,
        evidence_id: evidence.id,
        readback_state_sha256: HASH_A.to_string(),
        identity_coverage_sha256: HASH_B.to_string(),
        canonical_endpoint_sha256: HASH_A.to_string(),
        company_identity_sha256: HASH_B.to_string(),
    };
    repository
        .active_write_canary_preflight_evidence(preflight_gate.clone())
        .await
        .expect("fixture preflight is active before v25 demotion");

    sqlx::query(
        "UPDATE tally_companies \
         SET display_name = char(9) || char(10) || char(13) || ' ' \
         WHERE id = ?1",
    )
    .bind(&saved.company.id)
    .execute(&repository.pool)
    .await
    .expect("seed v25-incomplete observed company");
    repository
        .migrate()
        .await
        .expect("v25 demotes and revokes the fixture enrollment atomically");

    assert_eq!(
        repository
            .write_fixture_enrollment_status(&saved.company.id)
            .await
            .expect("read v25 migration enrollment status")
            .fixture_state,
        "revoked"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_write_fixture_revocations AS revocation \
             JOIN tally_write_fixture_enrollments AS enrollment \
               ON enrollment.id = revocation.enrollment_id \
             WHERE enrollment.company_id = ?1",
        )
        .bind(&saved.company.id)
        .fetch_one(&repository.pool)
        .await
        .expect("count v25 migration revocations"),
        1,
        "the active enrollment for the demoted company must be revoked"
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT identity_confidence FROM tally_companies WHERE id = ?1",
        )
        .bind(&saved.company.id)
        .fetch_one(&repository.pool)
        .await
        .expect("read v25-demoted company confidence"),
        "unknown"
    );
    let revocation = sqlx::query(
        "SELECT revocation.safe_reason_code, revocation.revocation_payload_sha256, \
                revocation.revoked_at_unix_ms, enrollment.id, enrollment.enrollment_payload_sha256 \
         FROM tally_write_fixture_revocations AS revocation \
         JOIN tally_write_fixture_enrollments AS enrollment \
           ON enrollment.id = revocation.enrollment_id \
         WHERE enrollment.company_id = ?1",
    )
    .bind(&saved.company.id)
    .fetch_one(&repository.pool)
    .await
    .expect("read v25 migration revocation");
    assert_eq!(
        revocation
            .try_get::<String, _>("safe_reason_code")
            .expect("read v25 migration reason"),
        "company_identity_reverification_required"
    );
    assert_eq!(
        revocation
            .try_get::<String, _>("revocation_payload_sha256")
            .expect("read v25 migration payload"),
        fixture_revocation_payload_sha256(
            &revocation
                .try_get::<String, _>("id")
                .expect("read v25 enrollment id"),
            &revocation
                .try_get::<String, _>("enrollment_payload_sha256")
                .expect("read v25 enrollment payload"),
            "company_identity_reverification_required",
            revocation
                .try_get("revoked_at_unix_ms")
                .expect("read v25 migration revocation timestamp"),
        )
        .expect("compute canonical v25 migration revocation payload")
    );
    assert!(matches!(
        repository
            .active_write_canary_preflight_evidence(preflight_gate)
            .await,
        Err(MirrorError::InvalidInput(
            "canary_preflight_evidence_not_active"
        ))
    ));
}

#[tokio::test]
async fn generic_guid_upsert_maps_observed_to_documented_before_tuple_constraint() {
    let (repository, snapshot, _) = seeded_repository().await;
    sqlx::query(
        "INSERT INTO tally_companies(\
           id, endpoint_id, display_name, company_guid, remote_id, master_id, \
           fallback_fingerprint, identity_confidence, first_observed_at_unix_ms, \
           last_observed_at_unix_ms, company_number, books_from_yyyymmdd\
         ) VALUES ('legacy-incomplete-generic', ?1, 'Legacy generic metadata', \
                   'generic-constraint-guid', NULL, NULL, NULL, 'unknown', 1, 1, NULL, NULL)",
    )
    .bind(&snapshot.endpoint_id)
    .execute(&repository.pool)
    .await
    .expect("seed incomplete non-observed legacy company");

    let saved = repository
        .upsert_company(CompanyInput {
            endpoint_id: snapshot.endpoint_id,
            display_name: "Generic metadata refresh".to_string(),
            identity: SourceIdentityInput {
                guid: Some("generic-constraint-guid".to_string()),
                confidence: Some(Confidence::Observed),
                ..Default::default()
            },
            observed_at_unix_ms: 2_000,
        })
        .await
        .expect("generic GUID metadata must not claim observed without a tuple");

    assert_eq!(saved.id, "legacy-incomplete-generic");
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT identity_confidence FROM tally_companies WHERE id = ?1",
        )
        .bind(saved.id)
        .fetch_one(&repository.pool)
        .await
        .expect("read generic confidence after refresh"),
        "documented"
    );
}

#[tokio::test]
async fn bomless_utf16_migration_preserves_existing_selected_read_evidence() {
    let repository = repository_through_v21().await;
    let legacy_scope_commitment =
        selected_read_scope_commitment_sha256_v1(&SelectedReadScopeCommitmentMaterialV1 {
            parent_review_commitment_sha256: HASH_B.to_string(),
            canonical_origin: "http://127.0.0.1:9000".to_string(),
            company_guid_ascii_casefolded: "company-guid".to_string(),
            ledger_profile_id: "bridge.tally.ledgers/1".to_string(),
            voucher_profile_id: "bridge.tally.vouchers/3".to_string(),
            voucher_from_yyyymmdd: "20260701".to_string(),
            voucher_to_yyyymmdd: "20260731".to_string(),
            observed_at_unix_ms: 2,
            observations: vec![SelectedReadObservationCommitmentMaterial {
                capability_key: "selected_ledger_read".to_string(),
                state: "supported".to_string(),
                confidence: "observed".to_string(),
                safe_reason_code: "selected_ledger_read_non_empty_observed".to_string(),
                result_bucket: "non_empty_observed".to_string(),
                request_sha256: Some(HASH_A.to_string()),
                decoded_response_sha256: Some(HASH_B.to_string()),
                response_encoding: Some("utf8".to_string()),
                company_context_verified: true,
                schema_verified: true,
                record_count_verified: true,
                identity_evidence_state: "verified".to_string(),
                date_window_verified: false,
            }],
        })
        .expect("compute historic v1 scope commitment");
    sqlx::raw_sql(
        "INSERT INTO tally_endpoints VALUES ('ep', 'http://127.0.0.1:9000', 1, 2);\
         INSERT INTO tally_capability_snapshots(id, endpoint_id, observed_at_unix_ms, profile_version, product, release, mode, mode_confidence) VALUES ('snap', 'ep', 1, 3, 'Unknown', NULL, NULL, 'unknown');\
         INSERT INTO tally_capability_items VALUES (\
           'snap', 'feature', 'selected_ledger_read', 'supported', 'observed',\
           'selected_ledger_read_non_empty_observed'\
         );\
         INSERT INTO tally_companies VALUES (\
           'company', 'ep', 'Synthetic', 'company-guid', NULL, NULL, NULL, 'observed', 1, 2\
         );"
    )
    .execute(&repository.pool)
    .await
    .expect("seed v21 selected-read evidence");
    sqlx::query(
        "INSERT INTO tally_selected_read_scopes VALUES (\
           'scope', 'snap', 'company', 1, ?1, ?2,\
           'bridge.tally.ledgers/1', 'bridge.tally.vouchers/3',\
           '20260701', '20260731', 2, 'not_claimed', 1, 0\
         )",
    )
    .bind(&legacy_scope_commitment)
    .bind(HASH_B)
    .execute(&repository.pool)
    .await
    .expect("seed historic v1 selected-read scope");
    sqlx::query(
        "INSERT INTO tally_selected_read_observations VALUES (\
           'scope', 'snap', 'feature', 'selected_ledger_read',\
           'supported', 'observed', 'selected_ledger_read_non_empty_observed',\
           'non_empty_observed', ?1, ?2, 'utf8', 1, 1, 1, 'verified', 0\
         )",
    )
    .bind(HASH_A)
    .bind(HASH_B)
    .execute(&repository.pool)
    .await
    .expect("seed historic v1 selected-read observation");

    repository
        .migrate()
        .await
        .expect("upgrade v21 selected-read evidence to v22");
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT response_encoding FROM tally_selected_read_observations \
             WHERE scope_id = 'scope' AND capability_key = 'selected_ledger_read'",
        )
        .fetch_one(&repository.pool)
        .await
        .expect("read preserved selected-read evidence"),
        "utf8"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tally_schema_migrations WHERE version = 22",
        )
        .fetch_one(&repository.pool)
        .await
        .expect("read v22 migration marker"),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT scope_contract_version FROM tally_selected_read_scopes WHERE id = 'scope'",
        )
        .fetch_one(&repository.pool)
        .await
        .expect("read preserved historic scope version"),
        1,
        "historic digest material must remain explicitly v1"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM pragma_foreign_key_check")
            .fetch_one(&repository.pool)
            .await
            .expect("check rebuilt selected-read foreign keys"),
        0,
        "observation rows must reference the rebuilt scope parent"
    );
    assert!(sqlx::query(
        "UPDATE tally_selected_read_observations SET response_encoding = 'utf16le' \
         WHERE scope_id = 'scope'",
    )
    .execute(&repository.pool)
    .await
    .is_err());
}

#[tokio::test]
async fn v3_receipt_binds_canonical_record_counts_and_rejects_pre_start_completion() {
    let (repository, snapshot, company) = seeded_repository().await;
    let batch_id = begin_batch(&repository, &snapshot, &company, "v3-count-binding").await;
    let record_counts = BTreeMap::from([
        ("group.accepted_unique".to_string(), 3),
        ("locally_staged.accepted".to_string(), 3),
        ("locally_staged.rejected".to_string(), 0),
    ]);
    let record_counts_sha256 = proof_record_counts_sha256(&record_counts);
    let receipt = repository
        .commit_batch(CommitBatchInput::test_only(CommitBatchParts {
            batch_id: batch_id.clone(),
            proof_contract_version: 3,
            outcome: RunOutcome::Failed,
            verification: VerificationState::Unverified,
            completed_at_unix_ms: 2_500,
            record_counts_sha256: Some(record_counts_sha256.clone()),
            snapshot_sha256: None,
            expected_checkpoint_before: None,
            checkpoint_after: None,
            freshness_target_seconds: 60,
            gap_codes: vec!["source_outcome_unknown".to_string()],
            warning_codes: Vec::new(),
        }))
        .await
        .expect("commit v3 count-bound proof");
    assert_eq!(
        receipt.facts.record_counts_sha256.as_deref(),
        Some(record_counts_sha256.as_str())
    );
    let recovered = repository
        .historical_commit_receipt_for_batch(&batch_id, "v3-count-binding")
        .await
        .expect("revalidate count-bound historical receipt");
    assert_eq!(recovered, receipt);

    let clock_batch = begin_batch(&repository, &snapshot, &company, "clock-regression").await;
    let error = repository
        .commit_batch(CommitBatchInput::test_only(CommitBatchParts {
            batch_id: clock_batch.clone(),
            proof_contract_version: 3,
            outcome: RunOutcome::Failed,
            verification: VerificationState::Unverified,
            completed_at_unix_ms: 1_999,
            record_counts_sha256: Some(proof_record_counts_sha256(&BTreeMap::new())),
            snapshot_sha256: None,
            expected_checkpoint_before: None,
            checkpoint_after: None,
            freshness_target_seconds: 60,
            gap_codes: vec!["source_outcome_unknown".to_string()],
            warning_codes: Vec::new(),
        }))
        .await
        .expect_err("proof completion before batch start must fail closed");
    assert!(matches!(
        error,
        MirrorError::InvalidInput("batch_completed_at")
    ));
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT state FROM tally_observation_batches WHERE id = ?1",
        )
        .bind(clock_batch)
        .fetch_one(&repository.pool)
        .await
        .unwrap(),
        "staging"
    );
}

#[tokio::test]
async fn v7_fails_closed_on_legacy_casefold_company_guid_collision() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("connect to legacy in-memory SQLite");
    let mut transaction = pool.begin().await.expect("begin legacy migration");
    for migration in [
        MIRROR_MIGRATION_V2,
        MIRROR_MIGRATION_V3,
        MIRROR_MIGRATION_V4,
        MIRROR_MIGRATION_V5,
        MIRROR_MIGRATION_V6,
    ] {
        sqlx::raw_sql(migration)
            .execute(&mut *transaction)
            .await
            .expect("install legacy migration");
    }
    sqlx::query(
        "INSERT INTO tally_endpoints(id, canonical_origin, created_at_unix_ms, last_observed_at_unix_ms) \
         VALUES ('endpoint-1', 'http://127.0.0.1:9000', 1, 1)",
    )
    .execute(&mut *transaction)
    .await
    .expect("seed endpoint");
    for (id, guid) in [("company-a", "CASE-GUID"), ("company-b", "case-guid")] {
        sqlx::query(
            "INSERT INTO tally_companies(\
               id, endpoint_id, display_name, company_guid, identity_confidence, \
               first_observed_at_unix_ms, last_observed_at_unix_ms\
             ) VALUES (?1, 'endpoint-1', ?1, ?2, 'observed', 1, 1)",
        )
        .bind(id)
        .bind(guid)
        .execute(&mut *transaction)
        .await
        .expect("seed case-variant company");
    }
    transaction.commit().await.expect("commit legacy database");

    let repository = TallyMirrorRepository::new(pool);
    assert!(matches!(
        repository.migrate().await,
        Err(MirrorError::InvalidInput("company_guid_casefold_collision"))
    ));
    let v7_markers = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM tally_schema_migrations WHERE version = 7",
    )
    .fetch_one(&repository.pool)
    .await
    .expect("inspect v7 marker");
    assert_eq!(v7_markers, 0);
    let index_sql = sqlx::query_scalar::<_, String>(
        "SELECT sql FROM sqlite_master WHERE type = 'index' AND name = 'uq_tally_companies_guid'",
    )
    .fetch_one(&repository.pool)
    .await
    .expect("inspect rolled-back index");
    assert!(!index_sql.contains("COLLATE NOCASE"));
}

#[tokio::test]
async fn commit_pending_restart_accepts_only_exact_sealed_core_evidence() {
    let (ordinary, ordinary_snapshot, ordinary_company) = seeded_repository().await;
    assert!(ordinary
        .capability_snapshot_matches_plan(
            &ordinary_snapshot.id,
            &ordinary_company.id,
            1,
            "TallyPrime",
            None,
            Some("Education"),
        )
        .await
        .expect("ordinary observed-support contract remains available to other flows"));

    let (repository, snapshot, company) = seeded_repository_with_core_evidence(
        CapabilityState::Unknown,
        Confidence::Observed,
        Some("sealed_profile_executed"),
    )
    .await;
    assert!(repository
        .core_snapshot_resume_evidence_matches_plan(
            &snapshot.id,
            &company.id,
            1,
            "TallyPrime",
            None,
            Some("Education"),
        )
        .await
        .expect("CommitPending restart accepts exact sealed Core evidence"));

    for (state, confidence, reason) in [
        (
            CapabilityState::Supported,
            Confidence::Observed,
            "sealed_profile_executed",
        ),
        (
            CapabilityState::Unknown,
            Confidence::Inferred,
            "sealed_profile_executed",
        ),
        (
            CapabilityState::Unknown,
            Confidence::Observed,
            "some_other_observation",
        ),
    ] {
        let (altered, altered_snapshot, altered_company) =
            seeded_repository_with_core_evidence(state, confidence, Some(reason)).await;
        assert!(
            !altered
                .core_snapshot_resume_evidence_matches_plan(
                    &altered_snapshot.id,
                    &altered_company.id,
                    1,
                    "TallyPrime",
                    None,
                    Some("Education"),
                )
                .await
                .expect("reject altered persisted Core evidence"),
            "resume must reject state={state:?}, confidence={confidence:?}, reason={reason}"
        );
    }

    assert!(!repository
        .core_snapshot_resume_evidence_matches_plan(
            &snapshot.id,
            &company.id,
            1,
            "TallyPrime",
            Some("different-release"),
            Some("Education"),
        )
        .await
        .expect("reject changed capability profile"));
}

#[tokio::test]
async fn batch_identity_remains_composite_across_capability_packs() {
    let (repository, snapshot, company) = seeded_repository().await;
    let core = begin_batch(&repository, &snapshot, &company, "multi-pack-run").await;
    let tax = repository
        .begin_batch(BeginBatchInput {
            run_id: "multi-pack-run".to_string(),
            capability_snapshot_id: snapshot.id,
            company_id: company.id,
            pack_id: "india_tax".to_string(),
            pack_schema_major: 1,
            pack_schema_minor: 0,
            source_transport: "xml_http".to_string(),
            source_release: None,
            requested_from_yyyymmdd: Some("20260401".to_string()),
            requested_to_yyyymmdd: Some("20260401".to_string()),
            started_at_unix_ms: 2_000,
        })
        .await
        .expect("same run may carry a distinct pack batch");
    assert_ne!(core, tax);
}

#[tokio::test]
async fn raw_multi_statement_migration_rolls_back_ddl_on_failure() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("connect to in-memory SQLite");
    let mut transaction = pool.begin().await.expect("begin migration transaction");
    let result = sqlx::raw_sql(
        "CREATE TABLE rollback_probe(id INTEGER PRIMARY KEY); \
         INSERT INTO rollback_probe(id) VALUES (1); \
         INSERT INTO table_that_does_not_exist(id) VALUES (1);",
    )
    .execute(&mut *transaction)
    .await;
    assert!(result.is_err(), "synthetic migration must fail");
    transaction
        .rollback()
        .await
        .expect("rollback failed migration");

    let table_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'rollback_probe'",
    )
    .fetch_one(&pool)
    .await
    .expect("inspect schema after rollback");
    assert_eq!(table_count, 0, "DDL must not survive migration rollback");
}

#[tokio::test]
async fn recovery_migration_fails_closed_on_duplicate_legacy_run_ids() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("connect to legacy in-memory SQLite");
    let mut transaction = pool.begin().await.expect("begin legacy migration");
    sqlx::raw_sql(MIRROR_MIGRATION_V2)
        .execute(&mut *transaction)
        .await
        .expect("install v2");
    sqlx::raw_sql(MIRROR_MIGRATION_V3)
        .execute(&mut *transaction)
        .await
        .expect("install v3");
    sqlx::raw_sql(MIRROR_MIGRATION_V4)
        .execute(&mut *transaction)
        .await
        .expect("install v4");
    transaction.commit().await.expect("commit legacy schema");
    for resume_key in ["legacy:a", "legacy:b"] {
        sqlx::query(
            "INSERT INTO tally_snapshot_run_states(\
               resume_key, run_id, generation, state_sha256, state_json, updated_at_unix_ms\
             ) VALUES (?1, 'duplicated-run', 1, ?2, '{}', 1)",
        )
        .bind(resume_key)
        .bind(HASH_A)
        .execute(&pool)
        .await
        .expect("seed ambiguous legacy recovery row");
    }
    let repository = TallyMirrorRepository::new(pool);
    assert!(matches!(
        repository.migrate().await,
        Err(MirrorError::InvalidInput("snapshot_state_duplicate_run_id"))
    ));
}

#[tokio::test]
async fn stable_company_identity_survives_rename() {
    let (repository, snapshot, original) = seeded_repository().await;
    let renamed = repository
        .upsert_company(CompanyInput {
            endpoint_id: snapshot.endpoint_id,
            display_name: "Synthetic Bridge Test Renamed".to_string(),
            identity: SourceIdentityInput {
                guid: Some("company-guid-1".to_string()),
                confidence: Some(Confidence::Observed),
                ..Default::default()
            },
            observed_at_unix_ms: 2_000,
        })
        .await
        .expect("rename company by stable identity");
    assert_eq!(original.id, renamed.id);
    assert_eq!(renamed.display_name, "Synthetic Bridge Test Renamed");
}

#[tokio::test]
async fn company_guid_casing_resolves_to_one_stable_pin() {
    let (repository, snapshot, _) = seeded_repository().await;
    let uppercase = repository
        .upsert_company(CompanyInput {
            endpoint_id: snapshot.endpoint_id.clone(),
            display_name: "Synthetic Case Company".to_string(),
            identity: SourceIdentityInput {
                guid: Some("CASE-GUID-2".to_string()),
                confidence: Some(Confidence::Observed),
                ..Default::default()
            },
            observed_at_unix_ms: 2_000,
        })
        .await
        .expect("save uppercase GUID");
    let lowercase = repository
        .upsert_company(CompanyInput {
            endpoint_id: snapshot.endpoint_id.clone(),
            display_name: "Synthetic Case Company Renamed".to_string(),
            identity: SourceIdentityInput {
                guid: Some("case-guid-2".to_string()),
                confidence: Some(Confidence::Observed),
                ..Default::default()
            },
            observed_at_unix_ms: 3_000,
        })
        .await
        .expect("save lowercase GUID");

    assert_eq!(uppercase.id, lowercase.id);
    let count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM tally_companies \
         WHERE endpoint_id = ?1 AND company_guid = ?2 COLLATE NOCASE",
    )
    .bind(&snapshot.endpoint_id)
    .bind("case-guid-2")
    .fetch_one(&repository.pool)
    .await
    .expect("count case-folded company pins");
    assert_eq!(count, 1);
}

#[test]
fn company_profile_correlation_is_casefolded_scoped_and_opaque() {
    let first = company_profile_correlation_key(
        "http://127.0.0.1:9000",
        "SENSITIVE-COMPANY-GUID",
        "100001",
        "Synthetic",
        "20260401",
    );
    let same = company_profile_correlation_key(
        "http://127.0.0.1:9000",
        "sensitive-company-guid",
        "100001",
        "Synthetic",
        "20260401",
    );
    let other_endpoint = company_profile_correlation_key(
        "http://127.0.0.1:9001",
        "sensitive-company-guid",
        "100001",
        "Synthetic",
        "20260401",
    );
    let other_company = company_profile_correlation_key(
        "http://127.0.0.1:9000",
        "other-company-guid",
        "100001",
        "Synthetic",
        "20260401",
    );
    let same_guid_different_book = company_profile_correlation_key(
        "http://127.0.0.1:9000",
        "sensitive-company-guid",
        "100014",
        "Synthetic",
        "20270401",
    );

    assert_eq!(first, same);
    assert_ne!(first, other_endpoint);
    assert_ne!(first, other_company);
    assert_ne!(first, same_guid_different_book);
    assert_eq!(first.len(), 64);
    assert!(!first.contains("sensitive"));
}

#[tokio::test]
async fn persisted_profiles_only_return_observed_stable_company_pins() {
    let (repository, snapshot, _) = seeded_repository().await;
    let observed = repository
        .save_reviewed_setup(reviewed_setup_input(HASH_A))
        .await
        .expect("save complete observed company tuple")
        .company;
    repository
        .upsert_company(CompanyInput {
            endpoint_id: snapshot.endpoint_id,
            display_name: "Synthetic Inferred Company".to_string(),
            identity: SourceIdentityInput {
                fallback_fingerprint: Some("inferred-company-fingerprint".to_string()),
                confidence: Some(Confidence::Inferred),
                ..Default::default()
            },
            observed_at_unix_ms: 2_000,
        })
        .await
        .expect("save inferred company");

    let page = repository
        .persisted_company_profiles()
        .await
        .expect("load persisted profiles");
    let profiles = page.profiles;
    assert_eq!(page.total_profiles, 1);
    assert!(!page.truncated);
    assert_eq!(profiles.len(), 1);
    assert_eq!(profiles[0].mirror_company_id, observed.id);
    assert!(profiles[0].guid_observed);
    assert_eq!(
        profiles[0].correlation_key,
        company_profile_correlation_key(
            "http://127.0.0.1:9000",
            "reviewed-company-guid",
            "100001",
            "Synthetic Reviewed Company",
            "20260401"
        )
    );
    assert_eq!(profiles[0].identity_confidence, "observed");
    assert_eq!(profiles[0].canonical_endpoint, "http://127.0.0.1:9000");
}

#[tokio::test]
async fn client_label_migration_profiles_include_unpaged_and_suppressed_history() {
    let (repository, snapshot, _) = seeded_repository().await;
    for index in 0..=500 {
        sqlx::query(
            "INSERT INTO tally_companies(\
               id, endpoint_id, display_name, company_guid, remote_id, master_id, \
               fallback_fingerprint, identity_confidence, first_observed_at_unix_ms, \
               last_observed_at_unix_ms, company_number, books_from_yyyymmdd\
             ) VALUES (?1, ?2, ?3, ?4, NULL, NULL, NULL, 'observed', ?5, ?5, ?6, ?7)",
        )
        .bind(format!("synthetic-migration-company-{index}"))
        .bind(&snapshot.endpoint_id)
        .bind("Synthetic Historical Book")
        .bind("synthetic-shared-guid")
        .bind(i64::from(index))
        .bind(format!("{:06}", 200_000 + index))
        .bind("20260401")
        .execute(&repository.pool)
        .await
        .expect("seed synthetic historical profile");
    }
    sqlx::query(
        "INSERT INTO tally_companies(\
           id, endpoint_id, display_name, company_guid, remote_id, master_id, \
           fallback_fingerprint, identity_confidence, first_observed_at_unix_ms, \
           last_observed_at_unix_ms, company_number, books_from_yyyymmdd\
         ) VALUES (?1, ?2, ?3, ?4, NULL, NULL, NULL, 'unknown', ?5, ?5, NULL, NULL)",
    )
    .bind("synthetic-suppressed-migration-company")
    .bind(&snapshot.endpoint_id)
    .bind("Synthetic Suppressed Book")
    .bind("synthetic-shared-guid")
    .bind(1_i64)
    .execute(&repository.pool)
    .await
    .expect("seed synthetic suppressed profile");

    let page = repository
        .persisted_company_profiles()
        .await
        .expect("load UI page");
    assert!(page.truncated);
    assert_eq!(page.profiles.len(), 500);

    let migration_profiles = repository
        .persisted_company_profiles_for_client_group_label_migration(&[
            "synthetic-shared-guid".to_string()
        ])
        .await
        .expect("load complete migration history");
    assert_eq!(migration_profiles.len(), 502);
    assert!(migration_profiles
        .iter()
        .any(|profile| profile.correlation_key.is_some()));
    assert!(migration_profiles.iter().any(|profile| {
        profile.identity_confidence == "unknown" && profile.correlation_key.is_none()
    }));
}

#[tokio::test]
async fn mirror_explorer_is_paged_and_omits_source_content() {
    let (repository, snapshot, company) = seeded_repository().await;
    let batch_id = begin_batch(&repository, &snapshot, &company, "explorer-run").await;
    for (guid, name, hash) in [
        ("ledger-guid-sensitive", "Private Sales Ledger", HASH_A),
        ("voucher-guid-sensitive", "Private Receipt Voucher", HASH_B),
    ] {
        repository
            .observe_record(ObservedRecordInput {
                batch_id: batch_id.clone(),
                object_type: if guid.starts_with("ledger") {
                    "ledger".to_string()
                } else {
                    "voucher".to_string()
                },
                display_name: Some(name.to_string()),
                identity: SourceIdentityInput {
                    guid: Some(guid.to_string()),
                    confidence: Some(Confidence::Observed),
                    ..Default::default()
                },
                observed_at_unix_ms: 2_100,
                raw_source_sha256: hash.to_string(),
                canonical_sha256: Some(hash.to_string()),
                canonical_payload: Some(json!({"private_amount": "1180.00"})),
                exact_decimals: BTreeMap::from([(
                    "private_amount".to_string(),
                    "1180.00".to_string(),
                )]),
                observed_alter_id: None,
                status: ObservationStatus::Accepted,
                safe_rejection_code: None,
            })
            .await
            .expect("store explorer record");
    }

    let first = repository
        .mirror_explorer_page(&company.id, "core_accounting", 0, 1)
        .await
        .expect("load first explorer page");
    let second = repository
        .mirror_explorer_page(&company.id, "core_accounting", 1, 1)
        .await
        .expect("load second explorer page");
    assert_eq!(first.total_records, 2);
    assert_eq!(first.records.len(), 1);
    assert_eq!(first.records[0].local_alias, "local-record-1");
    assert_eq!(second.records[0].local_alias, "local-record-2");

    let serialized = serde_json::to_string(&(first, second)).expect("serialize explorer pages");
    for sensitive in [
        "Private Sales Ledger",
        "Private Receipt Voucher",
        "ledger-guid-sensitive",
        "voucher-guid-sensitive",
        "1180.00",
        HASH_A,
        HASH_B,
    ] {
        assert!(!serialized.contains(sensitive));
    }
}

#[tokio::test]
async fn guid_only_company_pin_requires_reverification() {
    let (repository, _snapshot, company) = seeded_repository().await;
    sqlx::query("UPDATE tally_companies SET identity_confidence = 'inferred' WHERE id = ?1")
        .bind(&company.id)
        .execute(&repository.pool)
        .await
        .expect("weaken synthetic confidence");
    assert!(matches!(
        repository.snapshot_source_pin(&company.id).await,
        Err(MirrorError::InvalidInput(
            "company_identity_reverification_required"
        ))
    ));

    let saved = repository
        .save_reviewed_setup(reviewed_setup_input(HASH_A))
        .await
        .expect("full observed tuple re-verifies a separate eligible pin");
    assert!(matches!(
        repository.snapshot_source_pin(&company.id).await,
        Err(MirrorError::InvalidInput(
            "company_identity_reverification_required"
        ))
    ));
    let pin = repository
        .snapshot_source_pin(&saved.company.id)
        .await
        .expect("re-verified tuple is snapshot eligible");
    assert_eq!(pin.company_guid, "reviewed-company-guid");
}

#[tokio::test]
async fn composite_identity_migration_retires_prior_observed_guid_pin() {
    let (repository, _, company) = seed_repository(repository_through_v13().await).await;
    sqlx::query(
        "INSERT INTO tally_write_fixture_enrollments(\
            id, company_id, review_commitment_sha256, enrollment_payload_sha256, \
            contract_version, disposable_company_attested, no_customer_data_attested, \
            backup_guidance_acknowledged, enrolled_at_unix_ms\
         ) VALUES (?1, ?2, ?3, ?4, 1, 1, 1, 1, 100)",
    )
    .bind("pre-composite-enrollment")
    .bind(&company.id)
    .bind("a".repeat(64))
    .bind("b".repeat(64))
    .execute(&repository.pool)
    .await
    .expect("seed active GUID-only fixture enrollment");
    repository
        .migrate()
        .await
        .expect("apply the composite company identity migration");

    assert!(matches!(
        repository.snapshot_source_pin(&company.id).await,
        Err(MirrorError::InvalidInput(
            "company_identity_reverification_required"
        ))
    ));
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT identity_confidence FROM tally_companies WHERE id = ?1",
        )
        .bind(&company.id)
        .fetch_one(&repository.pool)
        .await
        .expect("read retired confidence"),
        "unknown"
    );
    let revocation = sqlx::query(
        "SELECT revocation.safe_reason_code, revocation.revocation_payload_sha256, \
                revocation.revoked_at_unix_ms, enrollment.enrollment_payload_sha256 \
         FROM tally_write_fixture_revocations AS revocation \
         JOIN tally_write_fixture_enrollments AS enrollment \
           ON enrollment.id = revocation.enrollment_id \
         WHERE revocation.enrollment_id = 'pre-composite-enrollment'",
    )
    .fetch_one(&repository.pool)
    .await
    .expect("read truthful migration revocation");
    let reason: String = revocation.try_get("safe_reason_code").expect("reason");
    let payload: String = revocation
        .try_get("revocation_payload_sha256")
        .expect("payload");
    let timestamp: i64 = revocation.try_get("revoked_at_unix_ms").expect("timestamp");
    let enrollment_payload: String = revocation
        .try_get("enrollment_payload_sha256")
        .expect("enrollment payload");
    assert_eq!(reason, "company_identity_reverification_required");
    assert!(
        timestamp > 100,
        "migration time must not reuse enrollment time"
    );
    assert_eq!(
        payload,
        fixture_revocation_payload_sha256(
            "pre-composite-enrollment",
            &enrollment_payload,
            "company_identity_reverification_required",
            timestamp,
        )
        .expect("canonical migration payload"),
        "immutable migration evidence must use the normal canonical binding"
    );
}

#[tokio::test]
async fn composite_identity_schema_rejects_legacy_v7_runner() {
    let repository = repository_through_v13().await;
    repository
        .migrate()
        .await
        .expect("upgrade through the composite identity migration");
    for (version, expected) in [(7, 0_i64), (23, 1_i64)] {
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM tally_schema_migrations WHERE version = ?1",
            )
            .bind(version)
            .fetch_one(&repository.pool)
            .await
            .expect("read migration marker"),
            expected
        );
    }

    let mut transaction = repository
        .pool
        .begin()
        .await
        .expect("begin legacy migration");
    let error = sqlx::raw_sql(MIRROR_MIGRATION_V7)
        .execute(&mut *transaction)
        .await
        .expect_err("a legacy GUID-only binary must fail closed");
    assert!(error
        .to_string()
        .contains("composite company identity requires a compatible Bridge binary"));
    transaction
        .rollback()
        .await
        .expect("roll back rejected legacy migration");

    repository
        .migrate()
        .await
        .expect("current binary skips the retired migration marker");
}

#[tokio::test]
async fn fallback_identity_is_not_silently_upgraded() {
    let repository = repository().await;
    let snapshot = repository
        .save_capability_snapshot(CapabilitySnapshotInput {
            canonical_origin: "http://127.0.0.1:9000".to_string(),
            observed_at_unix_ms: 1,
            profile_version: 1,
            product: "TallyPrime".to_string(),
            release: None,
            license_tier: None,
            mode: None,
            mode_confidence: Confidence::Unknown,
            items: vec![],
        })
        .await
        .expect("save snapshot");
    repository
        .upsert_company(CompanyInput {
            endpoint_id: snapshot.endpoint_id.clone(),
            display_name: "Synthetic".to_string(),
            identity: SourceIdentityInput {
                fallback_fingerprint: Some("fallback-1".to_string()),
                ..Default::default()
            },
            observed_at_unix_ms: 1,
        })
        .await
        .expect("save fallback identity");
    let error = repository
        .upsert_company(CompanyInput {
            endpoint_id: snapshot.endpoint_id,
            display_name: "Synthetic".to_string(),
            identity: SourceIdentityInput {
                guid: Some("new-guid".to_string()),
                fallback_fingerprint: Some("fallback-1".to_string()),
                confidence: Some(Confidence::Observed),
                ..Default::default()
            },
            observed_at_unix_ms: 2,
        })
        .await
        .expect_err("identity upgrade must require an audit event");
    assert!(matches!(error, MirrorError::IdentityUpgradeRequiresAudit));
}

#[tokio::test]
async fn verified_commit_atomically_advances_checkpoint_and_proof_is_immutable() {
    let (repository, snapshot, company) = seeded_repository().await;
    let batch_id = begin_batch(&repository, &snapshot, &company, "run-1").await;
    let resumed_batch_id = begin_batch(&repository, &snapshot, &company, "run-1").await;
    assert_eq!(resumed_batch_id, batch_id);
    repository
        .observe_record(ObservedRecordInput {
            batch_id: batch_id.clone(),
            object_type: "ledger".to_string(),
            display_name: Some("Synthetic Sales".to_string()),
            identity: SourceIdentityInput {
                guid: Some("ledger-guid-1".to_string()),
                confidence: Some(Confidence::Observed),
                ..Default::default()
            },
            observed_at_unix_ms: 2_100,
            raw_source_sha256: HASH_A.to_string(),
            canonical_sha256: Some(HASH_B.to_string()),
            canonical_payload: Some(json!({"amount": "1180.00", "name": "Synthetic Sales"})),
            exact_decimals: BTreeMap::from([(
                "opening_balance".to_string(),
                "1180.00".to_string(),
            )]),
            observed_alter_id: Some("42".to_string()),
            status: ObservationStatus::Accepted,
            safe_rejection_code: None,
        })
        .await
        .expect("store observed record");

    let commit = repository
        .commit_batch(CommitBatchInput::test_only(CommitBatchParts {
            batch_id: batch_id.clone(),
            proof_contract_version: 1,
            outcome: RunOutcome::Completed,
            verification: VerificationState::Verified,
            completed_at_unix_ms: 3_000,
            record_counts_sha256: None,
            snapshot_sha256: Some(HASH_B.to_string()),
            expected_checkpoint_before: None,
            checkpoint_after: Some("alter_id:42".to_string()),
            freshness_target_seconds: 60,
            gap_codes: vec![],
            warning_codes: vec!["report_tie_out_unavailable".to_string()],
        }))
        .await
        .expect("commit verified batch");
    assert!(commit.checkpoint_advanced);
    assert_eq!(commit.proof_sha256.len(), 64);
    let recovered = repository
        .commit_receipt_for_batch(&batch_id, "run-1")
        .await
        .expect("recover exact proof receipt");
    assert_eq!(recovered.proof_id, commit.proof_id);
    assert_eq!(recovered.proof_sha256, commit.proof_sha256);
    assert!(recovered.checkpoint_advanced);

    let fresh = repository
        .freshness(&company.id, "core_accounting", 30_000)
        .await
        .expect("read freshness");
    assert_eq!(fresh.state, FreshnessState::Fresh);
    assert_eq!(fresh.checkpoint_token.as_deref(), Some("alter_id:42"));
    let clock_skew = repository
        .freshness(&company.id, "core_accounting", 2_999)
        .await
        .expect("clock skew is fail-closed");
    assert_eq!(clock_skew.state, FreshnessState::Stale);
    assert_eq!(clock_skew.age_seconds, Some(0));

    let proofs = repository
        .latest_proofs(&company.id, 10)
        .await
        .expect("read immutable proof summary");
    assert_eq!(proofs.len(), 1);
    assert_eq!(proofs[0].selection_token, commit.proof_id);
    assert_eq!(proofs[0].proof_sha256, commit.proof_sha256);
    assert_eq!(proofs[0].verification_state, "verified");
    assert_eq!(proofs[0].accepted_records, 1);
    assert_eq!(
        proofs[0].warning_codes,
        vec!["report_tie_out_unavailable".to_string()]
    );

    let next_batch_id = begin_batch(&repository, &snapshot, &company, "run-2").await;
    repository
        .commit_batch(CommitBatchInput::test_only(CommitBatchParts {
            batch_id: next_batch_id,
            proof_contract_version: 1,
            outcome: RunOutcome::Completed,
            verification: VerificationState::Verified,
            completed_at_unix_ms: 4_000,
            record_counts_sha256: None,
            snapshot_sha256: Some(HASH_A.to_string()),
            expected_checkpoint_before: Some("alter_id:42".to_string()),
            checkpoint_after: Some("alter_id:43".to_string()),
            freshness_target_seconds: 60,
            gap_codes: vec![],
            warning_codes: vec![],
        }))
        .await
        .expect("advance generic checkpoint with a later verified proof");
    assert!(matches!(
        repository
            .commit_receipt_for_batch(&batch_id, "run-1")
            .await,
        Err(MirrorError::VerificationInvariant)
    ));
    let historical = repository
        .historical_commit_receipt_for_batch(&batch_id, "run-1")
        .await
        .expect("immutable historical proof remains authentic after a later checkpoint");
    assert_eq!(historical.proof_id, commit.proof_id);
    assert_eq!(historical.proof_sha256, commit.proof_sha256);

    let mutation = sqlx::query("UPDATE tally_proof_ledger SET entry_sha256 = ?1 WHERE id = ?2")
        .bind(HASH_A)
        .bind(&commit.proof_id)
        .execute(&repository.pool)
        .await;
    assert!(mutation.is_err(), "proof ledger must be append-only");
}

#[tokio::test]
async fn partial_batch_cannot_advance_checkpoint_and_float_payloads_are_rejected() {
    let (repository, snapshot, company) = seeded_repository().await;
    let batch_id = begin_batch(&repository, &snapshot, &company, "run-2").await;
    let float_error = repository
        .observe_record(ObservedRecordInput {
            batch_id: batch_id.clone(),
            object_type: "ledger".to_string(),
            display_name: None,
            identity: SourceIdentityInput {
                guid: Some("ledger-guid-2".to_string()),
                ..Default::default()
            },
            observed_at_unix_ms: 2_100,
            raw_source_sha256: HASH_A.to_string(),
            canonical_sha256: Some(HASH_B.to_string()),
            canonical_payload: Some(json!({"amount": 12.5})),
            exact_decimals: BTreeMap::new(),
            observed_alter_id: None,
            status: ObservationStatus::Accepted,
            safe_rejection_code: None,
        })
        .await
        .expect_err("floating accounting representation must not enter the mirror");
    assert!(matches!(
        float_error,
        MirrorError::InvalidInput("floating_point_payload_number")
    ));

    repository
        .observe_record(ObservedRecordInput {
            batch_id: batch_id.clone(),
            object_type: "ledger".to_string(),
            display_name: None,
            identity: SourceIdentityInput {
                guid: Some("ledger-guid-2".to_string()),
                ..Default::default()
            },
            observed_at_unix_ms: 2_100,
            raw_source_sha256: HASH_A.to_string(),
            canonical_sha256: None,
            canonical_payload: None,
            exact_decimals: BTreeMap::new(),
            observed_alter_id: None,
            status: ObservationStatus::Rejected,
            safe_rejection_code: Some("invalid_exact_decimal".to_string()),
        })
        .await
        .expect("store safe rejection evidence");

    repository
        .commit_batch(CommitBatchInput::test_only(CommitBatchParts {
            batch_id,
            proof_contract_version: 1,
            outcome: RunOutcome::Completed,
            verification: VerificationState::Partial,
            completed_at_unix_ms: 3_000,
            record_counts_sha256: None,
            snapshot_sha256: Some(HASH_B.to_string()),
            expected_checkpoint_before: None,
            checkpoint_after: None,
            freshness_target_seconds: 60,
            gap_codes: vec!["rejected_records_present".to_string()],
            warning_codes: vec![],
        }))
        .await
        .expect("commit partial proof without checkpoint");
    let freshness = repository
        .freshness(&company.id, "core_accounting", 4_000)
        .await
        .expect("read freshness");
    assert_eq!(freshness.state, FreshnessState::NeverVerified);
}

#[tokio::test]
async fn verified_checkpoint_commit_is_compare_and_swap_protected() {
    let (repository, snapshot, company) = seeded_repository().await;
    let first_batch = begin_batch(&repository, &snapshot, &company, "cas-run-1").await;
    let second_batch = begin_batch(&repository, &snapshot, &company, "cas-run-2").await;
    repository
        .commit_batch(CommitBatchInput::test_only(CommitBatchParts {
            batch_id: first_batch,
            proof_contract_version: 1,
            outcome: RunOutcome::Completed,
            verification: VerificationState::Verified,
            completed_at_unix_ms: 3_000,
            record_counts_sha256: None,
            snapshot_sha256: Some(HASH_A.to_string()),
            expected_checkpoint_before: None,
            checkpoint_after: Some("full:first".to_string()),
            freshness_target_seconds: 60,
            gap_codes: vec![],
            warning_codes: vec![],
        }))
        .await
        .expect("first verified run advances an empty checkpoint");

    let conflict = repository
        .commit_batch(CommitBatchInput::test_only(CommitBatchParts {
            batch_id: second_batch,
            proof_contract_version: 1,
            outcome: RunOutcome::Completed,
            verification: VerificationState::Verified,
            completed_at_unix_ms: 3_001,
            record_counts_sha256: None,
            snapshot_sha256: Some(HASH_B.to_string()),
            expected_checkpoint_before: None,
            checkpoint_after: Some("full:second".to_string()),
            freshness_target_seconds: 60,
            gap_codes: vec![],
            warning_codes: vec![],
        }))
        .await
        .expect_err("stale checkpoint authority must fail closed");
    assert!(matches!(conflict, MirrorError::ConcurrentCheckpoint));
    let freshness = repository
        .freshness(&company.id, "core_accounting", 3_001)
        .await
        .expect("read winning checkpoint");
    assert_eq!(freshness.checkpoint_token.as_deref(), Some("full:first"));
}

#[test]
fn public_support_export_is_minimized_uncorrelatable_and_checksum_stable() {
    let build = || RedactedProofPayload {
        schema: "bridge.tally.redacted-proof-of-sync",
        schema_version: 1,
        exported_at_unix_ms: 123,
        redaction_profile: "public_support_v1",
        subject: RedactedSubject {
            reference: "company-1",
            identity_disclosed: false,
        },
        proofs: vec![RedactedProofEntry {
            entry_index: 1,
            proof_contract_version: 1,
            pack_id: "core_accounting".to_string(),
            pack_schema_version: PackSchemaVersion { major: 2, minor: 0 },
            outcome: "completed".to_string(),
            verification_state: "partial".to_string(),
            started_at_unix_ms: 100,
            completed_at_unix_ms: 120,
            counts: RedactedCounts {
                provenance_backed_accepted_records: 2,
                provenance_unavailable_records: 0,
                rejected_records: 0,
            },
            gaps: vec!["report_tie_out_unavailable".to_string()],
            warnings: Vec::new(),
            local_ledger: RedactedLedgerEvidence {
                chain_validation: "valid_at_export",
            },
        }],
        current_status: RedactedCurrentStatus {
            freshness_state: "never_verified",
            verified_at_unix_ms: None,
            checkpoint_present: false,
        },
    };
    let first = finish_redacted_export(build()).expect("build public support artifact");
    let second = finish_redacted_export(build()).expect("repeat deterministic artifact");
    assert_eq!(first.payload_sha256, second.payload_sha256);
    assert_eq!(first.json, second.json);
    for forbidden in [
        "Synthetic Company",
        "company-guid",
        "run-id",
        "batch-id",
        "proof-id",
        "checkpoint-token",
        "1180.00",
        "snapshot_commitment_sha256",
        "entry_sha256",
        "rid:",
    ] {
        assert!(!first.json.contains(forbidden), "leaked {forbidden}");
    }
    assert!(first
        .json
        .contains("\"integrity_claim\": \"checksum_only\""));
    assert!(first.json.contains("\"authenticity_claim\": \"none\""));
    assert!(validate_export_code("report_tie_out_unavailable").is_ok());
    assert!(validate_export_code("period_report_profile_unobserved").is_ok());
    assert!(validate_export_code("future_unreviewed_code").is_err());
}

#[test]
fn redacted_proof_export_accepts_every_reviewed_precise_tally_terminal_code() {
    for code in REVIEWED_TALLY_TERMINAL_CODES {
        validate_export_code(code).expect("reviewed terminal code must be exportable");
    }
    assert!(REVIEWED_TALLY_TERMINAL_CODES.contains(&"window_membership_replay_conflict"));
    assert!(REVIEWED_TALLY_TERMINAL_CODES.contains(&"adaptive_window_limit_reached"));
    assert!(REVIEWED_TALLY_TERMINAL_CODES.contains(&"minimum_window_response_too_large"));
    let payload = RedactedProofPayload {
        schema: "bridge.tally.redacted-proof-of-sync",
        schema_version: 1,
        exported_at_unix_ms: 1,
        redaction_profile: "public_support_v1",
        subject: RedactedSubject {
            reference: "selected_company",
            identity_disclosed: false,
        },
        proofs: vec![RedactedProofEntry {
            entry_index: 0,
            proof_contract_version: 1,
            pack_id: "core_accounting".to_string(),
            pack_schema_version: PackSchemaVersion { major: 1, minor: 0 },
            outcome: "failed".to_string(),
            verification_state: "unverified".to_string(),
            started_at_unix_ms: 1,
            completed_at_unix_ms: 2,
            counts: RedactedCounts {
                provenance_backed_accepted_records: 0,
                provenance_unavailable_records: 0,
                rejected_records: 0,
            },
            gaps: REVIEWED_TALLY_TERMINAL_CODES
                .iter()
                .map(|code| (*code).to_string())
                .collect(),
            warnings: Vec::new(),
            local_ledger: RedactedLedgerEvidence {
                chain_validation: "valid_at_export",
            },
        }],
        current_status: RedactedCurrentStatus {
            freshness_state: "never_verified",
            verified_at_unix_ms: None,
            checkpoint_present: false,
        },
    };
    let export = finish_redacted_export(payload).expect("export every reviewed terminal code");
    for code in REVIEWED_TALLY_TERMINAL_CODES {
        assert!(export.json.contains(code), "export omitted {code}");
    }
}

#[test]
fn redacted_proof_export_accepts_every_declared_warning_code() {
    for warning in crate::warning_codes::WarningCode::ALL {
        validate_export_warning_code(warning.as_str())
            .expect("declared warning code must be exportable");
    }
}
