use std::collections::HashSet;

/// Lowercase, strip punctuation, split on whitespace.
fn normalize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .map(|s| s.to_string())
        .collect()
}

/// K-shingles (contiguous word n-grams) of a token stream, used for
/// near-duplicate detection that's more robust than raw string diffing.
fn shingles(tokens: &[String], k: usize) -> HashSet<String> {
    if tokens.is_empty() {
        return HashSet::new();
    }
    if tokens.len() < k {
        return HashSet::from([tokens.join(" ")]);
    }
    tokens.windows(k).map(|w| w.join(" ")).collect()
}

/// Jaccard similarity over k-shingles: |intersection| / |union|, in [0, 1].
pub fn jaccard_similarity(a: &str, b: &str, k: usize) -> f32 {
    let ta = normalize(a);
    let tb = normalize(b);
    if ta.is_empty() || tb.is_empty() {
        return 0.0;
    }
    let sa = shingles(&ta, k);
    let sb = shingles(&tb, k);
    let intersection = sa.intersection(&sb).count();
    let union = sa.union(&sb).count();
    if union == 0 {
        0.0
    } else {
        intersection as f32 / union as f32
    }
}

/// Normalized Levenshtein similarity: 1 - (edit distance / longer length).
pub fn levenshtein_similarity(a: &str, b: &str) -> f32 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let dist = strsim::levenshtein(a, b);
    let max_len = a.chars().count().max(b.chars().count()).max(1);
    1.0 - (dist as f32 / max_len as f32)
}

/// Blended similarity used by the pipeline: mostly shingle-overlap (catches
/// reordered/partial copies) with a smaller Levenshtein component (catches
/// near-verbatim copies with light edits). Result is clamped to [0, 1].
pub fn combined_similarity(a: &str, b: &str) -> f32 {
    let j = jaccard_similarity(a, b, 3);
    let l = levenshtein_similarity(a, b);
    (j * 0.7 + l * 0.3).clamp(0.0, 1.0)
}

/// Decodes a 64-bit Twitter Snowflake ID into its exact creation timestamp (UTC).
/// Twitter Snowflake epoch begins at 1288834974657 ms (Nov 04, 2010 01:42:54 UTC).
///
/// Returns `None` if the ID doesn't parse, or if it decodes to a date outside
/// the sane range [Snowflake rollout, now] — a real tweet can't be dated
/// before Twitter switched to Snowflake IDs, and can't be dated in the
/// future. Landing outside that range means we matched something that
/// *looked* like a status URL but wasn't (a mistyped example ID in a blog
/// post, a non-tweet numeric path, etc.) — better to drop it than report a
/// confidently wrong originator date.
pub fn parse_twitter_snowflake_id(id_str: &str) -> Option<(u64, chrono::DateTime<chrono::Utc>)> {
    const SNOWFLAKE_EPOCH_MS: u64 = 1288834974657;
    let id: u64 = id_str.parse().ok()?;
    let ms = (id >> 22) + SNOWFLAKE_EPOCH_MS;
    let secs = (ms / 1000) as i64;
    let nsecs = ((ms % 1000) * 1_000_000) as u32;
    let dt = chrono::DateTime::from_timestamp(secs, nsecs)?;

    // Allow a small buffer for clock skew rather than a hard `> now` cutoff.
    let not_after = chrono::Utc::now() + chrono::Duration::minutes(5);
    if dt < chrono::DateTime::from_timestamp((SNOWFLAKE_EPOCH_MS / 1000) as i64, 0)? || dt > not_after {
        return None;
    }

    Some((id, dt))
}

/// Handle-shaped URL segments that are never a real account: X's own
/// placeholder path for anonymized/logged-out share links
/// (`x.com/i/status/12345`) is by far the most common one search engines
/// index. Without this guard, that "i" gets reported as the author.
const NON_ACCOUNT_PATH_SEGMENTS: [&str; 1] = ["i"];

/// Inspects search results to extract all unique X (Twitter) status URLs and resolves
/// their author handle, snowflake ID, and chronological creation timestamp.
pub fn extract_x_posts_from_results(results: &[crate::search::SearchResult]) -> Vec<crate::models::XPostMatch> {
    use crate::models::XPostMatch;
    use regex::Regex;

    let mut posts = Vec::new();
    let re = Regex::new(r"(?:twitter\.com|x\.com)/([a-zA-Z0-9_]{1,30})/status/(\d{15,22})").unwrap();
    // Keyed on the lowercased handle: X handles are case-insensitive, so
    // "@WatcherGuru" and "@watcherguru" turning up in two different search
    // snippets must dedupe to one account, not inflate the copycat count.
    let mut seen_handles_lower: HashSet<String> = HashSet::new();

    for r in results {
        let text_to_check = format!("{} {}", r.url, r.snippet);
        for cap in re.captures_iter(&text_to_check) {
            let handle = cap[1].to_string();
            let id_str = &cap[2];
            let handle_lower = handle.to_lowercase();

            if NON_ACCOUNT_PATH_SEGMENTS.contains(&handle_lower.as_str()) {
                continue;
            }
            if seen_handles_lower.contains(&handle_lower) {
                continue;
            }
            seen_handles_lower.insert(handle_lower);

            if let Some((id, dt)) = parse_twitter_snowflake_id(id_str) {
                posts.push(XPostMatch {
                    author_handle: handle.clone(),
                    tweet_id: id,
                    tweet_url: format!("https://x.com/{}/status/{}", handle, id_str),
                    published_at: dt,
                    formatted_date: dt.format("%b %d, %Y").to_string(),
                    snippet: r.snippet.clone(),
                });
            }
        }
    }

    // Sort ascending: smallest Snowflake ID = earliest published on X
    posts.sort_by_key(|p| p.tweet_id);
    posts
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_text_is_fully_similar() {
        let s = "the quick brown fox jumps over the lazy dog";
        assert!((combined_similarity(s, s) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn unrelated_text_is_not_similar() {
        let a = "quarterly revenue grew twelve percent this year";
        let b = "the cat sat quietly on the warm windowsill";
        assert!(combined_similarity(a, b) < 0.2);
    }

    #[test]
    fn near_duplicate_scores_high() {
        let a = "Building a rust microservice for tweet originality checks";
        let b = "building a Rust microservice for tweet originality checks!";
        assert!(combined_similarity(a, b) > 0.85);
    }

    #[test]
    fn paraphrase_scores_in_the_middle() {
        let a = "Our new API rate limits will roll out next Monday for all users";
        let b = "Starting Monday, every user will see the new API rate limits";
        let sim = combined_similarity(a, b);
        assert!(sim > 0.1 && sim < 0.7, "sim was {sim}");
    }

    #[test]
    fn empty_strings_do_not_panic() {
        assert_eq!(combined_similarity("", ""), 0.3);
        assert_eq!(jaccard_similarity("", "hello", 3), 0.0);
    }

    #[test]
    fn test_parse_twitter_snowflake_id() {
        let (id, dt) = parse_twitter_snowflake_id("2040840042734588042").expect("valid snowflake");
        assert_eq!(id, 2040840042734588042);
        assert_eq!(dt.format("%Y-%m-%d").to_string(), "2026-04-05");
    }

    #[test]
    fn snowflake_rejects_future_dated_garbage() {
        // A 19-digit number that happens to parse as u64 but decodes to a
        // date far beyond "now" - e.g. a mistyped example ID in a blog post
        // that isn't a real tweet at all. Must not be reported as a match.
        assert!(parse_twitter_snowflake_id("9223372036854775807").is_none());
    }

    #[test]
    fn snowflake_accepts_real_recent_id() {
        // Sanity check that the new bounds check doesn't reject genuinely
        // valid, recently-issued IDs.
        assert!(parse_twitter_snowflake_id("2040840042734588042").is_some());
    }

    fn stub_result(url: &str, snippet: &str) -> crate::search::SearchResult {
        crate::search::SearchResult {
            url: url.to_string(),
            title: String::new(),
            snippet: snippet.to_string(),
        }
    }

    #[test]
    fn ignores_anonymized_i_status_placeholder() {
        // x.com/i/status/... is X's own placeholder for logged-out/share
        // links - it is never a real handle and must not be reported as
        // "posted by @i".
        let results = vec![stub_result(
            "https://x.com/i/status/1699999999999999999",
            "What's the lore behind your header?",
        )];
        let posts = extract_x_posts_from_results(&results);
        assert!(posts.is_empty(), "expected no posts, got {posts:?}");
    }

    #[test]
    fn dedupes_handles_case_insensitively() {
        // The same account, referenced with different capitalization across
        // two different search snippets, must count as ONE account - not
        // inflate the "N distinct accounts" copycat signal.
        let results = vec![
            stub_result(
                "https://x.com/WatcherGuru/status/2040840042734588042",
                "Warren Buffett stepped down.",
            ),
            stub_result(
                "https://x.com/watcherguru/status/2040840042734588042",
                "Warren Buffett stepped down. (duplicate index entry)",
            ),
        ];
        let posts = extract_x_posts_from_results(&results);
        assert_eq!(posts.len(), 1, "expected one deduped account, got {posts:?}");
    }

    #[test]
    fn keeps_distinct_real_handles() {
        let results = vec![
            stub_result("https://x.com/alice/status/2040840042734588042", "text"),
            stub_result("https://x.com/bob/status/2040840042734588142", "text"),
        ];
        let posts = extract_x_posts_from_results(&results);
        assert_eq!(posts.len(), 2);
    }
}
