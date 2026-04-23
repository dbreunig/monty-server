use anyhow::Result;
use sqlx::SqlitePool;
use time::Duration;
use tower_sessions::{Expiry, SessionManagerLayer};
use tower_sessions_sqlx_store::SqliteStore;

pub const STATE_KEY: &str = "oauth_state";
pub const USER_ID_KEY: &str = "user_id";

pub async fn install(pool: SqlitePool) -> Result<SessionManagerLayer<SqliteStore>> {
    let store = SqliteStore::new(pool);
    store.migrate().await?;
    let layer = SessionManagerLayer::new(store)
        .with_secure(false)
        .with_same_site(tower_sessions::cookie::SameSite::Lax)
        .with_expiry(Expiry::OnInactivity(Duration::days(7)));
    Ok(layer)
}
