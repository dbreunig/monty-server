use axum::{Router, extract::State, routing::get};
use sqlx::SqlitePool;
use std::sync::Arc;

pub mod auth;
pub mod config;
pub mod db;
pub mod envs;
pub mod error;
pub mod models;
pub mod routes;

pub use config::Config;
use envs::storage::EnvStorage;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub pool: SqlitePool,
    pub env_storage: EnvStorage,
}

pub async fn app(config: Config) -> anyhow::Result<Router> {
    let db_url = config
        .data_dir
        .join("monty.db")
        .to_string_lossy()
        .into_owned();
    let pool = db::connect(&format!("sqlite://{db_url}")).await?;
    db::migrate(&pool).await?;
    build(config, pool).await
}

pub async fn app_with_pool(config: Config, pool: SqlitePool) -> anyhow::Result<Router> {
    db::migrate(&pool).await?;
    build(config, pool).await
}

async fn build(config: Config, pool: SqlitePool) -> anyhow::Result<Router> {
    let session_layer = auth::session::install(pool.clone()).await?;
    let env_storage = EnvStorage::new(&config.data_dir);
    let state = AppState {
        config: Arc::new(config),
        pool,
        env_storage,
    };
    let router = Router::new()
        .route("/health", get(health))
        .merge(auth::router())
        .merge(routes::api::router())
        .merge(routes::web::router())
        .with_state(state)
        .layer(session_layer);
    Ok(router)
}

async fn health(State(_): State<AppState>) -> &'static str {
    "ok"
}
