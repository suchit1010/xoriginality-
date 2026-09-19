use async_trait::async_trait;
use regex::Regex;

use super::{SearchResult, WebSearchProvider};
use crate::error::AppError;

/// Free zero-cost web search provider that queries DuckDuckGo HTML directly.
/// Requires NO API keys, NO credit card, and has zero marginal cost.
pub struct DuckDuckGoProvider {
    http: reqwest::Client,
}

impl DuckDuckGoProvider {
    pub fn new() -> Self {
        Self {
            http: reqwest::Client::builder()
                .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.4 Safari/605.1.15")
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
        }
    }

    /// Clean HTML tags and decode common HTML entities
    fn clean_html(text: &str) -> String {
        let tag_re = Regex::new(r"<[^>]+>").unwrap();
        let stripped = tag_re.replace_all(text, "");
        stripped
            .replace("&amp;", "&")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&#x27;", "'")
            .replace("&#39;", "'")
            .replace("&nbsp;", " ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Extract actual destination URL from DuckDuckGo redirect link
    /// e.g. //duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2F...
    fn extract_url(href: &str) -> String {
        if let Some(pos) = href.find("uddg=") {
            let encoded = &href[pos + 5..];
            let end_pos = encoded.find('&').unwrap_or(encoded.len());
            let clean_encoded = &encoded[..end_pos];
            if let Ok(decoded) = percent_encoding_decode(clean_encoded) {
                return decoded;
            }
        }
        if href.starts_with("//") {
            format!("https:{href}")
        } else {
            href.to_string()
        }
    }
}

impl Default for DuckDuckGoProvider {
    fn default() -> Self {
        Self::new()
    }
}

/// Simple percent-decoding helper for extracted URLs
fn percent_encoding_decode(s: &str) -> Result<String, ()> {
    let mut bytes = Vec::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '%' {
            let h1 = chars.next().ok_or(())?;
            let h2 = chars.next().ok_or(())?;
            let hex = format!("{h1}{h2}");
            let byte = u8::from_str_radix(&hex, 16).map_err(|_| ())?;
            bytes.push(byte);
        } else if c == '+' {
            bytes.push(b' ');
        } else {
            bytes.push(c as u8);
        }
    }
    String::from_utf8(bytes).map_err(|_| ())
}

#[async_trait]
impl WebSearchProvider for DuckDuckGoProvider {
    async fn search(&self, query: &str) -> Result<Vec<SearchResult>, AppError> {
        let url = "https://lite.duckduckgo.com/lite/";
        let resp = self
            .http
            .post(url)
            .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8")
            .header("Accept-Language", "en-US,en;q=0.9")
            .header("Origin", "https://lite.duckduckgo.com")
            .header("Referer", "https://lite.duckduckgo.com/")
            .form(&[("q", query)])
            .send()
            .await
            .map_err(|e| AppError::Http(format!("duckduckgo request failed: {e}")))?;

        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| AppError::Http(format!("failed to read duckduckgo response: {e}")))?;

        let mut results = Self::parse_results(&body);
        tracing::info!(
            "duckduckgo status {} for query '{}': body {} bytes, parsed {} results",
            status,
            query,
            body.len(),
            results.len()
        );

        // If lite endpoint triggered an anomaly challenge (status 202 or 0 results), fallback to alternative user agent
        if results.is_empty() && (status.as_u16() == 202 || body.contains("anomaly")) {
            tracing::warn!("duckduckgo anomaly detected for query '{}', retrying with fallback headers", query);
            if let Ok(retry_resp) = self
                .http
                .post(url)
                .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36")
                .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8")
                .header("Accept-Language", "en-US,en;q=0.5")
                .header("Origin", "https://lite.duckduckgo.com")
                .header("Referer", "https://lite.duckduckgo.com/")
                .form(&[("q", query)])
                .send()
                .await
            {
                if let Ok(retry_body) = retry_resp.text().await {
                    let fallback_results = Self::parse_results(&retry_body);
                    if !fallback_results.is_empty() {
                        tracing::info!("duckduckgo fallback succeeded: parsed {} results", fallback_results.len());
                        results = fallback_results;
                    }
                }
            }
        }

        Ok(results)
    }
}

impl DuckDuckGoProvider {
    pub fn parse_results(html: &str) -> Vec<SearchResult> {
        let link_re = Regex::new(r#"(?s)<a[^>]+href="([^"]+)"[^>]*class=['"]result-link['"][^>]*>(.*?)</a>"#).unwrap();
        let snippet_re = Regex::new(r#"(?s)<td[^>]*class=['"]result-snippet['"][^>]*>(.*?)</td>"#).unwrap();

        let links: Vec<(String, String)> = link_re
            .captures_iter(html)
            .map(|cap| {
                let raw_href = cap.get(1).map(|m| m.as_str()).unwrap_or_default();
                let raw_title = cap.get(2).map(|m| m.as_str()).unwrap_or_default();
                (Self::extract_url(raw_href), Self::clean_html(raw_title))
            })
            .collect();

        let snippets: Vec<String> = snippet_re
            .captures_iter(html)
            .map(|cap| {
                let raw_snippet = cap.get(1).map(|m| m.as_str()).unwrap_or_default();
                Self::clean_html(raw_snippet)
            })
            .collect();

        let mut results = Vec::new();
        let total = links.len().min(snippets.len()).min(10);

        for i in 0..total {
            let (url, title) = &links[i];
            let snippet = &snippets[i];
            if !url.is_empty() && !title.is_empty() {
                results.push(SearchResult {
                    url: url.clone(),
                    title: title.clone(),
                    snippet: snippet.clone(),
                });
            }
        }

        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clean_html() {
        let dirty = "<b>Hello</b> &amp; welcome to &#x27;Rust&#x27;!";
        assert_eq!(DuckDuckGoProvider::clean_html(dirty), "Hello & welcome to 'Rust'!");
    }

    #[test]
    fn test_extract_url() {
        let href = "//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fpage%3Fid%3D1&rut=xyz";
        assert_eq!(DuckDuckGoProvider::extract_url(href), "https://example.com/page?id=1");
    }

    #[test]
    fn test_parse_results() {
        let sample_html = r#"
            <tr>
                <td>
                    <a rel="nofollow" href="https://example.com/hamlet" class='result-link'>To be or not to be</a>
                </td>
            </tr>
            <tr>
                <td class='result-snippet'>
                    <b>To</b> <b>be</b> or not to be, that is the question.
                </td>
            </tr>
        "#;
        let results = DuckDuckGoProvider::parse_results(sample_html);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].url, "https://example.com/hamlet");
        assert_eq!(results[0].title, "To be or not to be");
        assert!(results[0].snippet.contains("that is the question"));
    }

    #[tokio::test]
    async fn test_live_search() {
        let provider = DuckDuckGoProvider::new();
        let results = provider.search("To be or not to be that is the question").await.unwrap();
        println!("Live search results count: {}", results.len());
        for r in &results {
            println!("Hit: {} - {}", r.title, r.url);
        }
        assert!(!results.is_empty());
    }
}
