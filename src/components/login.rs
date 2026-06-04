use leptos::prelude::*;
use leptos_router::hooks::use_query_map;

use crate::api::auth_status;

#[component]
pub fn LoginPage() -> impl IntoView {
    let query = use_query_map();
    let has_error = move || query.read().get("error").is_some();

    // On an empty system the first login creates the admin account, so the
    // copy/labels change accordingly. This is resolved by a client-only effect
    // (not read in the SSR view tree) so the server and first hydration pass
    // render identical DOM — avoiding a hydration mismatch — then it updates.
    let status = Resource::new(|| (), |_| async move { auth_status().await });
    let needs_setup = RwSignal::new(false);
    Effect::new(move |_| {
        if let Some(Ok(s)) = status.get() {
            needs_setup.set(s.needs_setup);
        }
    });

    view! {
        <div class="login-wrap">
            <form class="login-card" method="post" action="/login">
                <h1>"StableFlow"</h1>
                <p class="muted">
                    {move || if needs_setup.get() {
                        "No accounts yet \u{2014} create the administrator account."
                    } else {
                        "Sign in to continue."
                    }}
                </p>
                <Show when=has_error>
                    <p class="error">"Incorrect username or password."</p>
                </Show>
                <input
                    type="text"
                    name="username"
                    placeholder="Username"
                    autocomplete="username"
                    autofocus
                />
                <input
                    type="password"
                    name="password"
                    placeholder="Password"
                    autocomplete="current-password"
                />
                <button type="submit">
                    {move || if needs_setup.get() { "Create admin" } else { "Log in" }}
                </button>
            </form>
        </div>
    }
}
