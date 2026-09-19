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

        // If DuckDuckGo triggered an anomaly challenge (status 202 or 0 results), fallback to Brave and Bing web scrapers
        if results.is_empty() {
            tracing::warn!("duckduckgo returned 0 results for '{}', trying Brave web search fallback", query);
            let brave_results = self.search_brave(query).await;
            if !brave_results.is_empty() {
                tracing::info!("Brave web fallback succeeded: parsed {} results", brave_results.len());
                results = brave_results;
            } else {
                tracing::warn!("Brave returned 0 results, trying Bing web search fallback");
                let bing_results = self.search_bing(query).await;
                if !bing_results.is_empty() {
                    tracing::info!("Bing web fallback succeeded: parsed {} results", bing_results.len());
                    results = bing_results;
                }
            }
        }

        // Always scan raw HTML snippets and results for direct x.com/twitter.com status URLs
        let mut all_results = Vec::new();
        let mut seen_urls = std::collections::HashSet::new();
        for r in results {
            if seen_urls.insert(r.url.clone()) {
                all_results.push(r);
            }
        }

        Ok(all_results)
    }
}

impl DuckDuckGoProvider {
    /// Free Brave Search scraper fallback (search.brave.com)
    async fn search_brave(&self, query: &str) -> Vec<SearchResult> {
        let resp = match self
            .http
            .get("https://search.brave.com/search")
            .query(&[("q", query)])
            .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36")
            .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8")
            .header("Accept-Language", "en-US,en;q=0.9")
            .send()
            .await
        {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };

        if !resp.status().is_success() {
            return Vec::new();
        }

        let html = match resp.text().await {
            Ok(h) => h,
            Err(_) => return Vec::new(),
        };

        Self::parse_brave_html(&html)
    }

    /// Free Bing Search scraper fallback with Base64 redirect decoding
    async fn search_bing(&self, query: &str) -> Vec<SearchResult> {
        let resp = match self
            .http
            .get("https://www.bing.com/search")
            .query(&[("q", query), ("count", "10")])
            .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36")
            .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8")
            .header("Accept-Language", "en-US,en;q=0.5")
            .send()
            .await
        {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };

        if !resp.status().is_success() {
            return Vec::new();
        }

        let html = match resp.text().await {
            Ok(h) => h,
            Err(_) => return Vec::new(),
        };

        Self::parse_bing_html(&html)
    }

    pub fn parse_brave_html(html: &str) -> Vec<SearchResult> {
        let mut results = Vec::new();
        let snippet_re = Regex::new(r#"(?s)<div class="snippet[^"]*"[^>]*data-type="web"[^>]*>(.*?)(?:<div class="snippet|$)"#).unwrap();
        let url_re = Regex::new(r#"<a\s+href="([^"]+)""#).unwrap();
        let title_re = Regex::new(r#"(?s)<div[^>]*class="[^"]*title[^"]*"[^>]*>(.*?)</div>"#).unwrap();
        let desc_re = Regex::new(r#"(?s)<div[^>]*class="[^"]*content[^"]*"[^>]*>(.*?)</div>"#).unwrap();

        for cap in snippet_re.captures_iter(html) {
            let block = cap.get(1).map(|m| m.as_str()).unwrap_or_default();
            let url = url_re
                .captures(block)
                .and_then(|c| c.get(1))
                .map(|m| m.as_str())
                .unwrap_or_default();

            if url.is_empty() || url.starts_with('/') {
                continue;
            }

            let title = title_re
                .captures(block)
                .and_then(|c| c.get(1))
                .map(|m| Self::clean_html(m.as_str()))
                .unwrap_or_else(|| url.to_string());

            let snippet = desc_re
                .captures(block)
                .and_then(|c| c.get(1))
                .map(|m| Self::clean_html(m.as_str()))
                .unwrap_or_default();

            results.push(SearchResult {
                url: url.to_string(),
                title,
                snippet,
            });
        }

        // Also scan page for explicit status URLs
        let tweet_re = Regex::new(r#"https://(?:x|twitter)\.com/([a-zA-Z0-9_]{1,30})/status/(\d{15,22})"#).unwrap();
        for cap in tweet_re.captures_iter(html) {
            let full_url = cap.get(0).unwrap().as_str();
            let handle = &cap[1];
            let tweet_id = &cap[2];
            if !results.iter().any(|r| r.url == full_url) {
                results.push(SearchResult {
                    url: full_url.to_string(),
                    title: format!("Post by @{} on X", handle),
                    snippet: format!("Direct status match on X for tweet id {}", tweet_id),
                });
            }
        }

        results
    }

    pub fn parse_bing_html(html: &str) -> Vec<SearchResult> {
        let mut results = Vec::new();
        let algo_re = Regex::new(r#"(?s)<li class="b_algo"[^>]*>(.*?)</li>"#).unwrap();
        let h2_re = Regex::new(r#"(?s)<h2[^>]*>\s*<a[^>]+href="([^"]+)"[^>]*>(.*?)</a>"#).unwrap();
        let caption_re = Regex::new(r#"(?s)<div class="b_caption"><p[^>]*>(.*?)</p>"#).unwrap();

        for block_cap in algo_re.captures_iter(html) {
            let block = block_cap.get(1).map(|m| m.as_str()).unwrap_or_default();

            if let Some(h2_cap) = h2_re.captures(block) {
                let raw_url = h2_cap.get(1).map(|m| m.as_str()).unwrap_or_default();
                let title = h2_cap.get(2).map(|m| Self::clean_html(m.as_str())).unwrap_or_default();

                let target_url = if raw_url.contains("/ck/a?!") || raw_url.contains("u=a1") {
                    Self::decode_bing_redirect(raw_url).unwrap_or_else(|| raw_url.to_string())
                } else {
                    raw_url.to_string()
                };

                let snippet = caption_re
                    .captures(block)
                    .and_then(|c| c.get(1))
                    .map(|m| Self::clean_html(m.as_str()))
                    .unwrap_or_default();

                if !target_url.is_empty() && !target_url.starts_with('/') {
                    results.push(SearchResult {
                        url: target_url,
                        title,
                        snippet,
                    });
                }
            }
        }

        // Fallback: if no <li class="b_algo"> blocks matched, match direct <h2> links
        if results.is_empty() {
            for cap in h2_re.captures_iter(html) {
                let raw_url = cap.get(1).map(|m| m.as_str()).unwrap_or_default();
                let title = cap.get(2).map(|m| Self::clean_html(m.as_str())).unwrap_or_default();
                let target_url = if raw_url.contains("/ck/a?!") || raw_url.contains("u=a1") {
                    Self::decode_bing_redirect(raw_url).unwrap_or_else(|| raw_url.to_string())
                } else {
                    raw_url.to_string()
                };
                if !target_url.is_empty() && !target_url.starts_with('/') {
                    results.push(SearchResult {
                        url: target_url,
                        title,
                        snippet: String::new(),
                    });
                }
            }
        }

        results
    }

    /// Decodes Bing's Base64 redirect parameter: ...&u=a1<BASE64>&... or &amp;u=a1<BASE64>
    pub fn decode_bing_redirect(url: &str) -> Option<String> {
        let u_idx = url.find("u=a1")?;
        let rest = &url[u_idx + 4..];
        let end_idx = rest.find('&').unwrap_or(rest.len());
        let b64 = &rest[..end_idx];
        Self::decode_base64(b64)
    }

    fn decode_base64(input: &str) -> Option<String> {
        let clean = input.replace('-', "+").replace('_', "/");
        let pad_len = (4 - (clean.len() % 4)) % 4;
        let padded = format!("{}{}", clean, "=".repeat(pad_len));

        let mut bytes = Vec::new();
        let mut buf: u32 = 0;
        let mut bits = 0;

        for b in padded.bytes() {
            let val = match b {
                b'A'..=b'Z' => b - b'A',
                b'a'..=b'z' => b - b'a' + 26,
                b'0'..=b'9' => b - b'0' + 52,
                b'+' => 62,
                b'/' => 63,
                b'=' => break,
                _ => continue,
            } as u32;
            buf = (buf << 6) | val;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                bytes.push((buf >> bits) as u8);
            }
        }
        String::from_utf8(bytes).ok()
    }

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

        // Also scan page for explicit status URLs
        let tweet_re = Regex::new(r#"https://(?:x|twitter)\.com/([a-zA-Z0-9_]{1,30})/status/(\d{15,22})"#).unwrap();
        for cap in tweet_re.captures_iter(html) {
            let full_url = cap.get(0).unwrap().as_str();
            let handle = &cap[1];
            let tweet_id = &cap[2];
            if !results.iter().any(|r| r.url == full_url) {
                results.push(SearchResult {
                    url: full_url.to_string(),
                    title: format!("Post by @{} on X", handle),
                    snippet: format!("Direct status match on X for tweet id {}", tweet_id),
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

    #[test]
    fn test_decode_bing_redirect() {
        let bing_url = "https://www.bing.com/ck/a?!&&p=123&u=a1aHR0cHM6Ly94LmNvbS9zb21ldXNlci9zdGF0dXMvMTk5MjY4MzczNDQ3OTY1NTM4NQ&ntb=1";
        let decoded = DuckDuckGoProvider::decode_bing_redirect(bing_url).unwrap();
        assert_eq!(decoded, "https://x.com/someuser/status/1992683734479655385");
    }

    #[test]
    fn test_parse_brave_html() {
        let brave_html = r#"
            <div class="snippet" data-type="web">
                <a href="https://x.com/SkusSkus/status/1992683734479655385">
                    <div class="title">SkusSkus on X: 'What is the lore behind your header'</div>
                </a>
                <div class="content">Viral tweet original post on X</div>
            </div>
        "#;
        let results = DuckDuckGoProvider::parse_brave_html(brave_html);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].url, "https://x.com/SkusSkus/status/1992683734479655385");
        assert!(results[0].title.contains("SkusSkus"));
    }

    #[test]
    fn test_parse_bing_html() {
        let sample_html = r#"
            <li class="b_algo">
                <h2><a href="https://www.bing.com/ck/a?!&amp;&amp;p=123&amp;u=a1aHR0cHM6Ly9tb3ouY29tL2xlYXJuL3Nlby93aGF0LWlzLXNlbw&amp;ntb=1">What Is SEO? Best Practices - Moz</a></h2>
                <div class="b_caption"><p>SEO stands for search engine optimization and improving your site.</p></div>
            </li>
        "#;
        let results = DuckDuckGoProvider::parse_bing_html(sample_html);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].url, "https://moz.com/learn/seo/what-is-seo");
        assert_eq!(results[0].title, "What Is SEO? Best Practices - Moz");
        assert_eq!(results[0].snippet, "SEO stands for search engine optimization and improving your site.");
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
