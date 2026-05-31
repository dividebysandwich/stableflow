//! Background queue worker: dispatches one job at a time to Forge, persists
//! results, and tracks status. Restart-resilient (see `AppState` startup reset).

use std::io::Cursor;
use std::path::PathBuf;
use std::time::Duration;

use base64::Engine;
use image::ImageFormat;

use crate::models::{JobParams, JobStatus};
use crate::server::{db, AppState};

const THUMB_MAX: u32 = 512;

/// Worker entry point. Loops forever, processing queued jobs FIFO and parking
/// on `notify` when the queue is empty.
pub async fn run(state: AppState) {
    loop {
        match db::next_queued_job(&state.pool).await {
            Ok(Some((id, params))) => {
                if let Err(e) = process_job(&state, id, &params).await {
                    tracing::error!("job {id} failed: {e}");
                    // Only mark failed if still running, so a concurrent cancel wins.
                    let _ = db::fail_if_running(&state.pool, id, &e).await;
                }
            }
            Ok(None) => {
                // Idle until something is enqueued.
                state.notify.notified().await;
            }
            Err(e) => {
                tracing::error!("worker db error: {e}");
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    }
}

async fn process_job(state: &AppState, id: i64, params: &JobParams) -> Result<(), String> {
    tracing::info!("starting job {id}");
    db::set_status(&state.pool, id, JobStatus::Running, None)
        .await
        .map_err(|e| e.to_string())?;
    let _ = db::set_progress(&state.pool, id, 0.0).await;

    let gallery_dir = state.gallery_dir(id);
    let thumb_dir = state.thumb_dir(id);
    tokio::fs::create_dir_all(&gallery_dir)
        .await
        .map_err(|e| e.to_string())?;
    tokio::fs::create_dir_all(&thumb_dir)
        .await
        .map_err(|e| e.to_string())?;

    // For inpaint, read + base64-encode this turn's init/mask PNGs once. The
    // worker treats mask vs sketch identically — the browser already baked the
    // difference into the init image.
    let inpaint_inputs = match &params.inpaint {
        Some(inp) => {
            let init = tokio::fs::read(&inp.init_path)
                .await
                .map_err(|e| format!("read init {}: {e}", inp.init_path))?;
            let mask = tokio::fs::read(&inp.mask_path)
                .await
                .map_err(|e| format!("read mask {}: {e}", inp.mask_path))?;
            let enc = base64::engine::general_purpose::STANDARD;
            Some((enc.encode(init), enc.encode(mask)))
        }
        None => None,
    };

    // Generate one image per Forge call so each result is persisted (and becomes
    // visible in the UI) the moment it finishes, rather than only at the end of
    // the whole batch. Inpaint turns produce `batch_size` variations (n_iter is
    // a txt2img concept).
    let total = if params.inpaint.is_some() {
        params.batch_size.max(1) as i64
    } else {
        (params.batch_size.max(1) * params.n_iter.max(1)) as i64
    };

    // Append to any existing results, so re-queuing a job adds more images to its
    // gallery rather than replacing them. New indices never reuse an old URL,
    // which also avoids stale browser-cached thumbnails.
    let start_idx = db::next_image_idx(&state.pool, id)
        .await
        .map_err(|e| e.to_string())?;

    for i in 0..total {
        // Stop early if the job was canceled mid-batch.
        if !db::is_running(&state.pool, id)
            .await
            .map_err(|e| e.to_string())?
        {
            break;
        }

        let idx = start_idx + i;
        let mut single = params.clone();
        single.batch_size = 1;
        single.n_iter = 1;
        // Mirror A1111 batch-seed behavior: increment from the base seed across
        // the whole (accumulating) gallery, so fixed-seed re-queues differ.
        single.seed = if params.seed < 0 {
            -1
        } else {
            params.seed + idx
        };

        // Per-run progress = finished images this run + fraction of the current.
        let base = i as f32 / total as f32;
        let span = 1.0 / total as f32;

        let images = match (&params.inpaint, &inpaint_inputs) {
            (Some(inp), Some((init_b64, mask_b64))) => {
                run_with_progress(
                    &state,
                    id,
                    base,
                    span,
                    state.forge.img2img(&single, inp, init_b64, mask_b64),
                )
                .await?
            }
            _ => {
                run_with_progress(&state, id, base, span, state.forge.txt2img(&single)).await?
            }
        };

        // Record the inputs behind inpaint results so the UI can show them.
        let (init_path, mask_path) = match &params.inpaint {
            Some(inp) => (Some(inp.init_path.as_str()), Some(inp.mask_path.as_str())),
            None => (None, None),
        };

        for (b64, seed) in images {
            let full_path = gallery_dir.join(format!("{idx}.png"));
            let thumb_path = thumb_dir.join(format!("{idx}.jpg"));
            let (w, h) = save_image(b64, full_path.clone(), thumb_path.clone())
                .await
                .map_err(|e| format!("image {idx}: {e}"))?;
            db::insert_image(
                &state.pool,
                id,
                idx,
                &full_path.to_string_lossy(),
                &thumb_path.to_string_lossy(),
                seed,
                w as i64,
                h as i64,
                init_path,
                mask_path,
            )
            .await
            .map_err(|e| e.to_string())?;
        }

        let _ = db::set_progress(&state.pool, id, (i + 1) as f32 / total as f32).await;
    }

    // Respect a cancel that may have landed while we were generating.
    let completed = db::complete_if_running(&state.pool, id)
        .await
        .map_err(|e| e.to_string())?;
    if completed {
        tracing::info!("completed job {id}");
    } else {
        tracing::info!("job {id} was canceled during generation");
    }
    Ok(())
}

/// Drive a single Forge generation future to completion while polling
/// `/progress` every 800ms and writing a per-run progress fraction
/// (`base + span * p`). Shared by the txt2img and img2img paths.
async fn run_with_progress<F>(
    state: &AppState,
    id: i64,
    base: f32,
    span: f32,
    gen: F,
) -> Result<Vec<(String, i64)>, String>
where
    F: std::future::Future<Output = Result<Vec<(String, i64)>, String>>,
{
    tokio::pin!(gen);
    loop {
        tokio::select! {
            res = &mut gen => break res,
            _ = tokio::time::sleep(Duration::from_millis(800)) => {
                let p = state.forge.progress().await;
                let _ = db::set_progress(&state.pool, id, base + span * p.progress as f32).await;
            }
        }
    }
}

/// Decode a base64 PNG, write the full-res file, and a JPEG thumbnail.
/// CPU-bound work runs on a blocking thread. Returns (width, height).
async fn save_image(
    b64: String,
    full_path: PathBuf,
    thumb_path: PathBuf,
) -> Result<(u32, u32), String> {
    tokio::task::spawn_blocking(move || {
        // Forge may or may not include a data-URL prefix.
        let payload = b64.split(',').next_back().unwrap_or(&b64);
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(payload.trim())
            .map_err(|e| format!("base64: {e}"))?;

        std::fs::write(&full_path, &bytes).map_err(|e| format!("write full: {e}"))?;

        let img = image::load_from_memory(&bytes).map_err(|e| format!("load: {e}"))?;
        let (w, h) = (img.width(), img.height());
        let thumb = img.thumbnail(THUMB_MAX, THUMB_MAX);
        let mut buf = Cursor::new(Vec::new());
        thumb
            .to_rgb8()
            .write_to(&mut buf, ImageFormat::Jpeg)
            .map_err(|e| format!("thumb encode: {e}"))?;
        std::fs::write(&thumb_path, buf.into_inner()).map_err(|e| format!("write thumb: {e}"))?;

        Ok((w, h))
    })
    .await
    .map_err(|e| format!("join: {e}"))?
}
