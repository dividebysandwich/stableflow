//! Raw byte / streaming endpoints: full images, thumbnails, and per-job zips.

use std::io::Write;

use axum::extract::{Extension, Path};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};

use crate::server::{db, AppState};

async fn read_file(path: &str) -> Option<Vec<u8>> {
    tokio::fs::read(path).await.ok()
}

/// Inline full-resolution PNG.
pub async fn serve_image(
    Extension(state): Extension<AppState>,
    Path((job_id, idx)): Path<(i64, i64)>,
) -> Response {
    serve(&state, job_id, idx, false, "image/png", None).await
}

/// Inline JPEG thumbnail.
pub async fn serve_thumb(
    Extension(state): Extension<AppState>,
    Path((job_id, idx)): Path<(i64, i64)>,
) -> Response {
    serve(&state, job_id, idx, true, "image/jpeg", None).await
}

/// Force-download a single full image.
pub async fn download_image(
    Extension(state): Extension<AppState>,
    Path((job_id, idx)): Path<(i64, i64)>,
) -> Response {
    let disp = format!("attachment; filename=\"job{job_id}_img{idx}.png\"");
    serve(&state, job_id, idx, false, "image/png", Some(disp)).await
}

async fn serve(
    state: &AppState,
    job_id: i64,
    idx: i64,
    thumb: bool,
    content_type: &str,
    disposition: Option<String>,
) -> Response {
    let path = match db::image_path(&state.pool, job_id, idx, thumb).await {
        Ok(Some(p)) => p,
        _ => return (StatusCode::NOT_FOUND, "not found").into_response(),
    };
    match read_file(&path).await {
        Some(bytes) => {
            let mut resp = ([(header::CONTENT_TYPE, content_type.to_string())], bytes).into_response();
            if let Some(d) = disposition {
                if let Ok(v) = header::HeaderValue::from_str(&d) {
                    resp.headers_mut().insert(header::CONTENT_DISPOSITION, v);
                }
            }
            resp
        }
        None => (StatusCode::NOT_FOUND, "file missing").into_response(),
    }
}

/// Zip of all full-res images for a job.
pub async fn download_job_zip(
    Extension(state): Extension<AppState>,
    Path(job_id): Path<i64>,
) -> Response {
    let files = match db::job_image_files(&state.pool, job_id).await {
        Ok(f) if !f.is_empty() => f,
        Ok(_) => return (StatusCode::NOT_FOUND, "no images for job").into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    let zip_bytes = tokio::task::spawn_blocking(move || build_zip(&files)).await;

    match zip_bytes {
        Ok(Ok(bytes)) => {
            let disp = format!("attachment; filename=\"job{job_id}.zip\"");
            (
                [
                    (header::CONTENT_TYPE, "application/zip".to_string()),
                    (header::CONTENT_DISPOSITION, disp),
                ],
                bytes,
            )
                .into_response()
        }
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

fn build_zip(files: &[(i64, String)]) -> Result<Vec<u8>, String> {
    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut cursor);
        let opts: zip::write::FileOptions<'_, ()> =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (idx, path) in files {
            let data = std::fs::read(path).map_err(|e| format!("read {path}: {e}"))?;
            zip.start_file(format!("{idx}.png"), opts)
                .map_err(|e| e.to_string())?;
            zip.write_all(&data).map_err(|e| e.to_string())?;
        }
        zip.finish().map_err(|e| e.to_string())?;
    }
    Ok(cursor.into_inner())
}
