use chrono::Utc;
use serde::Serialize;
use sqlx::SqlitePool;
use uuid::Uuid;

#[derive(Debug, Serialize)]
pub struct OutboxItem<T>
where
    T: Serialize,
{
    pub operation: String,
    pub target: String,
    pub payload: T,
}

pub async fn enqueue<T>(pool: &SqlitePool, item: OutboxItem<T>) -> anyhow::Result<String>
where
    T: Serialize,
{
    let id = Uuid::new_v4().to_string();
    let payload = serde_json::to_string(&item.payload)?;

    sqlx::query(
        "INSERT INTO outbox (id, created, operation, target, payload, status, attempts)
         VALUES (?1, ?2, ?3, ?4, ?5, 'pending', 0)",
    )
    .bind(&id)
    .bind(Utc::now().timestamp())
    .bind(item.operation)
    .bind(item.target)
    .bind(payload)
    .execute(pool)
    .await?;

    Ok(id)
}
