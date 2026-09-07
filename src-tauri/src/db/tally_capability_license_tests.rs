//! Local storage contracts; these synthetic records are not Tally qualification evidence.
use super::tests::{repository, repository_through_v24, reviewed_setup_input};
use super::*;
use bridge_tally_core::LicenseTier;

const REVIEW: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

#[tokio::test]
async fn license_tier_survives_snapshot_storage_without_inference() {
    let repository = repository().await;
    for tier in [Some(LicenseTier::Silver), Some(LicenseTier::Gold), None] {
        let mut input = reviewed_setup_input(REVIEW).capability;
        input.profile_version = 4;
        input.license_tier = tier;
        let snapshot = repository.save_capability_snapshot(input).await.unwrap();
        let stored: (i64, Option<String>) = sqlx::query_as(
            "SELECT profile_version, license_tier FROM tally_capability_snapshots WHERE id = ?1",
        )
        .bind(&snapshot.id)
        .fetch_one(&repository.pool)
        .await
        .unwrap();
        assert_eq!(
            stored,
            (4, tier.map(|tier| license_tier_key(tier).to_string()))
        );
        for statement in [
            "UPDATE tally_capability_snapshots SET license_tier = 'gold' WHERE id = ?1",
            "DELETE FROM tally_capability_snapshots WHERE id = ?1",
        ] {
            let error = sqlx::query(statement)
                .bind(&snapshot.id)
                .execute(&repository.pool)
                .await
                .unwrap_err();
            assert_eq!(
                error.as_database_error().unwrap().code().as_deref(),
                Some("1811")
            );
        }
        let error = sqlx::query(
            "INSERT INTO tally_capability_snapshots(
                id, endpoint_id, observed_at_unix_ms, profile_version, product, mode_confidence, license_tier
             ) VALUES ('invalid-tier', ?1, 1, 4, 'Synthetic', 'unknown', 'invalid')",
        )
        .bind(&snapshot.endpoint_id)
        .execute(&repository.pool)
        .await
        .unwrap_err();
        assert_eq!(
            error.as_database_error().unwrap().code().as_deref(),
            Some("275")
        );
    }
}

#[tokio::test]
async fn license_tier_changes_cannot_reuse_reviewed_payload_commitment() {
    let repository = repository().await;
    let mut input = reviewed_setup_input(REVIEW);
    input.capability.profile_version = 4;
    input.capability.license_tier = Some(LicenseTier::Silver);
    let saved = repository.save_reviewed_setup(input.clone()).await.unwrap();
    let repeated = repository.save_reviewed_setup(input.clone()).await.unwrap();
    assert_eq!(saved.snapshot, repeated.snapshot);
    let stored: Option<String> =
        sqlx::query_scalar("SELECT license_tier FROM tally_capability_snapshots WHERE id = ?1")
            .bind(&saved.snapshot.id)
            .fetch_one(&repository.pool)
            .await
            .unwrap();
    assert_eq!(stored.as_deref(), Some("silver"));
    for tier in [Some(LicenseTier::Gold), None] {
        let mut changed = input.clone();
        changed.capability.license_tier = tier;
        assert!(matches!(
            repository.save_reviewed_setup(changed).await,
            Err(MirrorError::InvalidInput("review_commitment_reused"))
        ));
    }
    let snapshots: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tally_capability_snapshots")
        .fetch_one(&repository.pool)
        .await
        .unwrap();
    assert_eq!(snapshots, 1);
}

#[tokio::test]
async fn license_tier_migration_preserves_historical_records_and_none_payload_replay() {
    let repository = repository_through_v24().await;
    let input = reviewed_setup_input(REVIEW);
    // Golden digest of this record under the historical payload/2 shape.
    assert_eq!(
        reviewed_setup_payload_sha256(&input).unwrap(),
        "b9491ff70c28308bb17d20d634650d025d6ac4d643a14442bf5824cc03b7495c"
    );
    let before = repository.save_reviewed_setup(input.clone()).await.unwrap();
    let mut historical_v4 = input.capability.clone();
    historical_v4.profile_version = 4;
    let historical_v4 = repository
        .save_capability_snapshot(historical_v4)
        .await
        .unwrap();
    let payload_before: String = sqlx::query_scalar(
        "SELECT setup_payload_sha256 FROM tally_reviewed_setup_consumptions WHERE review_commitment_sha256 = ?1",
    ).bind(REVIEW).fetch_one(&repository.pool).await.unwrap();
    let legacy_row = |id: String| {
        sqlx::query_as::<_, (String, i64, i64, String, Option<String>, Option<String>, String)>(
        "SELECT endpoint_id, observed_at_unix_ms, profile_version, product, release, mode, mode_confidence
         FROM tally_capability_snapshots WHERE id = ?1",
    ).bind(id)
    };
    let row_before = legacy_row(before.snapshot.id.clone())
        .fetch_one(&repository.pool)
        .await
        .unwrap();
    repository.migrate().await.unwrap();
    repository.migrate().await.unwrap();
    assert_eq!(
        row_before,
        legacy_row(before.snapshot.id.clone())
            .fetch_one(&repository.pool)
            .await
            .unwrap()
    );
    let tier: Option<String> =
        sqlx::query_scalar("SELECT license_tier FROM tally_capability_snapshots WHERE id = ?1")
            .bind(&before.snapshot.id)
            .fetch_one(&repository.pool)
            .await
            .unwrap();
    assert_eq!(tier, None);
    let historical: (i64, Option<String>) = sqlx::query_as(
        "SELECT profile_version, license_tier FROM tally_capability_snapshots WHERE id = ?1",
    )
    .bind(&historical_v4.id)
    .fetch_one(&repository.pool)
    .await
    .unwrap();
    assert_eq!(historical, (4, None));
    assert_eq!(
        reviewed_setup_payload_sha256(&input).unwrap(),
        payload_before
    );
    let repeated = repository.save_reviewed_setup(input).await.unwrap();
    assert_eq!(before.snapshot, repeated.snapshot);
    // The previous binary's named-column INSERT remains valid and creates no tier claim.
    sqlx::query("INSERT INTO tally_capability_snapshots(id, endpoint_id, observed_at_unix_ms, profile_version, product, release, mode, mode_confidence)
        VALUES ('old-writer', ?1, 3, 3, 'Synthetic', NULL, NULL, 'unknown')")
        .bind(&before.snapshot.endpoint_id).execute(&repository.pool).await.unwrap();
    let tier: Option<String> = sqlx::query_scalar(
        "SELECT license_tier FROM tally_capability_snapshots WHERE id = 'old-writer'",
    )
    .fetch_one(&repository.pool)
    .await
    .unwrap();
    assert_eq!(tier, None);
    let applied: (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*), MIN(applied_at_unix_ms) FROM tally_schema_migrations WHERE version = 26",
    )
    .fetch_one(&repository.pool)
    .await
    .unwrap();
    assert_eq!(applied.0, 1);
    assert!(applied.1 > 0);
}
