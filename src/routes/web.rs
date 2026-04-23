use askama::Template;
use askama_axum::IntoResponse;
use axum::body::Bytes;
use axum::extract::{Form, Multipart, Path, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{delete, get, post};
use axum::Router;
use serde::Deserialize;

use crate::AppState;
use crate::auth::middleware::SessionUser;
use crate::envs::executor::EnvExecutor;
use crate::envs::storage::FileEntry;
use crate::error::AppResult;
use crate::models::{ApiKey, Env, User};

const MAX_UI_UPLOAD_BYTES: usize = 50 * 1024 * 1024;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/dashboard", get(dashboard))
        .route("/api-keys", post(create_api_key))
        .route("/api-keys/:id/delete", post(delete_api_key))
        .route("/envs", get(env_list).post(create_env_ui))
        .route("/envs/:id", get(env_detail).delete(delete_env_ui))
        .route("/envs/:id/repl", post(repl))
        .route("/envs/:id/files", post(upload_file_ui))
        .route("/envs/:id/files/:name", delete(delete_file_ui))
}

#[derive(Template)]
#[template(path = "dashboard.html")]
struct DashboardTemplate {
    user: User,
    api_keys: Vec<ApiKey>,
    new_plaintext: Option<String>,
}

#[derive(Template)]
#[template(path = "_api_keys.html")]
struct ApiKeysFragment {
    api_keys: Vec<ApiKey>,
    new_plaintext: Option<String>,
}

#[derive(Template)]
#[template(path = "env_list.html")]
struct EnvListTemplate {
    user: User,
    envs: Vec<Env>,
}

#[derive(Template)]
#[template(path = "_env_list.html")]
struct EnvListFragment {
    envs: Vec<Env>,
}

#[derive(Template)]
#[template(path = "env_detail.html")]
struct EnvDetailTemplate {
    env: Env,
    env_id: String,
    files: Vec<FileEntry>,
}

#[derive(Template)]
#[template(path = "_file_list.html")]
struct FileListFragment {
    env_id: String,
    files: Vec<FileEntry>,
}

#[derive(Template)]
#[template(path = "_repl_output.html")]
struct ReplOutputFragment {
    code: String,
    stdout: String,
    stderr: String,
    result: Option<String>,
    error: Option<String>,
}

async fn dashboard(
    State(state): State<AppState>,
    SessionUser(user): SessionUser,
) -> AppResult<Response> {
    let api_keys = ApiKey::list_for_user(&state.pool, user.id).await?;
    Ok(DashboardTemplate {
        user,
        api_keys,
        new_plaintext: None,
    }
    .into_response())
}

#[derive(Deserialize)]
struct CreateApiKeyForm {
    name: String,
}

async fn create_api_key(
    SessionUser(user): SessionUser,
    State(state): State<AppState>,
    Form(form): Form<CreateApiKeyForm>,
) -> AppResult<Response> {
    let (_, plaintext) = ApiKey::create(&state.pool, user.id, &form.name).await?;
    let api_keys = ApiKey::list_for_user(&state.pool, user.id).await?;
    Ok(ApiKeysFragment {
        api_keys,
        new_plaintext: Some(plaintext),
    }
    .into_response())
}

async fn delete_api_key(
    SessionUser(user): SessionUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Response> {
    ApiKey::delete(&state.pool, user.id, id).await?;
    let api_keys = ApiKey::list_for_user(&state.pool, user.id).await?;
    Ok(ApiKeysFragment {
        api_keys,
        new_plaintext: None,
    }
    .into_response())
}

async fn env_list(
    SessionUser(user): SessionUser,
    State(state): State<AppState>,
) -> AppResult<Response> {
    let envs = Env::list_for_user(&state.pool, user.id).await?;
    Ok(EnvListTemplate { user, envs }.into_response())
}

#[derive(Deserialize)]
struct CreateEnvForm {
    name: String,
}

async fn create_env_ui(
    SessionUser(user): SessionUser,
    State(state): State<AppState>,
    Form(form): Form<CreateEnvForm>,
) -> AppResult<Response> {
    let env = Env::create(&state.pool, user.id, &form.name).await?;
    if let Err(e) = state.env_storage.create(&env.id).await {
        let _ = Env::delete_for_user(&state.pool, user.id, &env.id).await;
        return Err(e.into());
    }
    let envs = Env::list_for_user(&state.pool, user.id).await?;
    Ok(EnvListFragment { envs }.into_response())
}

async fn delete_env_ui(
    SessionUser(user): SessionUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Response> {
    if Env::delete_for_user(&state.pool, user.id, &id).await? {
        let _ = state.env_storage.delete(&id).await;
    }
    let envs = Env::list_for_user(&state.pool, user.id).await?;
    Ok(EnvListFragment { envs }.into_response())
}

async fn env_detail(
    SessionUser(user): SessionUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Response> {
    let Some(env) = Env::find_for_user(&state.pool, user.id, &id).await? else {
        return Ok((StatusCode::NOT_FOUND, "not found").into_response());
    };
    let files = state.env_storage.list_files(&env.id).await?;
    let env_id = env.id.clone();
    Ok(EnvDetailTemplate { env, env_id, files }.into_response())
}

#[derive(Deserialize)]
struct ReplForm {
    code: String,
}

async fn repl(
    SessionUser(user): SessionUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Form(form): Form<ReplForm>,
) -> AppResult<Response> {
    if Env::find_for_user(&state.pool, user.id, &id).await?.is_none() {
        return Ok((StatusCode::NOT_FOUND, "not found").into_response());
    }
    let output = state.executor.execute(&id, &form.code).await?;
    Ok(ReplOutputFragment {
        code: form.code,
        stdout: output.stdout,
        stderr: output.stderr,
        result: output.result,
        error: output.error,
    }
    .into_response())
}

async fn upload_file_ui(
    SessionUser(user): SessionUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
    mut multipart: Multipart,
) -> AppResult<Response> {
    if Env::find_for_user(&state.pool, user.id, &id).await?.is_none() {
        return Ok((StatusCode::NOT_FOUND, "not found").into_response());
    }
    let _guard = state.executor.locks().acquire(&id).await;
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
        if data.len() > MAX_UI_UPLOAD_BYTES {
            return Ok((StatusCode::PAYLOAD_TOO_LARGE, "file too large").into_response());
        }
        state.env_storage.write_file(&id, &name, &data).await?;
    }
    let files = state.env_storage.list_files(&id).await?;
    Ok(FileListFragment { env_id: id, files }.into_response())
}

async fn delete_file_ui(
    SessionUser(user): SessionUser,
    State(state): State<AppState>,
    Path((id, name)): Path<(String, String)>,
) -> AppResult<Response> {
    if Env::find_for_user(&state.pool, user.id, &id).await?.is_none() {
        return Ok((StatusCode::NOT_FOUND, "not found").into_response());
    }
    let _guard = state.executor.locks().acquire(&id).await;
    state.env_storage.delete_file(&id, &name).await?;
    let files = state.env_storage.list_files(&id).await?;
    Ok(FileListFragment { env_id: id, files }.into_response())
}
