use askama::Template;
use askama_axum::IntoResponse;
use axum::extract::{Form, State};
use axum::http::StatusCode;
use axum::response::Response;
use serde::Deserialize;
use tower_sessions::Session;

use crate::AppState;
use crate::auth::session::USER_ID_KEY;
use crate::error::AppResult;
use crate::models::User;

#[derive(Template)]
#[template(path = "signup.html")]
struct SignupTemplate {
    error: Option<String>,
}

#[derive(Template)]
#[template(path = "login.html")]
struct LoginTemplate {
    error: Option<String>,
    info: Option<String>,
    github_enabled: bool,
}

pub async fn signup_form() -> Response {
    SignupTemplate { error: None }.into_response()
}

#[derive(Deserialize)]
pub struct SignupForm {
    pub email: String,
    pub password: String,
}

pub async fn signup_submit(
    State(state): State<AppState>,
    Form(form): Form<SignupForm>,
) -> AppResult<Response> {
    if let Err(msg) = validate_signup(&form) {
        return Ok(SignupTemplate {
            error: Some(msg.into()),
        }
        .into_response());
    }
    match User::create_local(&state.pool, form.email.trim(), &form.password).await {
        Ok(_) => {
            let info = "Account created. An administrator must approve it before you can sign in.";
            Ok(LoginTemplate {
                error: None,
                info: Some(info.into()),
                github_enabled: !state.config.github_client_id.is_empty(),
            }
            .into_response())
        }
        Err(e) => {
            let msg = if e.to_string().to_lowercase().contains("unique") {
                "An account with that email already exists.".into()
            } else {
                format!("Could not create account: {e}")
            };
            Ok(SignupTemplate { error: Some(msg) }.into_response())
        }
    }
}

pub async fn login_form(State(state): State<AppState>) -> Response {
    LoginTemplate {
        error: None,
        info: None,
        github_enabled: !state.config.github_client_id.is_empty(),
    }
    .into_response()
}

#[derive(Deserialize)]
pub struct LoginForm {
    pub email: String,
    pub password: String,
}

pub async fn login_submit(
    State(state): State<AppState>,
    session: Session,
    Form(form): Form<LoginForm>,
) -> AppResult<Response> {
    let user = User::verify_local_login(&state.pool, form.email.trim(), &form.password).await?;
    let Some(user) = user else {
        return Ok(LoginTemplate {
            error: Some("Invalid email or password.".into()),
            info: None,
            github_enabled: !state.config.github_client_id.is_empty(),
        }
        .into_response());
    };
    session.insert(USER_ID_KEY, user.id.to_string()).await?;
    Ok(redirect("/dashboard"))
}

fn redirect(location: &str) -> Response {
    axum::response::Response::builder()
        .status(StatusCode::FOUND)
        .header("location", location)
        .body(axum::body::Body::empty())
        .unwrap()
}

fn validate_signup(form: &SignupForm) -> Result<(), &'static str> {
    let email = form.email.trim();
    if !email.contains('@') || !email.contains('.') || email.len() < 3 {
        return Err("Please enter a valid email address.");
    }
    if form.password.len() < 8 {
        return Err("Password must be at least 8 characters.");
    }
    Ok(())
}
