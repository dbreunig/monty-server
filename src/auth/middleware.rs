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
        let user_id: Option<i64> = session.get(USER_ID_KEY).await.map_err(|_| unauthorized())?;
        let Some(user_id) = user_id else {
            return Err(unauthorized());
        };
        let user = User::find_by_id(&state.pool, user_id)
            .await
            .map_err(|_| unauthorized())?
            .ok_or_else(unauthorized)?;
        Ok(SessionUser(user))
    }
}

/// Extracts the current user from a `Authorization: Bearer mk_...` header. 401 if unauthenticated.
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
        Ok(BearerUser(user))
    }
}

fn unauthorized() -> Response {
    (StatusCode::UNAUTHORIZED, "unauthorized").into_response()
}
