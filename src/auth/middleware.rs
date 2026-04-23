use axum::async_trait;
use axum::extract::FromRequestParts;
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use tower_sessions::Session;

use crate::AppState;
use crate::auth::session::USER_ID_KEY;
use crate::models::{ApiKey, User};

/// Extracts the current user from a session cookie. 401 if unauthenticated.
/// Approval is NOT checked — use `ApprovedSessionUser` for gated routes.
pub struct SessionUser(pub User);

#[async_trait]
impl FromRequestParts<AppState> for SessionUser {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let session = Session::from_request_parts(parts, state)
            .await
            .map_err(|_| unauthorized())?;
        // Stored as a string because some deps enable serde_json's
        // `arbitrary_precision` feature, which breaks msgpack round-trip of
        // `Value::Number` via `tower-sessions-sqlx-store`.
        let raw: Option<String> = session.get(USER_ID_KEY).await.map_err(|_| unauthorized())?;
        let Some(user_id) = raw.and_then(|s| s.parse::<i64>().ok()) else {
            return Err(unauthorized());
        };
        let user = User::find_by_id(&state.pool, user_id)
            .await
            .map_err(|_| unauthorized())?
            .ok_or_else(unauthorized)?;
        Ok(SessionUser(user))
    }
}

/// Like `SessionUser` but requires `approved_at IS NOT NULL`. 403 if pending.
pub struct ApprovedSessionUser(pub User);

#[async_trait]
impl FromRequestParts<AppState> for ApprovedSessionUser {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let SessionUser(user) = SessionUser::from_request_parts(parts, state).await?;
        if !user.is_approved() {
            return Err(pending_approval_html());
        }
        Ok(ApprovedSessionUser(user))
    }
}

/// Like `SessionUser` but requires `is_admin = 1`.
pub struct AdminUser(pub User);

#[async_trait]
impl FromRequestParts<AppState> for AdminUser {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let SessionUser(user) = SessionUser::from_request_parts(parts, state).await?;
        if !user.is_admin {
            return Err((StatusCode::FORBIDDEN, "admin only").into_response());
        }
        Ok(AdminUser(user))
    }
}

/// Extracts the current user from a `Authorization: Bearer mk_...` header.
/// Requires the user to be approved — pending users' bearer tokens are rejected
/// with 403 so admin revocation takes effect immediately.
pub struct BearerUser(pub User);

#[async_trait]
impl FromRequestParts<AppState> for BearerUser {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let header = parts
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .ok_or_else(unauthorized)?;
        let token = header.strip_prefix("Bearer ").ok_or_else(unauthorized)?;
        let api_key = ApiKey::verify(&state.pool, token)
            .await
            .map_err(|_| unauthorized())?
            .ok_or_else(unauthorized)?;
        let user = User::find_by_id(&state.pool, api_key.user_id)
            .await
            .map_err(|_| unauthorized())?
            .ok_or_else(unauthorized)?;
        if !user.is_approved() {
            return Err((StatusCode::FORBIDDEN, "account pending approval").into_response());
        }
        Ok(BearerUser(user))
    }
}

fn unauthorized() -> Response {
    (StatusCode::UNAUTHORIZED, "unauthorized").into_response()
}

fn pending_approval_html() -> Response {
    let body = r#"<!doctype html><html><body style="font-family:system-ui;max-width:600px;margin:4rem auto;padding:0 1rem">
<h1>Pending approval</h1>
<p>Your account is waiting for an administrator to approve it. You'll be able to create API keys and environments once approved.</p>
<p><a href="/dashboard">← Back to dashboard</a></p>
</body></html>"#;
    (
        StatusCode::FORBIDDEN,
        [("content-type", "text/html; charset=utf-8")],
        body,
    )
        .into_response()
}
