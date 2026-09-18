use chrono::{Duration, Utc};

use crate::models::{AccountReport, TweetAnalysis, Verdict};

/// Builds the "80% original / 20% copied" style report over the last
/// `window_days` days of already-analyzed tweets.
pub fn build_report(handle: &str, analyses: &[TweetAnalysis], window_days: i64) -> AccountReport {
    let cutoff = Utc::now() - Duration::days(window_days);
    let windowed: Vec<&TweetAnalysis> = analyses.iter().filter(|a| a.created_at >= cutoff).collect();
    let total = windowed.len();

    if total == 0 {
        return AccountReport {
            handle: handle.to_string(),
            window_days,
            tweets_analyzed: 0,
            original_pct: 0.0,
            paraphrased_pct: 0.0,
            copied_pct: 0.0,
            avg_originality_score: 0.0,
            generated_at: Utc::now(),
        };
    }

    let (mut original, mut paraphrased, mut copied) = (0usize, 0usize, 0usize);
    let mut score_sum = 0.0f32;

    for a in &windowed {
        score_sum += a.originality_score;
        match a.verdict {
            Verdict::Original | Verdict::LikelyOriginal => original += 1,
            Verdict::Paraphrased => paraphrased += 1,
            Verdict::Copied => copied += 1,
        }
    }

    AccountReport {
        handle: handle.to_string(),
        window_days,
        tweets_analyzed: total,
        original_pct: pct(original, total),
        paraphrased_pct: pct(paraphrased, total),
        copied_pct: pct(copied, total),
        avg_originality_score: score_sum / total as f32,
        generated_at: Utc::now(),
    }
}

fn pct(part: usize, total: usize) -> f32 {
    (part as f32 / total as f32) * 100.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn analysis(score: f32, verdict: Verdict, days_ago: i64) -> TweetAnalysis {
        TweetAnalysis {
            tweet_id: format!("{score}-{days_ago}"),
            author_handle: "acct".into(),
            text: "x".into(),
            created_at: Utc::now() - Duration::days(days_ago),
            originality_score: score,
            verdict,
            best_match: None,
            method: "lexical".into(),
            analyzed_at: Utc::now(),
        }
    }

    #[test]
    fn buckets_and_filters_by_window() {
        let analyses = vec![
            analysis(95.0, Verdict::Original, 5),
            analysis(90.0, Verdict::Original, 10),
            analysis(20.0, Verdict::Copied, 15),
            analysis(50.0, Verdict::Paraphrased, 200), // outside 30d window
        ];
        let report = build_report("acct", &analyses, 30);
        assert_eq!(report.tweets_analyzed, 3);
        assert!((report.original_pct - 66.666664).abs() < 0.01);
        assert!((report.copied_pct - 33.333332).abs() < 0.01);
    }

    #[test]
    fn empty_window_does_not_divide_by_zero() {
        let report = build_report("acct", &[], 30);
        assert_eq!(report.tweets_analyzed, 0);
        assert_eq!(report.original_pct, 0.0);
    }
}
