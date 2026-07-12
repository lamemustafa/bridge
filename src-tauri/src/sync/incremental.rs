use chrono::Utc;
use sqlx::SqlitePool;

pub async fn update_altmastid(
    pool: &SqlitePool,
    company: &str,
    obj_type: &str,
    last_id: i64,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO altmastid_cache (company, obj_type, last_id, synced_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(company, obj_type) DO UPDATE SET
           last_id = excluded.last_id,
           synced_at = excluded.synced_at",
    )
    .bind(company)
    .bind(obj_type)
    .bind(last_id)
    .bind(Utc::now().timestamp())
    .execute(pool)
    .await?;

    Ok(())
}
