pub mod brave;
pub mod duckduckgo;
pub mod google_cse;
pub mod serpapi;

use async_trait::async_trait;

use crate::config::Config;
use crate::error::AppError;

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub url: String,
    pub title: String,
    pub snippet: String,
}

/// Abstraction over "search the web for this text and give me back
/// url/title/snippet hits". Swap in a different backend (Brave, DuckDuckGo,
/// Google, SerpApi, a self-hosted index, X's own search) by implementing this trait.
#[async_trait]
pub trait WebSearchProvider: Send + Sync {
    async fn search(&self, query: &str) -> Result<Vec<SearchResult>, AppError>;
}

/// Used when search is explicitly disabled or no provider is desired.
pub struct NoopSearchProvider;

#[async_trait]
impl WebSearchProvider for NoopSearchProvider {
    async fn search(&self, _query: &str) -> Result<Vec<SearchResult>, AppError> {
        Ok(vec![])
    }
}

/// Picks the search backend based on configured credentials:
/// 1. Google CSE (if GOOGLE_CSE_API_KEY + GOOGLE_CSE_CX set) - 100 free queries/day
/// 2. Brave Search (if BRAVE_SEARCH_API_KEY set) - 2,000 free queries/month
/// 3. SerpApi (if SERPAPI_API_KEY set)
/// 4. DuckDuckGo (Free fallback - requires NO API keys or credit card)
pub fn build_provider(config: &Config) -> Box<dyn WebSearchProvider> {
    if let (Some(key), Some(cx)) = (&config.google_cse_api_key, &config.google_cse_cx) {
        tracing::info!("web search: using Google Programmable Search Engine");
        return Box::new(google_cse::GoogleCseProvider::new(key.clone(), cx.clone()));
    }
    if let Some(key) = &config.brave_search_api_key {
        tracing::info!("web search: using Brave Search API");
        return Box::new(brave::BraveSearchProvider::new(key.clone()));
    }
    if let Some(key) = &config.serpapi_api_key {
        tracing::info!("web search: using SerpApi");
        return Box::new(serpapi::SerpApiProvider::new(key.clone()));
    }
    if config.disable_search {
        tracing::warn!("web search explicitly disabled - using no-op provider");
        return Box::new(NoopSearchProvider);
    }

    tracing::info!("web search: using DuckDuckGo free provider (zero cost, no API keys required)");
    Box::new(duckduckgo::DuckDuckGoProvider::new())
}

