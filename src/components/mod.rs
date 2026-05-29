pub mod gallery;
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
