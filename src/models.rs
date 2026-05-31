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

/// Which kind of inpainting a turn performs. The distinction is purely
/// client-side: in both cases the browser uploads an init image + a mask, and
/// the worker runs the same Forge img2img call. `Mask` keeps the base pixels and
/// only paints a mask; `Sketch` additionally bakes colored strokes into the init
/// image so the painted colors guide the regenerated region.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum InpaintMode {
    Mask,
    Sketch,
}

impl InpaintMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            InpaintMode::Mask => "mask",
            InpaintMode::Sketch => "sketch",
        }
    }
    pub fn from_str(s: &str) -> InpaintMode {
        match s {
            "sketch" => InpaintMode::Sketch,
            _ => InpaintMode::Mask,
        }
    }
}

/// Inpaint-specific settings. Present (`Some`) on a [`JobParams`] ⇒ the worker
/// runs Forge img2img instead of txt2img. `init_path`/`mask_path` point at the
/// current turn's input PNGs on disk — we store paths, never base64, so
/// `params_json` (read for every job in the queue list) stays small.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InpaintParams {
    pub mode: InpaintMode,
    pub denoising_strength: f32,
    pub mask_blur: u32,
    /// 0 fill, 1 original, 2 latent noise, 3 latent nothing.
    pub inpainting_fill: u32,
    /// "Only masked" when true (inpaint at full res within the masked bbox).
    pub inpaint_full_res: bool,
    pub inpaint_full_res_padding: u32,
    /// 0 = inpaint the masked area, 1 = inpaint everything except the mask.
    pub mask_invert: u32,
    /// On-disk PNG paths for the current turn (filled in server-side).
    pub init_path: String,
    pub mask_path: String,
    /// Provenance: the gallery image this inpaint session started from.
    pub src_job: i64,
    pub src_idx: i64,
}

impl Default for InpaintParams {
    fn default() -> Self {
        InpaintParams {
            mode: InpaintMode::Mask,
            denoising_strength: 0.75,
            mask_blur: 4,
            inpainting_fill: 1,
            inpaint_full_res: true,
            inpaint_full_res_padding: 32,
            mask_invert: 0,
            init_path: String::new(),
            mask_path: String::new(),
            src_job: 0,
            src_idx: 0,
        }
    }
}

/// Everything needed to run (and to reproduce / template) a job. A `None`
/// `inpaint` is an ordinary txt2img job; `Some` makes it an inpaint job.
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
    /// `Some` ⇒ this is an inpaint job (worker runs img2img). Defaulted so old
    /// `params_json` rows (which lack the field) still deserialize.
    #[serde(default)]
    pub inpaint: Option<InpaintParams>,
}

impl JobParams {
    /// Whether this job is an inpaint job (vs txt2img).
    pub fn is_inpaint(&self) -> bool {
        self.inpaint.is_some()
    }
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
            inpaint: None,
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
    /// How many of this job's images are starred. Used to block whole-job
    /// deletion while favorites remain.
    pub starred_count: i64,
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
    /// Whether the user has starred this image. Starred images appear in the
    /// favorites gallery and cannot be deleted until un-starred.
    pub starred: bool,
    /// For inpaint results: whether the init/mask PNGs that produced this image
    /// were recorded on disk (and so can be viewed). False for txt2img images.
    pub has_inputs: bool,
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
