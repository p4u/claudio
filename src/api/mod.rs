//! OpenAI-compatible API server (`claudio --api`).
//!
//! Exposes `POST /v1/chat/completions` (streaming + non-streaming) and
//! `GET /v1/models`. Point any OpenAI client at it and your calls are served by
//! your local `claude` install, using whatever auth it already has.
//!
//! Every request is resolved through a pool of persistent *interactive* `claude`
//! sessions (see [`pool`]). **claudio never invokes `claude -p`.**
//!
//! Module map: `config` (env), `auth` (optional bearer), `error` (OpenAI error
//! envelope), `types` (lenient wire types), `routes::{chat,models}`,
//! `prompt`/`agentic` (request → prompt), `pool` (persistent-session pool),
//! `backend` (pool dispatch + model/usage mapping), `usage` (token accounting),
//! `util` (ids/time).

mod agentic;
mod auth;
mod backend;
mod config;
mod error;
mod pool;
mod prompt;
mod routes;
mod types;
mod usage;
mod util;

use axum::routing::{get, post};
use axum::{Router, middleware};
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

use config::{AppState, Config};

/// Entry point for `claudio --api`. Builds a Tokio runtime, sets up the clean
/// working directory, and serves until terminated. Returns a process exit code.
pub fn serve_blocking() -> std::process::ExitCode {
    // Default to fast prompt delivery for the server: the human-cadence typing
    // (relevant only to single interactive-style turns) would make the long
    // flattened transcripts the API sends take minutes. Opt back in explicitly
    // by setting CLAUDIO_CADENCE=1 before launch.
    if std::env::var("CLAUDIO_CADENCE").is_err() {
        std::env::set_var("CLAUDIO_CADENCE", "0");
    }
    // And strip the remaining human-like delays (quiescence wait, pre-Enter
    // dwell) — the API wants lowest latency, especially for multi-turn agentic
    // loops. Override with CLAUDIO_FAST=0 to restore them.
    if std::env::var("CLAUDIO_FAST").is_err() {
        std::env::set_var("CLAUDIO_FAST", "1");
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,tower_http=info")),
        )
        .init();

    let config = Config::from_env();

    // Run the backend from a clean cwd so the repo's CLAUDE.md / local settings
    // don't leak into every request. Every PTY child inherits this cwd.
    if let Err(e) = std::env::set_current_dir(&config.cwd) {
        tracing::warn!(cwd = %config.cwd.display(), error = %e, "could not set working directory");
    }

    let runtime = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("claudio: failed to start async runtime: {e}");
            return std::process::ExitCode::from(2);
        }
    };

    runtime.block_on(async move { run(config).await })
}

async fn run(config: Config) -> std::process::ExitCode {
    let bind = config.bind;
    tracing::info!(
        %bind,
        cwd = %config.cwd.display(),
        claude_bin = %config.claude_bin,
        default_model = %config.default_model,
        agentic = config.agentic,
        setting_sources = %config.setting_sources,
        auth = config.api_key.is_some(),
        max_concurrency = config.max_concurrency,
        "starting claudio --api (OpenAI-compatible server, PTY backend)"
    );

    let state = AppState::new(config);
    let app = build_router(state);

    let listener = match tokio::net::TcpListener::bind(bind).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("claudio: failed to bind {bind}: {e}");
            return std::process::ExitCode::from(2);
        }
    };

    if let Err(e) = axum::serve(listener, app).await {
        eprintln!("claudio: server error: {e}");
        return std::process::ExitCode::from(2);
    }
    std::process::ExitCode::SUCCESS
}

fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/v1/chat/completions", post(routes::chat::completions))
        .route("/v1/models", get(routes::models::list))
        .route("/v1/models/{id}", get(routes::models::retrieve))
        .route("/health", get(|| async { "ok" }))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_api_key,
        ))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
