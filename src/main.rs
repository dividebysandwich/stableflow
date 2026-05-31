#![recursion_limit = "512"]

#[cfg(feature = "ssr")]
#[tokio::main]
async fn main() {
    use std::path::PathBuf;
    use std::sync::Arc;

    use axum::routing::{get, post};
    use axum::{Extension, Router};
    use leptos::logging::log;
    use leptos::prelude::*;
    use leptos_axum::{generate_route_list, LeptosRoutes};
    use tokio::sync::{Notify, RwLock};

    use stableflow::app::{shell, App};
    use stableflow::server::forge::Forge;
    use stableflow::server::{auth, db, files, worker, AppState};

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,stableflow=info".into()),
        )
        .init();

    let conf = get_configuration(None).unwrap();
    let leptos_options = conf.leptos_options;
    let addr = leptos_options.site_addr;

    // --- Configuration via env ---
    let forge_url =
        std::env::var("FORGE_URL").unwrap_or_else(|_| "http://127.0.0.1:7860".to_string());
    let data_dir = std::env::var("STABLEFLOW_DATA_DIR").unwrap_or_else(|_| "data".to_string());
    let password = std::env::var("STABLEFLOW_PASSWORD").unwrap_or_else(|_| {
        log!("WARNING: STABLEFLOW_PASSWORD not set; using 'changeme'");
        "changeme".to_string()
    });

    std::fs::create_dir_all(&data_dir).expect("create data dir");
    let db_path = format!("{data_dir}/stableflow.db");
    let pool = db::init_pool(&db_path).await.expect("init db");

    // Restart resilience: re-queue any job that was mid-flight.
    match db::reset_running_to_queued(&pool).await {
        Ok(n) if n > 0 => log!("re-queued {n} interrupted job(s)"),
        _ => {}
    }

    let forge = Forge::new(&forge_url);
    log!("fetching Forge options from {forge_url}");
    let options = forge.fetch_form_options().await;
    log!(
        "Forge: {} checkpoints, {} samplers, {} schedulers, {} styles, {} upscalers",
        options.checkpoints.len(),
        options.samplers.len(),
        options.schedulers.len(),
        options.styles.len(),
        options.upscalers.len()
    );

    let state = AppState {
        pool,
        forge,
        data_dir: PathBuf::from(&data_dir),
        notify: Arc::new(Notify::new()),
        options: Arc::new(RwLock::new(options)),
        password,
    };

    tokio::spawn(worker::run(state.clone()));
    state.notify.notify_one();

    let routes = generate_route_list(App);

    let app = Router::<LeptosOptions>::new()
        .route("/login", post(auth::login))
        .route("/logout", post(auth::logout))
        .route("/img/{job}/{idx}", get(files::serve_image))
        .route("/thumb/{job}/{idx}", get(files::serve_thumb))
        .route("/input/{job}/{idx}/{kind}", get(files::serve_input))
        .route("/download/img/{job}/{idx}", get(files::download_image))
        .route("/download/job/{id}", get(files::download_job_zip))
        .leptos_routes_with_context(
            &leptos_options,
            routes,
            {
                let state = state.clone();
                move || provide_context(state.clone())
            },
            {
                let leptos_options = leptos_options.clone();
                move || shell(leptos_options.clone())
            },
        )
        .fallback(leptos_axum::file_and_error_handler(shell))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::require_auth,
        ))
        .layer(Extension(state.clone()))
        .with_state(leptos_options);

    log!("StableFlow listening on http://{addr}");
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app.into_make_service())
        .await
        .unwrap();
}

#[cfg(not(feature = "ssr"))]
fn main() {
    // Client (WASM) entry is `hydrate()` in lib.rs; this binary target is SSR-only.
}
