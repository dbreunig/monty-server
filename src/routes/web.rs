use askama::Template;
use askama_axum::IntoResponse;
use axum::body::Bytes;
use axum::extract::{Form, Multipart, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{Redirect, Response};
use axum::routing::{get, post};
use axum::Router;
use serde::Deserialize;

use crate::AppState;
use crate::auth::middleware::{ApprovedSessionUser, SessionUser};
use crate::envs::executor::EnvExecutor;
use crate::error::AppResult;
use crate::functions::signature::parse_handler;
use crate::models::function::validate_slug;
use crate::models::{ApiKey, Env, Function, FunctionCall, ReplCall, User};

const MAX_UI_UPLOAD_BYTES: usize = 50 * 1024 * 1024;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(root))
        .route("/dashboard", get(dashboard))
        .route("/api-keys", post(create_api_key))
        .route("/api-keys/:id/delete", post(delete_api_key))
        .route("/envs", post(create_env_ui))
        .route("/envs/:id", get(env_detail).delete(delete_env_ui))
        .route("/envs/:id/repl", post(repl))
        .route(
            "/envs/:id/files",
            get(list_files_ui).post(upload_file_ui).delete(delete_file_ui),
        )
        .route("/functions", post(create_function_ui))
        .route("/functions/:id/delete", post(delete_function_ui))
        .route("/functions/:id", get(function_detail).post(update_function_ui))
        .route("/functions/:id/rotate", post(rotate_public_token_ui))
}

#[derive(Template)]
#[template(path = "dashboard.html")]
struct DashboardTemplate {
    user: User,
    api_keys: Vec<ApiKey>,
    new_plaintext: Option<String>,
    envs: Vec<Env>,
    functions: Vec<Function>,
    error_message: Option<String>,
}

#[derive(Template)]
#[template(path = "_api_keys.html")]
struct ApiKeysFragment {
    api_keys: Vec<ApiKey>,
    new_plaintext: Option<String>,
}

#[derive(Template)]
#[template(path = "_env_list.html")]
struct EnvListFragment {
    envs: Vec<Env>,
}

#[derive(Template)]
#[template(path = "env_detail.html")]
struct EnvDetailTemplate {
    user: User,
    env: Env,
    env_id: String,
    panel_path: String,
    panel_crumbs: Vec<Crumb>,
    panel_entries: Vec<PanelEntry>,
    repl_calls: Vec<ReplCall>,
}

#[derive(Template)]
#[template(path = "_files_panel.html")]
struct FilesPanelFragment {
    env_id: String,
    panel_path: String,
    panel_crumbs: Vec<Crumb>,
    panel_entries: Vec<PanelEntry>,
}

#[derive(Debug, Clone)]
struct Crumb {
    name: String,
    path: String,
}

#[derive(Debug, Clone)]
struct PanelEntry {
    name: String,
    relpath: String,
    size: u64,
    is_dir: bool,
}

fn build_crumbs(path: &str) -> Vec<Crumb> {
    if path.is_empty() {
        return Vec::new();
    }
    let mut acc = String::new();
    let mut out = Vec::new();
    for seg in path.split('/') {
        if !acc.is_empty() {
            acc.push('/');
        }
        acc.push_str(seg);
        out.push(Crumb {
            name: seg.to_string(),
            path: acc.clone(),
        });
    }
    out
}

fn join_relpath(base: &str, name: &str) -> String {
    if base.is_empty() {
        name.to_string()
    } else {
        format!("{base}/{name}")
    }
}

async fn load_panel(
    state: &AppState,
    env_id: &str,
    path: &str,
) -> AppResult<(Vec<Crumb>, Vec<PanelEntry>)> {
    let entries: Vec<PanelEntry> = state
        .env_storage
        .list_dir(env_id, path)
        .await?
        .into_iter()
        .map(|e| {
            let relpath = join_relpath(path, &e.name);
            PanelEntry {
                name: e.name,
                relpath,
                size: e.size,
                is_dir: e.is_dir,
            }
        })
        .collect();
    Ok((build_crumbs(path), entries))
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

async fn root(user: Option<SessionUser>) -> Redirect {
    if user.is_some() {
        Redirect::to("/dashboard")
    } else {
        Redirect::to("/auth/login")
    }
}

#[derive(Debug, Default, Deserialize)]
struct DashboardQuery {
    #[serde(default)]
    error: Option<String>,
}

async fn dashboard(
    State(state): State<AppState>,
    SessionUser(user): SessionUser,
    Query(q): Query<DashboardQuery>,
) -> AppResult<Response> {
    let (api_keys, envs, functions) = if user.is_approved() {
        (
            ApiKey::list_for_user(&state.pool, user.id).await?,
            Env::list_for_user(&state.pool, user.id).await?,
            Function::list_for_user(&state.pool, user.id).await?,
        )
    } else {
        (Vec::new(), Vec::new(), Vec::new())
    };
    Ok(DashboardTemplate {
        user,
        api_keys,
        new_plaintext: None,
        envs,
        functions,
        error_message: q.error,
    }
    .into_response())
}

#[derive(Deserialize)]
struct CreateApiKeyForm {
    name: String,
}

async fn create_api_key(
    ApprovedSessionUser(user): ApprovedSessionUser,
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
    ApprovedSessionUser(user): ApprovedSessionUser,
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

#[derive(Deserialize)]
struct CreateEnvForm {
    name: String,
}

async fn create_env_ui(
    ApprovedSessionUser(user): ApprovedSessionUser,
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
    ApprovedSessionUser(user): ApprovedSessionUser,
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
    ApprovedSessionUser(user): ApprovedSessionUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Response> {
    let Some(env) = Env::find_for_user(&state.pool, user.id, &id).await? else {
        return Ok((StatusCode::NOT_FOUND, "not found").into_response());
    };
    let (panel_crumbs, panel_entries) = load_panel(&state, &env.id, "").await?;
    let repl_calls = ReplCall::list_for_env(&state.pool, &env.id).await?;
    let env_id = env.id.clone();
    Ok(EnvDetailTemplate {
        user,
        env,
        env_id,
        panel_path: String::new(),
        panel_crumbs,
        panel_entries,
        repl_calls,
    }
    .into_response())
}

#[derive(Deserialize)]
struct ReplForm {
    code: String,
}

async fn repl(
    ApprovedSessionUser(user): ApprovedSessionUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Form(form): Form<ReplForm>,
) -> AppResult<Response> {
    if Env::find_for_user(&state.pool, user.id, &id).await?.is_none() {
        return Ok((StatusCode::NOT_FOUND, "not found").into_response());
    }
    let output = state.executor.execute(&id, &form.code).await?;
    ReplCall::create(
        &state.pool,
        &id,
        &form.code,
        &output.stdout,
        &output.stderr,
        output.result.as_deref(),
        output.error.as_deref(),
    )
    .await?;
    Ok(ReplOutputFragment {
        code: form.code,
        stdout: output.stdout,
        stderr: output.stderr,
        result: output.result,
        error: output.error,
    }
    .into_response())
}

#[derive(Debug, Default, Deserialize)]
struct PanelQuery {
    #[serde(default)]
    path: String,
}

#[derive(Debug, Deserialize)]
struct DeleteQuery {
    #[serde(default)]
    path: String,
    name: String,
}

fn panel_response(
    env_id: String,
    panel_path: String,
    panel_crumbs: Vec<Crumb>,
    panel_entries: Vec<PanelEntry>,
) -> Response {
    FilesPanelFragment {
        env_id,
        panel_path,
        panel_crumbs,
        panel_entries,
    }
    .into_response()
}

async fn list_files_ui(
    ApprovedSessionUser(user): ApprovedSessionUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<PanelQuery>,
) -> AppResult<Response> {
    if Env::find_for_user(&state.pool, user.id, &id).await?.is_none() {
        return Ok((StatusCode::NOT_FOUND, "not found").into_response());
    }
    if !q.path.is_empty() && !state.env_storage.dir_exists(&id, &q.path).await? {
        return Ok((StatusCode::NOT_FOUND, "folder not found").into_response());
    }
    let (crumbs, entries) = load_panel(&state, &id, &q.path).await?;
    Ok(panel_response(id, q.path, crumbs, entries))
}

async fn upload_file_ui(
    ApprovedSessionUser(user): ApprovedSessionUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<PanelQuery>,
    mut multipart: Multipart,
) -> AppResult<Response> {
    if Env::find_for_user(&state.pool, user.id, &id).await?.is_none() {
        return Ok((StatusCode::NOT_FOUND, "not found").into_response());
    }
    if !q.path.is_empty() && !state.env_storage.dir_exists(&id, &q.path).await? {
        return Ok((StatusCode::NOT_FOUND, "folder not found").into_response());
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
        let target = join_relpath(&q.path, &name);
        state.env_storage.write_file(&id, &target, &data).await?;
    }
    let (crumbs, entries) = load_panel(&state, &id, &q.path).await?;
    Ok(panel_response(id, q.path, crumbs, entries))
}

async fn delete_file_ui(
    ApprovedSessionUser(user): ApprovedSessionUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<DeleteQuery>,
) -> AppResult<Response> {
    if Env::find_for_user(&state.pool, user.id, &id).await?.is_none() {
        return Ok((StatusCode::NOT_FOUND, "not found").into_response());
    }
    let _guard = state.executor.locks().acquire(&id).await;
    state.env_storage.delete_file(&id, &q.name).await?;
    let (crumbs, entries) = load_panel(&state, &id, &q.path).await?;
    Ok(panel_response(id, q.path, crumbs, entries))
}

// ---------- Functions ----------

#[derive(Deserialize)]
struct CreateFunctionForm {
    slug: String,
    mode: String,
    #[serde(default)]
    env_id: Option<String>,
    code: String,
    #[serde(default)]
    is_public: Option<String>,
}

fn percent_encode_query(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'0'..=b'9' | b'A'..=b'Z' | b'a'..=b'z' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                out.push('%');
                out.push_str(&format!("{b:02X}"));
            }
        }
    }
    out
}

fn dashboard_error_redirect(msg: &str) -> Response {
    let encoded = percent_encode_query(msg);
    Redirect::to(&format!("/dashboard?error={encoded}")).into_response()
}

async fn create_function_ui(
    ApprovedSessionUser(user): ApprovedSessionUser,
    State(state): State<AppState>,
    Form(form): Form<CreateFunctionForm>,
) -> AppResult<Response> {
    // Validate mode.
    if form.mode != "stateless" && form.mode != "bound" {
        return Ok(dashboard_error_redirect(
            "mode must be 'stateless' or 'bound'",
        ));
    }

    // Validate env pairing.
    let env_id_ref: Option<&str> = if form.mode == "bound" {
        let env_id = form
            .env_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let Some(env_id) = env_id else {
            return Ok(dashboard_error_redirect(
                "env_id is required for bound functions",
            ));
        };
        if Env::find_for_user(&state.pool, user.id, env_id)
            .await?
            .is_none()
        {
            return Ok(dashboard_error_redirect(
                "env_id does not exist or is not owned by you",
            ));
        }
        Some(env_id)
    } else {
        None
    };

    // Validate slug shape.
    if let Err(e) = validate_slug(&form.slug) {
        return Ok(dashboard_error_redirect(&format!("invalid slug: {e}")));
    }

    // Validate code.
    if let Err(e) = parse_handler(&form.code) {
        return Ok(dashboard_error_redirect(&format!("invalid code: {e}")));
    }

    let is_public = form.is_public.as_deref().is_some_and(|v| {
        matches!(v, "on" | "true" | "1" | "yes")
    });

    match Function::create(
        &state.pool,
        user.id,
        &form.slug,
        &form.code,
        &form.mode,
        env_id_ref,
        is_public,
        true,
    )
    .await
    {
        Ok(_) => Ok(Redirect::to("/dashboard").into_response()),
        Err(e) => {
            let msg = e.to_string();
            if msg.to_uppercase().contains("UNIQUE") {
                Ok(dashboard_error_redirect("slug already in use"))
            } else {
                Ok(dashboard_error_redirect(&format!(
                    "could not create function: {msg}"
                )))
            }
        }
    }
}

async fn delete_function_ui(
    ApprovedSessionUser(user): ApprovedSessionUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Response> {
    Function::delete_for_user(&state.pool, user.id, &id).await?;
    Ok(Redirect::to("/dashboard").into_response())
}

#[derive(Template)]
#[template(path = "function_detail.html")]
struct FunctionDetailTemplate {
    user: User,
    function: Function,
    calls: Vec<FunctionCall>,
    error_message: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct FunctionDetailQuery {
    #[serde(default)]
    error: Option<String>,
}

fn function_error_redirect(id: &str, msg: &str) -> Response {
    let encoded = percent_encode_query(msg);
    Redirect::to(&format!("/functions/{id}?error={encoded}")).into_response()
}

async fn function_detail(
    ApprovedSessionUser(user): ApprovedSessionUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<FunctionDetailQuery>,
) -> AppResult<Response> {
    let Some(function) = Function::find_for_user(&state.pool, user.id, &id).await? else {
        return Ok((StatusCode::NOT_FOUND, "not found").into_response());
    };
    let calls = FunctionCall::list_recent(&state.pool, &function.id, 100).await?;
    Ok(FunctionDetailTemplate {
        user,
        function,
        calls,
        error_message: q.error,
    }
    .into_response())
}

#[derive(Deserialize)]
struct UpdateFunctionForm {
    code: String,
    #[serde(default)]
    is_public: Option<String>,
    #[serde(default)]
    truncate_logs: Option<String>,
}

async fn update_function_ui(
    ApprovedSessionUser(user): ApprovedSessionUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Form(form): Form<UpdateFunctionForm>,
) -> AppResult<Response> {
    // Ownership check — 404 if the function isn't the caller's.
    let Some(_function) = Function::find_for_user(&state.pool, user.id, &id).await? else {
        return Ok((StatusCode::NOT_FOUND, "not found").into_response());
    };

    // Validate code.
    if let Err(e) = parse_handler(&form.code) {
        return Ok(function_error_redirect(
            &id,
            &format!("invalid code: {e}"),
        ));
    }

    let is_public = form.is_public.is_some();
    let truncate_logs = form.truncate_logs.is_some();

    Function::update_code(&state.pool, &id, &form.code).await?;
    Function::set_public(&state.pool, &id, is_public).await?;
    Function::set_truncate_logs(&state.pool, &id, truncate_logs).await?;

    Ok(Redirect::to(&format!("/functions/{id}")).into_response())
}

async fn rotate_public_token_ui(
    ApprovedSessionUser(user): ApprovedSessionUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Response> {
    let Some(_function) = Function::find_for_user(&state.pool, user.id, &id).await? else {
        return Ok((StatusCode::NOT_FOUND, "not found").into_response());
    };
    if let Err(e) = Function::rotate_public_token(&state.pool, &id).await {
        return Ok(function_error_redirect(
            &id,
            &format!("could not rotate public URL: {e}"),
        ));
    }
    Ok(Redirect::to(&format!("/functions/{id}")).into_response())
}
