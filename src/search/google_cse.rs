use async_trait::async_trait;
use serde::Deserialize;

use super::{SearchResult, WebSearchProvider};
use crate::error::AppError;

/// https://developers.google.com/custom-search/v1/overview
pub struct GoogleCseProvider {
    api_key: String,
    cx: String,
    http: reqwest::Client,
}

impl GoogleCseProvider {
    pub fn new(api_key: String, cx: String) -> Self {
        Self {
            api_key,
            cx,
            http: reqwest::Client::new(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct CseResponse {
    items: Option<Vec<CseItem>>,
}

#[derive(Debug, Deserialize)]
struct CseItem {
    title: String,
    link: String,
    snippet: Option<String>,
}

#[async_trait]
impl WebSearchProvider for GoogleCseProvider {
    async fn search(&self, query: &str) -> Result<Vec<SearchResult>, AppError> {
        let resp = self
            .http
            .get("https://www.googleapis.com/customsearch/v1")
            .query(&[
                ("key", self.api_key.as_str()),
                ("cx", self.cx.as_str()),
                ("q", query),
            ])
            .send()
            .await
            .map_err(|e| AppError::Http(format!("google cse request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(AppError::Http(format!(
                "google cse error {status}: {body}"
            )));
        }

        let parsed: CseResponse = resp
            .json()
            .await
            .map_err(|e| AppError::Http(format!("google cse response parse failed: {e}")))?;

        Ok(parsed
            .items
            .unwrap_or_default()
            .into_iter()
            .map(|i| SearchResult {
                url: i.link,
                title: i.title,
                snippet: i.snippet.unwrap_or_default(),
            })
            .collect())
    }
}
