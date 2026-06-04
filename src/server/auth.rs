//! Per-user login backed by a persistent session table + cookie, plus a
//! middleware that gates every route except the login page and static assets.
//! On an empty system the first login bootstraps the admin account.

use axum::extract::{Extension, Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use axum::Form;
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use serde::Deserialize;

use crate::server::{db, now_unix, AppState};

pub const COOKIE_NAME: &str = "stableflow_session";
const SESSION_TTL_SECS: i64 = 60 * 60 * 24 * 30; // 30 days

/// Hash a plaintext password for storage (bcrypt, default cost).
pub fn hash_password(plain: &str) -> Result<String, String> {
    bcrypt::hash(plain, bcrypt::DEFAULT_COST).map_err(|e| e.to_string())
}

/// Verify a plaintext password against a stored bcrypt hash.
pub fn verify_password(plain: &str, hash: &str) -> bool {
    bcrypt::verify(plain, hash).unwrap_or(false)
}

#[derive(Deserialize)]
pub struct LoginForm {
    pub username: String,
    pub password: String,
}

/// Set the session cookie for `user_id` and redirect home, or bounce back to the
/// login page with an error if the session row can't be written.
async fn establish_session(state: &AppState, jar: CookieJar, user_id: i64) -> Response {
    let token = uuid::Uuid::new_v4().to_string();
    let now = now_unix();
    if db::create_session(&state.pool, &token, user_id, now, now + SESSION_TTL_SECS)
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

pub async fn login(
    Extension(state): Extension<AppState>,
    jar: CookieJar,
    Form(form): Form<LoginForm>,
) -> Response {
    let username = form.username.trim();
    if username.is_empty() || form.password.is_empty() {
        return Redirect::to("/login?error=1").into_response();
    }

    // Empty system: this first login creates the admin account and claims any
    // pre-existing (ownerless) jobs.
    match db::count_users(&state.pool).await {
        Ok(0) => {
            let hash = match hash_password(&form.password) {
                Ok(h) => h,
                Err(_) => return Redirect::to("/login?error=1").into_response(),
            };
            let uuid = uuid::Uuid::new_v4().to_string();
            let id = match db::create_user(&state.pool, username, &hash, &uuid, true).await {
                Ok(id) => id,
                Err(_) => return Redirect::to("/login?error=1").into_response(),
            };
            let _ = db::claim_orphan_jobs(&state.pool, id).await;
            return establish_session(&state, jar, id).await;
        }
        Ok(_) => {}
        Err(_) => return Redirect::to("/login?error=1").into_response(),
    }

    // Normal login: verify against the stored bcrypt hash.
    match db::get_user_by_username(&state.pool, username).await {
        Ok(Some((id, hash, _is_admin))) if verify_password(&form.password, &hash) => {
            establish_session(&state, jar, id).await
        }
        _ => Redirect::to("/login?error=1").into_response(),
    }
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
        || path == "/api/auth_status"
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
        Some(c) => db::session_valid(&state.pool, c.value(), now_unix())
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
