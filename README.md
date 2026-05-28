# StableFlow

A Rust + [Leptos](https://leptos.dev) web app that sits in front of a
**Stable Diffusion Forge** instance and gives you a clean, externally-reachable UI for
**queueing txt2img jobs** and **collecting their results**.

- Persistent, restart-resilient job queue (one job dispatched at a time).
- Full parameter form: diffusion model (SD / SDXL / Flux), checkpoint, sampling steps,
  CFG + distilled CFG, width/height, batch size & count, sampler, schedule type, prompt /
  negative prompt, styles, and the hires-fix upscaler.
- Reload any past job's parameters as a template for a new job.
- Persistent job history with live status & progress.
- Local image gallery: thumbnails in the history, full per-job galleries, and downloads of
  individual images or a whole job as a `.zip`.
- Single-password login (sessions persisted in SQLite) gating the externally-exposed UI.

## Prerequisites

- Rust (stable) with the `wasm32-unknown-unknown` target:
  `rustup target add wasm32-unknown-unknown`
- [`cargo-leptos`](https://github.com/leptos-rs/cargo-leptos): `cargo install cargo-leptos`
- A **Forge** instance reachable on the LAN, launched **with the REST API enabled**:
  `./webui.sh --api` (StableFlow talks to `/sdapi/v1/*`).

## Configuration (environment variables)

| Variable | Default | Purpose |
|---|---|---|
| `STABLEFLOW_PASSWORD` | `changeme` | Shared login password (set this!). |
| `FORGE_URL` | `http://127.0.0.1:7860` | Base URL of the Forge REST API. |
| `STABLEFLOW_DATA_DIR` | `data` | Where the SQLite DB + image gallery live. |
| `LEPTOS_SITE_ADDR` | `0.0.0.0:3000` | Address StableFlow binds (exposed to the outside). |

The data dir layout:

```
data/stableflow.db          SQLite (jobs, images, sessions)
data/gallery/<job>/<i>.png  full-resolution results
data/thumbs/<job>/<i>.jpg   generated thumbnails
```

## Running

Development (auto-rebuild + reload):

```bash
STABLEFLOW_PASSWORD=secret cargo leptos watch
```

Production build + run:

```bash
cargo leptos build --release
STABLEFLOW_PASSWORD=secret \
FORGE_URL=http://127.0.0.1:7860 \
LEPTOS_SITE_ADDR=0.0.0.0:3000 \
./target/release/stableflow
```

Then open the bound address, log in with the password, and queue a job.

## How it works

- An Axum server serves the SSR + hydrated Leptos UI, the Leptos server-function RPC API
  (`/api/*`), the login routes, and raw image / zip download handlers.
- A single background tokio worker pulls the oldest `queued` job, calls Forge's
  `txt2img` (polling `/progress` for live updates), saves each image + a thumbnail to disk,
  and records metadata in SQLite.
- On startup any job left `running` (e.g. after a crash/restart) is automatically
  re-queued, so you pick up roughly where you left off.

