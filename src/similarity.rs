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
pub fn parse_twitter_snowflake_id(id_str: &str) -> Option<(u64, chrono::DateTime<chrono::Utc>)> {
    let id: u64 = id_str.parse().ok()?;
    // Twitter epoch: 1288834974657 ms
    let ms = (id >> 22) + 1288834974657;
    let secs = (ms / 1000) as i64;
    let nsecs = ((ms % 1000) * 1_000_000) as u32;
    let dt = chrono::DateTime::from_timestamp(secs, nsecs)?;
    Some((id, dt))
}

/// Inspects search results to extract all unique X (Twitter) status URLs and resolves
/// their author handle, snowflake ID, and chronological creation timestamp.
pub fn extract_x_posts_from_results(results: &[crate::search::SearchResult]) -> Vec<crate::models::XPostMatch> {
    use crate::models::XPostMatch;
    use regex::Regex;

    let mut posts = Vec::new();
    let re = Regex::new(r"(?:twitter\.com|x\.com)/([a-zA-Z0-9_]{1,30})/status/(\d{15,22})").unwrap();
    let mut seen_handles = HashSet::new();

    for r in results {
        let text_to_check = format!("{} {}", r.url, r.snippet);
        for cap in re.captures_iter(&text_to_check) {
            let handle = cap[1].to_string();
            let id_str = &cap[2];

            if seen_handles.contains(&handle) {
                continue;
            }
            seen_handles.insert(handle.clone());

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
}
