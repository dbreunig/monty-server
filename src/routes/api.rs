use axum::body::Bytes;
use axum::extract::{Multipart, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::Deserialize;

use crate::AppState;
use crate::auth::middleware::BearerUser;
use crate::envs::executor::{EnvExecutor, ExecOutput};
use crate::envs::storage::FileEntry;
use crate::error::AppResult;
use crate::models::Env;

const MAX_FILE_UPLOAD_BYTES: usize = 50 * 1024 * 1024;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/envs", get(list_envs).post(create_env))
        .route("/api/v1/envs/:id", delete(delete_env))
        .route("/api/v1/envs/:id/execute", post(execute_in_env))
        .route("/api/v1/execute", post(execute_one_off))
        .route("/api/v1/envs/:id/files", get(list_files).post(upload_file))
        .route(
            "/api/v1/envs/:id/files/:name",
            get(download_file).delete(delete_file),
        )
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

async fn list_files(
    State(state): State<AppState>,
    BearerUser(user): BearerUser,
    Path(id): Path<String>,
) -> AppResult<Response> {
    if Env::find_for_user(&state.pool, user.id, &id).await?.is_none() {
        return Ok((StatusCode::NOT_FOUND, "not found").into_response());
    }
    let files: Vec<FileEntry> = state.env_storage.list_files(&id).await?;
    Ok(Json(files).into_response())
}

async fn upload_file(
    State(state): State<AppState>,
    BearerUser(user): BearerUser,
    Path(id): Path<String>,
    mut multipart: Multipart,
) -> AppResult<Response> {
    if Env::find_for_user(&state.pool, user.id, &id).await?.is_none() {
        return Ok((StatusCode::NOT_FOUND, "not found").into_response());
    }
    let _guard = state.executor.locks().acquire(&id).await;
    let mut uploaded = Vec::new();
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| anyhow::anyhow!("multipart: {e}"))?
    {
        let Some(name) = field.file_name().map(|s| s.to_string()) else {
            continue;
        };
        let data: Bytes = field
            .bytes()
            .await
            .map_err(|e| anyhow::anyhow!("multipart read: {e}"))?;
        if data.len() > MAX_FILE_UPLOAD_BYTES {
            return Ok((StatusCode::PAYLOAD_TOO_LARGE, "file too large").into_response());
        }
        state.env_storage.write_file(&id, &name, &data).await?;
        uploaded.push(name);
    }
    Ok(Json(serde_json::json!({ "uploaded": uploaded })).into_response())
}

async fn download_file(
    State(state): State<AppState>,
    BearerUser(user): BearerUser,
    Path((id, name)): Path<(String, String)>,
) -> AppResult<Response> {
    if Env::find_for_user(&state.pool, user.id, &id).await?.is_none() {
        return Ok((StatusCode::NOT_FOUND, "not found").into_response());
    }
    match state.env_storage.read_file(&id, &name).await {
        Ok(bytes) => Ok((
            StatusCode::OK,
            [("content-type", "application/octet-stream")],
            bytes,
        )
            .into_response()),
        Err(_) => Ok((StatusCode::NOT_FOUND, "not found").into_response()),
    }
}

async fn delete_file(
    State(state): State<AppState>,
    BearerUser(user): BearerUser,
    Path((id, name)): Path<(String, String)>,
) -> AppResult<Response> {
    if Env::find_for_user(&state.pool, user.id, &id).await?.is_none() {
        return Ok((StatusCode::NOT_FOUND, "not found").into_response());
    }
    let _guard = state.executor.locks().acquire(&id).await;
    let deleted = state.env_storage.delete_file(&id, &name).await?;
    if !deleted {
        return Ok((StatusCode::NOT_FOUND, "not found").into_response());
    }
    Ok((StatusCode::OK, "deleted").into_response())
}
