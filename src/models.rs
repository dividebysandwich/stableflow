//! Types shared between the server and the WASM client.
//! No `cfg` gating here: both sides use the same definitions.

use serde::{Deserialize, Serialize};

/// Which diffusion architecture a job targets. Drives which form fields are
/// relevant (e.g. distilled CFG is a Flux concept).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModelType {
    SD,
    XL,
    Flux,
}

impl ModelType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ModelType::SD => "SD",
            ModelType::XL => "XL",
            ModelType::Flux => "Flux",
        }
    }
    pub fn from_str(s: &str) -> ModelType {
        match s {
            "XL" => ModelType::XL,
            "Flux" => ModelType::Flux,
            _ => ModelType::SD,
        }
    }
    /// Distilled CFG only meaningful for Flux-family checkpoints.
    pub fn uses_distilled_cfg(&self) -> bool {
        matches!(self, ModelType::Flux)
    }
}

/// Everything needed to run (and to reproduce / template) a txt2img job.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JobParams {
    pub model_type: ModelType,
    pub checkpoint: String,
    pub prompt: String,
    pub negative_prompt: String,
    pub styles: Vec<String>,
    pub steps: u32,
    pub cfg_scale: f32,
    pub distilled_cfg_scale: f32,
    pub width: u32,
    pub height: u32,
    pub batch_size: u32,
    pub n_iter: u32,
    pub sampler_name: String,
    pub scheduler: String,
    pub seed: i64,
    // Hires fix
    pub enable_hr: bool,
    pub hr_upscaler: String,
    pub hr_scale: f32,
    pub hr_second_pass_steps: u32,
    pub denoising_strength: f32,
}

impl Default for JobParams {
    fn default() -> Self {
        JobParams {
            model_type: ModelType::SD,
            checkpoint: String::new(),
            prompt: String::new(),
            negative_prompt: String::new(),
            styles: Vec::new(),
            steps: 31,
            cfg_scale: 7.0,
            distilled_cfg_scale: 3.5,
            width: 1024,
            height: 1024,
            batch_size: 1,
            n_iter: 1,
            sampler_name: "Euler".into(),
            scheduler: "automatic".into(),
            seed: -1,
            enable_hr: false,
            hr_upscaler: "Latent".into(),
            hr_scale: 2.0,
            hr_second_pass_steps: 0,
            denoising_strength: 0.7,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum JobStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Canceled,
}

impl JobStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            JobStatus::Queued => "queued",
            JobStatus::Running => "running",
            JobStatus::Completed => "completed",
            JobStatus::Failed => "failed",
            JobStatus::Canceled => "canceled",
        }
    }
    pub fn from_str(s: &str) -> JobStatus {
        match s {
            "running" => JobStatus::Running,
            "completed" => JobStatus::Completed,
            "failed" => JobStatus::Failed,
            "canceled" => JobStatus::Canceled,
            _ => JobStatus::Queued,
        }
    }
}

/// A job row plus a few computed fields for list/detail views.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Job {
    pub id: i64,
    pub name: String,
    pub status: JobStatus,
    pub error: Option<String>,
    pub progress: f32,
    pub params: JobParams,
    pub created_at: String,
    pub updated_at: String,
    pub image_count: i64,
    /// Actual idx values of the most-recent images (newest first), for the
    /// queue-list thumbnail strip. Real indices — not a 0..count range — so
    /// they stay correct after individual images are deleted (which leaves
    /// gaps in the idx sequence).
    pub thumb_idxs: Vec<i64>,
}

/// Metadata for a single produced image.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImageMeta {
    pub id: i64,
    pub job_id: i64,
    pub idx: i64,
    pub seed: i64,
    pub width: i64,
    pub height: i64,
}

/// Dropdown choices pulled from Forge for the job form.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FormOptions {
    pub checkpoints: Vec<String>,
    pub samplers: Vec<String>,
    pub schedulers: Vec<String>,
    pub styles: Vec<String>,
    pub upscalers: Vec<String>,
}

/// Live progress for the currently-running job (if any).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RunningProgress {
    pub job_id: Option<i64>,
    pub job_name: String,
    pub progress: f32,
    pub eta_seconds: f32,
}
