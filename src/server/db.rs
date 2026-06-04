//! SQLite persistence: schema, job/image CRUD, and login sessions.

use sqlx::sqlite::{SqlitePool, SqlitePoolOptions, SqliteRow};
use sqlx::Row;

use crate::models::{ImageMeta, Job, JobParams, JobStatus, RunningProgress, UserInfo};

/// Server-side identity resolved from a session token (joined sessions→users).
#[derive(Clone, Debug)]
pub struct UserCtx {
    pub id: i64,
    pub username: String,
    pub is_admin: bool,
    pub uuid: String,
}

pub async fn init_pool(db_path: &str) -> Result<SqlitePool, sqlx::Error> {
    let url = format!("sqlite:{db_path}?mode=rwc");
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect(&url)
        .await?;
    // WAL improves concurrent reads (UI) vs. the single writer (worker).
    sqlx::query("PRAGMA journal_mode=WAL;")
        .execute(&pool)
        .await?;
    migrate(&pool).await?;
    Ok(pool)
}

async fn migrate(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS jobs (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            name        TEXT NOT NULL DEFAULT '',
            status      TEXT NOT NULL,
            error       TEXT,
            progress    REAL NOT NULL DEFAULT 0,
            params_json TEXT NOT NULL,
            created_at  TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
        );
        "#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS images (
            id         INTEGER PRIMARY KEY AUTOINCREMENT,
            job_id     INTEGER NOT NULL,
            idx        INTEGER NOT NULL,
            file_path  TEXT NOT NULL,
            thumb_path TEXT NOT NULL,
            seed       INTEGER NOT NULL DEFAULT -1,
            width      INTEGER NOT NULL DEFAULT 0,
            height     INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        "#,
    )
    .execute(pool)
    .await?;

    // Additive migration for existing databases: the favorites flag. New DBs
    // get it implicitly here too. Guarded by a pragma check so it runs at most
    // once (ALTER TABLE ADD COLUMN errors if the column already exists).
    let has_starred = sqlx::query("SELECT 1 FROM pragma_table_info('images') WHERE name = 'starred'")
        .fetch_optional(pool)
        .await?
        .is_some();
    if !has_starred {
        sqlx::query("ALTER TABLE images ADD COLUMN starred INTEGER NOT NULL DEFAULT 0")
            .execute(pool)
            .await?;
    }

    // Additive migration: per-result inpaint provenance. NULL for txt2img images
    // and for pre-existing rows; set to the init/mask PNGs that produced an
    // inpaint result so the UI can show "view the mask that made this".
    let has_init_path = sqlx::query("SELECT 1 FROM pragma_table_info('images') WHERE name = 'init_path'")
        .fetch_optional(pool)
        .await?
        .is_some();
    if !has_init_path {
        sqlx::query("ALTER TABLE images ADD COLUMN init_path TEXT")
            .execute(pool)
            .await?;
        sqlx::query("ALTER TABLE images ADD COLUMN mask_path TEXT")
            .execute(pool)
            .await?;
    }

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS sessions (
            token      TEXT PRIMARY KEY,
            created_at INTEGER NOT NULL,
            expires_at INTEGER NOT NULL
        );
        "#,
    )
    .execute(pool)
    .await?;

    // Multi-user: accounts with a per-user UUID (used in file URLs) and an
    // admin flag. The first user created (on an empty system) becomes admin.
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS users (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            username      TEXT NOT NULL UNIQUE,
            password_hash TEXT NOT NULL,
            uuid          TEXT NOT NULL UNIQUE,
            is_admin      INTEGER NOT NULL DEFAULT 0,
            created_at    TEXT NOT NULL DEFAULT (datetime('now'))
        );
        "#,
    )
    .execute(pool)
    .await?;

    // Additive migration: ownership of jobs. NULL for pre-multi-user rows until
    // claimed by the first admin on bootstrap (see `claim_orphan_jobs`).
    let has_job_user = sqlx::query("SELECT 1 FROM pragma_table_info('jobs') WHERE name = 'user_id'")
        .fetch_optional(pool)
        .await?
        .is_some();
    if !has_job_user {
        sqlx::query("ALTER TABLE jobs ADD COLUMN user_id INTEGER")
            .execute(pool)
            .await?;
    }

    // Additive migration: bind sessions to a user. Pre-existing session rows get
    // NULL and so stop authenticating (forcing a re-login under the new scheme).
    let has_sess_user =
        sqlx::query("SELECT 1 FROM pragma_table_info('sessions') WHERE name = 'user_id'")
            .fetch_optional(pool)
            .await?
            .is_some();
    if !has_sess_user {
        sqlx::query("ALTER TABLE sessions ADD COLUMN user_id INTEGER")
            .execute(pool)
            .await?;
    }

    Ok(())
}

fn row_to_job(row: &SqliteRow) -> Job {
    let params_json: String = row.get("params_json");
    let params: JobParams = serde_json::from_str(&params_json).unwrap_or_default();
    // group_concat yields NULL for jobs with no images.
    let thumb_idxs = row
        .get::<Option<String>, _>("thumb_idxs")
        .unwrap_or_default()
        .split(',')
        .filter_map(|s| s.parse::<i64>().ok())
        .collect();
    Job {
        id: row.get("id"),
        name: row.get("name"),
        status: JobStatus::from_str(&row.get::<String, _>("status")),
        error: row.get("error"),
        progress: row.get::<f64, _>("progress") as f32,
        params,
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
        image_count: row.get("image_count"),
        starred_count: row.get("starred_count"),
        owner_uuid: row.get::<Option<String>, _>("owner_uuid").unwrap_or_default(),
        thumb_idxs,
    }
}

// `thumb_idxs` is a comma-separated list of the 12 most-recent image idx
// values (newest first) so the queue list can render thumbnails by real index
// — robust to gaps left by deleting individual images.
const JOB_SELECT: &str = r#"
    SELECT j.id, j.name, j.status, j.error, j.progress, j.params_json,
           j.created_at, j.updated_at,
           u.uuid AS owner_uuid,
           (SELECT COUNT(*) FROM images i WHERE i.job_id = j.id) AS image_count,
           (SELECT COUNT(*) FROM images i WHERE i.job_id = j.id AND i.starred = 1) AS starred_count,
           (SELECT group_concat(idx) FROM
               (SELECT idx FROM images WHERE job_id = j.id ORDER BY idx DESC LIMIT 12)
           ) AS thumb_idxs
    FROM jobs j
    LEFT JOIN users u ON u.id = j.user_id
"#;

pub async fn insert_job(
    pool: &SqlitePool,
    user_id: i64,
    name: &str,
    params: &JobParams,
) -> Result<i64, sqlx::Error> {
    let params_json = serde_json::to_string(params).unwrap_or_default();
    let id = sqlx::query(
        "INSERT INTO jobs (name, status, params_json, user_id) VALUES (?, 'queued', ?, ?) RETURNING id",
    )
    .bind(name)
    .bind(params_json)
    .bind(user_id)
    .fetch_one(pool)
    .await?
    .get::<i64, _>("id");
    Ok(id)
}

/// Insert a job in a non-runnable `'draft'` state — the worker only picks up
/// `'queued'` rows, so this lets the caller write input files and backfill
/// `params_json` (with their paths) before flipping the job to queued via
/// [`requeue_job`], avoiding a race where the worker grabs a half-initialized
/// inpaint job. `JobStatus::from_str` maps the unknown `'draft'` to `Queued`,
/// so the brief draft window just shows as "queued" in the UI.
pub async fn insert_job_parked(
    pool: &SqlitePool,
    user_id: i64,
    name: &str,
    params: &JobParams,
) -> Result<i64, sqlx::Error> {
    let params_json = serde_json::to_string(params).unwrap_or_default();
    let id = sqlx::query(
        "INSERT INTO jobs (name, status, params_json, user_id) VALUES (?, 'draft', ?, ?) RETURNING id",
    )
    .bind(name)
    .bind(params_json)
    .bind(user_id)
    .fetch_one(pool)
    .await?
    .get::<i64, _>("id");
    Ok(id)
}

/// Overwrite a job's serialized params (used to record each inpaint turn's
/// settings + input paths before re-queuing).
pub async fn set_job_params(
    pool: &SqlitePool,
    id: i64,
    params: &JobParams,
) -> Result<(), sqlx::Error> {
    let params_json = serde_json::to_string(params).unwrap_or_default();
    sqlx::query("UPDATE jobs SET params_json = ?, updated_at = datetime('now') WHERE id = ?")
        .bind(params_json)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Jobs visible to a user: their own, or all of them when `is_admin`.
pub async fn list_jobs(
    pool: &SqlitePool,
    user_id: i64,
    is_admin: bool,
) -> Result<Vec<Job>, sqlx::Error> {
    let rows = if is_admin {
        sqlx::query(&format!("{JOB_SELECT} ORDER BY j.id DESC"))
            .fetch_all(pool)
            .await?
    } else {
        sqlx::query(&format!("{JOB_SELECT} WHERE j.user_id = ? ORDER BY j.id DESC"))
            .bind(user_id)
            .fetch_all(pool)
            .await?
    };
    Ok(rows.iter().map(row_to_job).collect())
}

/// A single job, but only if owned by `user_id` (or the caller is an admin).
pub async fn get_job(
    pool: &SqlitePool,
    id: i64,
    user_id: i64,
    is_admin: bool,
) -> Result<Option<Job>, sqlx::Error> {
    let row = if is_admin {
        sqlx::query(&format!("{JOB_SELECT} WHERE j.id = ?"))
            .bind(id)
            .fetch_optional(pool)
            .await?
    } else {
        sqlx::query(&format!("{JOB_SELECT} WHERE j.id = ? AND j.user_id = ?"))
            .bind(id)
            .bind(user_id)
            .fetch_optional(pool)
            .await?
    };
    Ok(row.as_ref().map(row_to_job))
}

/// Resolve the owner (id + uuid) of a job, regardless of who is asking. Callers
/// use this to authorize access before serving files or mutating a job.
pub async fn job_owner(
    pool: &SqlitePool,
    job_id: i64,
) -> Result<Option<(i64, String)>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT j.user_id AS uid, u.uuid AS uuid FROM jobs j \
         LEFT JOIN users u ON u.id = j.user_id WHERE j.id = ?",
    )
    .bind(job_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.and_then(|r| {
        match (r.get::<Option<i64>, _>("uid"), r.get::<Option<String>, _>("uuid")) {
            (Some(uid), Some(uuid)) => Some((uid, uuid)),
            _ => None,
        }
    }))
}

pub async fn get_job_params(
    pool: &SqlitePool,
    id: i64,
) -> Result<Option<JobParams>, sqlx::Error> {
    let row = sqlx::query("SELECT params_json FROM jobs WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| {
        serde_json::from_str(&r.get::<String, _>("params_json")).unwrap_or_default()
    }))
}

// Image rows are joined to their job's owner so every ImageMeta carries the
// owner uuid needed to build per-user file URLs.
const IMAGE_SELECT: &str = "SELECT i.id, i.job_id, i.idx, i.seed, i.width, i.height, \
    i.starred, i.init_path, u.uuid AS owner_uuid \
    FROM images i LEFT JOIN jobs j ON j.id = i.job_id LEFT JOIN users u ON u.id = j.user_id";

pub async fn get_job_images(
    pool: &SqlitePool,
    job_id: i64,
) -> Result<Vec<ImageMeta>, sqlx::Error> {
    let rows = sqlx::query(&format!("{IMAGE_SELECT} WHERE i.job_id = ? ORDER BY i.idx ASC"))
        .bind(job_id)
        .fetch_all(pool)
        .await?;
    Ok(rows.iter().map(row_to_image).collect())
}

fn row_to_image(r: &SqliteRow) -> ImageMeta {
    ImageMeta {
        id: r.get("id"),
        job_id: r.get("job_id"),
        idx: r.get("idx"),
        seed: r.get("seed"),
        width: r.get("width"),
        height: r.get("height"),
        starred: r.get::<i64, _>("starred") != 0,
        has_inputs: r.get::<Option<String>, _>("init_path").is_some(),
        owner_uuid: r.get::<Option<String>, _>("owner_uuid").unwrap_or_default(),
    }
}

/// Starred images visible to a user (their own, or all for an admin), newest
/// first — the favorites gallery.
pub async fn list_favorites(
    pool: &SqlitePool,
    user_id: i64,
    is_admin: bool,
) -> Result<Vec<ImageMeta>, sqlx::Error> {
    let rows = if is_admin {
        sqlx::query(&format!("{IMAGE_SELECT} WHERE i.starred = 1 ORDER BY i.id DESC"))
            .fetch_all(pool)
            .await?
    } else {
        sqlx::query(&format!(
            "{IMAGE_SELECT} WHERE i.starred = 1 AND j.user_id = ? ORDER BY i.id DESC"
        ))
        .bind(user_id)
        .fetch_all(pool)
        .await?
    };
    Ok(rows.iter().map(row_to_image).collect())
}

/// Set (or clear) the starred flag on a single image.
pub async fn set_image_star(
    pool: &SqlitePool,
    job_id: i64,
    idx: i64,
    starred: bool,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE images SET starred = ? WHERE job_id = ? AND idx = ?")
        .bind(starred as i64)
        .bind(job_id)
        .bind(idx)
        .execute(pool)
        .await?;
    Ok(())
}

/// Count of starred images in a job — used to block whole-job deletion.
pub async fn job_starred_count(pool: &SqlitePool, job_id: i64) -> Result<i64, sqlx::Error> {
    let row = sqlx::query("SELECT COUNT(*) AS c FROM images WHERE job_id = ? AND starred = 1")
        .bind(job_id)
        .fetch_one(pool)
        .await?;
    Ok(row.get::<i64, _>("c"))
}

/// Resolve the on-disk path for one image (full or thumbnail).
pub async fn image_path(
    pool: &SqlitePool,
    job_id: i64,
    idx: i64,
    thumb: bool,
) -> Result<Option<String>, sqlx::Error> {
    let col = if thumb { "thumb_path" } else { "file_path" };
    let row = sqlx::query(&format!(
        "SELECT {col} AS p FROM images WHERE job_id = ? AND idx = ?"
    ))
    .bind(job_id)
    .bind(idx)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| r.get::<String, _>("p")))
}

/// Returns `(idx, seed, file_path)` for each image of a job, ordered by idx.
pub async fn job_image_files(
    pool: &SqlitePool,
    job_id: i64,
) -> Result<Vec<(i64, i64, String)>, sqlx::Error> {
    let rows =
        sqlx::query("SELECT idx, seed, file_path FROM images WHERE job_id = ? ORDER BY idx ASC")
            .bind(job_id)
            .fetch_all(pool)
            .await?;
    Ok(rows
        .iter()
        .map(|r| {
            (
                r.get::<i64, _>("idx"),
                r.get::<i64, _>("seed"),
                r.get::<String, _>("file_path"),
            )
        })
        .collect())
}

/// Oldest queued job (FIFO by id).
pub async fn next_queued_job(
    pool: &SqlitePool,
) -> Result<Option<(i64, JobParams)>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT id, params_json FROM jobs WHERE status = 'queued' ORDER BY id ASC LIMIT 1",
    )
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| {
        let params = serde_json::from_str(&r.get::<String, _>("params_json")).unwrap_or_default();
        (r.get::<i64, _>("id"), params)
    }))
}

pub async fn set_status(
    pool: &SqlitePool,
    id: i64,
    status: JobStatus,
    error: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE jobs SET status = ?, error = ?, updated_at = datetime('now') WHERE id = ?",
    )
    .bind(status.as_str())
    .bind(error)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn set_progress(pool: &SqlitePool, id: i64, progress: f32) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE jobs SET progress = ? WHERE id = ?")
        .bind(progress as f64)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Mark completed only if still `running` — so a concurrent cancel isn't
/// clobbered when generation finishes.
pub async fn complete_if_running(pool: &SqlitePool, id: i64) -> Result<bool, sqlx::Error> {
    let res = sqlx::query(
        "UPDATE jobs SET status = 'completed', progress = 1.0, updated_at = datetime('now') WHERE id = ? AND status = 'running'",
    )
    .bind(id)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn is_running(pool: &SqlitePool, id: i64) -> Result<bool, sqlx::Error> {
    let row = sqlx::query("SELECT status FROM jobs WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(matches!(row.map(|r| r.get::<String, _>("status")).as_deref(), Some("running")))
}

/// Mark failed only if still `running` — so a concurrent cancel isn't clobbered.
pub async fn fail_if_running(pool: &SqlitePool, id: i64, error: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE jobs SET status = 'failed', error = ?, updated_at = datetime('now') WHERE id = ? AND status = 'running'",
    )
    .bind(error)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Progress of the currently-running job, as seen by `user_id`. If the running
/// job is theirs, full details are returned; if it belongs to someone else,
/// only `busy_with_other` is set (no name/progress leak); if nothing is running,
/// the default (idle) is returned.
pub async fn running_progress(
    pool: &SqlitePool,
    user_id: i64,
) -> Result<RunningProgress, sqlx::Error> {
    let row = sqlx::query(
        "SELECT id, name, progress, user_id FROM jobs WHERE status = 'running' ORDER BY id ASC LIMIT 1",
    )
    .fetch_optional(pool)
    .await?;
    Ok(match row {
        Some(r) if r.get::<Option<i64>, _>("user_id") == Some(user_id) => RunningProgress {
            job_id: Some(r.get("id")),
            job_name: r.get("name"),
            progress: r.get::<f64, _>("progress") as f32,
            eta_seconds: 0.0,
            busy_with_other: false,
        },
        Some(_) => RunningProgress {
            busy_with_other: true,
            ..Default::default()
        },
        None => RunningProgress::default(),
    })
}

/// On startup, any job left `running` is re-queued (restart resilience).
pub async fn reset_running_to_queued(pool: &SqlitePool) -> Result<u64, sqlx::Error> {
    let res = sqlx::query(
        "UPDATE jobs SET status = 'queued', progress = 0 WHERE status = 'running'",
    )
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
}

pub async fn requeue_job(pool: &SqlitePool, id: i64) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE jobs SET status = 'queued', error = NULL, progress = 0, updated_at = datetime('now') WHERE id = ?",
    )
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn delete_job(pool: &SqlitePool, id: i64) -> Result<Vec<String>, sqlx::Error> {
    // Return file paths so the caller can remove them from disk.
    let rows = sqlx::query("SELECT file_path, thumb_path FROM images WHERE job_id = ?")
        .bind(id)
        .fetch_all(pool)
        .await?;
    let mut paths = Vec::new();
    for r in &rows {
        paths.push(r.get::<String, _>("file_path"));
        paths.push(r.get::<String, _>("thumb_path"));
    }
    sqlx::query("DELETE FROM images WHERE job_id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM jobs WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(paths)
}

/// Delete a single image row, returning its on-disk paths so the caller can
/// remove the files. Other images keep their `idx` (gaps are fine — serving is
/// by idx lookup, not position), and a later re-queue still appends via
/// [`next_image_idx`]. Starred images are protected: the `starred = 0` guard
/// makes this a no-op (empty paths) for them even if a caller slips past the
/// UI checks.
pub async fn delete_image(
    pool: &SqlitePool,
    job_id: i64,
    idx: i64,
) -> Result<Vec<String>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT file_path, thumb_path FROM images WHERE job_id = ? AND idx = ? AND starred = 0",
    )
    .bind(job_id)
    .bind(idx)
    .fetch_all(pool)
    .await?;
    let mut paths = Vec::new();
    for r in &rows {
        paths.push(r.get::<String, _>("file_path"));
        paths.push(r.get::<String, _>("thumb_path"));
    }
    sqlx::query("DELETE FROM images WHERE job_id = ? AND idx = ? AND starred = 0")
        .bind(job_id)
        .bind(idx)
        .execute(pool)
        .await?;
    Ok(paths)
}

/// Next free image index for a job = max(idx) + 1 (0 if it has none). Used so a
/// re-queued job appends new images instead of overwriting existing ones.
pub async fn next_image_idx(pool: &SqlitePool, job_id: i64) -> Result<i64, sqlx::Error> {
    let row = sqlx::query("SELECT COALESCE(MAX(idx), -1) AS m FROM images WHERE job_id = ?")
        .bind(job_id)
        .fetch_one(pool)
        .await?;
    Ok(row.get::<i64, _>("m") + 1)
}

#[allow(clippy::too_many_arguments)]
pub async fn insert_image(
    pool: &SqlitePool,
    job_id: i64,
    idx: i64,
    file_path: &str,
    thumb_path: &str,
    seed: i64,
    width: i64,
    height: i64,
    // For inpaint results: the init/mask PNGs that produced this image (None for
    // txt2img). Recorded so the UI can show the exact inputs behind a result.
    init_path: Option<&str>,
    mask_path: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO images (job_id, idx, file_path, thumb_path, seed, width, height, init_path, mask_path) VALUES (?,?,?,?,?,?,?,?,?)",
    )
    .bind(job_id)
    .bind(idx)
    .bind(file_path)
    .bind(thumb_path)
    .bind(seed)
    .bind(width)
    .bind(height)
    .bind(init_path)
    .bind(mask_path)
    .execute(pool)
    .await?;
    Ok(())
}

/// Resolve the recorded init/mask PNG path for one inpaint result image.
pub async fn image_input_path(
    pool: &SqlitePool,
    job_id: i64,
    idx: i64,
    mask: bool,
) -> Result<Option<String>, sqlx::Error> {
    let col = if mask { "mask_path" } else { "init_path" };
    let row = sqlx::query(&format!(
        "SELECT {col} AS p FROM images WHERE job_id = ? AND idx = ?"
    ))
    .bind(job_id)
    .bind(idx)
    .fetch_optional(pool)
    .await?;
    Ok(row.and_then(|r| r.get::<Option<String>, _>("p")))
}

// ---- Sessions ----

pub async fn create_session(
    pool: &SqlitePool,
    token: &str,
    user_id: i64,
    now: i64,
    expires_at: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO sessions (token, user_id, created_at, expires_at) VALUES (?,?,?,?)")
        .bind(token)
        .bind(user_id)
        .bind(now)
        .bind(expires_at)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn session_valid(pool: &SqlitePool, token: &str, now: i64) -> Result<bool, sqlx::Error> {
    Ok(session_user(pool, token, now).await?.is_some())
}

/// Resolve a session token to its (still-valid) user. Joins sessions→users so a
/// session whose user was deleted, or which lacks a `user_id`, is treated as
/// invalid.
pub async fn session_user(
    pool: &SqlitePool,
    token: &str,
    now: i64,
) -> Result<Option<UserCtx>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT u.id, u.username, u.is_admin, u.uuid \
         FROM sessions s JOIN users u ON u.id = s.user_id \
         WHERE s.token = ? AND s.expires_at > ?",
    )
    .bind(token)
    .bind(now)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| UserCtx {
        id: r.get("id"),
        username: r.get("username"),
        is_admin: r.get::<i64, _>("is_admin") != 0,
        uuid: r.get("uuid"),
    }))
}

pub async fn delete_session(pool: &SqlitePool, token: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM sessions WHERE token = ?")
        .bind(token)
        .execute(pool)
        .await?;
    Ok(())
}

// ---- Users ----

pub async fn count_users(pool: &SqlitePool) -> Result<i64, sqlx::Error> {
    let row = sqlx::query("SELECT COUNT(*) AS c FROM users")
        .fetch_one(pool)
        .await?;
    Ok(row.get::<i64, _>("c"))
}

pub async fn count_admins(pool: &SqlitePool) -> Result<i64, sqlx::Error> {
    let row = sqlx::query("SELECT COUNT(*) AS c FROM users WHERE is_admin = 1")
        .fetch_one(pool)
        .await?;
    Ok(row.get::<i64, _>("c"))
}

pub async fn create_user(
    pool: &SqlitePool,
    username: &str,
    password_hash: &str,
    uuid: &str,
    is_admin: bool,
) -> Result<i64, sqlx::Error> {
    let id = sqlx::query(
        "INSERT INTO users (username, password_hash, uuid, is_admin) VALUES (?,?,?,?) RETURNING id",
    )
    .bind(username)
    .bind(password_hash)
    .bind(uuid)
    .bind(is_admin as i64)
    .fetch_one(pool)
    .await?
    .get::<i64, _>("id");
    Ok(id)
}

/// Returns `(id, password_hash, is_admin)` for password verification on login.
pub async fn get_user_by_username(
    pool: &SqlitePool,
    username: &str,
) -> Result<Option<(i64, String, bool)>, sqlx::Error> {
    let row = sqlx::query("SELECT id, password_hash, is_admin FROM users WHERE username = ?")
        .bind(username)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| {
        (
            r.get::<i64, _>("id"),
            r.get::<String, _>("password_hash"),
            r.get::<i64, _>("is_admin") != 0,
        )
    }))
}

pub async fn is_user_admin(pool: &SqlitePool, id: i64) -> Result<bool, sqlx::Error> {
    let row = sqlx::query("SELECT is_admin FROM users WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(matches!(row.map(|r| r.get::<i64, _>("is_admin")), Some(1)))
}

pub async fn list_users(pool: &SqlitePool) -> Result<Vec<UserInfo>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT u.id, u.username, u.is_admin, u.created_at, \
         (SELECT COUNT(*) FROM jobs j WHERE j.user_id = u.id) AS job_count \
         FROM users u ORDER BY u.id ASC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .iter()
        .map(|r| UserInfo {
            id: r.get("id"),
            username: r.get("username"),
            is_admin: r.get::<i64, _>("is_admin") != 0,
            created_at: r.get("created_at"),
            job_count: r.get("job_count"),
        })
        .collect())
}

pub async fn set_user_password(
    pool: &SqlitePool,
    id: i64,
    password_hash: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE users SET password_hash = ? WHERE id = ?")
        .bind(password_hash)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Delete a user along with all their jobs and images. Returns the on-disk file
/// paths of those images so the caller can unlink them.
pub async fn delete_user(pool: &SqlitePool, id: i64) -> Result<Vec<String>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT i.file_path, i.thumb_path FROM images i \
         JOIN jobs j ON j.id = i.job_id WHERE j.user_id = ?",
    )
    .bind(id)
    .fetch_all(pool)
    .await?;
    let mut paths = Vec::new();
    for r in &rows {
        paths.push(r.get::<String, _>("file_path"));
        paths.push(r.get::<String, _>("thumb_path"));
    }
    sqlx::query("DELETE FROM images WHERE job_id IN (SELECT id FROM jobs WHERE user_id = ?)")
        .bind(id)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM jobs WHERE user_id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM sessions WHERE user_id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM users WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(paths)
}

/// Job ids belonging to a user — used to clean up their on-disk directories.
pub async fn user_job_ids(pool: &SqlitePool, id: i64) -> Result<Vec<i64>, sqlx::Error> {
    let rows = sqlx::query("SELECT id FROM jobs WHERE user_id = ?")
        .bind(id)
        .fetch_all(pool)
        .await?;
    Ok(rows.iter().map(|r| r.get::<i64, _>("id")).collect())
}

/// Assign every ownerless job (pre-multi-user rows) to `user_id`. Run once when
/// the first admin is bootstrapped so existing galleries aren't orphaned.
pub async fn claim_orphan_jobs(pool: &SqlitePool, user_id: i64) -> Result<u64, sqlx::Error> {
    let res = sqlx::query("UPDATE jobs SET user_id = ? WHERE user_id IS NULL")
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}
