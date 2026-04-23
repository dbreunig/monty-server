use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get};
use axum::{Json, Router};
use serde::Deserialize;

use crate::AppState;
use crate::auth::middleware::BearerUser;
use crate::error::AppResult;
use crate::models::Env;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/envs", get(list_envs).post(create_env))
        .route("/api/v1/envs/:id", delete(delete_env))
}

async fn list_envs(
    State(state): State<AppState>,
    BearerUser(user): BearerUser,
) -> AppResult<Json<Vec<Env>>> {
    let envs = Env::list_for_user(&state.pool, user.id).await?;
    Ok(Json(envs))
}

#[derive(Deserialize)]
pub struct CreateEnvBody {
    pub name: String,
}

async fn create_env(
    State(state): State<AppState>,
    BearerUser(user): BearerUser,
    Json(body): Json<CreateEnvBody>,
) -> AppResult<Json<Env>> {
    let env = Env::create(&state.pool, user.id, &body.name).await?;
    // Create disk storage; if it fails, roll back the DB row.
    if let Err(e) = state.env_storage.create(&env.id).await {
        let _ = Env::delete_for_user(&state.pool, user.id, &env.id).await;
        return Err(e.into());
    }
    Ok(Json(env))
}

async fn delete_env(
    State(state): State<AppState>,
    BearerUser(user): BearerUser,
    Path(id): Path<String>,
) -> AppResult<Response> {
    let Some(env) = Env::find_for_user(&state.pool, user.id, &id).await? else {
        return Ok((StatusCode::NOT_FOUND, "not found").into_response());
    };
    let deleted = Env::delete_for_user(&state.pool, user.id, &env.id).await?;
    if !deleted {
        return Ok((StatusCode::NOT_FOUND, "not found").into_response());
    }
    state.env_storage.delete(&env.id).await?;
    Ok((StatusCode::OK, "deleted").into_response())
}

// Not all routes use the helper yet; re-expose for execute endpoints in task 13.
#[allow(dead_code)]
pub(crate) async fn authorize_env(
    state: &AppState,
    user_id: i64,
    id: &str,
) -> AppResult<Option<Env>> {
    Ok(Env::find_for_user(&state.pool, user_id, id).await?)
}
