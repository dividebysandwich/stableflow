use std::time::Duration;

use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::use_params_map;

use crate::api::{delete_image, get_job, get_job_images};
use crate::models::{Job, JobParams};

#[component]
pub fn JobDetailPage() -> impl IntoView {
    let params = use_params_map();
    let job_id = Memo::new(move |_| {
        params
            .read()
            .get("id")
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(0)
    });

    // Refresh images periodically so an in-progress job's results stream in.
    let (tick, set_tick) = signal(0u32);
    Effect::new(move |_| {
        set_interval(
            move || set_tick.update(|t| *t = t.wrapping_add(1)),
            Duration::from_millis(3000),
        );
    });

    let del_img = Action::new(|input: &(i64, i64)| {
        let (job_id, idx) = *input;
        async move { delete_image(job_id, idx).await }
    });

    let job = Resource::new(
        move || (job_id.get(), tick.get()),
        |(id, _)| async move { get_job(id).await.ok().flatten() },
    );
    let images = Resource::new(
        // Refetch on each poll tick and immediately after a deletion.
        move || (job_id.get(), tick.get(), del_img.version().get()),
        |(id, _, _)| async move { get_job_images(id).await.unwrap_or_default() },
    );

    view! {
        <div class="page">
            <A href="/">"\u{2190} Back to queue"</A>
            <Transition fallback=|| view! { <p class="muted">"Loading\u{2026}"</p> }>
                {move || {
                    job.get().map(|maybe| match maybe {
                        None => view! { <p class="error">"Job not found."</p> }.into_any(),
                        Some(job) => job_detail(job).into_any(),
                    })
                }}
            </Transition>

            <h2>"Results"</h2>
            // Transition + keyed <For> let new images stream in (while the job is
            // still running) without remounting / flashing the existing ones.
            <Transition fallback=|| view! { <p class="muted">"Loading images\u{2026}"</p> }>
                <Show
                    when=move || !images.get().unwrap_or_default().is_empty()
                    fallback=|| view! { <p class="muted">"No images yet."</p> }
                >
                    <div class="gallery-grid">
                        <For
                            each=move || images.get().unwrap_or_default()
                            key=|im| (im.idx, im.seed)
                            children=move |im| {
                                let id = job_id.get();
                                let idx = im.idx;
                                view! {
                                    <div class="gallery-item">
                                        <a href=format!("/img/{id}/{idx}") target="_blank">
                                            <img src=format!("/thumb/{id}/{idx}") loading="lazy" alt=""/>
                                        </a>
                                        <div class="gallery-cap">
                                            <span class="muted">{format!("seed {}", im.seed)}</span>
                                            <span class="cap-actions">
                                                <a href=format!("/download/img/{id}/{idx}")>"download"</a>
                                                <button
                                                    class="del-btn"
                                                    on:click=move |_| {
                                                        if crate::components::confirm(
                                                            "Delete this image? This cannot be undone."
                                                        ) {
                                                            del_img.dispatch((id, idx));
                                                        }
                                                    }
                                                >"delete"</button>
                                            </span>
                                        </div>
                                    </div>
                                }
                            }
                        />
                    </div>
                </Show>
            </Transition>
        </div>
    }
}

fn job_detail(job: Job) -> impl IntoView {
    let id = job.id;
    let status = job.status.as_str().to_string();
    let p: JobParams = job.params.clone();
    let has_images = job.image_count > 0;
    let show_distilled = p.model_type.uses_distilled_cfg();

    let title = if job.name.trim().is_empty() {
        format!("Job #{id}")
    } else {
        format!("{} (#{id})", job.name)
    };

    let hr_line = if p.enable_hr {
        format!(
            "{} \u{00d7}{:.2}, {} steps, denoise {:.2}",
            p.hr_upscaler, p.hr_scale, p.hr_second_pass_steps, p.denoising_strength
        )
    } else {
        "off".to_string()
    };

    view! {
        <div class="detail-head">
            <h1>{title}</h1>
            <span class=format!("badge badge-{status}")>{status.clone()}</span>
        </div>

        {job.error.clone().map(|e| view! { <div class="error">{e}</div> })}

        <div class="detail-actions">
            <Show when=move || has_images>
                <a class="btn" href=format!("/download/job/{id}")>"Download all (zip)"</a>
            </Show>
            <A href=format!("/new?from={id}")>"Reload as template"</A>
        </div>

        <table class="params">
            <tbody>
                <tr><td>"Model type"</td><td>{p.model_type.as_str()}</td></tr>
                <tr><td>"Checkpoint"</td><td>{p.checkpoint.clone()}</td></tr>
                <tr><td>"Sampler / schedule"</td><td>{format!("{} / {}", p.sampler_name, p.scheduler)}</td></tr>
                <tr><td>"Steps"</td><td>{p.steps}</td></tr>
                <tr><td>"CFG scale"</td><td>{p.cfg_scale}</td></tr>
                <Show when=move || show_distilled>
                    <tr><td>"Distilled CFG"</td><td>{p.distilled_cfg_scale}</td></tr>
                </Show>
                <tr><td>"Size"</td><td>{format!("{}\u{00d7}{}", p.width, p.height)}</td></tr>
                <tr><td>"Batch"</td><td>{format!("{} \u{00d7} {} iter", p.batch_size, p.n_iter)}</td></tr>
                <tr><td>"Seed"</td><td>{p.seed}</td></tr>
                <tr><td>"Hires"</td><td>{hr_line}</td></tr>
                <tr><td>"Styles"</td><td>{p.styles.join(", ")}</td></tr>
                <tr><td>"Prompt"</td><td class="prompt-cell">{p.prompt.clone()}</td></tr>
                <tr><td>"Negative"</td><td class="prompt-cell">{p.negative_prompt.clone()}</td></tr>
            </tbody>
        </table>
    }
}
