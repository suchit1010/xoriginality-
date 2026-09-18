use async_trait::async_trait;
use serde::Deserialize;

use super::{SearchResult, WebSearchProvider};
use crate::error::AppError;

/// Brave Search API provider: https://brave.com/search/api/
/// Free tier offers 2,000 queries per month.
pub struct BraveSearchProvider {
    api_key: String,
    http: reqwest::Client,
}

impl BraveSearchProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            http: reqwest::Client::new(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct BraveResponse {
    web: Option<BraveWeb>,
}

#[derive(Debug, Deserialize)]
struct BraveWeb {
    results: Option<Vec<BraveResultItem>>,
}

#[derive(Debug, Deserialize)]
struct BraveResultItem {
    title: String,
    url: String,
    description: Option<String>,
}

#[async_trait]
impl WebSearchProvider for BraveSearchProvider {
    async fn search(&self, query: &str) -> Result<Vec<SearchResult>, AppError> {
        let resp = self
            .http
            .get("https://api.search.brave.com/res/v1/web/search")
            .query(&[("q", query), ("count", "10")])
            .header("Accept", "application/json")
            .header("X-Subscription-Token", &self.api_key)
            .send()
            .await
            .map_err(|e| AppError::Http(format!("brave search request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(AppError::Http(format!("brave search error {status}: {body}")));
        }

        let parsed: BraveResponse = resp
            .json()
            .await
            .map_err(|e| AppError::Http(format!("brave search parse error: {e}")))?;

        let results = parsed
            .web
            .and_then(|w| w.results)
            .unwrap_or_default()
            .into_iter()
            .map(|r| SearchResult {
                url: r.url,
                title: r.title,
                snippet: r.description.unwrap_or_default(),
            })
            .collect();

        Ok(results)
    }
}
