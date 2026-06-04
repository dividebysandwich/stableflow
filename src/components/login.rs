use leptos::prelude::*;
use leptos_router::hooks::use_query_map;

use crate::api::auth_status;

#[component]
pub fn LoginPage() -> impl IntoView {
    let query = use_query_map();
    let has_error = move || query.read().get("error").is_some();

    // On an empty system the first login creates the admin account, so the
    // copy/labels change accordingly.
    let status = Resource::new(|| (), |_| async move { auth_status().await });
    let needs_setup = move || {
        status
            .get()
            .and_then(|r| r.ok())
            .map(|s| s.needs_setup)
            .unwrap_or(false)
    };

    view! {
        <div class="login-wrap">
            <form class="login-card" method="post" action="/login">
                <h1>"StableFlow"</h1>
                <Transition>
                    {move || {
                        if needs_setup() {
                            view! {
                                <p class="muted">
                                    "No accounts yet \u{2014} create the administrator account."
                                </p>
                            }
                                .into_any()
                        } else {
                            view! {
                                <p class="muted">"Sign in to continue."</p>
                            }
                                .into_any()
                        }
                    }}
                </Transition>
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
                    {move || if needs_setup() { "Create admin" } else { "Log in" }}
                </button>
            </form>
        </div>
    }
}
