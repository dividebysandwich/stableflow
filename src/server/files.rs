//! Raw byte / streaming endpoints: full images, thumbnails, and per-job zips.
//! Every URL is scoped by the owner's per-user UUID (`/u/{uuid}/...`) and gated:
//! the request must carry a valid session whose user owns the job (or is an
//! admin), and the path UUID must match the job owner's UUID.

use std::io::Write;

use axum::extract::{Extension, Path};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum_extra::extract::cookie::CookieJar;

use crate::server::{auth, db, now_unix, AppState};

async fn read_file(path: &str) -> Option<Vec<u8>> {
    tokio::fs::read(path).await.ok()
}

/// Authorize a file request for `job_id` under `path_uuid`. Returns `Ok` only
/// when the session user owns the job (or is an admin) AND the path UUID matches
/// the job owner's UUID. Any failure is reported as a bare 404 so the endpoint
/// never confirms the existence of another user's job.
async fn authorize(state: &AppState, jar: &CookieJar, path_uuid: &str, job_id: i64) -> bool {
    let user = match jar.get(auth::COOKIE_NAME) {
        Some(c) => db::session_user(&state.pool, c.value(), now_unix())
            .await
            .ok()
            .flatten(),
        None => None,
    };
    let user = match user {
        Some(u) => u,
        None => return false,
    };
    match db::job_owner(&state.pool, job_id).await {
        Ok(Some((owner_id, owner_uuid))) => {
            owner_uuid == path_uuid && (user.id == owner_id || user.is_admin)
        }
        _ => false,
    }
}

fn not_found() -> Response {
    (StatusCode::NOT_FOUND, "not found").into_response()
}

/// Inline full-resolution PNG.
pub async fn serve_image(
    Extension(state): Extension<AppState>,
    jar: CookieJar,
    Path((uuid, job_id, idx)): Path<(String, i64, i64)>,
) -> Response {
    if !authorize(&state, &jar, &uuid, job_id).await {
        return not_found();
    }
    serve(&state, job_id, idx, false, "image/png", None).await
}

/// Inline JPEG thumbnail.
pub async fn serve_thumb(
    Extension(state): Extension<AppState>,
    jar: CookieJar,
    Path((uuid, job_id, idx)): Path<(String, i64, i64)>,
) -> Response {
    if !authorize(&state, &jar, &uuid, job_id).await {
        return not_found();
    }
    serve(&state, job_id, idx, true, "image/jpeg", None).await
}

/// Inline the recorded init/mask PNG behind an inpaint result. `kind` is
/// "init" or "mask"; anything else is treated as "init".
pub async fn serve_input(
    Extension(state): Extension<AppState>,
    jar: CookieJar,
    Path((uuid, job_id, idx, kind)): Path<(String, i64, i64, String)>,
) -> Response {
    if !authorize(&state, &jar, &uuid, job_id).await {
        return not_found();
    }
    let mask = kind == "mask";
    let path = match db::image_input_path(&state.pool, job_id, idx, mask).await {
        Ok(Some(p)) => p,
        _ => return not_found(),
    };
    match read_file(&path).await {
        Some(bytes) => {
            ([(header::CONTENT_TYPE, "image/png".to_string())], bytes).into_response()
        }
        None => (StatusCode::NOT_FOUND, "file missing").into_response(),
    }
}

/// Force-download a single full image.
pub async fn download_image(
    Extension(state): Extension<AppState>,
    jar: CookieJar,
    Path((uuid, job_id, idx)): Path<(String, i64, i64)>,
) -> Response {
    if !authorize(&state, &jar, &uuid, job_id).await {
        return not_found();
    }
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
    jar: CookieJar,
    Path((uuid, job_id)): Path<(String, i64)>,
) -> Response {
    if !authorize(&state, &jar, &uuid, job_id).await {
        return not_found();
    }
    let files = match db::job_image_files(&state.pool, job_id).await {
        Ok(f) if !f.is_empty() => f,
        Ok(_) => return (StatusCode::NOT_FOUND, "no images for job").into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    let zip_bytes = tokio::task::spawn_blocking(move || build_zip(job_id, &files)).await;

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

fn build_zip(job_id: i64, files: &[(i64, i64, String)]) -> Result<Vec<u8>, String> {
    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut cursor);
        let base_opts: zip::write::FileOptions<'_, ()> =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (idx, seed, path) in files {
            let meta = std::fs::metadata(path).map_err(|e| format!("stat {path}: {e}"))?;
            let data = std::fs::read(path).map_err(|e| format!("read {path}: {e}"))?;
            // Naming scheme: jobnumber-seed-sequencenumber.ext
            let ext = std::path::Path::new(path)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("png");
            // Stamp the zip entry with the source file's creation date (fall back to mtime).
            let mut opts = base_opts;
            let created = meta.created().or_else(|_| meta.modified());
            if let Some(dt) = created.ok().and_then(system_time_to_zip) {
                opts = opts.last_modified_time(dt);
            }
            zip.start_file(format!("{job_id}-{seed}-{idx}.{ext}"), opts)
                .map_err(|e| e.to_string())?;
            zip.write_all(&data).map_err(|e| e.to_string())?;
        }
        zip.finish().map_err(|e| e.to_string())?;
    }
    Ok(cursor.into_inner())
}

/// Convert a filesystem timestamp into a zip `DateTime` (UTC). Returns `None`
/// if the time predates the zip epoch (1980) or otherwise can't be represented.
fn system_time_to_zip(t: std::time::SystemTime) -> Option<zip::DateTime> {
    let odt = time::OffsetDateTime::from(t).to_offset(time::UtcOffset::UTC);
    zip::DateTime::from_date_and_time(
        odt.year() as u16,
        u8::from(odt.month()),
        odt.day(),
        odt.hour(),
        odt.minute(),
        odt.second(),
    )
    .ok()
}
