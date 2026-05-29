use std::time::Duration;

use leptos::ev;
use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::use_params_map;

use crate::api::{delete_image, get_job, get_job_images};
use crate::models::{ImageMeta, Job, JobParams};

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

    // Always-available view of the image list, plus which one (by list
    // position) the fullscreen viewer is showing, if any.
    let imgs = Signal::derive(move || images.get().unwrap_or_default());
    let viewer = RwSignal::new(None::<usize>);

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
                            each=move || imgs.get()
                            key=|im| (im.idx, im.seed)
                            children=move |im| {
                                let id = job_id.get();
                                let idx = im.idx;
                                // Open the fullscreen viewer at this image's position.
                                let open = move |_| {
                                    if let Some(p) = imgs.get().iter().position(|m| m.idx == idx) {
                                        viewer.set(Some(p));
                                    }
                                };
                                view! {
                                    <div class="gallery-item">
                                        <button class="thumb-btn" on:click=open>
                                            <img src=format!("/thumb/{id}/{idx}") loading="lazy" alt=""/>
                                        </button>
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

            <Show when=move || viewer.get().is_some()>
                <ImageViewer job_id=job_id.get() images=imgs open=viewer/>
            </Show>
        </div>
    }
}

/// Fullscreen image viewer: scroll to zoom, drag to pan, arrow keys to switch
/// images, Escape (or the backdrop / close button) to dismiss. Mounted only
/// while open, so its window-level key listener lives exactly as long as it.
#[component]
fn ImageViewer(
    job_id: i64,
    images: Signal<Vec<ImageMeta>>,
    open: RwSignal<Option<usize>>,
) -> impl IntoView {
    let scale = RwSignal::new(1.0_f64);
    let tx = RwSignal::new(0.0_f64);
    let ty = RwSignal::new(0.0_f64);
    // Last cursor position while a drag is in progress (None = not dragging).
    let drag = RwSignal::new(None::<(f64, f64)>);
    // Whether the current press moved — so a click that ends a pan doesn't
    // also count as a backdrop "click to close".
    let moved = RwSignal::new(false);

    // Closures capture only Copy signals, so they're Copy and reusable across
    // every handler (keyboard + buttons) without cloning.
    let reset = move || {
        scale.set(1.0);
        tx.set(0.0);
        ty.set(0.0);
        drag.set(None);
    };
    let go = move |delta: i32| {
        let n = images.get().len();
        if n == 0 {
            return;
        }
        if let Some(p) = open.get() {
            let next = (p as i32 + delta).rem_euclid(n as i32) as usize;
            open.set(Some(next));
            reset();
        }
    };
    let close = move || open.set(None);

    // Arrow keys navigate, Escape closes. Removed when the viewer unmounts.
    let handle = window_event_listener(ev::keydown, move |e: ev::KeyboardEvent| {
        match e.key().as_str() {
            "Escape" => close(),
            "ArrowLeft" => {
                e.prevent_default();
                go(-1);
            }
            "ArrowRight" => {
                e.prevent_default();
                go(1);
            }
            _ => {}
        }
    });
    on_cleanup(move || handle.remove());

    let src = move || {
        open.get()
            .and_then(|p| images.get().get(p).map(|im| im.idx))
            .map(|idx| format!("/img/{job_id}/{idx}"))
            .unwrap_or_default()
    };
    let caption = move || {
        let n = images.get().len();
        open.get()
            .map(|p| format!("{} / {n}", p + 1))
            .unwrap_or_default()
    };
    let style = move || {
        format!(
            "transform: translate({}px, {}px) scale({});",
            tx.get(),
            ty.get(),
            scale.get()
        )
    };

    let on_wheel = move |e: ev::WheelEvent| {
        e.prevent_default();
        let factor = if e.delta_y() < 0.0 { 1.15 } else { 1.0 / 1.15 };
        let next = (scale.get() * factor).clamp(1.0, 10.0);
        scale.set(next);
        // Snap back to centered when fully zoomed out.
        if next <= 1.0 {
            tx.set(0.0);
            ty.set(0.0);
        }
    };
    let on_down = move |e: ev::MouseEvent| {
        e.prevent_default();
        moved.set(false);
        drag.set(Some((e.client_x() as f64, e.client_y() as f64)));
    };
    let on_move = move |e: ev::MouseEvent| {
        if let Some((lx, ly)) = drag.get() {
            let (x, y) = (e.client_x() as f64, e.client_y() as f64);
            tx.update(|v| *v += x - lx);
            ty.update(|v| *v += y - ly);
            drag.set(Some((x, y)));
            moved.set(true);
        }
    };
    let end_drag = move |_| drag.set(None);
    // Click on the backdrop closes — but not the click that finishes a drag.
    let on_backdrop_click = move |_| {
        if !moved.get() {
            close();
        }
    };

    view! {
        <div
            class="viewer"
            on:wheel=on_wheel
            on:mousedown=on_down
            on:mousemove=on_move
            on:mouseup=end_drag
            on:mouseleave=end_drag
            on:click=on_backdrop_click
        >
            <button
                class="viewer-close"
                on:click=move |e: ev::MouseEvent| { e.stop_propagation(); close(); }
            >"\u{2715}"</button>
            <button
                class="viewer-nav prev"
                on:click=move |e: ev::MouseEvent| { e.stop_propagation(); go(-1); }
            >"\u{2039}"</button>
            <button
                class="viewer-nav next"
                on:click=move |e: ev::MouseEvent| { e.stop_propagation(); go(1); }
            >"\u{203a}"</button>
            <img
                class="viewer-img"
                src=src
                style=style
                draggable="false"
                alt=""
                on:click=move |e: ev::MouseEvent| e.stop_propagation()
            />
            <div class="viewer-cap">{caption}</div>
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
