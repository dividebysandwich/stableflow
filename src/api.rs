//! Leptos server functions — the typed RPC surface the UI calls. Bodies run on
//! the server only; signatures/types are shared with the WASM client.

use leptos::prelude::*;
use leptos::server_fn::codec::Json;

use crate::models::{FormOptions, ImageMeta, Job, JobParams, RunningProgress};

#[cfg(feature = "ssr")]
fn state() -> crate::server::AppState {
    expect_context::<crate::server::AppState>()
}

#[cfg(feature = "ssr")]
fn err<E: std::fmt::Display>(e: E) -> ServerFnError {
    ServerFnError::new(e.to_string())
}

#[server(name = ListJobs, prefix = "/api", input = Json)]
pub async fn list_jobs() -> Result<Vec<Job>, ServerFnError> {
    crate::server::db::list_jobs(&state().pool)
        .await
        .map_err(err)
}

#[server(name = GetJob, prefix = "/api", input = Json)]
pub async fn get_job(id: i64) -> Result<Option<Job>, ServerFnError> {
    crate::server::db::get_job(&state().pool, id)
        .await
        .map_err(err)
}

#[server(name = GetJobImages, prefix = "/api", input = Json)]
pub async fn get_job_images(id: i64) -> Result<Vec<ImageMeta>, ServerFnError> {
    crate::server::db::get_job_images(&state().pool, id)
        .await
        .map_err(err)
}

#[server(name = ListFavorites, prefix = "/api", input = Json)]
pub async fn list_favorites() -> Result<Vec<ImageMeta>, ServerFnError> {
    crate::server::db::list_favorites(&state().pool)
        .await
        .map_err(err)
}

#[server(name = SetStar, prefix = "/api", input = Json)]
pub async fn set_star(job_id: i64, idx: i64, starred: bool) -> Result<(), ServerFnError> {
    crate::server::db::set_image_star(&state().pool, job_id, idx, starred)
        .await
        .map_err(err)
}

#[server(name = GetJobParams, prefix = "/api", input = Json)]
pub async fn get_job_params(id: i64) -> Result<Option<JobParams>, ServerFnError> {
    crate::server::db::get_job_params(&state().pool, id)
        .await
        .map_err(err)
}

#[server(name = GetFormOptions, prefix = "/api", input = Json)]
pub async fn get_form_options() -> Result<FormOptions, ServerFnError> {
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
    let st = state();
    let fresh = st.forge.fetch_form_options().await;
    *st.options.write().await = fresh.clone();
    Ok(fresh)
}

#[server(name = GetRunningProgress, prefix = "/api", input = Json)]
pub async fn get_running_progress() -> Result<RunningProgress, ServerFnError> {
    crate::server::db::running_progress(&state().pool)
        .await
        .map_err(err)
}

#[server(name = CreateJob, prefix = "/api", input = Json)]
pub async fn create_job(name: String, params: JobParams) -> Result<i64, ServerFnError> {
    let st = state();
    let id = crate::server::db::insert_job(&st.pool, &name, &params)
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
    let st = state();
    let id = db::insert_job_parked(&st.pool, &name, &params)
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
    let st = state();
    if let Some(job) = crate::server::db::get_job(&st.pool, id).await.map_err(err)? {
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
    let st = state();
    crate::server::db::requeue_job(&st.pool, id)
        .await
        .map_err(err)?;
    st.notify.notify_one();
    Ok(())
}

#[server(name = DeleteImage, prefix = "/api", input = Json)]
pub async fn delete_image(job_id: i64, idx: i64) -> Result<(), ServerFnError> {
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
