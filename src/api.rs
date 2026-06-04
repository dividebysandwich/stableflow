//! Leptos server functions — the typed RPC surface the UI calls. Bodies run on
//! the server only; signatures/types are shared with the WASM client.

use leptos::prelude::*;
use leptos::server_fn::codec::Json;

use crate::models::{
    AuthStatus, CurrentUser, FormOptions, ImageMeta, Job, JobParams, RunningProgress, UserInfo,
};

#[cfg(feature = "ssr")]
fn state() -> crate::server::AppState {
    expect_context::<crate::server::AppState>()
}

#[cfg(feature = "ssr")]
fn err<E: std::fmt::Display>(e: E) -> ServerFnError {
    ServerFnError::new(e.to_string())
}

/// Resolve the authenticated user from the request's session cookie, or error
/// with "unauthorized". The cookie is pulled from the live request via
/// `leptos_axum::extract`.
#[cfg(feature = "ssr")]
async fn require_user() -> Result<crate::server::db::UserCtx, ServerFnError> {
    use axum_extra::extract::cookie::CookieJar;
    let jar: CookieJar = leptos_axum::extract().await.map_err(err)?;
    let token = jar
        .get(crate::server::auth::COOKIE_NAME)
        .map(|c| c.value().to_string())
        .ok_or_else(|| err("unauthorized"))?;
    crate::server::db::session_user(&state().pool, &token, crate::server::now_unix())
        .await
        .map_err(err)?
        .ok_or_else(|| err("unauthorized"))
}

/// Like [`require_user`] but additionally requires the admin flag.
#[cfg(feature = "ssr")]
async fn require_admin() -> Result<crate::server::db::UserCtx, ServerFnError> {
    let u = require_user().await?;
    if u.is_admin {
        Ok(u)
    } else {
        Err(err("forbidden"))
    }
}

/// Authorize access to a specific job: the caller must own it (or be an admin).
/// Returns the resolved user on success.
#[cfg(feature = "ssr")]
async fn require_job_access(job_id: i64) -> Result<crate::server::db::UserCtx, ServerFnError> {
    let u = require_user().await?;
    match crate::server::db::job_owner(&state().pool, job_id)
        .await
        .map_err(err)?
    {
        Some((owner_id, _)) if owner_id == u.id || u.is_admin => Ok(u),
        _ => Err(err("forbidden")),
    }
}

#[server(name = ListJobs, prefix = "/api", input = Json)]
pub async fn list_jobs() -> Result<Vec<Job>, ServerFnError> {
    let u = require_user().await?;
    crate::server::db::list_jobs(&state().pool, u.id, u.is_admin)
        .await
        .map_err(err)
}

#[server(name = GetJob, prefix = "/api", input = Json)]
pub async fn get_job(id: i64) -> Result<Option<Job>, ServerFnError> {
    let u = require_user().await?;
    crate::server::db::get_job(&state().pool, id, u.id, u.is_admin)
        .await
        .map_err(err)
}

#[server(name = GetJobImages, prefix = "/api", input = Json)]
pub async fn get_job_images(id: i64) -> Result<Vec<ImageMeta>, ServerFnError> {
    require_job_access(id).await?;
    crate::server::db::get_job_images(&state().pool, id)
        .await
        .map_err(err)
}

#[server(name = ListFavorites, prefix = "/api", input = Json)]
pub async fn list_favorites() -> Result<Vec<ImageMeta>, ServerFnError> {
    let u = require_user().await?;
    crate::server::db::list_favorites(&state().pool, u.id, u.is_admin)
        .await
        .map_err(err)
}

#[server(name = SetStar, prefix = "/api", input = Json)]
pub async fn set_star(job_id: i64, idx: i64, starred: bool) -> Result<(), ServerFnError> {
    require_job_access(job_id).await?;
    crate::server::db::set_image_star(&state().pool, job_id, idx, starred)
        .await
        .map_err(err)
}

#[server(name = GetJobParams, prefix = "/api", input = Json)]
pub async fn get_job_params(id: i64) -> Result<Option<JobParams>, ServerFnError> {
    require_job_access(id).await?;
    crate::server::db::get_job_params(&state().pool, id)
        .await
        .map_err(err)
}

#[server(name = GetFormOptions, prefix = "/api", input = Json)]
pub async fn get_form_options() -> Result<FormOptions, ServerFnError> {
    require_user().await?;
    let st = state();
    // Serve cached options; refetch lazily if empty (e.g. Forge was down at boot).
    {
        let cached = st.options.read().await;
        if !cached.checkpoints.is_empty() || !cached.samplers.is_empty() {
            return Ok(cached.clone());
        }
    }
    let fresh = st.forge.fetch_form_options().await;
    *st.options.write().await = fresh.clone();
    Ok(fresh)
}

#[server(name = RefreshFormOptions, prefix = "/api", input = Json)]
pub async fn refresh_form_options() -> Result<FormOptions, ServerFnError> {
    require_user().await?;
    let st = state();
    let fresh = st.forge.fetch_form_options().await;
    *st.options.write().await = fresh.clone();
    Ok(fresh)
}

#[server(name = GetRunningProgress, prefix = "/api", input = Json)]
pub async fn get_running_progress() -> Result<RunningProgress, ServerFnError> {
    let u = require_user().await?;
    crate::server::db::running_progress(&state().pool, u.id)
        .await
        .map_err(err)
}

#[server(name = CreateJob, prefix = "/api", input = Json)]
pub async fn create_job(name: String, params: JobParams) -> Result<i64, ServerFnError> {
    let u = require_user().await?;
    let st = state();
    let id = crate::server::db::insert_job(&st.pool, u.id, &name, &params)
        .await
        .map_err(err)?;
    st.notify.notify_one();
    Ok(id)
}

/// Decode the two base64 PNGs and write them into `dir` named by `start_idx`
/// (so each turn's inputs are retained). Returns their absolute path strings.
#[cfg(feature = "ssr")]
async fn write_inputs(
    dir: &std::path::Path,
    start_idx: i64,
    init_b64: &str,
    mask_b64: &str,
) -> Result<(String, String), String> {
    use base64::Engine;
    let dec = |s: &str| {
        // Tolerate an optional "data:image/png;base64," prefix.
        let p = s.split(',').next_back().unwrap_or(s).trim();
        base64::engine::general_purpose::STANDARD
            .decode(p)
            .map_err(|e| format!("base64: {e}"))
    };
    let init = dec(init_b64)?;
    let mask = dec(mask_b64)?;
    let init_path = dir.join(format!("{start_idx}_init.png"));
    let mask_path = dir.join(format!("{start_idx}_mask.png"));
    tokio::fs::write(&init_path, &init)
        .await
        .map_err(|e| format!("write init: {e}"))?;
    tokio::fs::write(&mask_path, &mask)
        .await
        .map_err(|e| format!("write mask: {e}"))?;
    Ok((
        init_path.to_string_lossy().into_owned(),
        mask_path.to_string_lossy().into_owned(),
    ))
}

/// Start a new inpaint job from the editor's first "Generate". `params.inpaint`
/// is `Some` with empty paths; we insert the job parked, write the turn-0 input
/// PNGs, backfill their paths, then queue it. Returns the new job id.
#[server(name = CreateInpaintJob, prefix = "/api", input = Json)]
pub async fn create_inpaint_job(
    name: String,
    params: JobParams,
    init_png_b64: String,
    mask_png_b64: String,
) -> Result<i64, ServerFnError> {
    use crate::server::db;
    // Server-fn params can't be declared `mut`; rebind to mutate locally.
    let mut params = params;
    if params.inpaint.is_none() {
        return Err(err("create_inpaint_job called without inpaint params"));
    }
    let u = require_user().await?;
    let st = state();
    let id = db::insert_job_parked(&st.pool, u.id, &name, &params)
        .await
        .map_err(err)?;
    let dir = st.input_dir(id);
    tokio::fs::create_dir_all(&dir).await.map_err(err)?;
    let (init_path, mask_path) = write_inputs(&dir, 0, &init_png_b64, &mask_png_b64)
        .await
        .map_err(err)?;
    if let Some(inp) = params.inpaint.as_mut() {
        inp.init_path = init_path;
        inp.mask_path = mask_path;
    }
    db::set_job_params(&st.pool, id, &params).await.map_err(err)?;
    // Flip draft -> queued and wake the worker, now that inputs exist.
    db::requeue_job(&st.pool, id).await.map_err(err)?;
    st.notify.notify_one();
    Ok(id)
}

/// Run another turn on an existing inpaint job: write the new turn's input PNGs,
/// record the updated params, and re-queue (appends one batch of results).
#[server(name = RunInpaintTurn, prefix = "/api", input = Json)]
pub async fn run_inpaint_turn(
    id: i64,
    params: JobParams,
    init_png_b64: String,
    mask_png_b64: String,
) -> Result<(), ServerFnError> {
    use crate::server::db;
    let mut params = params;
    if params.inpaint.is_none() {
        return Err(err("run_inpaint_turn called without inpaint params"));
    }
    require_job_access(id).await?;
    let st = state();
    let start_idx = db::next_image_idx(&st.pool, id).await.map_err(err)?;
    let dir = st.input_dir(id);
    tokio::fs::create_dir_all(&dir).await.map_err(err)?;
    let (init_path, mask_path) = write_inputs(&dir, start_idx, &init_png_b64, &mask_png_b64)
        .await
        .map_err(err)?;
    if let Some(inp) = params.inpaint.as_mut() {
        inp.init_path = init_path;
        inp.mask_path = mask_path;
    }
    db::set_job_params(&st.pool, id, &params).await.map_err(err)?;
    db::requeue_job(&st.pool, id).await.map_err(err)?;
    st.notify.notify_one();
    Ok(())
}

#[server(name = CancelJob, prefix = "/api", input = Json)]
pub async fn cancel_job(id: i64) -> Result<(), ServerFnError> {
    use crate::models::JobStatus;
    require_job_access(id).await?;
    let st = state();
    if let Some(job) = crate::server::db::get_job(&st.pool, id, 0, true)
        .await
        .map_err(err)?
    {
        match job.status {
            JobStatus::Running => {
                st.forge.interrupt().await;
                crate::server::db::set_status(&st.pool, id, JobStatus::Canceled, None)
                    .await
                    .map_err(err)?;
            }
            JobStatus::Queued => {
                crate::server::db::set_status(&st.pool, id, JobStatus::Canceled, None)
                    .await
                    .map_err(err)?;
            }
            _ => {}
        }
    }
    Ok(())
}

#[server(name = RequeueJob, prefix = "/api", input = Json)]
pub async fn requeue_job(id: i64) -> Result<(), ServerFnError> {
    require_job_access(id).await?;
    let st = state();
    crate::server::db::requeue_job(&st.pool, id)
        .await
        .map_err(err)?;
    st.notify.notify_one();
    Ok(())
}

#[server(name = DeleteImage, prefix = "/api", input = Json)]
pub async fn delete_image(job_id: i64, idx: i64) -> Result<(), ServerFnError> {
    require_job_access(job_id).await?;
    let st = state();
    let paths = crate::server::db::delete_image(&st.pool, job_id, idx)
        .await
        .map_err(err)?;
    for p in paths {
        let _ = tokio::fs::remove_file(&p).await;
    }
    Ok(())
}

#[server(name = DeleteImages, prefix = "/api", input = Json)]
pub async fn delete_images(job_id: i64, idxs: Vec<i64>) -> Result<(), ServerFnError> {
    require_job_access(job_id).await?;
    let st = state();
    for idx in idxs {
        let paths = crate::server::db::delete_image(&st.pool, job_id, idx)
            .await
            .map_err(err)?;
        for p in paths {
            let _ = tokio::fs::remove_file(&p).await;
        }
    }
    Ok(())
}

#[server(name = DeleteJob, prefix = "/api", input = Json)]
pub async fn delete_job(id: i64) -> Result<(), ServerFnError> {
    require_job_access(id).await?;
    let st = state();
    // Refuse while any image in the job is starred — favorites must be
    // un-starred first, so a whole-job delete can't take them down with it.
    if crate::server::db::job_starred_count(&st.pool, id).await.map_err(err)? > 0 {
        return Err(err(
            "This job has starred images. Un-star them before deleting the job.",
        ));
    }
    let paths = crate::server::db::delete_job(&st.pool, id)
        .await
        .map_err(err)?;
    for p in paths {
        let _ = tokio::fs::remove_file(&p).await;
    }
    let _ = tokio::fs::remove_dir_all(st.gallery_dir(id)).await;
    let _ = tokio::fs::remove_dir_all(st.thumb_dir(id)).await;
    let _ = tokio::fs::remove_dir_all(st.input_dir(id)).await;
    Ok(())
}

// ---- Auth / users ----

/// The currently-authenticated user, for the UI (navbar admin link, building
/// per-user file URLs for the user's own source images).
#[server(name = CurrentUserFn, prefix = "/api", input = Json)]
pub async fn current_user() -> Result<CurrentUser, ServerFnError> {
    let u = require_user().await?;
    Ok(CurrentUser {
        username: u.username,
        is_admin: u.is_admin,
        uuid: u.uuid,
    })
}

/// Whether the system needs first-time setup (no users yet). **Public** — the
/// login page calls this before any session exists, so its path
/// (`/api/auth_status`) is allow-listed in the auth middleware.
#[server(name = AuthStatusFn, prefix = "/api", endpoint = "auth_status", input = Json)]
pub async fn auth_status() -> Result<AuthStatus, ServerFnError> {
    let n = crate::server::db::count_users(&state().pool)
        .await
        .map_err(err)?;
    Ok(AuthStatus { needs_setup: n == 0 })
}

#[server(name = ListUsersFn, prefix = "/api", input = Json)]
pub async fn list_users() -> Result<Vec<UserInfo>, ServerFnError> {
    require_admin().await?;
    crate::server::db::list_users(&state().pool)
        .await
        .map_err(err)
}

#[server(name = AdminCreateUser, prefix = "/api", input = Json)]
pub async fn admin_create_user(
    username: String,
    password: String,
    is_admin: bool,
) -> Result<(), ServerFnError> {
    require_admin().await?;
    let username = username.trim().to_string();
    if username.is_empty() || password.is_empty() {
        return Err(err("username and password are required"));
    }
    let hash = crate::server::auth::hash_password(&password).map_err(err)?;
    let uuid = uuid::Uuid::new_v4().to_string();
    crate::server::db::create_user(&state().pool, &username, &hash, &uuid, is_admin)
        .await
        .map_err(|e| {
            // Surface the common case (duplicate username) clearly.
            if e.to_string().contains("UNIQUE") {
                err("a user with that name already exists")
            } else {
                err(e)
            }
        })?;
    Ok(())
}

#[server(name = AdminSetPassword, prefix = "/api", input = Json)]
pub async fn admin_set_password(id: i64, password: String) -> Result<(), ServerFnError> {
    require_admin().await?;
    if password.is_empty() {
        return Err(err("password is required"));
    }
    let hash = crate::server::auth::hash_password(&password).map_err(err)?;
    crate::server::db::set_user_password(&state().pool, id, &hash)
        .await
        .map_err(err)
}

#[server(name = AdminDeleteUser, prefix = "/api", input = Json)]
pub async fn admin_delete_user(id: i64) -> Result<(), ServerFnError> {
    let me = require_admin().await?;
    if me.id == id {
        return Err(err("you cannot delete your own account"));
    }
    let st = state();
    // Never remove the last admin — that would lock the system's management out.
    if crate::server::db::is_user_admin(&st.pool, id).await.map_err(err)?
        && crate::server::db::count_admins(&st.pool).await.map_err(err)? <= 1
    {
        return Err(err("cannot delete the last remaining admin"));
    }
    let job_ids = crate::server::db::user_job_ids(&st.pool, id)
        .await
        .map_err(err)?;
    let paths = crate::server::db::delete_user(&st.pool, id)
        .await
        .map_err(err)?;
    for p in paths {
        let _ = tokio::fs::remove_file(&p).await;
    }
    for jid in job_ids {
        let _ = tokio::fs::remove_dir_all(st.gallery_dir(jid)).await;
        let _ = tokio::fs::remove_dir_all(st.thumb_dir(jid)).await;
        let _ = tokio::fs::remove_dir_all(st.input_dir(jid)).await;
    }
    Ok(())
}
