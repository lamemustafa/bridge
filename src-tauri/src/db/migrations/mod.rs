use sqlx::SqlitePool;

use super::schema::INITIAL_SCHEMA;

pub async fn run(pool: &SqlitePool) -> anyhow::Result<()> {
    for statement in INITIAL_SCHEMA
        .split(';')
        .map(str::trim)
        .filter(|sql| !sql.is_empty())
    {
        sqlx::query(statement).execute(pool).await?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use sqlx::sqlite::SqlitePoolOptions;

    use super::run;

    #[tokio::test]
    async fn initial_schema_runs_idempotently_with_sqlx() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect to an in-memory SQLite database");

        run(&pool).await.expect("create initial schema");
        run(&pool).await.expect("reapply initial schema");

        let table_count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM sqlite_master \
             WHERE type = 'table' AND name IN (\
             'outbox', 'sync_log', 'altmastid_cache', \
             'conflict_queue', 'companies', 'ledgers')",
        )
        .fetch_one(&pool)
        .await
        .expect("count initial schema tables");

        assert_eq!(table_count, 6);
        pool.close().await;
    }
}
