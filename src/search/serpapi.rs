use async_trait::async_trait;
use serde::Deserialize;

use super::{SearchResult, WebSearchProvider};
use crate::error::AppError;

/// https://serpapi.com/search-api
pub struct SerpApiProvider {
    api_key: String,
    http: reqwest::Client,
}

impl SerpApiProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            http: reqwest::Client::new(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct SerpApiResponse {
    organic_results: Option<Vec<OrganicResult>>,
}

#[derive(Debug, Deserialize)]
struct OrganicResult {
    title: Option<String>,
    link: Option<String>,
    snippet: Option<String>,
}

#[async_trait]
impl WebSearchProvider for SerpApiProvider {
    async fn search(&self, query: &str) -> Result<Vec<SearchResult>, AppError> {
        let resp = self
            .http
            .get("https://serpapi.com/search.json")
            .query(&[
                ("engine", "google"),
                ("q", query),
                ("api_key", self.api_key.as_str()),
            ])
            .send()
            .await
            .map_err(|e| AppError::Http(format!("serpapi request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(AppError::Http(format!("serpapi error {status}: {body}")));
        }

        let parsed: SerpApiResponse = resp
            .json()
            .await
            .map_err(|e| AppError::Http(format!("serpapi response parse failed: {e}")))?;

        Ok(parsed
            .organic_results
            .unwrap_or_default()
            .into_iter()
            .filter_map(|r| {
                Some(SearchResult {
                    url: r.link?,
                    title: r.title.unwrap_or_default(),
                    snippet: r.snippet.unwrap_or_default(),
                })
            })
            .collect())
    }
}
