use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::Value;
use std::time::Instant;

use crate::AppState;
use crate::auth::middleware::BearerUser;
use crate::error::AppResult;
use crate::functions::signature::parse_handler;
use crate::functions::validate::validate;
use crate::models::{Function, FunctionCall};

const PRUNE_KEEP: i64 = 500;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/fn/:slug", post(call_authed))
        .route("/fn/p/:public_token", post(call_public))
}

async fn call_authed(
    State(state): State<AppState>,
    BearerUser(user): BearerUser,
    Path(slug): Path<String>,
    body: Bytes,
) -> AppResult<Response> {
    let function = match Function::find_by_slug_for_user(&state.pool, user.id, &slug).await? {
        Some(f) => f,
        None => return Ok((StatusCode::NOT_FOUND, "not found").into_response()),
    };
    dispatch(state, function, body).await
}

async fn call_public(
    State(state): State<AppState>,
    Path(token): Path<String>,
    body: Bytes,
) -> AppResult<Response> {
    // Belt-and-suspenders: even if the row comes back, insist on is_public.
    // (T7's model method no longer filters on is_public in SQL.)
    let function = match Function::find_by_public_token(&state.pool, &token).await? {
        Some(f) if f.is_public => f,
        _ => return Ok((StatusCode::NOT_FOUND, "not found").into_response()),
    };
    dispatch(state, function, body).await
}

async fn dispatch(state: AppState, function: Function, body: Bytes) -> AppResult<Response> {
    let started = Instant::now();

    // Parse JSON body. Empty body is treated as {}.
    let body_str = match std::str::from_utf8(&body) {
        Ok(s) => s,
        Err(_) => {
            let err_body = serde_json::json!({ "error": "invalid json" });
            let resp = (StatusCode::BAD_REQUEST, Json(err_body.clone())).into_response();
            log_call(
                &state,
                &function,
                400,
                Some("<binary body>"),
                Some(&err_body.to_string()),
                Some("invalid utf-8 body"),
                started,
            )
            .await?;
            return Ok(resp);
        }
    };
    let body_json: Value = if body_str.trim().is_empty() {
        Value::Object(serde_json::Map::new())
    } else {
        match serde_json::from_str(body_str) {
            Ok(v) => v,
            Err(e) => {
                let err_body =
                    serde_json::json!({ "error": "invalid json", "detail": e.to_string() });
                let resp = (StatusCode::BAD_REQUEST, Json(err_body.clone())).into_response();
                log_call(
                    &state,
                    &function,
                    400,
                    Some(body_str),
                    Some(&err_body.to_string()),
                    Some(&e.to_string()),
                    started,
                )
                .await?;
                return Ok(resp);
            }
        }
    };

    // Parse handler signature.
    let sig = match parse_handler(&function.code) {
        Ok(s) => s,
        Err(e) => {
            let err_body =
                serde_json::json!({ "error": "handler unparseable", "detail": e.to_string() });
            let resp =
                (StatusCode::INTERNAL_SERVER_ERROR, Json(err_body.clone())).into_response();
            log_call(
                &state,
                &function,
                500,
                Some(body_str),
                Some(&err_body.to_string()),
                Some(&e.to_string()),
                started,
            )
            .await?;
            return Ok(resp);
        }
    };

    // Validate.
    let kwargs = match validate(&sig, &body_json) {
        Ok(k) => k,
        Err(schema_err) => {
            let err_body = serde_json::to_value(&schema_err)?;
            let resp = (StatusCode::BAD_REQUEST, Json(err_body.clone())).into_response();
            log_call(
                &state,
                &function,
                400,
                Some(body_str),
                Some(&err_body.to_string()),
                None,
                started,
            )
            .await?;
            return Ok(resp);
        }
    };

    // Execute.
    let outcome = match function.mode.as_str() {
        "stateless" => {
            state
                .fn_executor
                .call_stateless(&function.code, &sig, kwargs)
                .await?
        }
        "bound" => {
            let env_id = function
                .env_id
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("bound function missing env_id"))?;
            state
                .fn_executor
                .call_bound(env_id, &function.code, &sig, kwargs)
                .await?
        }
        other => {
            let err_body = serde_json::json!({ "error": format!("unknown mode: {other}") });
            let resp =
                (StatusCode::INTERNAL_SERVER_ERROR, Json(err_body.clone())).into_response();
            log_call(
                &state,
                &function,
                500,
                Some(body_str),
                Some(&err_body.to_string()),
                Some(other),
                started,
            )
            .await?;
            return Ok(resp);
        }
    };

    let status = StatusCode::from_u16(outcome.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let (resp, resp_text) = match (&outcome.response_json, &outcome.error) {
        (Some(json_str), _) => {
            // Ship the handler's JSON return value as-is with the right content
            // type. `response_json` is already a serialized JSON string.
            let text = json_str.clone();
            (
                (
                    status,
                    [(header::CONTENT_TYPE, "application/json")],
                    text.clone(),
                )
                    .into_response(),
                text,
            )
        }
        (None, Some(err)) => {
            let body = serde_json::json!({ "error": "handler failed", "detail": err });
            let s = body.to_string();
            ((status, Json(body)).into_response(), s)
        }
        (None, None) => {
            let body = serde_json::json!({ "error": "handler produced no result" });
            let s = body.to_string();
            (
                (StatusCode::INTERNAL_SERVER_ERROR, Json(body)).into_response(),
                s,
            )
        }
    };

    log_call(
        &state,
        &function,
        status.as_u16(),
        Some(body_str),
        Some(&resp_text),
        outcome.error.as_deref(),
        started,
    )
    .await?;
    Ok(resp)
}

async fn log_call(
    state: &AppState,
    function: &Function,
    status: u16,
    request: Option<&str>,
    response: Option<&str>,
    error: Option<&str>,
    started: Instant,
) -> anyhow::Result<()> {
    let duration_ms = started.elapsed().as_millis() as u64;
    FunctionCall::insert(
        &state.pool,
        &function.id,
        status,
        request,
        response,
        error,
        duration_ms,
        function.truncate_logs,
    )
    .await?;
    // Fire-and-forget prune; errors are logged but not surfaced.
    if let Err(e) = FunctionCall::prune(&state.pool, &function.id, PRUNE_KEEP).await {
        tracing::warn!(?e, function_id = %function.id, "function_calls prune failed");
    }
    Ok(())
}
