use askama::Template;
use askama_axum::IntoResponse;
use axum::extract::{Path, State};
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;

use crate::AppState;
use crate::auth::middleware::AdminUser;
use crate::error::AppResult;
use crate::models::User;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/admin", get(index))
        .route("/admin/approve/:id", post(approve))
        .route("/admin/revoke/:id", post(revoke))
}

#[derive(Template)]
#[template(path = "admin.html")]
struct AdminTemplate {
    pending: Vec<User>,
}

#[derive(Template)]
#[template(path = "_admin_pending.html")]
struct PendingFragment {
    pending: Vec<User>,
}

async fn index(
    State(state): State<AppState>,
    AdminUser(_admin): AdminUser,
) -> AppResult<Response> {
    let pending = User::list_pending(&state.pool).await?;
    Ok(AdminTemplate { pending }.into_response())
}

async fn approve(
    State(state): State<AppState>,
    AdminUser(_admin): AdminUser,
    Path(id): Path<i64>,
) -> AppResult<Response> {
    User::approve(&state.pool, id).await?;
    let pending = User::list_pending(&state.pool).await?;
    Ok(PendingFragment { pending }.into_response())
}

async fn revoke(
    State(state): State<AppState>,
    AdminUser(_admin): AdminUser,
    Path(id): Path<i64>,
) -> AppResult<Response> {
    User::revoke(&state.pool, id).await?;
    let pending = User::list_pending(&state.pool).await?;
    Ok(PendingFragment { pending }.into_response())
}
