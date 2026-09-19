use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::Json;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::aggregate::build_report;
use crate::error::AppError;
use crate::models::{
    AccountReport, EvaluationCriteria, MatchedSource, OriginatorInfo, ThinkingStep, Tweet,
    TweetAnalysis, Verdict,
};
use crate::pipeline::analyze_batch;
use crate::AppState;

pub async fn health_handler() -> &'static str {
    "ok"
}

pub async fn ui_handler() -> axum::response::Html<&'static str> {
    axum::response::Html(include_str!("../public/index.html"))
}

// ---- live fetch: pulls recent tweets from the X API and analyzes them ----

#[derive(Deserialize)]
pub struct AnalyzeRequest {
    /// Numeric X user id. Optional - if omitted, it is automatically resolved
    /// from the @handle in the URL route using X API v2.
    pub user_id: Option<String>,
    #[serde(default = "default_lookback")]
    pub lookback_days: i64,
}
fn default_lookback() -> i64 {
    90
}

pub async fn analyze_handler(
    State(state): State<Arc<AppState>>,
    Path(handle): Path<String>,
    Json(req): Json<AnalyzeRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let bearer = state
        .config
        .x_bearer_token
        .clone()
        .ok_or_else(|| AppError::Config("X_BEARER_TOKEN is not set".to_string()))?;

    let client = crate::x_client::XClient::new(bearer);
    let user_id = match req.user_id {
        Some(id) => id,
        None => client.resolve_user_id(&handle).await?,
    };

    let since = Utc::now() - chrono::Duration::days(req.lookback_days);
    let tweets = client.fetch_user_tweets(&user_id, &handle, 100, Some(since)).await?;

    let n = analyze_batch(&state, &handle, tweets).await?;
    Ok(Json(serde_json::json!({ "handle": handle, "tweets_analyzed": n })))
}

// ---- bulk import: for a full account backfill from the official X data ----
// ---- archive export ("Settings -> Your account -> Download an archive")  ----

#[derive(Deserialize)]
pub struct ImportTweet {
    pub id: String,
    pub text: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Deserialize)]
pub struct ImportRequest {
    pub tweets: Vec<ImportTweet>,
}

pub async fn import_handler(
    State(state): State<Arc<AppState>>,
    Path(handle): Path<String>,
    Json(req): Json<ImportRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let tweets: Vec<Tweet> = req
        .tweets
        .into_iter()
        .map(|t| Tweet {
            id: t.id.clone(),
            author_handle: handle.clone(),
            text: t.text,
            created_at: t.created_at,
            url: format!("https://x.com/{handle}/status/{}", t.id),
        })
        .collect();

    let n = analyze_batch(&state, &handle, tweets).await?;
    Ok(Json(serde_json::json!({ "handle": handle, "tweets_analyzed": n })))
}

// ---- read back results ----

pub async fn list_analyses_handler(
    State(state): State<Arc<AppState>>,
    Path(handle): Path<String>,
) -> Json<Vec<TweetAnalysis>> {
    Json(state.store.get_all(&handle))
}

#[derive(Deserialize)]
pub struct ReportQuery {
    #[serde(default = "default_report_days")]
    pub days: i64,
}
fn default_report_days() -> i64 {
    30
}

pub async fn report_handler(
    State(state): State<Arc<AppState>>,
    Path(handle): Path<String>,
    Query(q): Query<ReportQuery>,
) -> Json<AccountReport> {
    let analyses = state.store.get_all(&handle);
    Json(build_report(&handle, &analyses, q.days))
}

// ---- pre-publish verify: checks draft originality before posting on X ----

#[derive(Deserialize)]
pub struct VerifyDraftRequest {
    /// The draft tweet text to verify before publishing.
    pub text: String,
    /// Optional account handle to associate and store the draft check with.
    #[serde(default)]
    pub handle: Option<String>,
}

#[derive(Serialize)]
pub struct VerifyDraftResponse {
    pub text: String,
    pub originality_score: f32,
    pub verdict: Verdict,
    pub safe_to_publish: bool,
    pub advice: &'static str,
    pub best_match: Option<MatchedSource>,
    pub method: String,
    pub analyzed_at: DateTime<Utc>,
    pub thinking_steps: Vec<ThinkingStep>,
    pub criteria: EvaluationCriteria,
    pub reasoning: String,
    pub algo_multiplier: String,
    pub originator: Option<OriginatorInfo>,
}

pub async fn verify_draft_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<VerifyDraftRequest>,
) -> Result<Json<VerifyDraftResponse>, AppError> {
    let eval = crate::pipeline::evaluate_text(&state, &req.text).await?;
    let analysis = eval.analysis;

    let safe_to_publish = analysis.originality_score >= state.config.likely_original_threshold;

    let advice = match analysis.verdict {
        Verdict::Original => "High originality: Safe to publish with strong potential algorithmic reach.",
        Verdict::LikelyOriginal => "Good originality: Safe to publish. Minimal web overlap found.",
        Verdict::Paraphrased => "Caution: Moderate similarity to existing web sources. Consider rephrasing or attributing.",
        Verdict::Copied => "Warning: High duplicate overlap detected. May be suppressed or penalized by recommendation algorithms.",
    };

    if let Some(handle) = req.handle {
        let mut recorded = analysis.clone();
        recorded.author_handle = handle.clone();
        state.store.upsert(&handle, recorded);
    }

    Ok(Json(VerifyDraftResponse {
        text: analysis.text,
        originality_score: analysis.originality_score,
        verdict: analysis.verdict,
        safe_to_publish,
        advice,
        best_match: analysis.best_match,
        method: analysis.method,
        analyzed_at: analysis.analyzed_at,
        thinking_steps: eval.thinking_steps,
        criteria: eval.criteria,
        reasoning: eval.reasoning,
        algo_multiplier: eval.algo_multiplier,
        originator: eval.originator,
    }))
}

