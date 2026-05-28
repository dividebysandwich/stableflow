//! Shared-password login backed by a persistent session table + cookie, plus a
//! middleware that gates every route except the login page and static assets.

use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{Extension, Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use axum::Form;
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use serde::Deserialize;

use crate::server::{db, AppState};

pub const COOKIE_NAME: &str = "stableflow_session";
const SESSION_TTL_SECS: i64 = 60 * 60 * 24 * 30; // 30 days

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[derive(Deserialize)]
pub struct LoginForm {
    pub password: String,
}

pub async fn login(
    Extension(state): Extension<AppState>,
    jar: CookieJar,
    Form(form): Form<LoginForm>,
) -> Response {
    if form.password.is_empty() || form.password != state.password {
        return Redirect::to("/login?error=1").into_response();
    }
    let token = uuid::Uuid::new_v4().to_string();
    let now = now();
    if db::create_session(&state.pool, &token, now, now + SESSION_TTL_SECS)
        .await
        .is_err()
    {
        return Redirect::to("/login?error=1").into_response();
    }
    let mut cookie = Cookie::new(COOKIE_NAME, token);
    cookie.set_http_only(true);
    cookie.set_path("/");
    cookie.set_same_site(SameSite::Lax);
    cookie.set_max_age(time::Duration::seconds(SESSION_TTL_SECS));
    (jar.add(cookie), Redirect::to("/")).into_response()
}

pub async fn logout(Extension(state): Extension<AppState>, jar: CookieJar) -> Response {
    if let Some(c) = jar.get(COOKIE_NAME) {
        let _ = db::delete_session(&state.pool, c.value()).await;
    }
    let jar = jar.remove(Cookie::from(COOKIE_NAME));
    (jar, Redirect::to("/login")).into_response()
}

fn is_public(path: &str) -> bool {
    path == "/login"
        || path == "/favicon.ico"
        || path.starts_with("/pkg/")
        || path.starts_with("/assets/")
}

/// Gate middleware. Unauthenticated requests to protected paths redirect to the
/// login page (or get 401 for API calls).
pub async fn require_auth(
    State(state): State<AppState>,
    jar: CookieJar,
    req: Request,
    next: Next,
) -> Response {
    if is_public(req.uri().path()) {
        return next.run(req).await;
    }

    let authed = match jar.get(COOKIE_NAME) {
        Some(c) => db::session_valid(&state.pool, c.value(), now())
            .await
            .unwrap_or(false),
        None => false,
    };

    if authed {
        next.run(req).await
    } else if req.uri().path().starts_with("/api/") {
        (axum::http::StatusCode::UNAUTHORIZED, "unauthorized").into_response()
    } else {
        Redirect::to("/login").into_response()
    }
}
