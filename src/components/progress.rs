use std::time::Duration;

use leptos::prelude::*;

use crate::api::get_running_progress;

/// Live progress banner for the currently-running job. Polls the server every
/// 1.5s on the client.
#[component]
pub fn RunningProgressBar() -> impl IntoView {
    let (tick, set_tick) = signal(0u32);

    // Client-only polling loop.
    Effect::new(move |_| {
        set_interval(
            move || set_tick.update(|t| *t = t.wrapping_add(1)),
            Duration::from_millis(1500),
        );
    });

    let progress = Resource::new(
        move || tick.get(),
        |_| async move { get_running_progress().await.unwrap_or_default() },
    );

    view! {
        // Transition keeps the banner visible across polls (no flash to fallback).
        <Transition>
            {move || {
                progress.get().map(|rp| {
                    match rp.job_id {
                        Some(id) => {
                            let pct = (rp.progress * 100.0).clamp(0.0, 100.0);
                            view! {
                                <div class="running-banner">
                                    <div class="running-head">
                                        <strong>"Running: "</strong>
                                        {if rp.job_name.is_empty() {
                                            format!("Job #{id}")
                                        } else {
                                            format!("{} (#{id})", rp.job_name)
                                        }}
                                        <span class="pct">{format!("{pct:.0}%")}</span>
                                    </div>
                                    <div class="bar-track">
                                        <div class="bar-fill" style:width=move || format!("{pct}%")></div>
                                    </div>
                                </div>
                            }.into_any()
                        }
                        None if rp.busy_with_other => view! {
                            <div class="running-banner busy-other">
                                <div class="running-head">
                                    <strong>"Busy with other jobs"</strong>
                                    <span class="muted">" \u{2014} the queue is processing another user's request."</span>
                                </div>
                            </div>
                        }.into_any(),
                        None => view! { <div class="idle-banner">"Queue idle \u{2014} no job running."</div> }.into_any(),
                    }
                })
            }}
        </Transition>
    }
}
