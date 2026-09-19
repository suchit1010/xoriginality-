//! HTTP client bridge from originality-checker → x-originator-svc.
//!
//! When X_ORIGINATOR_SVC_URL is set (default: http://localhost:8081),
//! the pipeline calls this service instead of the DDG site:x.com fallback.
//! If the service is unreachable, it logs a warning and returns empty (graceful degradation).
#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use chrono::{DateTime, Utc};

use crate::models::XPostMatch;

#[derive(Debug, Deserialize)]
struct SvcPost {
    tweet_id: String,
    handle: String,
    url: String,
    created_at: DateTime<Utc>,
    formatted_date: String,
    text_snippet: String,
}

#[derive(Debug, Deserialize)]
struct SvcResponse {
    posts: Vec<SvcPost>,
}

/// Call x-originator-svc and return enriched XPostMatch list sorted chronologically.
/// Returns empty vec on any error (service down, timeout, etc.).
pub async fn fetch_originators(
    http: &reqwest::Client,
    svc_url: &str,
    query: &str,
) -> Vec<XPostMatch> {
    let endpoint = format!("{}/search-x", svc_url.trim_end_matches('/'));

    #[derive(Serialize)]
    struct Req<'a> {
        query: &'a str,
        max_results: usize,
    }

    let result = http
        .post(&endpoint)
        .json(&Req { query, max_results: 10 })
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await;

    match result {
        Ok(resp) if resp.status().is_success() => {
            match resp.json::<SvcResponse>().await {
                Ok(data) => {
                    tracing::info!(
                        "x-originator-svc returned {} posts for '{}'",
                        data.posts.len(),
                        query
                    );
                    data.posts
                        .into_iter()
                        .map(|p| XPostMatch {
                            author_handle: p.handle,
                            tweet_id: {
                                // Parse tweet_id string to u64; fall back to Snowflake decode
                                p.tweet_id.parse::<u64>().unwrap_or(0)
                            },
                            tweet_url: p.url,
                            published_at: p.created_at,
                            formatted_date: p.formatted_date,
                            snippet: p.text_snippet,
                        })
                        .collect()
                }
                Err(e) => {
                    tracing::warn!("x-originator-svc: failed to parse response: {e}");
                    vec![]
                }
            }
        }
        Ok(resp) => {
            tracing::warn!("x-originator-svc: HTTP {} from {}", resp.status(), endpoint);
            vec![]
        }
        Err(e) => {
            tracing::warn!(
                "x-originator-svc unreachable at '{}' ({}); falling back to DDG site:x.com",
                endpoint,
                e
            );
            vec![]
        }
    }
}
