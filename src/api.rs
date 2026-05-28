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

#[server(name = DeleteJob, prefix = "/api", input = Json)]
pub async fn delete_job(id: i64) -> Result<(), ServerFnError> {
    let st = state();
    let paths = crate::server::db::delete_job(&st.pool, id)
        .await
        .map_err(err)?;
    for p in paths {
        let _ = tokio::fs::remove_file(&p).await;
    }
    let _ = tokio::fs::remove_dir_all(st.gallery_dir(id)).await;
    let _ = tokio::fs::remove_dir_all(st.thumb_dir(id)).await;
    Ok(())
}
