mod aggregate;
mod claim_detector;
mod config;
mod error;
mod handlers;
mod llm_judge;
mod models;
mod pipeline;
mod search;
mod similarity;
mod store;
mod x_client;

use std::sync::Arc;

use axum::routing::{get, post};
use axum::Router;

use config::Config;
use search::WebSearchProvider;
use store::Store;

/// Shared state handed to every handler.
pub struct AppState {
    pub config: Config,
    pub http: reqwest::Client,
    pub search: Box<dyn WebSearchProvider>,
    pub store: Store,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "originality_checker=info,tower_http=info".into()),
        )
        .init();

    let config = Config::from_env()?;
    let port = config.port;
    let search_provider = search::build_provider(&config);

    if config.x_bearer_token.is_none() {
        tracing::warn!(
            "X_BEARER_TOKEN is not set - POST /accounts/:handle/analyze (live fetch) will \
             return a config error until it's set. /import still works for backfills."
        );
    }

    let state = Arc::new(AppState {
        http: reqwest::Client::new(),
        search: search_provider,
        store: Store::new(),
        config,
    });

    let app = Router::new()
        .route("/", get(handlers::ui_handler))
        .route("/health", get(handlers::health_handler))
        .route("/verify", post(handlers::verify_draft_handler))
        .route("/check", post(handlers::verify_draft_handler))
        .route("/accounts/:handle/analyze", post(handlers::analyze_handler))
        .route("/accounts/:handle/import", post(handlers::import_handler))
        .route("/accounts/:handle/analyses", get(handlers::list_analyses_handler))
        .route("/accounts/:handle/report", get(handlers::report_handler))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await?;
    tracing::info!("originality-checker listening on http://0.0.0.0:{port}");
    axum::serve(listener, app).await?;

    Ok(())
}
