use anyhow::{Context, Result};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::{ConnectOptions, SqlitePool};
use std::str::FromStr;

pub async fn connect(url: &str) -> Result<SqlitePool> {
    let options = SqliteConnectOptions::from_str(url)
        .with_context(|| format!("invalid sqlite url: {url}"))?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .foreign_keys(true)
        .disable_statement_logging();

    // `:memory:` gives each connection its own DB; pin the pool to 1 so test
    // data stays consistent. File-backed DBs use a real pool.
    let is_memory = url.contains(":memory:") || url == ":memory:";
    let max_connections = if is_memory { 1 } else { 8 };

    let pool = SqlitePoolOptions::new()
        .max_connections(max_connections)
        .connect_with(options)
        .await
        .context("failed to connect to sqlite")?;
    Ok(pool)
}

pub async fn migrate(pool: &SqlitePool) -> Result<()> {
    sqlx::migrate!("./migrations")
        .run(pool)
        .await
        .context("sqlx migrations failed")?;
    Ok(())
}
