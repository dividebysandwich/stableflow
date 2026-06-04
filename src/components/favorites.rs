use std::time::Duration;

use leptos::prelude::*;
use leptos_router::components::A;

use crate::api::{delete_image, list_favorites, set_star};
use crate::components::gallery::{ImageViewer, StarAction};

/// Gallery of every starred image across all jobs. Un-starring removes an image
/// from this list (and makes it deletable again on its job page).
#[component]
pub fn FavoritesPage() -> impl IntoView {
    // Poll occasionally so images starred from another tab/page show up, and
    // refetch immediately whenever something is (un)starred here.
    let (tick, set_tick) = signal(0u32);
    Effect::new(move |_| {
        set_interval(
            move || set_tick.update(|t| *t = t.wrapping_add(1)),
            Duration::from_millis(4000),
        );
    });

    let star: StarAction = Action::new(|input: &(i64, i64, bool)| {
        let (job_id, idx, starred) = *input;
        async move { set_star(job_id, idx, starred).await }
    });
    // The viewer needs a delete action for its signature; favorites are all
    // starred, so the viewer's own guard prevents it ever firing here.
    let del = Action::new(|input: &(i64, i64)| {
        let (job_id, idx) = *input;
        async move { delete_image(job_id, idx).await }
    });

    let favs = Resource::new(
        move || (tick.get(), star.version().get()),
        |_| async move { list_favorites().await.unwrap_or_default() },
    );
    let imgs = Signal::derive(move || favs.get().unwrap_or_default());
    // Which image (by globally-unique id) the fullscreen viewer is showing.
    let viewer = RwSignal::new(None::<i64>);

    view! {
        <div class="page">
            <h1>"Favorites"</h1>
            <Transition fallback=|| view! { <p class="muted">"Loading\u{2026}"</p> }>
                <Show
                    when=move || !imgs.get().is_empty()
                    fallback=|| view! {
                        <p class="muted">"No favorites yet. Star an image to add it here."</p>
                    }
                >
                    <div class="gallery-grid">
                        <For
                            each=move || imgs.get()
                            key=|im| im.id
                            children=move |im| {
                                let img_id = im.id;
                                let job = im.job_id;
                                let idx = im.idx;
                                let uuid = im.owner_uuid.clone();
                                let open = move |_| viewer.set(Some(img_id));
                                let unstar = move |_| { star.dispatch((job, idx, false)); };
                                view! {
                                    <div class="gallery-item starred">
                                        <button
                                            class="star-btn on"
                                            title="Un-star (remove from favorites)"
                                            on:click=unstar
                                        >"\u{2605}"</button>
                                        <button class="thumb-btn" on:click=open>
                                            <img src=format!("/u/{uuid}/thumb/{job}/{idx}") loading="lazy" alt=""/>
                                        </button>
                                        <div class="gallery-cap">
                                            <span class="muted">{format!("seed {}", im.seed)}</span>
                                            <span class="cap-actions">
                                                <a href=format!("/u/{uuid}/download/img/{job}/{idx}")>"download"</a>
                                                <A href=format!("/job/{job}")>"job"</A>
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
                <ImageViewer images=imgs open=viewer delete=del star=star/>
            </Show>
        </div>
    }
}
