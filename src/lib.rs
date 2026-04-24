use axum::{Router, extract::State, routing::get};
use sqlx::SqlitePool;
use std::sync::Arc;
use tower_http::services::ServeDir;

pub mod auth;
pub mod config;
pub mod db;
pub mod envs;
pub mod error;
pub mod functions;
pub mod models;
pub mod routes;

pub use config::Config;
use envs::executor::MontyExecutor;
use envs::locks::LockRegistry;
use envs::storage::EnvStorage;
use models::User;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub pool: SqlitePool,
    pub env_storage: EnvStorage,
    pub executor: Arc<MontyExecutor>,
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
    let locks = LockRegistry::new();
    let executor = Arc::new(MontyExecutor::new(env_storage.clone(), locks));
    if let (Some(email), Some(password)) = (&config.admin_email, &config.admin_password) {
        User::ensure_admin(&pool, email, password).await?;
        tracing::info!(admin_email = %email, "admin user seeded");
    }
    let state = AppState {
        config: Arc::new(config),
        pool,
        env_storage,
        executor,
    };
    let assets_dir = std::env::var("ASSETS_DIR").unwrap_or_else(|_| ".".to_string());
    let static_dir = std::path::PathBuf::from(&assets_dir).join("static");
    let plex_dir = std::path::PathBuf::from(&assets_dir).join("ibm-plex-sans");
    let router = Router::new()
        .route("/health", get(health))
        .merge(auth::router())
        .merge(routes::admin::router())
        .merge(routes::api::router())
        .merge(routes::web::router())
        .with_state(state)
        .nest_service("/static", ServeDir::new(static_dir))
        .nest_service("/ibm-plex-sans", ServeDir::new(plex_dir))
        .layer(session_layer);
    Ok(router)
}

async fn health(State(_): State<AppState>) -> &'static str {
    "ok"
}
