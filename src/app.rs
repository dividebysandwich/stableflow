use leptos::prelude::*;
use leptos_meta::{provide_meta_context, MetaTags, Stylesheet, Title};
use leptos_router::components::{Route, Router, Routes, A};
use leptos_router::hooks::use_location;
use leptos_router::path;

use crate::api::current_user;
use crate::components::admin::AdminPage;
use crate::components::favorites::FavoritesPage;
use crate::components::gallery::JobDetailPage;
use crate::components::inpaint::InpaintPage;
use crate::components::job_form::NewJobPage;
use crate::components::job_list::JobsPage;
use crate::components::login::LoginPage;
use crate::models::CurrentUser;

/// The authenticated user, shared app-wide via context. The resource resolves to
/// `None` while loading or when unauthenticated (e.g. on the login page).
#[derive(Clone, Copy)]
pub struct CurrentUserCtx(pub Resource<Option<CurrentUser>>);

/// HTML document shell used for SSR + hydration.
pub fn shell(options: LeptosOptions) -> impl IntoView {
    view! {
        <!DOCTYPE html>
        <html lang="en">
            <head>
                <meta charset="utf-8"/>
                <meta name="viewport" content="width=device-width, initial-scale=1"/>
                <AutoReload options=options.clone()/>
                <HydrationScripts options/>
                <MetaTags/>
            </head>
            <body>
                <App/>
            </body>
        </html>
    }
}

#[component]
pub fn App() -> impl IntoView {
    provide_meta_context();

    // Resolve the current user once and share it via context (navbar admin link,
    // inpaint source-image URLs).
    let user = Resource::new(|| (), |_| async move { current_user().await.ok() });
    provide_context(CurrentUserCtx(user));

    view! {
        <Stylesheet id="leptos" href="/pkg/stableflow.css"/>
        <Title text="StableFlow"/>

        <Router>
            <NavBar/>
            <main class="content">
                <Routes fallback=|| view! { <p class="pad">"Page not found."</p> }>
                    <Route path=path!("/") view=JobsPage/>
                    <Route path=path!("/new") view=NewJobPage/>
                    <Route path=path!("/job/:id") view=JobDetailPage/>
                    <Route path=path!("/inpaint/new") view=InpaintPage/>
                    <Route path=path!("/inpaint/:id") view=InpaintPage/>
                    <Route path=path!("/favorites") view=FavoritesPage/>
                    <Route path=path!("/admin") view=AdminPage/>
                    <Route path=path!("/login") view=LoginPage/>
                </Routes>
            </main>
        </Router>
    }
}

/// Top navigation. Hidden on the login page (the only route reachable while
/// unauthenticated), so the unauthenticated view shows no app links.
#[component]
fn NavBar() -> impl IntoView {
    let location = use_location();
    let show = move || location.pathname.get() != "/login";

    let user = expect_context::<CurrentUserCtx>().0;
    // Whether to show the admin link. It stays `false` for SSR *and* the first
    // hydration pass so the server and client produce identical DOM (no
    // hydration mismatch); a client-only effect then fills it in from the
    // current-user resource, reactively revealing the link for admins.
    let is_admin = RwSignal::new(false);
    Effect::new(move |_| {
        if let Some(Some(u)) = user.get() {
            is_admin.set(u.is_admin);
        }
    });

    let (menu_open, set_menu_open) = signal(false);
    let toggle = move |_| set_menu_open.update(|o| *o = !*o);
    let close = move |_| set_menu_open.set(false);
    // Collapse the menu automatically whenever the route changes.
    Effect::new(move |_| {
        location.pathname.track();
        set_menu_open.set(false);
    });

    view! {
        <Show when=show>
            <nav class="topnav">
                <A href="/">"StableFlow"</A>
                <button
                    class="hamburger"
                    aria-label="Menu"
                    aria-expanded=move || menu_open.get().to_string()
                    on:click=toggle
                >
                    <span></span><span></span><span></span>
                </button>
                <div class="navlinks" class:open=move || menu_open.get()>
                    <A href="/new" on:click=close attr:class="navlink">"+ New job"</A>
                    <A href="/" on:click=close attr:class="navlink">"Queue & history"</A>
                    <A href="/favorites" on:click=close attr:class="navlink">"\u{2605} Favorites"</A>
                    {move || is_admin.get().then(|| view! {
                        <A href="/admin" on:click=close attr:class="navlink">"Admin"</A>
                    })}
                    <form method="post" action="/logout" class="logout-form">
                        <button type="submit" class="link-btn">"Logout"</button>
                    </form>
                </div>
            </nav>
        </Show>
    }
}
