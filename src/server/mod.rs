//! Server-only modules (compiled under the `ssr` feature).

pub mod auth;
pub mod db;
pub mod files;
pub mod forge;
pub mod worker;

use std::path::PathBuf;
use std::sync::Arc;

use sqlx::sqlite::SqlitePool;
use tokio::sync::{Notify, RwLock};

use crate::models::FormOptions;
use forge::Forge;

/// Shared application state, cheap to clone (all heavy fields are pooled/Arc).
/// Provided to Leptos server functions via context and to Axum handlers via an
/// `Extension` layer.
#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub forge: Forge,
    pub data_dir: PathBuf,
    /// Woken whenever a new job is enqueued so the worker stops idling.
    pub notify: Arc<Notify>,
    /// Cached Forge dropdown choices (refreshable).
    pub options: Arc<RwLock<FormOptions>>,
    pub password: String,
}

impl AppState {
    pub fn gallery_dir(&self, job_id: i64) -> PathBuf {
        self.data_dir.join("gallery").join(job_id.to_string())
    }
    pub fn thumb_dir(&self, job_id: i64) -> PathBuf {
        self.data_dir.join("thumbs").join(job_id.to_string())
    }
    /// Per-job directory holding the inpaint turn input PNGs (init + mask),
    /// named by the result index they produced so each turn is retained.
    pub fn input_dir(&self, job_id: i64) -> PathBuf {
        self.data_dir.join("inputs").join(job_id.to_string())
    }
}
