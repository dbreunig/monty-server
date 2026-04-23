use axum::{Json, Router, routing::get};
use serde_json::Value;

use crate::AppState;
use crate::auth::middleware::BearerUser;

pub fn router() -> Router<AppState> {
    Router::new().route("/api/v1/envs", get(list_envs))
}

async fn list_envs(BearerUser(_user): BearerUser) -> Json<Vec<Value>> {
    Json(vec![])
}
