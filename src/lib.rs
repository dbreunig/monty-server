use axum::{Router, extract::State, routing::get};
use sqlx::SqlitePool;
use std::sync::Arc;

pub mod config;
pub mod db;

pub use config::Config;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub pool: SqlitePool,
}

pub async fn app(config: Config) -> anyhow::Result<Router> {
    let db_url = config
        .data_dir
        .join("monty.db")
        .to_string_lossy()
        .into_owned();
    let pool = db::connect(&format!("sqlite://{db_url}")).await?;
    db::migrate(&pool).await?;
    let state = AppState {
        config: Arc::new(config),
        pool,
    };
    Ok(build_router(state))
}

pub async fn app_with_pool(config: Config, pool: SqlitePool) -> anyhow::Result<Router> {
    db::migrate(&pool).await?;
    let state = AppState {
        config: Arc::new(config),
        pool,
    };
    Ok(build_router(state))
}

fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .with_state(state)
}

async fn health(State(_): State<AppState>) -> &'static str {
    "ok"
}
