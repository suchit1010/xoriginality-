use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A single tweet, whether pulled live from the X API or imported from an
/// account's official data archive.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tweet {
    pub id: String,
    pub author_handle: String,
    pub text: String,
    pub created_at: DateTime<Utc>,
    pub url: String,
}

/// Bucketed verdict derived from the originality score.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Original,
    LikelyOriginal,
    Paraphrased,
    Copied,
}

/// The closest web result found for a tweet, with a lexical similarity score.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchedSource {
    pub url: String,
    pub title: String,
    pub snippet: String,
    /// 0.0 (unrelated) .. 1.0 (near-identical text)
    pub similarity: f32,
}

/// Result of analyzing one tweet.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TweetAnalysis {
    pub tweet_id: String,
    pub author_handle: String,
    pub text: String,
    pub created_at: DateTime<Utc>,
    /// 0 (verbatim copy) .. 100 (fully original)
    pub originality_score: f32,
    pub verdict: Verdict,
    pub best_match: Option<MatchedSource>,
    /// "lexical" or "lexical+llm" depending on whether the LLM judge ran.
    pub method: String,
    pub analyzed_at: DateTime<Utc>,
}

/// Aggregate originality report over a rolling window, e.g. "last 30 days".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountReport {
    pub handle: String,
    pub window_days: i64,
    pub tweets_analyzed: usize,
    pub original_pct: f32,
    pub paraphrased_pct: f32,
    pub copied_pct: f32,
    pub avg_originality_score: f32,
    pub generated_at: DateTime<Utc>,
}

/// Step in the evaluation chain-of-thought execution trace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkingStep {
    pub stage: u8,
    pub stage_name: String,
    pub title: String,
    pub detail: String,
    pub status: String, // "pass", "warn", "flag", "skip", "info"
    pub execution_time_ms: u64,
}

/// Score for an individual Twitter/X evaluation criterion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CriterionScore {
    pub name: String,
    pub score: f32,
    pub weight_pct: u8,
    pub status: String,
    pub description: String,
    pub impact: String,
}

/// Detailed evaluation criteria according to Twitter/X's recommendation algorithm.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvaluationCriteria {
    pub lexical_uniqueness: CriterionScore,
    pub claim_substantiveness: CriterionScore,
    pub attribution_integrity: CriterionScore,
    pub algo_distribution_impact: CriterionScore,
}

/// Comprehensive evaluation result for draft verification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DraftEvaluation {
    pub analysis: TweetAnalysis,
    pub thinking_steps: Vec<ThinkingStep>,
    pub criteria: EvaluationCriteria,
    pub reasoning: String,
    pub algo_multiplier: String,
}

