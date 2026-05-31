pub mod favorites;
pub mod gallery;
pub mod inpaint;
pub mod job_form;
pub mod job_list;
pub mod login;
pub mod progress;

/// Native browser confirmation dialog. Only meaningful on the wasm client
/// (where click handlers actually run); on the server it's a no-op that
/// returns `true` so shared view code compiles for both targets.
pub fn confirm(message: &str) -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        leptos::prelude::window()
            .confirm_with_message(message)
            .unwrap_or(true)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = message;
        true
    }
}

/// Native browser alert dialog. No-op off the wasm client (see [`confirm`]).
pub fn alert(message: &str) {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = leptos::prelude::window().alert_with_message(message);
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = message;
    }
}
