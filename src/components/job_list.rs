use std::time::Duration;

use leptos::prelude::*;
use leptos_router::components::A;

use crate::api::{cancel_job, delete_job, list_jobs, requeue_job};
use crate::components::progress::RunningProgressBar;
use crate::models::{Job, JobStatus};

fn job_label(job: &Job) -> String {
    if !job.name.trim().is_empty() {
        return job.name.clone();
    }
    let p = job.params.prompt.trim();
    if p.is_empty() {
        format!("Job #{}", job.id)
    } else if p.len() > 60 {
        format!("{}\u{2026}", &p[..60])
    } else {
        p.to_string()
    }
}

#[component]
pub fn JobsPage() -> impl IntoView {
    let (tick, set_tick) = signal(0u32);
    Effect::new(move |_| {
        set_interval(
            move || set_tick.update(|t| *t = t.wrapping_add(1)),
            Duration::from_millis(2500),
        );
    });

    let cancel = Action::new(|id: &i64| {
        let id = *id;
        async move { cancel_job(id).await }
    });
    let requeue = Action::new(|id: &i64| {
        let id = *id;
        async move { requeue_job(id).await }
    });
    let delete = Action::new(|id: &i64| {
        let id = *id;
        async move { delete_job(id).await }
    });

    let jobs = Resource::new(
        move || {
            (
                tick.get(),
                cancel.version().get(),
                requeue.version().get(),
                delete.version().get(),
            )
        },
        |_| async move { list_jobs().await.unwrap_or_default() },
    );

    // Client-side pagination, 10 jobs per page. The list is already fully
    // fetched (and polled), so we just slice it.
    const PER_PAGE: usize = 10;
    let all_jobs = Signal::derive(move || jobs.get().unwrap_or_default());
    let page = RwSignal::new(0usize);
    let page_count = move || all_jobs.get().len().div_ceil(PER_PAGE).max(1);
    // Clamp the stored page in case the job count shrank (e.g. deletions).
    let cur_page = move || page.get().min(page_count() - 1);
    let page_jobs = move || {
        all_jobs
            .get()
            .into_iter()
            .skip(cur_page() * PER_PAGE)
            .take(PER_PAGE)
            .collect::<Vec<_>>()
    };
    let prev = move |_| page.set(cur_page().saturating_sub(1));
    let next = move |_| page.set((cur_page() + 1).min(page_count() - 1));
    // Named to keep comparison operators out of the `view!` macro (where a bare
    // `>` would be parsed as a tag close).
    let many_pages = move || page_count() > 1;
    let at_first = move || cur_page() == 0;
    let at_last = move || cur_page() + 1 >= page_count();

    view! {
        <div class="page">
            <div class="page-head">
                <h1>"Queue & history"</h1>
                <A href="/new" attr:class="btn">"+ New job"</A>
            </div>

            <RunningProgressBar/>

            // Transition (not Suspense) keeps the previous list on screen during
            // each poll, and the keyed <For> only re-renders cards whose
            // (status, image_count) actually changed — so nothing flashes.
            <Transition fallback=|| view! { <p class="muted">"Loading jobs\u{2026}"</p> }>
                <Show
                    when=move || !all_jobs.get().is_empty()
                    fallback=|| view! { <p class="muted">"No jobs yet. Create one to get started."</p> }
                >
                    <div class="job-list">
                        <For
                            each=move || page_jobs()
                            key=|job| (job.id, job.status, job.image_count, job.error.is_some())
                            children=move |job| job_card(job, cancel, requeue, delete)
                        />
                    </div>
                    <Show when=many_pages>
                        <div class="pager">
                            <button class="link-btn" disabled=at_first on:click=prev>
                                "\u{2039} Prev"
                            </button>
                            <span class="muted">
                                {move || format!("Page {} of {}", cur_page() + 1, page_count())}
                            </span>
                            <button class="link-btn" disabled=at_last on:click=next>
                                "Next \u{203a}"
                            </button>
                        </div>
                    </Show>
                </Show>
            </Transition>
        </div>
    }
}

fn job_card(
    job: Job,
    cancel: Action<i64, Result<(), ServerFnError>>,
    requeue: Action<i64, Result<(), ServerFnError>>,
    delete: Action<i64, Result<(), ServerFnError>>,
) -> impl IntoView {
    let id = job.id;
    let status = job.status;
    let label = job_label(&job);
    let status_str = status.as_str().to_string();
    // Most-recent images, newest first (already limited to 12 by the query).
    // Uses real idx values so it stays correct after individual deletions leave
    // gaps in the idx sequence. CSS shows as many as fit the row's width and
    // scrolls the rest, so wide screens fill up while narrow ones stay tidy.
    let thumb_idxs = job.thumb_idxs.clone();
    let has_images = job.image_count > 0;

    let can_cancel = matches!(status, JobStatus::Queued | JobStatus::Running);
    let can_requeue = matches!(
        status,
        JobStatus::Failed | JobStatus::Canceled | JobStatus::Completed
    );

    view! {
        <div class=format!("job-card status-{status_str}")>
            <div class="job-row">
                <span class=format!("badge badge-{status_str}")>{status_str.clone()}</span>
                <span class="jid">{format!("#{id}")}</span>
                <span class="jlabel">{label}</span>
                <span class="jtime muted">{job.created_at.clone()}</span>
            </div>

            {job.error.clone().map(|e| view! { <div class="error">{e}</div> })}

            <Show when=move || has_images>
                <div class="thumb-strip">
                    {thumb_idxs.iter().map(|&idx| view! {
                        <a href=format!("/job/{id}") class="thumb">
                            <img src=format!("/thumb/{id}/{idx}") loading="lazy" alt=""/>
                        </a>
                    }).collect_view()}
                </div>
            </Show>

            <div class="actions">
                <A href=format!("/job/{id}")>"View gallery"</A>
                <A href=format!("/new?from={id}")>"Reload as template"</A>
                <Show when=move || has_images>
                    <a href=format!("/download/job/{id}")>"Download zip"</a>
                </Show>
                <Show when=move || can_cancel>
                    <button class="link-btn" on:click=move |_| { cancel.dispatch(id); }>"Cancel"</button>
                </Show>
                <Show when=move || can_requeue>
                    <button class="link-btn" on:click=move |_| { requeue.dispatch(id); }>"Re-queue"</button>
                </Show>
                <button
                    class="link-btn danger"
                    on:click=move |_| {
                        if crate::components::confirm(
                            "Delete this job and all its images? This cannot be undone."
                        ) {
                            delete.dispatch(id);
                        }
                    }
                >"Delete"</button>
            </div>
        </div>
    }
}
