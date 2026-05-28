use leptos::prelude::*;
use leptos_router::hooks::use_query_map;

#[component]
pub fn LoginPage() -> impl IntoView {
    let query = use_query_map();
    let has_error = move || query.read().get("error").is_some();

    view! {
        <div class="login-wrap">
            <form class="login-card" method="post" action="/login">
                <h1>"StableFlow"</h1>
                <p class="muted">"Enter the access password to continue."</p>
                <Show when=has_error>
                    <p class="error">"Incorrect password."</p>
                </Show>
                <input
                    type="password"
                    name="password"
                    placeholder="Password"
                    autocomplete="current-password"
                    autofocus
                />
                <button type="submit">"Log in"</button>
            </form>
        </div>
    }
}
