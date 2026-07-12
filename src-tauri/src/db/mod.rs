pub mod audit;
pub mod migrations;
pub mod outbox;
pub mod schema;

use sqlx::{sqlite::SqlitePoolOptions, SqlitePool};

pub async fn connect(database_url: &str) -> anyhow::Result<SqlitePool> {
    Ok(SqlitePoolOptions::new()
        .max_connections(5)
        .connect(database_url)
        .await?)
}
