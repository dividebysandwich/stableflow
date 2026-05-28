//! Thin client for the Stable Diffusion Forge REST API (`/sdapi/v1/*`).

use serde::Deserialize;
use serde_json::json;

use crate::models::{FormOptions, JobParams};

#[derive(Clone)]
pub struct Forge {
    client: reqwest::Client,
    base: String,
}

#[derive(Debug, Deserialize)]
pub struct Txt2ImgResponse {
    pub images: Vec<String>,
    #[serde(default)]
    pub info: String,
}

#[derive(Debug, Deserialize, Default)]
pub struct ProgressResponse {
    #[serde(default)]
    pub progress: f64,
    #[serde(default)]
    pub eta_relative: f64,
}

/// `info` is a JSON-encoded string; we only need the per-image seeds.
#[derive(Debug, Deserialize, Default)]
struct Txt2ImgInfo {
    #[serde(default)]
    all_seeds: Vec<i64>,
    #[serde(default)]
    seed: i64,
}

impl Forge {
    pub fn new(base: &str) -> Self {
        let client = reqwest::Client::builder()
            // Generation can take a long time; don't time out mid-render.
            .timeout(std::time::Duration::from_secs(60 * 30))
            .build()
            .expect("reqwest client");
        Forge {
            client,
            base: base.trim_end_matches('/').to_string(),
        }
    }

    async fn get_json<T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
    ) -> Result<T, reqwest::Error> {
        self.client
            .get(format!("{}{}", self.base, path))
            .send()
            .await?
            .error_for_status()?
            .json::<T>()
            .await
    }

    /// Pull all dropdown choices. Individual failures degrade to empty lists so
    /// the form still renders if one endpoint is unavailable.
    pub async fn fetch_form_options(&self) -> FormOptions {
        #[derive(Deserialize)]
        struct Model {
            title: String,
        }
        #[derive(Deserialize)]
        struct Named {
            name: String,
        }

        let checkpoints = self
            .get_json::<Vec<Model>>("/sdapi/v1/sd-models")
            .await
            .map(|v| v.into_iter().map(|m| m.title).collect())
            .unwrap_or_default();
        let samplers = self
            .get_json::<Vec<Named>>("/sdapi/v1/samplers")
            .await
            .map(|v| v.into_iter().map(|m| m.name).collect())
            .unwrap_or_default();
        let schedulers = self
            .get_json::<Vec<Named>>("/sdapi/v1/schedulers")
            .await
            .map(|v| v.into_iter().map(|m| m.name).collect())
            .unwrap_or_default();
        // Forge's style list includes decorative separator entries (e.g.
        // "---------------- STYLES ----------------") whose prompt/negative are
        // both null. Selecting one makes Forge 500 ("NoneType is not iterable"),
        // so drop any style that has no usable prompt or negative text.
        #[derive(Deserialize)]
        struct Style {
            name: String,
            #[serde(default)]
            prompt: Option<String>,
            #[serde(default)]
            negative_prompt: Option<String>,
        }
        let styles = self
            .get_json::<Vec<Style>>("/sdapi/v1/prompt-styles")
            .await
            .map(|v| {
                v.into_iter()
                    .filter(|s| {
                        let p = s.prompt.as_deref().unwrap_or("").trim();
                        let n = s.negative_prompt.as_deref().unwrap_or("").trim();
                        !(p.is_empty() && n.is_empty())
                    })
                    .map(|s| s.name)
                    .collect()
            })
            .unwrap_or_default();
        // The hires dropdown combines latent-space modes (a separate endpoint)
        // with the pixel-space upscalers, matching the Forge UI. "None" is not a
        // valid hires upscaler, so drop it.
        let latent = self
            .get_json::<Vec<Named>>("/sdapi/v1/latent-upscale-modes")
            .await
            .map(|v| v.into_iter().map(|m| m.name).collect::<Vec<_>>())
            .unwrap_or_default();
        let mut upscalers = latent;
        upscalers.extend(
            self.get_json::<Vec<Named>>("/sdapi/v1/upscalers")
                .await
                .map(|v| {
                    v.into_iter()
                        .map(|m| m.name)
                        .filter(|n| n != "None")
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default(),
        );

        FormOptions {
            checkpoints,
            samplers,
            schedulers,
            styles,
            upscalers,
        }
    }

    pub async fn progress(&self) -> ProgressResponse {
        self.get_json::<ProgressResponse>("/sdapi/v1/progress")
            .await
            .unwrap_or_default()
    }

    pub async fn interrupt(&self) {
        let _ = self
            .client
            .post(format!("{}/sdapi/v1/interrupt", self.base))
            .send()
            .await;
    }

    /// Run a txt2img generation. Returns `(base64_png, seed)` per image.
    pub async fn txt2img(
        &self,
        params: &JobParams,
    ) -> Result<Vec<(String, i64)>, String> {
        let mut payload = json!({
            "prompt": params.prompt,
            "negative_prompt": params.negative_prompt,
            "styles": params.styles,
            "steps": params.steps,
            "cfg_scale": params.cfg_scale,
            "distilled_cfg_scale": params.distilled_cfg_scale,
            "width": params.width,
            "height": params.height,
            "batch_size": params.batch_size,
            "n_iter": params.n_iter,
            "sampler_name": params.sampler_name,
            "scheduler": params.scheduler,
            "seed": params.seed,
            "save_images": false,
            "send_images": true,
            "override_settings": { "sd_model_checkpoint": params.checkpoint },
            "override_settings_restore_afterwards": true,
        });

        if params.enable_hr {
            payload["enable_hr"] = json!(true);
            payload["hr_upscaler"] = json!(params.hr_upscaler);
            payload["hr_scale"] = json!(params.hr_scale);
            payload["hr_second_pass_steps"] = json!(params.hr_second_pass_steps);
            payload["denoising_strength"] = json!(params.denoising_strength);
            // Forge iterates over this list; if omitted it is None and the whole
            // request 500s ("NoneType is not iterable"). "Use same choices"
            // inherits the base model's text encoders / VAE.
            payload["hr_additional_modules"] = json!(["Use same choices"]);
        }

        let resp = self
            .client
            .post(format!("{}/sdapi/v1/txt2img", self.base))
            .json(&payload)
            .send()
            .await
            .map_err(|e| format!("request failed: {e}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("forge {status}: {body}"));
        }

        let result: Txt2ImgResponse = resp
            .json()
            .await
            .map_err(|e| format!("decode failed: {e}"))?;

        let info: Txt2ImgInfo = serde_json::from_str(&result.info).unwrap_or_default();
        let seeds = &info.all_seeds;

        Ok(result
            .images
            .into_iter()
            .enumerate()
            .map(|(i, img)| {
                let seed = seeds.get(i).copied().unwrap_or(info.seed);
                (img, seed)
            })
            .collect())
    }
}
