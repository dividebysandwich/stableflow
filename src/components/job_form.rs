use leptos::prelude::*;
use leptos_router::hooks::{use_navigate, use_query_map};

use crate::api::{create_job, get_form_options, get_job_params};
use crate::models::{FormOptions, JobParams, ModelType};

/// New-job page. Loads Forge dropdown options and, if `?from=<id>` is present,
/// the params of a past job to use as a template, then mounts the form.
#[component]
pub fn NewJobPage() -> impl IntoView {
    let query = use_query_map();
    let from_id = Memo::new(move |_| {
        query.read().get("from").and_then(|s| s.parse::<i64>().ok())
    });

    let options = Resource::new(
        || (),
        |_| async move { get_form_options().await.unwrap_or_default() },
    );
    let template = Resource::new(
        move || from_id.get(),
        |id| async move {
            match id {
                Some(id) => get_job_params(id).await.ok().flatten(),
                None => None,
            }
        },
    );

    view! {
        <div class="page">
            <h1>{move || if from_id.get().is_some() { "New job (from template)" } else { "New job" }}</h1>
            <Suspense fallback=|| view! { <p class="muted">"Loading options\u{2026}"</p> }>
                {move || {
                    match (options.get(), template.get()) {
                        (Some(opts), Some(tmpl)) => {
                            let initial = tmpl.unwrap_or_default();
                            view! { <JobForm initial=initial options=opts/> }.into_any()
                        }
                        _ => ().into_any(),
                    }
                }}
            </Suspense>
        </div>
    }
}

/// Pick `current` if it's a valid option, else fall back to the first option.
fn pick(current: &str, opts: &[String]) -> String {
    if opts.iter().any(|o| o == current) || opts.is_empty() {
        current.to_string()
    } else {
        opts.first().cloned().unwrap_or_default()
    }
}

#[component]
fn JobForm(initial: JobParams, options: FormOptions) -> impl IntoView {
    let name = RwSignal::new(String::new());
    let model_type = RwSignal::new(initial.model_type.as_str().to_string());
    let checkpoint = RwSignal::new(pick(&initial.checkpoint, &options.checkpoints));
    let prompt = RwSignal::new(initial.prompt.clone());
    let negative = RwSignal::new(initial.negative_prompt.clone());
    let styles = RwSignal::new(initial.styles.clone());
    let steps = RwSignal::new(initial.steps.to_string());
    let cfg = RwSignal::new(initial.cfg_scale.to_string());
    let dcfg = RwSignal::new(initial.distilled_cfg_scale.to_string());
    let width = RwSignal::new(initial.width.to_string());
    let height = RwSignal::new(initial.height.to_string());
    let batch = RwSignal::new(initial.batch_size.to_string());
    let niter = RwSignal::new(initial.n_iter.to_string());
    let sampler = RwSignal::new(pick(&initial.sampler_name, &options.samplers));
    let scheduler = RwSignal::new(pick(&initial.scheduler, &options.schedulers));
    let seed = RwSignal::new(initial.seed.to_string());
    let enable_hr = RwSignal::new(initial.enable_hr);
    let hr_upscaler = RwSignal::new(pick(&initial.hr_upscaler, &options.upscalers));
    let hr_scale = RwSignal::new(initial.hr_scale.to_string());
    let hr_steps = RwSignal::new(initial.hr_second_pass_steps.to_string());
    let denoise = RwSignal::new(initial.denoising_strength.to_string());

    let create = Action::new(move |input: &(String, JobParams)| {
        let (n, p) = input.clone();
        async move { create_job(n, p).await }
    });

    // Navigate home once the job is queued.
    let navigate = use_navigate();
    Effect::new(move |_| {
        if let Some(Ok(_)) = create.value().get() {
            navigate("/", Default::default());
        }
    });

    let on_submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let p = JobParams {
            model_type: ModelType::from_str(&model_type.get()),
            checkpoint: checkpoint.get(),
            prompt: prompt.get(),
            negative_prompt: negative.get(),
            styles: styles.get(),
            steps: steps.get().trim().parse().unwrap_or(20),
            cfg_scale: cfg.get().trim().parse().unwrap_or(7.0),
            distilled_cfg_scale: dcfg.get().trim().parse().unwrap_or(3.5),
            width: width.get().trim().parse().unwrap_or(512),
            height: height.get().trim().parse().unwrap_or(512),
            batch_size: batch.get().trim().parse().unwrap_or(1),
            n_iter: niter.get().trim().parse().unwrap_or(1),
            sampler_name: sampler.get(),
            scheduler: scheduler.get(),
            seed: seed.get().trim().parse().unwrap_or(-1),
            enable_hr: enable_hr.get(),
            hr_upscaler: hr_upscaler.get(),
            hr_scale: hr_scale.get().trim().parse().unwrap_or(2.0),
            hr_second_pass_steps: hr_steps.get().trim().parse().unwrap_or(0),
            denoising_strength: denoise.get().trim().parse().unwrap_or(0.7),
        };
        create.dispatch((name.get(), p));
    };

    let pending = create.pending();

    let checkpoints = options.checkpoints.clone();
    let samplers = options.samplers.clone();
    let schedulers = options.schedulers.clone();
    let upscalers = options.upscalers.clone();
    let style_opts = options.styles.clone();

    view! {
        <form class="job-form" on:submit=on_submit>
            <div class="form-grid">
                <label class="full">
                    "Job label (optional)"
                    <input type="text" prop:value=move || name.get()
                        on:input=move |ev| name.set(event_target_value(&ev))/>
                </label>

                <label>
                    "Diffusion model"
                    <select prop:value=move || model_type.get()
                        on:change=move |ev| model_type.set(event_target_value(&ev))>
                        <option value="SD">"SD 1.5"</option>
                        <option value="XL">"SDXL"</option>
                        <option value="Flux">"Flux"</option>
                    </select>
                </label>

                <label>
                    "Checkpoint"
                    <select prop:value=move || checkpoint.get()
                        on:change=move |ev| checkpoint.set(event_target_value(&ev))>
                        {checkpoints.into_iter().map(|c|
                            view! { <option value=c.clone()>{c.clone()}</option> }).collect_view()}
                    </select>
                </label>

                <label>
                    "Sampling method"
                    <select prop:value=move || sampler.get()
                        on:change=move |ev| sampler.set(event_target_value(&ev))>
                        {samplers.into_iter().map(|c|
                            view! { <option value=c.clone()>{c.clone()}</option> }).collect_view()}
                    </select>
                </label>

                <label>
                    "Schedule type"
                    <select prop:value=move || scheduler.get()
                        on:change=move |ev| scheduler.set(event_target_value(&ev))>
                        {schedulers.into_iter().map(|c|
                            view! { <option value=c.clone()>{c.clone()}</option> }).collect_view()}
                    </select>
                </label>

                <label>
                    "Sampling steps"
                    <input type="number" min="1" prop:value=move || steps.get()
                        on:input=move |ev| steps.set(event_target_value(&ev))/>
                </label>

                <label>
                    "CFG scale"
                    <input type="number" step="0.1" prop:value=move || cfg.get()
                        on:input=move |ev| cfg.set(event_target_value(&ev))/>
                </label>

                <Show when=move || model_type.get() == "Flux">
                    <label>
                        "Distilled CFG scale"
                        <input type="number" step="0.1" prop:value=move || dcfg.get()
                            on:input=move |ev| dcfg.set(event_target_value(&ev))/>
                    </label>
                </Show>

                <label>
                    "Width"
                    <input type="number" step="8" min="64" prop:value=move || width.get()
                        on:input=move |ev| width.set(event_target_value(&ev))/>
                </label>

                <label>
                    "Height"
                    <input type="number" step="8" min="64" prop:value=move || height.get()
                        on:input=move |ev| height.set(event_target_value(&ev))/>
                </label>

                <label>
                    "Batch size"
                    <input type="number" min="1" prop:value=move || batch.get()
                        on:input=move |ev| batch.set(event_target_value(&ev))/>
                </label>

                <label>
                    "Batch count (iterations)"
                    <input type="number" min="1" prop:value=move || niter.get()
                        on:input=move |ev| niter.set(event_target_value(&ev))/>
                </label>

                <label>
                    "Seed (-1 = random)"
                    <input type="number" prop:value=move || seed.get()
                        on:input=move |ev| seed.set(event_target_value(&ev))/>
                </label>
            </div>

            <label class="full">
                "Prompt"
                <textarea rows="4" prop:value=move || prompt.get()
                    on:input=move |ev| prompt.set(event_target_value(&ev))></textarea>
            </label>

            <label class="full">
                "Negative prompt"
                <textarea rows="3" prop:value=move || negative.get()
                    on:input=move |ev| negative.set(event_target_value(&ev))></textarea>
            </label>

            <fieldset>
                <legend>"Styles"</legend>
                <div class="style-box">
                    {style_opts.into_iter().map(|s| {
                        let s_check = s.clone();
                        let s_name = s.clone();
                        view! {
                            <label class="style-item">
                                <input type="checkbox"
                                    prop:checked=move || styles.get().iter().any(|x| x == &s_check)
                                    on:change=move |ev| {
                                        let checked = event_target_checked(&ev);
                                        let n = s_name.clone();
                                        styles.update(|v| {
                                            if checked {
                                                if !v.iter().any(|x| x == &n) { v.push(n); }
                                            } else {
                                                v.retain(|x| x != &n);
                                            }
                                        });
                                    }/>
                                <span>{s}</span>
                            </label>
                        }
                    }).collect_view()}
                </div>
            </fieldset>

            <fieldset>
                <legend>
                    <label class="inline">
                        <input type="checkbox" prop:checked=move || enable_hr.get()
                            on:change=move |ev| enable_hr.set(event_target_checked(&ev))/>
                        "Hires fix (upscale)"
                    </label>
                </legend>
                <Show when=move || enable_hr.get()>
                    <div class="form-grid">
                        <label>
                            "Upscaler"
                            <select prop:value=move || hr_upscaler.get()
                                on:change=move |ev| hr_upscaler.set(event_target_value(&ev))>
                                {upscalers.clone().into_iter().map(|c|
                                    view! { <option value=c.clone()>{c.clone()}</option> }).collect_view()}
                            </select>
                        </label>
                        <label>
                            "Upscale by"
                            <input type="number" step="0.05" prop:value=move || hr_scale.get()
                                on:input=move |ev| hr_scale.set(event_target_value(&ev))/>
                        </label>
                        <label>
                            "Hires steps (0 = same)"
                            <input type="number" min="0" prop:value=move || hr_steps.get()
                                on:input=move |ev| hr_steps.set(event_target_value(&ev))/>
                        </label>
                        <label>
                            "Denoising strength"
                            <input type="number" step="0.01" min="0" max="1" prop:value=move || denoise.get()
                                on:input=move |ev| denoise.set(event_target_value(&ev))/>
                        </label>
                    </div>
                </Show>
            </fieldset>

            <div class="form-actions">
                <button type="submit" disabled=move || pending.get()>
                    {move || if pending.get() { "Queuing\u{2026}" } else { "Queue job" }}
                </button>
            </div>
        </form>
    }
}
