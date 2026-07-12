use chrono::Utc;
use serde::Serialize;
use sqlx::SqlitePool;
use uuid::Uuid;

#[derive(Debug, Serialize)]
pub struct AuditEntry {
    pub company: String,
    pub record_type: String,
    pub record_id: String,
    pub action: String,
    pub old_val: Option<String>,
    pub new_val: Option<String>,
    pub actor: String,
}

pub async fn append(pool: &SqlitePool, entry: AuditEntry) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO sync_log (id, ts, company, record_type, record_id, action, old_val, new_val, actor)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(Utc::now().timestamp())
    .bind(entry.company)
    .bind(entry.record_type)
    .bind(entry.record_id)
    .bind(entry.action)
    .bind(entry.old_val)
    .bind(entry.new_val)
    .bind(entry.actor)
    .execute(pool)
    .await?;

    Ok(())
}
