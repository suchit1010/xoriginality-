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
}
