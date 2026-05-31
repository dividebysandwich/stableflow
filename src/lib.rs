// Leptos nests view/async types deeply enough (especially the inpaint editor)
// to exceed the default type-recursion/layout depth limit.
#![recursion_limit = "512"]

pub mod api;
pub mod app;
pub mod components;
pub mod models;

#[cfg(feature = "ssr")]
pub mod server;

#[cfg(feature = "hydrate")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn hydrate() {
    use crate::app::App;
    console_error_panic_hook::set_once();
    leptos::mount::hydrate_body(App);
}
