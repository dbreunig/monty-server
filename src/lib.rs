use axum::{Router, routing::get};

pub mod config;

pub use config::Config;

pub async fn app(_config: Config) -> Router {
    Router::new().route("/health", get(|| async { "ok" }))
}
