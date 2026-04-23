pub mod local;
pub mod middleware;
pub mod oauth;
pub mod session;

use axum::Router;
use axum::routing::{get, post};

use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/auth/login", get(local::login_form).post(local::login_submit))
        .route("/auth/signup", get(local::signup_form).post(local::signup_submit))
        .route("/auth/login-github", get(oauth::login))
        .route("/auth/callback", get(oauth::callback))
        .route("/auth/logout", post(oauth::logout))
}
