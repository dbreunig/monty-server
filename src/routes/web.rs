use askama::Template;
use askama_axum::IntoResponse;
use axum::extract::{Form, Path, State};
use axum::response::Response;
use axum::{Router, routing::post};
use serde::Deserialize;

use crate::AppState;
use crate::auth::middleware::SessionUser;
use crate::error::AppResult;
use crate::models::{ApiKey, User};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/dashboard", axum::routing::get(dashboard))
        .route("/api-keys", post(create_api_key))
        .route("/api-keys/:id/delete", post(delete_api_key))
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
    State(state): State<AppState>,
    SessionUser(user): SessionUser,
    Form(form): Form<CreateApiKeyForm>,
) -> AppResult<Response> {
    let (_key, plaintext) = ApiKey::create(&state.pool, user.id, &form.name).await?;
    let api_keys = ApiKey::list_for_user(&state.pool, user.id).await?;
    Ok(ApiKeysFragment {
        api_keys,
        new_plaintext: Some(plaintext),
    }
    .into_response())
}

async fn delete_api_key(
    State(state): State<AppState>,
    SessionUser(user): SessionUser,
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
