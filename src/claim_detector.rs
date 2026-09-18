use regex::Regex;

/// Result of evaluating whether a tweet contains verifiable claims or should be searched.
#[derive(Debug, Clone, PartialEq)]
pub struct ClaimEvaluation {
    pub should_search: bool,
    pub reason: &'static str,
    pub clean_query: String,
}

/// Evaluates if a tweet has enough substantive content / factual claims to warrant
/// an expensive web search, or if it is obvious conversational chatter, greetings,
/// or personal status updates that are trivially original.
pub fn evaluate_tweet(text: &str) -> ClaimEvaluation {
    let trimmed = text.trim();

    // 1. Empty or extremely short
    if trimmed.is_empty() {
        return ClaimEvaluation {
            should_search: false,
            reason: "empty_text",
            clean_query: String::new(),
        };
    }

    // Strip out mentions (@user) and URLs (https://...) for analysis
    let words: Vec<&str> = trimmed
        .split_whitespace()
        .filter(|w| !w.starts_with('@') && !w.starts_with("http://") && !w.starts_with("https://"))
        .collect();

    // Less than 4 words after stripping mentions/urls
    if words.len() < 4 {
        return ClaimEvaluation {
            should_search: false,
            reason: "too_short_for_claim",
            clean_query: String::new(),
        };
    }

    let lower = trimmed.to_lowercase();

    // 2. Common social media greetings & sign-offs (single phrase or greeting + emoji)
    let greetings = [
        "gm", "gn", "good morning", "good night", "good evening", "good afternoon",
        "happy monday", "happy friday", "happy weekend", "happy sunday",
        "hello everyone", "hey guys", "have a great day", "have a blessed day",
        "welcome back", "thank you all", "thanks for having me", "congrats", "congratulations"
    ];

    for g in greetings {
        if lower.starts_with(g) && words.len() <= 10 {
            return ClaimEvaluation {
                should_search: false,
                reason: "greeting_or_pleasantry",
                clean_query: String::new(),
            };
        }
    }

    // 3. Pure personal status / subjective feeling statements (no general claim)
    let personal_prefixes = [
        "i feel like", "i am so tired", "i'm so tired", "just woke up", "going to sleep",
        "drinking my coffee", "eating lunch", "omg lol", "lmao so true", "so true lol",
        "i love this so much", "can't believe it's already", "excited for the weekend"
    ];
    for p in personal_prefixes {
        if lower.starts_with(p) && words.len() <= 8 {
            return ClaimEvaluation {
                should_search: false,
                reason: "personal_subjective_chatter",
                clean_query: String::new(),
            };
        }
    }

    // 4. Extract optimized search query
    let clean_query = extract_search_query(trimmed);

    // If query resulted in fewer than 3 usable words
    if clean_query.split_whitespace().count() < 3 {
        return ClaimEvaluation {
            should_search: false,
            reason: "insufficient_search_tokens",
            clean_query,
        };
    }

    ClaimEvaluation {
        should_search: true,
        reason: "verifiable_claim_detected",
        clean_query,
    }
}

/// Formulates a high-signal search query from tweet text:
/// - Extracts exact quotes if present ("...")
/// - Removes hashtags, handles, URLs, and noisy punctuation
/// - Selects the top 10-16 informative words for search engines
pub fn extract_search_query(text: &str) -> String {
    // If text contains an explicit quote ("..."), that is often the central claim or borrowed snippet
    let quote_re = Regex::new(r#""([^"]{15,120})""#).ok();
    if let Some(re) = quote_re {
        if let Some(caps) = re.captures(text) {
            if let Some(quoted) = caps.get(1) {
                return format!("\"{}\"", quoted.as_str().trim());
            }
        }
    }

    // Otherwise clean words
    let mut selected_words = Vec::new();
    for word in text.split_whitespace() {
        if word.starts_with('@') || word.starts_with("http://") || word.starts_with("https://") {
            continue;
        }

        // Clean surrounding punctuation while preserving hyphens/apostrophes
        let cleaned = word.trim_matches(|c: char| {
            !c.is_alphanumeric() && c != '\'' && c != '-'
        });

        if !cleaned.is_empty() {
            selected_words.push(cleaned);
        }

        if selected_words.len() >= 16 {
            break;
        }
    }

    selected_words.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_skip_greetings() {
        let eval = evaluate_tweet("Good morning everyone! Have a wonderful day.");
        assert!(!eval.should_search);
        assert_eq!(eval.reason, "greeting_or_pleasantry");

        let eval2 = evaluate_tweet("gm all! ☕");
        assert!(!eval2.should_search);
    }

    #[test]
    fn test_skip_short_chatter() {
        let eval = evaluate_tweet("lol so true!");
        assert!(!eval.should_search);
        assert_eq!(eval.reason, "too_short_for_claim");
    }

    #[test]
    fn test_keep_substantive_claims() {
        let tweet = "The best SEO strategy is simply creating pages for every question your customers ask.";
        let eval = evaluate_tweet(tweet);
        assert!(eval.should_search);
        assert_eq!(eval.reason, "verifiable_claim_detected");
        assert!(eval.clean_query.contains("SEO strategy"));
    }

    #[test]
    fn test_extract_quoted_claim() {
        let tweet = "As Einstein once famously said, \"creativity is intelligence having fun\" and that remains true today.";
        let q = extract_search_query(tweet);
        assert_eq!(q, "\"creativity is intelligence having fun\"");
    }
}
