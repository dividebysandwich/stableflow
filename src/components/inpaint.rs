//! Iterative inpainting editor (`/inpaint/new` and `/inpaint/:id`).
//!
//! A turn = paint a mask (and, in sketch mode, colored strokes) over a base
//! image, tweak the diffusion params, and Generate. Server-side this is a Forge
//! img2img call; the mask-vs-sketch difference lives entirely here in the
//! browser — we compose the init image and the mask and upload both as base64.
//! The first Generate lazily creates the job; later turns append to it.

use std::time::Duration;

use leptos::ev;
use leptos::html;
use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::{use_params_map, use_query_map};

use crate::api::{
    create_inpaint_job, delete_image, get_form_options, get_job, get_job_images, get_job_params,
    run_inpaint_turn,
};
use crate::components::progress::RunningProgressBar;
use crate::models::{FormOptions, InpaintMode, InpaintParams, JobParams, ModelType};

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::{closure::Closure, JsCast};

// ---------------------------------------------------------------------------
// Canvas engine — all real work is wasm-only; non-wasm builds get no-op stubs
// so the shared component compiles for SSR.
// ---------------------------------------------------------------------------

// Client-only: a `LocalStorage` StoredValue holds a `SendWrapper`. Creating one
// during SSR (multi-threaded) and dropping it on another worker thread panics
// ("Dropped SendWrapper from a different thread"), so this type and the store
// exist only on wasm.
#[cfg(target_arch = "wasm32")]
type EngineStore = StoredValue<EngineData, LocalStorage>;

#[cfg(target_arch = "wasm32")]
#[derive(Default)]
struct EngineData {
    /// Clean base image pixels (the init in mask mode; the undo source).
    base: Option<web_sys::HtmlCanvasElement>,
    /// Black background, white where painted — exported directly as the mask.
    mask: Option<web_sys::HtmlCanvasElement>,
    /// Kept alive so the image onload callback isn't dropped mid-load.
    _img: Option<web_sys::HtmlImageElement>,
    _onload: Option<Closure<dyn FnMut()>>,
    /// (visible, mask) ImageData snapshots for undo.
    undo: Vec<(web_sys::ImageData, web_sys::ImageData)>,
    painting: bool,
    last: Option<(f64, f64)>,
    strokes: u32,
}

#[cfg(target_arch = "wasm32")]
mod engine {
    use super::*;

    fn ctx(c: &web_sys::HtmlCanvasElement) -> web_sys::CanvasRenderingContext2d {
        c.get_context("2d").unwrap().unwrap().dyn_into().unwrap()
    }

    fn make_canvas(w: u32, h: u32) -> web_sys::HtmlCanvasElement {
        let c: web_sys::HtmlCanvasElement = leptos::prelude::document()
            .create_element("canvas")
            .unwrap()
            .dyn_into()
            .unwrap();
        c.set_width(w);
        c.set_height(h);
        c
    }

    fn client_to_canvas(el: &web_sys::HtmlCanvasElement, cx: f64, cy: f64) -> (f64, f64) {
        let rect = el.get_bounding_client_rect();
        let sx = if rect.width() > 0.0 { el.width() as f64 / rect.width() } else { 1.0 };
        let sy = if rect.height() > 0.0 { el.height() as f64 / rect.height() } else { 1.0 };
        ((cx - rect.left()) * sx, (cy - rect.top()) * sy)
    }

    fn stroke(c: &web_sys::CanvasRenderingContext2d, a: (f64, f64), b: (f64, f64), r: f64, style: &str) {
        c.set_line_cap("round");
        c.set_line_join("round");
        c.set_line_width(r * 2.0);
        c.set_stroke_style_str(style);
        c.begin_path();
        c.move_to(a.0, a.1);
        c.line_to(b.0, b.1);
        c.stroke();
    }

    fn to_b64(c: &web_sys::HtmlCanvasElement) -> String {
        let url = c.to_data_url_with_type("image/png").unwrap_or_default();
        url.split_once(',').map(|(_, b)| b.to_string()).unwrap_or(url)
    }

    fn draw_seg(
        el: &web_sys::HtmlCanvasElement,
        eng: EngineStore,
        mode: InpaintMode,
        brush: f64,
        color: &str,
        a: (f64, f64),
        b: (f64, f64),
    ) {
        let vctx = ctx(el);
        eng.with_value(|d| {
            if let Some(mask) = &d.mask {
                stroke(&ctx(mask), a, b, brush, "#ffffff");
            }
        });
        match mode {
            InpaintMode::Mask => stroke(&vctx, a, b, brush, "rgba(255,0,64,0.5)"),
            InpaintMode::Sketch => stroke(&vctx, a, b, brush, color),
        }
        eng.update_value(|d| d.strokes = d.strokes.saturating_add(1));
    }

    pub fn load(canvas: NodeRef<html::Canvas>, eng: EngineStore, url: String, wsig: RwSignal<u32>, hsig: RwSignal<u32>) {
        let img = match web_sys::HtmlImageElement::new() {
            Ok(i) => i,
            Err(_) => return,
        };
        img.set_cross_origin(Some("anonymous"));
        let img2 = img.clone();
        let cb = Closure::<dyn FnMut()>::new(move || {
            let Some(el) = canvas.get_untracked() else { return };
            let w = img2.natural_width().max(1);
            let h = img2.natural_height().max(1);
            el.set_width(w);
            el.set_height(h);
            let base = make_canvas(w, h);
            let mask = make_canvas(w, h);
            let _ = ctx(&base).draw_image_with_html_image_element(&img2, 0.0, 0.0);
            let _ = ctx(&el).draw_image_with_html_image_element(&img2, 0.0, 0.0);
            let mctx = ctx(&mask);
            mctx.set_fill_style_str("#000000");
            mctx.fill_rect(0.0, 0.0, w as f64, h as f64);
            eng.update_value(|d| {
                d.base = Some(base);
                d.mask = Some(mask);
                d.undo.clear();
                d.strokes = 0;
                d.last = None;
                d.painting = false;
            });
            wsig.set(w);
            hsig.set(h);
        });
        img.set_onload(Some(cb.as_ref().unchecked_ref()));
        img.set_src(&url);
        eng.update_value(|d| {
            d._img = Some(img);
            d._onload = Some(cb);
        });
    }

    pub fn down(canvas: NodeRef<html::Canvas>, eng: EngineStore, mode: InpaintMode, brush: f64, color: String, cx: f64, cy: f64) {
        let Some(el) = canvas.get_untracked() else { return };
        let p = client_to_canvas(&el, cx, cy);
        eng.update_value(|d| {
            d.painting = true;
            d.last = Some(p);
        });
        draw_seg(&el, eng, mode, brush, &color, p, p);
    }

    pub fn mv(canvas: NodeRef<html::Canvas>, eng: EngineStore, mode: InpaintMode, brush: f64, color: String, cx: f64, cy: f64) {
        let Some(el) = canvas.get_untracked() else { return };
        let prev = eng.with_value(|d| if d.painting { d.last } else { None });
        let Some(prev) = prev else { return };
        let p = client_to_canvas(&el, cx, cy);
        draw_seg(&el, eng, mode, brush, &color, prev, p);
        eng.update_value(|d| d.last = Some(p));
    }

    pub fn up(canvas: NodeRef<html::Canvas>, eng: EngineStore) {
        let Some(el) = canvas.get_untracked() else { return };
        let was = eng.with_value(|d| d.painting);
        if !was {
            return;
        }
        // Snapshot visible + mask for undo (capped).
        let vd = ctx(&el)
            .get_image_data(0.0, 0.0, el.width() as f64, el.height() as f64)
            .ok();
        eng.update_value(|d| {
            d.painting = false;
            d.last = None;
            if let (Some(vd), Some(mask)) = (vd, &d.mask) {
                if let Ok(md) = ctx(mask).get_image_data(0.0, 0.0, mask.width() as f64, mask.height() as f64) {
                    d.undo.push((vd, md));
                    if d.undo.len() > 8 {
                        d.undo.remove(0);
                    }
                }
            }
        });
    }

    pub fn clear(canvas: NodeRef<html::Canvas>, eng: EngineStore) {
        let Some(el) = canvas.get_untracked() else { return };
        eng.update_value(|d| {
            if let Some(base) = &d.base {
                let _ = ctx(&el).draw_image_with_html_canvas_element(base, 0.0, 0.0);
            }
            if let Some(mask) = &d.mask {
                let mctx = ctx(mask);
                mctx.set_fill_style_str("#000000");
                mctx.fill_rect(0.0, 0.0, mask.width() as f64, mask.height() as f64);
            }
            d.undo.clear();
            d.strokes = 0;
            d.last = None;
            d.painting = false;
        });
    }

    pub fn undo(canvas: NodeRef<html::Canvas>, eng: EngineStore) {
        let Some(el) = canvas.get_untracked() else { return };
        eng.update_value(|d| {
            if let Some((vd, md)) = d.undo.pop() {
                let _ = ctx(&el).put_image_data(&vd, 0.0, 0.0);
                if let Some(mask) = &d.mask {
                    let _ = ctx(mask).put_image_data(&md, 0.0, 0.0);
                }
            } else if let Some(base) = &d.base {
                // Nothing left: reset to the clean base.
                let _ = ctx(&el).draw_image_with_html_canvas_element(base, 0.0, 0.0);
                if let Some(mask) = &d.mask {
                    let mctx = ctx(mask);
                    mctx.set_fill_style_str("#000000");
                    mctx.fill_rect(0.0, 0.0, mask.width() as f64, mask.height() as f64);
                }
                d.strokes = 0;
            }
        });
    }

    pub fn export(canvas: NodeRef<html::Canvas>, eng: EngineStore, mode: InpaintMode) -> Option<(String, String)> {
        let el = canvas.get_untracked()?;
        let mask_b64 = eng.with_value(|d| d.mask.as_ref().map(to_b64))?;
        let init_b64 = match mode {
            InpaintMode::Mask => eng.with_value(|d| d.base.as_ref().map(to_b64))?,
            InpaintMode::Sketch => to_b64(&el),
        };
        Some((init_b64, mask_b64))
    }

    // Available for gating Generate on a non-empty mask; not currently wired.
    #[allow(dead_code)]
    pub fn has_strokes(eng: EngineStore) -> bool {
        eng.with_value(|d| d.strokes > 0)
    }

    pub fn capture_pointer(canvas: NodeRef<html::Canvas>, pointer_id: i32) {
        if let Some(el) = canvas.get_untracked() {
            let _ = el.set_pointer_capture(pointer_id);
        }
    }
}

// ---------------------------------------------------------------------------
// Page
// ---------------------------------------------------------------------------

#[component]
pub fn InpaintPage() -> impl IntoView {
    let params_map = use_params_map();
    let query = use_query_map();

    let existing_id = Memo::new(move |_| {
        params_map
            .read()
            .get("id")
            .and_then(|s| s.parse::<i64>().ok())
    });
    let src_job = Memo::new(move |_| {
        query.read().get("src_job").and_then(|s| s.parse::<i64>().ok()).unwrap_or(0)
    });
    let src_idx = Memo::new(move |_| {
        query.read().get("src_idx").and_then(|s| s.parse::<i64>().ok()).unwrap_or(0)
    });
    let init_mode = Memo::new(move |_| InpaintMode::from_str(&query.read().get("mode").unwrap_or_default()));

    let options = Resource::new(|| (), |_| async move { get_form_options().await.unwrap_or_default() });
    let job = Resource::new(
        move || existing_id.get(),
        |id| async move {
            match id {
                Some(id) => get_job(id).await.ok().flatten(),
                None => None,
            }
        },
    );
    // For a fresh session, prefill prompts/params from the source image's job.
    let src_params = Resource::new(
        move || (existing_id.get(), src_job.get()),
        |(eid, sj)| async move {
            if eid.is_none() && sj > 0 {
                get_job_params(sj).await.ok().flatten()
            } else {
                None
            }
        },
    );

    view! {
        <div class="page">
            <div class="page-head">
                <h1>"Inpainting"</h1>
                <A href="/" attr:class="link-btn">"\u{2190} Back"</A>
            </div>
            <RunningProgressBar/>
            // Transition (not Suspense): the editor polls `images` every 3s, and a
            // Suspense would re-show its fallback on every poll — unmounting and
            // remounting the editor (restarting the canvas load, stacking intervals).
            // Transition keeps the editor mounted across those refetches.
            <Transition fallback=|| view! { <p class="muted">"Loading\u{2026}"</p> }>
                {move || {
                    let Some(opts) = options.get() else { return ().into_any() };
                    let eid = existing_id.get();
                    // Build the editor only once the params it needs are loaded, so
                    // it mounts a single time (no remount churn / duplicate polling).
                    let (init_job, base0) = if eid.is_some() {
                        match job.get() {
                            Some(Some(j)) => {
                                let p = j.params.clone();
                                let (sj, si) = p.inpaint.as_ref().map(|i| (i.src_job, i.src_idx)).unwrap_or((j.id, 0));
                                (p, (sj, si))
                            }
                            Some(None) => return view! { <p class="error">"Job not found."</p> }.into_any(),
                            None => return ().into_any(),
                        }
                    } else {
                        match src_params.get() {
                            Some(opt) => (opt.unwrap_or_default(), (src_job.get(), src_idx.get())),
                            None => return ().into_any(),
                        }
                    };
                    view! {
                        <InpaintEditor
                            options=opts
                            initial=init_job
                            existing_id=eid
                            base0=base0
                            init_mode=init_mode.get()
                        />
                    }
                    .into_any()
                }}
            </Transition>
        </div>
    }
}

/// Pick `current` if it's a valid option, else the first option.
fn pick(current: &str, opts: &[String]) -> String {
    if opts.iter().any(|o| o == current) || opts.is_empty() {
        current.to_string()
    } else {
        opts.first().cloned().unwrap_or_default()
    }
}

#[component]
fn InpaintEditor(
    options: FormOptions,
    initial: JobParams,
    existing_id: Option<i64>,
    base0: (i64, i64),
    init_mode: InpaintMode,
) -> impl IntoView {
    let ip = initial.inpaint.clone().unwrap_or_default();
    let mode_seed = if existing_id.is_some() { ip.mode } else { init_mode };

    // ---- form signals ----
    let model_type = RwSignal::new(initial.model_type.as_str().to_string());
    let checkpoint = RwSignal::new(pick(&initial.checkpoint, &options.checkpoints));
    let sampler = RwSignal::new(pick(&initial.sampler_name, &options.samplers));
    let scheduler = RwSignal::new(pick(&initial.scheduler, &options.schedulers));
    let prompt = RwSignal::new(initial.prompt.clone());
    let negative = RwSignal::new(initial.negative_prompt.clone());
    let steps = RwSignal::new(initial.steps.to_string());
    let cfg = RwSignal::new(initial.cfg_scale.to_string());
    let dcfg = RwSignal::new(initial.distilled_cfg_scale.to_string());
    let seed = RwSignal::new(initial.seed.to_string());
    let batch = RwSignal::new(initial.batch_size.max(1).to_string());

    let denoise = RwSignal::new(ip.denoising_strength.to_string());
    let mask_blur = RwSignal::new(ip.mask_blur.to_string());
    let fill = RwSignal::new(ip.inpainting_fill.to_string());
    let full_res = RwSignal::new(ip.inpaint_full_res);
    let padding = RwSignal::new(ip.inpaint_full_res_padding.to_string());
    let mask_invert = RwSignal::new(ip.mask_invert != 0);

    // ---- editor signals ----
    let mode = RwSignal::new(mode_seed);
    let brush = RwSignal::new(40.0_f64);
    let color = RwSignal::new("#ffffff".to_string());
    let native_w = RwSignal::new(0u32);
    let native_h = RwSignal::new(0u32);
    let base = RwSignal::new(base0);
    let job_id = RwSignal::new(existing_id);

    let canvas_ref = NodeRef::<html::Canvas>::new();
    // The canvas engine store is client-only (see EngineStore note above).
    #[cfg(target_arch = "wasm32")]
    let eng: EngineStore = StoredValue::new_local(EngineData::default());

    // (Re)load the base image whenever it changes (initial mount + base switch).
    // Client-only: the canvas store is !Send, and effects don't run during SSR.
    // The source image is always one of the current user's own gallery images, so
    // its file URL uses the current user's UUID (resolved from context).
    #[cfg(target_arch = "wasm32")]
    {
        let user = expect_context::<crate::app::CurrentUserCtx>().0;
        Effect::new(move |_| {
            let (j, i) = base.get();
            let uuid = user.get().flatten().map(|u| u.uuid).unwrap_or_default();
            if uuid.is_empty() {
                return; // wait until the current user resolves
            }
            engine::load(canvas_ref, eng, format!("/u/{uuid}/img/{j}/{i}"), native_w, native_h);
        });
    }

    // ---- pointer handlers ----
    let on_down = move |e: ev::PointerEvent| {
        #[cfg(target_arch = "wasm32")]
        {
            engine::capture_pointer(canvas_ref, e.pointer_id());
            engine::down(canvas_ref, eng, mode.get(), brush.get(), color.get(), e.client_x() as f64, e.client_y() as f64);
        }
        #[cfg(not(target_arch = "wasm32"))]
        let _ = e;
    };
    let on_move = move |e: ev::PointerEvent| {
        #[cfg(target_arch = "wasm32")]
        engine::mv(canvas_ref, eng, mode.get(), brush.get(), color.get(), e.client_x() as f64, e.client_y() as f64);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = e;
    };
    let on_up = move |_e: ev::PointerEvent| {
        #[cfg(target_arch = "wasm32")]
        engine::up(canvas_ref, eng);
    };
    let do_clear = move |_| {
        #[cfg(target_arch = "wasm32")]
        engine::clear(canvas_ref, eng);
    };
    let do_undo = move |_| {
        #[cfg(target_arch = "wasm32")]
        engine::undo(canvas_ref, eng);
    };

    // ---- actions ----
    let create_act = Action::new(|input: &(JobParams, String, String)| {
        let (p, init_b64, mask_b64) = input.clone();
        async move { create_inpaint_job(String::new(), p, init_b64, mask_b64).await }
    });
    let turn_act = Action::new(|input: &(i64, JobParams, String, String)| {
        let (id, p, init_b64, mask_b64) = input.clone();
        async move { run_inpaint_turn(id, p, init_b64, mask_b64).await }
    });

    // After lazy creation, adopt the new job id in place. We deliberately do NOT
    // navigate to /inpaint/{id}: a route change would remount the editor, restart
    // the results poll, and make the page visibly churn after the first turn.
    Effect::new(move |_| {
        if let Some(Ok(id)) = create_act.value().get() {
            if job_id.get_untracked().is_none() {
                job_id.set(Some(id));
            }
        }
    });

    let build_params = move || -> JobParams {
        let w = native_w.get_untracked();
        let h = native_h.get_untracked();
        JobParams {
            model_type: ModelType::from_str(&model_type.get()),
            checkpoint: checkpoint.get(),
            prompt: prompt.get(),
            negative_prompt: negative.get(),
            styles: Vec::new(),
            steps: steps.get().trim().parse().unwrap_or(20),
            cfg_scale: cfg.get().trim().parse().unwrap_or(7.0),
            distilled_cfg_scale: dcfg.get().trim().parse().unwrap_or(3.5),
            width: if w > 0 { w } else { initial.width },
            height: if h > 0 { h } else { initial.height },
            batch_size: batch.get().trim().parse::<u32>().unwrap_or(1).max(1),
            n_iter: 1,
            sampler_name: sampler.get(),
            scheduler: scheduler.get(),
            seed: seed.get().trim().parse().unwrap_or(-1),
            enable_hr: false,
            hr_upscaler: initial.hr_upscaler.clone(),
            hr_scale: initial.hr_scale,
            hr_second_pass_steps: initial.hr_second_pass_steps,
            denoising_strength: initial.denoising_strength,
            inpaint: Some(InpaintParams {
                mode: mode.get(),
                denoising_strength: denoise.get().trim().parse().unwrap_or(0.75),
                mask_blur: mask_blur.get().trim().parse().unwrap_or(4),
                inpainting_fill: fill.get().trim().parse().unwrap_or(1),
                inpaint_full_res: full_res.get(),
                inpaint_full_res_padding: padding.get().trim().parse().unwrap_or(32),
                mask_invert: if mask_invert.get() { 1 } else { 0 },
                init_path: String::new(),
                mask_path: String::new(),
                src_job: base0.0,
                src_idx: base0.1,
            }),
        }
    };

    let pending = move || create_act.pending().get() || turn_act.pending().get();

    let on_generate = move |_| {
        let exported: Option<(String, String)> = {
            #[cfg(target_arch = "wasm32")]
            {
                engine::export(canvas_ref, eng, mode.get())
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                None
            }
        };
        let Some((init_b64, mask_b64)) = exported else {
            crate::components::alert("Could not read the canvas.");
            return;
        };
        let p = build_params();
        match job_id.get() {
            Some(id) => {
                turn_act.dispatch((id, p, init_b64, mask_b64));
            }
            None => {
                create_act.dispatch((p, init_b64, mask_b64));
            }
        }
    };

    // ---- results polling ----
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
    let images = Resource::new(
        move || (job_id.get(), tick.get(), turn_act.version().get(), del_img.version().get()),
        |(id, ..)| async move {
            match id {
                Some(id) => get_job_images(id).await.unwrap_or_default(),
                None => Vec::new(),
            }
        },
    );
    let imgs = Signal::derive(move || images.get().unwrap_or_default());

    // Auto-advance: when a newer result appears, make it the base (clears strokes
    // via the reload effect). Also fires on first load when resuming a job.
    let last_max = RwSignal::new(-1_i64);
    Effect::new(move |_| {
        let v = imgs.get();
        let Some(mx) = v.iter().map(|m| m.idx).max() else { return };
        if mx > last_max.get_untracked() {
            last_max.set(mx);
            if let Some(jid) = job_id.get_untracked() {
                base.set((jid, mx));
            }
        }
    });

    let checkpoints = options.checkpoints.clone();
    let samplers = options.samplers.clone();
    let schedulers = options.schedulers.clone();

    view! {
        <div class="inpaint">
            <div class="inpaint-canvas-pane">
                <div class="brush-bar">
                    <div class="seg">
                        <button
                            class="link-btn" class:on=move || mode.get() == InpaintMode::Mask
                            on:click=move |_| mode.set(InpaintMode::Mask)
                        >"Mask"</button>
                        <button
                            class="link-btn" class:on=move || mode.get() == InpaintMode::Sketch
                            on:click=move |_| mode.set(InpaintMode::Sketch)
                        >"Sketch"</button>
                    </div>
                    <label class="inline">
                        "Brush"
                        <input type="range" min="2" max="200" prop:value=move || brush.get().to_string()
                            on:input=move |ev| brush.set(event_target_value(&ev).parse().unwrap_or(40.0))/>
                    </label>
                    <Show when=move || mode.get() == InpaintMode::Sketch>
                        <label class="inline">
                            "Color"
                            <input type="color" prop:value=move || color.get()
                                on:input=move |ev| color.set(event_target_value(&ev))/>
                        </label>
                    </Show>
                    <button class="link-btn" on:click=do_undo>"Undo"</button>
                    <button class="link-btn" on:click=do_clear>"Clear"</button>
                    <button class="link-btn" title="Load the original source image as the base"
                        on:click=move |_| base.set(base0)>"Original"</button>
                </div>
                <div class="canvas-wrap">
                    <canvas
                        node_ref=canvas_ref
                        class="inpaint-canvas"
                        on:pointerdown=on_down
                        on:pointermove=on_move
                        on:pointerup=on_up
                        on:pointerleave=on_up
                        on:pointercancel=on_up
                    ></canvas>
                </div>
                <p class="muted">
                    {move || match mode.get() {
                        InpaintMode::Mask => "Paint over the area to regenerate from the prompt.",
                        InpaintMode::Sketch => "Paint colored strokes; the painted area regenerates guided by them.",
                    }}
                </p>
            </div>

            <div class="inpaint-params">
                <div class="form-grid">
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
                        "Steps"
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
                            "Distilled CFG"
                            <input type="number" step="0.1" prop:value=move || dcfg.get()
                                on:input=move |ev| dcfg.set(event_target_value(&ev))/>
                        </label>
                    </Show>
                    <label>
                        "Denoising strength"
                        <input type="number" step="0.01" min="0" max="1" prop:value=move || denoise.get()
                            on:input=move |ev| denoise.set(event_target_value(&ev))/>
                    </label>
                    <label>
                        "Mask blur"
                        <input type="number" min="0" prop:value=move || mask_blur.get()
                            on:input=move |ev| mask_blur.set(event_target_value(&ev))/>
                    </label>
                    <label>
                        "Masked content"
                        <select prop:value=move || fill.get()
                            on:change=move |ev| fill.set(event_target_value(&ev))>
                            <option value="0">"fill"</option>
                            <option value="1">"original"</option>
                            <option value="2">"latent noise"</option>
                            <option value="3">"latent nothing"</option>
                        </select>
                    </label>
                    <label>
                        "Inpaint padding"
                        <input type="number" min="0" prop:value=move || padding.get()
                            on:input=move |ev| padding.set(event_target_value(&ev))/>
                    </label>
                    <label>
                        "Variations"
                        <input type="number" min="1" max="8" prop:value=move || batch.get()
                            on:input=move |ev| batch.set(event_target_value(&ev))/>
                    </label>
                    <label>
                        "Seed (-1 = random)"
                        <input type="number" prop:value=move || seed.get()
                            on:input=move |ev| seed.set(event_target_value(&ev))/>
                    </label>
                    <label class="inline">
                        <input type="checkbox" prop:checked=move || full_res.get()
                            on:change=move |ev| full_res.set(event_target_checked(&ev))/>
                        "Only masked (full res)"
                    </label>
                    <label class="inline">
                        <input type="checkbox" prop:checked=move || mask_invert.get()
                            on:change=move |ev| mask_invert.set(event_target_checked(&ev))/>
                        "Invert mask"
                    </label>
                </div>

                <label class="full">
                    "Prompt"
                    <textarea rows="3" prop:value=move || prompt.get()
                        on:input=move |ev| prompt.set(event_target_value(&ev))></textarea>
                </label>
                <label class="full">
                    "Negative prompt"
                    <textarea rows="2" prop:value=move || negative.get()
                        on:input=move |ev| negative.set(event_target_value(&ev))></textarea>
                </label>

                <div class="form-actions">
                    <button on:click=on_generate disabled=move || pending() || native_w.get() == 0>
                        {move || if pending() { "Generating\u{2026}" } else { "Generate" }}
                    </button>
                </div>
            </div>

            <div class="inpaint-results">
                <h2>"Results"</h2>
                <Show
                    when=move || !imgs.get().is_empty()
                    fallback=|| view! { <p class="muted">"No results yet \u{2014} paint a mask and Generate."</p> }
                >
                    <div class="result-strip">
                        <For
                            each=move || imgs.get()
                            key=|im| (im.idx, im.seed)
                            children=move |im| {
                                let jid = im.job_id;
                                let idx = im.idx;
                                let uuid = im.owner_uuid.clone();
                                let uuid_inputs = uuid.clone();
                                let use_base = move |_| base.set((jid, idx));
                                view! {
                                    <div class="result-item">
                                        <button class="thumb-btn" title="Use as base"
                                            on:click=use_base>
                                            <img src=format!("/u/{uuid}/thumb/{jid}/{idx}") loading="lazy" alt=""/>
                                        </button>
                                        <div class="gallery-cap">
                                            <span class="muted">{format!("seed {}", im.seed)}</span>
                                            <span class="cap-actions">
                                                <Show when=move || im.has_inputs>
                                                    <a href=format!("/u/{uuid_inputs}/input/{jid}/{idx}/mask") target="_blank">"mask"</a>
                                                    <a href=format!("/u/{uuid_inputs}/input/{jid}/{idx}/init") target="_blank">"init"</a>
                                                </Show>
                                                <a href=format!("/u/{uuid}/download/img/{jid}/{idx}")>"download"</a>
                                                <button class="del-btn"
                                                    on:click=move |_| {
                                                        if crate::components::confirm("Delete this image? This cannot be undone.") {
                                                            del_img.dispatch((jid, idx));
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
            </div>
        </div>
    }
}
