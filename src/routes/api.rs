use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::Deserialize;

use crate::AppState;
use crate::auth::middleware::BearerUser;
use crate::envs::executor::{EnvExecutor, ExecOutput};
use crate::error::AppResult;
use crate::models::Env;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/envs", get(list_envs).post(create_env))
        .route("/api/v1/envs/:id", delete(delete_env))
        .route("/api/v1/envs/:id/execute", post(execute_in_env))
        .route("/api/v1/execute", post(execute_one_off))
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

#[derive(Deserialize)]
pub struct ExecuteBody {
    pub code: String,
}

async fn execute_in_env(
    State(state): State<AppState>,
    BearerUser(user): BearerUser,
    Path(id): Path<String>,
    Json(body): Json<ExecuteBody>,
) -> AppResult<Response> {
    let Some(_env) = Env::find_for_user(&state.pool, user.id, &id).await? else {
        return Ok((StatusCode::NOT_FOUND, "not found").into_response());
    };
    let output: ExecOutput = state.executor.execute(&id, &body.code).await?;
    Env::touch(&state.pool, &id).await?;
    Ok(Json(output).into_response())
}

async fn execute_one_off(
    State(state): State<AppState>,
    BearerUser(_user): BearerUser,
    Json(body): Json<ExecuteBody>,
) -> AppResult<Response> {
    let output = state.executor.execute_one_off(&body.code).await?;
    Ok(Json(output).into_response())
}
