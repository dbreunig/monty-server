use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use oauth2::basic::BasicClient;
use oauth2::{
    AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, RedirectUrl, Scope,
    TokenResponse, TokenUrl,
};
use serde::Deserialize;
use tower_sessions::Session;

use crate::auth::session::{STATE_KEY, USER_ID_KEY};
use crate::error::AppResult;
use crate::models::User;
use crate::{AppState, Config};

fn found(location: &str) -> Response {
    Response::builder()
        .status(StatusCode::FOUND)
        .header("location", location)
        .body(axum::body::Body::empty())
        .unwrap()
}

pub fn build_oauth_client(cfg: &Config) -> anyhow::Result<BasicClient> {
    let client = BasicClient::new(
        ClientId::new(cfg.github_client_id.clone()),
        Some(ClientSecret::new(cfg.github_client_secret.clone())),
        AuthUrl::new(cfg.github_auth_url.clone())?,
        Some(TokenUrl::new(cfg.github_token_url.clone())?),
    )
    .set_redirect_uri(RedirectUrl::new(format!(
        "{}/auth/callback",
        cfg.base_url.trim_end_matches('/')
    ))?);
    Ok(client)
}

pub async fn login(State(state): State<AppState>, session: Session) -> AppResult<Response> {
    let client = build_oauth_client(&state.config)?;
    let (auth_url, csrf) = client
        .authorize_url(CsrfToken::new_random)
        .add_scope(Scope::new("read:user".into()))
        .add_scope(Scope::new("user:email".into()))
        .url();
    session.insert(STATE_KEY, csrf.secret()).await?;
    Ok(found(auth_url.as_ref()))
}

#[derive(Deserialize)]
pub struct CallbackParams {
    pub code: String,
    pub state: String,
}

pub async fn callback(
    State(state): State<AppState>,
    session: Session,
    Query(params): Query<CallbackParams>,
) -> AppResult<Response> {
    let stored: Option<String> = session.get(STATE_KEY).await?;
    let Some(stored) = stored else {
        return Ok((StatusCode::BAD_REQUEST, "missing oauth state").into_response());
    };
    if stored != params.state {
        return Ok((StatusCode::BAD_REQUEST, "state mismatch").into_response());
    }
    session.remove::<String>(STATE_KEY).await?;

    let client = build_oauth_client(&state.config)?;
    let token = client
        .exchange_code(AuthorizationCode::new(params.code))
        .request_async(oauth2::reqwest::async_http_client)
        .await
        .map_err(|e| anyhow::anyhow!("oauth token exchange failed: {e}"))?;
    let access_token = token.access_token().secret();

    let http = reqwest::Client::new();
    let user_json: serde_json::Value = http
        .get(format!("{}/user", state.config.github_api_base))
        .bearer_auth(access_token)
        .header("User-Agent", "monty-server")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let github_id = user_json["id"]
        .as_i64()
        .ok_or_else(|| anyhow::anyhow!("github /user returned no id"))?;
    let github_login = user_json["login"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("github /user returned no login"))?;

    let emails_json: serde_json::Value = http
        .get(format!("{}/user/emails", state.config.github_api_base))
        .bearer_auth(access_token)
        .header("User-Agent", "monty-server")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let email = emails_json
        .as_array()
        .and_then(|arr| {
            arr.iter()
                .find(|e| e["primary"].as_bool() == Some(true))
                .or_else(|| arr.first())
        })
        .and_then(|e| e["email"].as_str())
        .map(|s| s.to_string());

    let user = User::upsert(&state.pool, github_id, github_login, email.as_deref()).await?;
    session.insert(USER_ID_KEY, user.id.to_string()).await?;

    Ok(found("/dashboard"))
}

pub async fn logout(session: Session) -> Response {
    let _ = session.flush().await;
    found("/")
}
