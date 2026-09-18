use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::error::AppError;
use crate::models::Tweet;

/// Thin wrapper over the X API v2 user-tweets timeline endpoint.
///
/// Note: X's free/basic API tiers only expose a limited recent-tweet
/// window, not full account history - that's why `/import` (fed from the
/// account's official "Download your data" archive) is the recommended path
/// for a true whole-account backfill. This client is meant for the rolling
/// 30-90 day monitoring case, where the timeline endpoint is enough.
pub struct XClient {
    http: reqwest::Client,
    bearer_token: String,
}

impl XClient {
    pub fn new(bearer_token: String) -> Self {
        Self {
            http: reqwest::Client::new(),
            bearer_token,
        }
    }

    /// Resolve a public @username handle to its internal numeric X user ID
    /// via GET /2/users/by/username/:username.
    pub async fn resolve_user_id(&self, username: &str) -> Result<String, AppError> {
        let clean = username.trim_start_matches('@');
        let url = format!("https://api.x.com/2/users/by/username/{clean}");

        let resp = self
            .http
            .get(&url)
            .bearer_auth(&self.bearer_token)
            .send()
            .await
            .map_err(|e| AppError::Http(format!("x user lookup failed for @{clean}: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(AppError::Http(format!("x user lookup error {status}: {body}")));
        }

        let parsed: UserLookupResponse = resp
            .json()
            .await
            .map_err(|e| AppError::Http(format!("x user lookup parse error: {e}")))?;

        parsed
            .data
            .map(|u| u.id)
            .ok_or_else(|| AppError::Http(format!("user @{clean} was not found on X")))
    }

    /// Fetch tweets for an X account, paginating until either the
    /// API stops returning a next page or `since` is reached.
    ///
    /// Requests extended fields (`note_tweet`) to ensure long-form tweets (>280 chars)
    /// have their complete, original text captured rather than a truncated string.
    pub async fn fetch_user_tweets(
        &self,
        user_id: &str,
        handle: &str,
        page_size: u32,
        since: Option<DateTime<Utc>>,
    ) -> Result<Vec<Tweet>, AppError> {
        let mut tweets = Vec::new();
        let mut pagination_token: Option<String> = None;
        let clean_handle = handle.trim_start_matches('@');

        loop {
            let mut url = format!(
                "https://api.x.com/2/users/{user_id}/tweets?max_results={page_size}&tweet.fields=created_at,text,note_tweet&exclude=retweets,replies"
            );
            if let Some(tok) = &pagination_token {
                url.push_str(&format!("&pagination_token={tok}"));
            }
            if let Some(since_ts) = since {
                url.push_str(&format!("&start_time={}", since_ts.to_rfc3339()));
            }

            let resp = self
                .http
                .get(&url)
                .bearer_auth(&self.bearer_token)
                .send()
                .await
                .map_err(|e| AppError::Http(format!("x api request failed: {e}")))?;

            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                return Err(AppError::Http(format!("x api error {status}: {body}")));
            }

            let page: TweetsPage = resp
                .json()
                .await
                .map_err(|e| AppError::Http(format!("x api response parse failed: {e}")))?;

            let got_any = page.data.as_ref().map(|d| !d.is_empty()).unwrap_or(false);

            if let Some(data) = page.data {
                for d in data {
                    // Modern long-form tweets (Articles / Note Tweets) store full text in note_tweet.text
                    let full_text = d.note_tweet.map(|n| n.text).unwrap_or(d.text);

                    tweets.push(Tweet {
                        id: d.id.clone(),
                        author_handle: clean_handle.to_string(),
                        text: full_text,
                        created_at: d.created_at.unwrap_or_else(Utc::now),
                        url: format!("https://x.com/{clean_handle}/status/{}", d.id),
                    });
                }
            }

            match page.meta.and_then(|m| m.next_token) {
                Some(tok) if got_any => pagination_token = Some(tok),
                _ => break,
            }
        }

        Ok(tweets)
    }
}

#[derive(Debug, Deserialize)]
struct UserLookupResponse {
    data: Option<UserData>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct UserData {
    id: String,
    username: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TweetsPage {
    data: Option<Vec<TweetData>>,
    meta: Option<TweetsMeta>,
}

#[derive(Debug, Deserialize)]
struct TweetData {
    id: String,
    text: String,
    created_at: Option<DateTime<Utc>>,
    note_tweet: Option<NoteTweet>,
}

#[derive(Debug, Deserialize)]
struct NoteTweet {
    text: String,
}

#[derive(Debug, Deserialize)]
struct TweetsMeta {
    next_token: Option<String>,
}

