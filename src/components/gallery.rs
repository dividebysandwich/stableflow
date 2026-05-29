use std::collections::HashSet;
use std::time::Duration;

use leptos::ev;
use leptos::html;
use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::use_params_map;

use crate::api::{delete_image, delete_images, get_job, get_job_images};
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
    let del_multi = Action::new(|input: &(i64, Vec<i64>)| {
        let (job_id, idxs) = input.clone();
        async move { delete_images(job_id, idxs).await }
    });

    let job = Resource::new(
        move || (job_id.get(), tick.get()),
        |(id, _)| async move { get_job(id).await.ok().flatten() },
    );
    let images = Resource::new(
        // Refetch on each poll tick and immediately after any deletion.
        move || {
            (
                job_id.get(),
                tick.get(),
                del_img.version().get(),
                del_multi.version().get(),
            )
        },
        |(id, ..)| async move { get_job_images(id).await.unwrap_or_default() },
    );

    // Always-available view of the image list, plus which image (by stable
    // idx, not list position) the fullscreen viewer is showing, if any.
    // Tracking by idx keeps deletion/navigation correct even while a running
    // job appends new images and shifts list positions underneath us.
    let imgs = Signal::derive(move || images.get().unwrap_or_default());
    let viewer = RwSignal::new(None::<i64>);

    // idx values marked (via checkboxes) for batch deletion.
    let selected = RwSignal::new(HashSet::<i64>::new());
    let sel_count = move || selected.get().len();
    let select_all = move |_| selected.set(imgs.get().iter().map(|im| im.idx).collect());
    let select_none = move |_| selected.set(HashSet::new());
    let delete_selected = move |_| {
        let ids: Vec<i64> = selected.get().iter().copied().collect();
        if ids.is_empty() {
            return;
        }
        if crate::components::confirm(&format!(
            "Delete {} selected image(s)? This cannot be undone.",
            ids.len()
        )) {
            del_multi.dispatch((job_id.get(), ids));
            selected.set(HashSet::new());
        }
    };
    // Drop selections whose images no longer exist (deleted elsewhere / by
    // polling), so the "(N)" count stays truthful. Only writes when needed.
    Effect::new(move |_| {
        let existing: HashSet<i64> = imgs.get().iter().map(|im| im.idx).collect();
        let has_stale = selected.with_untracked(|s| s.iter().any(|i| !existing.contains(i)));
        if has_stale {
            selected.update(|s| s.retain(|i| existing.contains(i)));
        }
    });

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
                    <div class="gallery-tools">
                        <button class="link-btn" on:click=select_all>"Select all"</button>
                        <button class="link-btn" on:click=select_none>"Select none"</button>
                        <button
                            class="link-btn danger"
                            disabled=move || sel_count() == 0
                            on:click=delete_selected
                        >
                            {move || format!("Delete selected ({})", sel_count())}
                        </button>
                    </div>
                    <div class="gallery-grid">
                        <For
                            each=move || imgs.get()
                            key=|im| (im.idx, im.seed)
                            children=move |im| {
                                let id = job_id.get();
                                let idx = im.idx;
                                // Open the fullscreen viewer on this image.
                                let open = move |_| viewer.set(Some(idx));
                                let is_sel = move || selected.with(|s| s.contains(&idx));
                                let toggle = move |_| {
                                    selected.update(|s| {
                                        if !s.insert(idx) {
                                            s.remove(&idx);
                                        }
                                    });
                                };
                                view! {
                                    <div class="gallery-item" class:selected=is_sel>
                                        <label class="select-box" title="Mark for deletion">
                                            <input
                                                type="checkbox"
                                                prop:checked=is_sel
                                                on:change=toggle
                                            />
                                        </label>
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
                <ImageViewer job_id=job_id.get() images=imgs open=viewer delete=del_img/>
            </Show>
        </div>
    }
}

/// Clamp one axis of the pan offset so the scaled image never reveals empty
/// space past its edge. With center-origin scaling the image extends
/// `±(scale*image - container)/2` from center; if it's smaller than the
/// container on this axis it stays centered (offset 0).
fn clamp_axis(offset: f64, scale: f64, image: f64, container: f64) -> f64 {
    let overflow = scale * image - container;
    if overflow <= 0.0 {
        0.0
    } else {
        offset.clamp(-overflow / 2.0, overflow / 2.0)
    }
}

/// Distance between two tracked pointers `(id, x, y)`.
fn pointer_dist(a: (i32, f64, f64), b: (i32, f64, f64)) -> f64 {
    ((a.1 - b.1).powi(2) + (a.2 - b.2).powi(2)).sqrt()
}

/// Minimum horizontal travel (px) of a one-finger gesture to count as a swipe.
const SWIPE_PX: f64 = 50.0;

/// Fullscreen image viewer. Desktop: scroll to zoom (toward cursor), drag to
/// pan, arrow keys to switch, Delete to remove, Escape/backdrop/✕ to close.
/// Touch: one-finger swipe to switch images, pinch to zoom, drag to pan.
/// `open` holds the *idx* of the shown image (stable across list changes).
#[component]
fn ImageViewer(
    job_id: i64,
    images: Signal<Vec<ImageMeta>>,
    open: RwSignal<Option<i64>>,
    delete: Action<(i64, i64), Result<(), ServerFnError>>,
) -> impl IntoView {
    let scale = RwSignal::new(1.0_f64);
    let tx = RwSignal::new(0.0_f64);
    let ty = RwSignal::new(0.0_f64);
    // Anchor of the in-progress one-pointer pan (None = not panning).
    let drag = RwSignal::new(None::<(f64, f64)>);
    // Whether the current gesture moved — so the click that ends a drag/swipe
    // doesn't also count as a backdrop "click to close".
    let moved = RwSignal::new(false);
    // Active pointers (id, x, y) — unifies mouse + touch and enables pinch.
    let pointers = RwSignal::new(Vec::<(i32, f64, f64)>::new());
    // Previous two-finger distance during a pinch.
    let pinch_prev = RwSignal::new(None::<f64>);
    // Start x of a one-finger swipe at scale 1 (touch only; None = not armed).
    let swipe_x = RwSignal::new(None::<f64>);

    // Element handles, used to read live geometry for zoom-to-cursor and pan
    // clamping. The container fills the viewport (inset:0), so client coords
    // are already container-relative.
    let view_ref = NodeRef::<html::Div>::new();
    let img_ref = NodeRef::<html::Img>::new();
    // (image_w, image_h, container_w, container_h) at the current layout —
    // image dims are the untransformed layout box (offset_*), unaffected by
    // the CSS transform we apply.
    let geom = move || -> Option<(f64, f64, f64, f64)> {
        let img = img_ref.get_untracked()?;
        let cont = view_ref.get_untracked()?;
        Some((
            img.offset_width() as f64,
            img.offset_height() as f64,
            cont.client_width() as f64,
            cont.client_height() as f64,
        ))
    };

    // Closures capture only Copy signals, so they're Copy and reusable across
    // every handler (keyboard + buttons) without cloning.
    let reset = move || {
        scale.set(1.0);
        tx.set(0.0);
        ty.set(0.0);
        drag.set(None);
    };
    // 0-based position of the shown image within the current list.
    let cur_pos = move || {
        open.get()
            .and_then(|cur| images.get().iter().position(|m| m.idx == cur))
    };
    let go = move |delta: i32| {
        let list = images.get();
        let n = list.len();
        if n == 0 {
            return;
        }
        let Some(pos) = open.get().and_then(|cur| list.iter().position(|m| m.idx == cur)) else {
            return;
        };
        let np = (pos as i32 + delta).rem_euclid(n as i32) as usize;
        open.set(Some(list[np].idx));
        reset();
    };
    let close = move || open.set(None);
    let del_current = move || {
        let Some(cur) = open.get() else { return };
        if !crate::components::confirm("Delete this image? This cannot be undone.") {
            return;
        }
        // Advance to a neighbour by *idx* before dispatching, so we never rely
        // on positional indices (which shift as a running job appends images).
        let list = images.get();
        if let Some(pos) = list.iter().position(|m| m.idx == cur) {
            let next = list
                .get(pos + 1)
                .or_else(|| pos.checked_sub(1).and_then(|p| list.get(p)));
            open.set(next.map(|m| m.idx)); // None (was the only image) → closes
            reset();
        }
        delete.dispatch((job_id, cur));
    };

    // If the shown image vanishes (deleted elsewhere, or not yet in a fresh
    // fetch), jump to the nearest remaining image by idx — or close if none.
    Effect::new(move |_| {
        let list = images.get();
        let Some(cur) = open.get_untracked() else { return };
        if list.is_empty() {
            open.set(None);
        } else if !list.iter().any(|m| m.idx == cur) {
            let next = list
                .iter()
                .map(|m| m.idx)
                .filter(|&i| i > cur)
                .min()
                .or_else(|| list.iter().map(|m| m.idx).max());
            open.set(next);
            reset();
        }
    });

    // Arrow keys navigate, Delete removes the current image, Escape closes.
    // Removed when the viewer unmounts.
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
            "Delete" => {
                e.prevent_default();
                del_current();
            }
            _ => {}
        }
    });
    on_cleanup(move || handle.remove());

    // Lock background scroll while open so the page can't shift under the fixed
    // overlay on mobile. Toggled via a body class; removed on unmount.
    #[cfg(target_arch = "wasm32")]
    {
        let set_lock = |on: bool| {
            if let Some(body) = leptos::prelude::document().body() {
                let list = body.class_list();
                let _ = if on {
                    list.add_1("viewer-open")
                } else {
                    list.remove_1("viewer-open")
                };
            }
        };
        set_lock(true);
        on_cleanup(move || set_lock(false));
    }

    let src = move || {
        open.get()
            .map(|idx| format!("/img/{job_id}/{idx}"))
            .unwrap_or_default()
    };
    let caption = move || {
        let n = images.get().len();
        cur_pos()
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

    // Zoom to `ns`, keeping the screen point (mx, my) fixed. The image is
    // centered, so its on-screen center is (cw/2 + tx, ch/2 + ty); scaling is
    // about that center and translation is in screen px. Shared by wheel + pinch.
    let zoom_to = move |ns: f64, mx: f64, my: f64| {
        let Some((iw, ih, cw, ch)) = geom() else { return };
        let s = scale.get();
        if s <= 0.0 {
            return;
        }
        let cx = cw / 2.0 + tx.get();
        let cy = ch / 2.0 + ty.get();
        let ntx = mx - (ns / s) * (mx - cx) - cw / 2.0;
        let nty = my - (ns / s) * (my - cy) - ch / 2.0;
        scale.set(ns);
        tx.set(clamp_axis(ntx, ns, iw, cw));
        ty.set(clamp_axis(nty, ns, ih, ch));
    };

    let on_wheel = move |e: ev::WheelEvent| {
        e.prevent_default();
        let s = scale.get();
        let factor = if e.delta_y() < 0.0 { 1.15 } else { 1.0 / 1.15 };
        let ns = (s * factor).clamp(1.0, 10.0);
        if (ns - s).abs() >= f64::EPSILON {
            zoom_to(ns, e.client_x() as f64, e.client_y() as f64);
        }
    };

    let on_pdown = move |e: ev::PointerEvent| {
        let id = e.pointer_id();
        let (x, y) = (e.client_x() as f64, e.client_y() as f64);
        pointers.update(|v| {
            v.retain(|p| p.0 != id);
            v.push((id, x, y));
        });
        moved.set(false);
        if pointers.with_untracked(|v| v.len()) >= 2 {
            // Two fingers down → start a pinch, suspend pan/swipe.
            drag.set(None);
            swipe_x.set(None);
            let (a, b) = pointers.with_untracked(|v| (v[0], v[1]));
            pinch_prev.set(Some(pointer_dist(a, b)));
        } else {
            drag.set(Some((x, y)));
            // Arm a swipe only for touch at scale 1 (mouse uses arrows/wheel).
            let armed = e.pointer_type() == "touch" && (scale.get() - 1.0).abs() < f64::EPSILON;
            swipe_x.set(armed.then_some(x));
        }
    };
    let on_pmove = move |e: ev::PointerEvent| {
        let id = e.pointer_id();
        let (x, y) = (e.client_x() as f64, e.client_y() as f64);
        let n = pointers.with_untracked(|v| v.len());
        pointers.update(|v| {
            if let Some(p) = v.iter_mut().find(|p| p.0 == id) {
                p.1 = x;
                p.2 = y;
            }
        });
        if n >= 2 {
            // Pinch: scale by the ratio of the two-finger distance, toward the
            // midpoint between the fingers.
            let (a, b) = pointers.with_untracked(|v| (v[0], v[1]));
            let d = pointer_dist(a, b);
            if let Some(prev) = pinch_prev.get_untracked() {
                if prev > 0.0 {
                    let ns = (scale.get() * (d / prev)).clamp(1.0, 10.0);
                    zoom_to(ns, (a.1 + b.1) / 2.0, (a.2 + b.2) / 2.0);
                }
            }
            pinch_prev.set(Some(d));
            moved.set(true);
        } else if let Some((lx, ly)) = drag.get_untracked() {
            if (x - lx).abs() + (y - ly).abs() > 2.0 {
                moved.set(true);
            }
            // Pan only when zoomed in (otherwise a one-finger move is a swipe).
            if scale.get() > 1.0 {
                if let Some((iw, ih, cw, ch)) = geom() {
                    let s = scale.get();
                    tx.set(clamp_axis(tx.get() + (x - lx), s, iw, cw));
                    ty.set(clamp_axis(ty.get() + (y - ly), s, ih, ch));
                }
            }
            drag.set(Some((x, y)));
        }
    };
    let on_pup = move |e: ev::PointerEvent| {
        let id = e.pointer_id();
        let x = e.client_x() as f64;
        let before = pointers.with_untracked(|v| v.len());
        pointers.update(|v| v.retain(|p| p.0 != id));
        let after = pointers.with_untracked(|v| v.len());
        // A one-finger lift at scale 1 with enough horizontal travel = swipe.
        if before == 1 && (scale.get() - 1.0).abs() < f64::EPSILON {
            if let Some(sx) = swipe_x.get_untracked() {
                let dx = x - sx;
                if dx <= -SWIPE_PX {
                    go(1);
                } else if dx >= SWIPE_PX {
                    go(-1);
                }
            }
        }
        if after < 2 {
            pinch_prev.set(None);
        }
        swipe_x.set(None);
        if after == 0 {
            drag.set(None);
        } else {
            // Resume single-pointer pan with whatever finger remains.
            let p = pointers.with_untracked(|v| v[0]);
            drag.set(Some((p.1, p.2)));
        }
    };
    let on_pcancel = move |e: ev::PointerEvent| {
        let id = e.pointer_id();
        pointers.update(|v| v.retain(|p| p.0 != id));
        let after = pointers.with_untracked(|v| v.len());
        if after < 2 {
            pinch_prev.set(None);
        }
        if after == 0 {
            drag.set(None);
        }
        swipe_x.set(None);
    };
    // Click on the backdrop closes — but not the click that ends a drag/swipe.
    let on_backdrop_click = move |_| {
        if !moved.get() {
            close();
        }
    };

    view! {
        <div
            class="viewer"
            node_ref=view_ref
            on:wheel=on_wheel
            on:pointerdown=on_pdown
            on:pointermove=on_pmove
            on:pointerup=on_pup
            on:pointercancel=on_pcancel
            on:pointerleave=on_pcancel
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
                node_ref=img_ref
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
