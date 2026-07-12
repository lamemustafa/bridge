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
