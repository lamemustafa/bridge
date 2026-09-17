use sqlx::sqlite::SqlitePoolOptions;

use super::*;

async fn empty_repository() -> TallyMirrorRepository {
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
        .expect("connect synthetic mirror");
    let repository = TallyMirrorRepository::new(pool);
    repository
        .migrate()
        .await
        .expect("migrate synthetic mirror");
    repository
}

#[tokio::test]
async fn foundation_evidence_rejects_invalid_company_id_and_is_honest_about_absent_evidence() {
    let repository = empty_repository().await;

    assert!(matches!(
        repository.incremental_foundation_evidence("").await,
        Err(MirrorError::InvalidInput("company_id"))
    ));
    assert!(matches!(
        repository
            .incremental_foundation_evidence(&"x".repeat(129))
            .await,
        Err(MirrorError::InvalidInput("company_id"))
    ));

    let foundation = repository
        .incremental_foundation_evidence("no-such-company")
        .await
        .expect("read count-only evidence for an unknown company");
    assert!(!foundation.execution_enabled);
    assert_eq!(foundation.affirmative_exact_capability_receipts, 0);
    assert_eq!(foundation.establishment_receipts, 0);
    assert_eq!(foundation.active_checkpoint_heads, 0);
    assert_eq!(foundation.state, "exact_capability_not_observed");
    assert_eq!(
        foundation.fallback_warning_code,
        "incremental_execution_disabled_full_snapshot_required"
    );
}
