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

    /// Search for all recent public tweets matching a text query.
    ///
    /// This is the critical endpoint for finding the original writer of a
    /// low-impression tweet (e.g. Tweet A with 300 views). Unlike web search
    /// engines that only index popular/viral content, X's internal index
    /// contains EVERY public tweet within 7 days, regardless of impression count.
    ///
    /// The results are returned sorted by `recency` (newest first). The caller
    /// should sort by Snowflake ID ascending to find the earliest original poster.
    ///
    /// Endpoint: GET https://api.x.com/2/tweets/search/recent
    pub async fn search_recent_tweets(
        &self,
        query: &str,
        max_results: u32,
    ) -> Result<Vec<XSearchResult>, AppError> {
        // Clamp to X API limits: min 10, max 100 per page
        let page_size = max_results.clamp(10, 100);

        // Exclude retweets and replies so we only get original authored tweets
        let full_query = format!("{} -is:retweet -is:reply", query);

        let url = format!(
            "https://api.x.com/2/tweets/search/recent\
             ?query={}\
             &max_results={}\
             &sort_order=recency\
             &tweet.fields=created_at,author_id,public_metrics,text\
             &expansions=author_id\
             &user.fields=username,name,public_metrics",
            percent_encode(&full_query),
            page_size
        );

        let resp = self
            .http
            .get(&url)
            .bearer_auth(&self.bearer_token)
            .send()
            .await
            .map_err(|e| AppError::Http(format!("x search/recent request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(AppError::Http(format!("x search/recent error {status}: {body}")));
        }

        let raw: SearchRecentResponse = resp
            .json()
            .await
            .map_err(|e| AppError::Http(format!("x search/recent parse error: {e}")))?;

        // Build a handle lookup map from expanded users
        let user_map: std::collections::HashMap<String, String> = raw
            .includes
            .as_ref()
            .and_then(|inc| inc.users.as_ref())
            .map(|users| {
                users
                    .iter()
                    .map(|u| (u.id.clone(), u.username.clone()))
                    .collect()
            })
            .unwrap_or_default();

        let results = raw
            .data
            .unwrap_or_default()
            .into_iter()
            .filter_map(|t| {
                let handle = user_map.get(&t.author_id)?.clone();
                let tweet_id: u64 = t.id.parse().ok()?;
                let created_at = t.created_at?;
                Some(XSearchResult {
                    tweet_id,
                    handle: handle.clone(),
                    tweet_url: format!("https://x.com/{}/status/{}", handle, t.id),
                    created_at,
                    text: t.text,
                    impression_count: t.public_metrics
                        .as_ref()
                        .map(|m| m.impression_count)
                        .unwrap_or(0),
                    like_count: t.public_metrics
                        .as_ref()
                        .map(|m| m.like_count)
                        .unwrap_or(0),
                })
            })
            .collect();

        Ok(results)
    }
}

/// URL-encode a query string for X API search parameter.
#[allow(dead_code)]
fn percent_encode(s: &str) -> String {
    let mut encoded = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9'
            | b'-' | b'_' | b'.' | b'~' => encoded.push(b as char),
            b' ' => encoded.push('+'),
            _ => {
                encoded.push('%');
                encoded.push_str(&format!("{:02X}", b));
            }
        }
    }
    encoded
}

/// A single tweet result from the X API v2 search/recent endpoint.
/// Contains all fields needed to determine if this is the original poster
/// (via Snowflake ID ordering) and how much reach it has (impression_count).
#[derive(Debug, Clone)]
pub struct XSearchResult {
    /// Raw Snowflake ID — smaller = posted earlier = original creator
    pub tweet_id: u64,
    /// @handle of the author (without the @ prefix)
    pub handle: String,
    /// Full tweet URL: https://x.com/{handle}/status/{tweet_id}
    pub tweet_url: String,
    /// Exact UTC creation time decoded from the API (not from Snowflake math)
    pub created_at: DateTime<Utc>,
    /// Full tweet text
    pub text: String,
    /// Total impressions (low-impression tweets ARE returned — this is the
    /// field that proves web search misses the 300-view original creator)
    pub impression_count: u64,
    /// Like count
    pub like_count: u64,
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

/// Response from GET /2/tweets/search/recent
#[derive(Debug, Deserialize)]
struct SearchRecentResponse {
    data: Option<Vec<SearchTweetData>>,
    includes: Option<SearchIncludes>,
}

#[derive(Debug, Deserialize)]
struct SearchTweetData {
    id: String,
    text: String,
    author_id: String,
    created_at: Option<DateTime<Utc>>,
    public_metrics: Option<TweetPublicMetrics>,
}

#[derive(Debug, Deserialize)]
struct TweetPublicMetrics {
    impression_count: u64,
    like_count: u64,
}

#[derive(Debug, Deserialize)]
struct SearchIncludes {
    users: Option<Vec<SearchUser>>,
}

#[derive(Debug, Deserialize)]
struct SearchUser {
    id: String,
    username: String,
}
