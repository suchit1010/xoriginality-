use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::error::AppError;
use crate::models::MatchedSource;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmVerdict {
    pub originality_score: f32,
    pub reasoning: String,
}

// ---------------- Google Gemini Flash ----------------
#[derive(Serialize)]
struct GeminiRequest {
    contents: Vec<GeminiContent>,
    #[serde(rename = "generationConfig")]
    generation_config: GeminiGenerationConfig,
}

#[derive(Serialize)]
struct GeminiContent {
    parts: Vec<GeminiPart>,
}

#[derive(Serialize)]
struct GeminiPart {
    text: String,
}

#[derive(Serialize)]
struct GeminiGenerationConfig {
    temperature: f32,
    #[serde(rename = "maxOutputTokens")]
    max_output_tokens: u32,
    #[serde(rename = "responseMimeType")]
    response_mime_type: String,
}

#[derive(Debug, Deserialize)]
struct GeminiResponse {
    candidates: Option<Vec<GeminiCandidate>>,
}

#[derive(Debug, Deserialize)]
struct GeminiCandidate {
    content: Option<GeminiResponseContent>,
}

#[derive(Debug, Deserialize)]
struct GeminiResponseContent {
    parts: Option<Vec<GeminiResponsePart>>,
}

#[derive(Debug, Deserialize)]
struct GeminiResponsePart {
    text: Option<String>,
}

pub async fn judge_with_gemini(
    http: &reqwest::Client,
    api_key: &str,
    model: &str,
    tweet_text: &str,
    candidates: &[MatchedSource],
) -> Result<LlmVerdict, AppError> {
    let prompt = build_judge_prompt(tweet_text, candidates);

    let body = GeminiRequest {
        contents: vec![GeminiContent {
            parts: vec![GeminiPart { text: prompt }],
        }],
        generation_config: GeminiGenerationConfig {
            temperature: 0.1,
            max_output_tokens: 300,
            response_mime_type: "application/json".to_string(),
        },
    };

    let url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent?key={api_key}"
    );

    let resp = http
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(|e| AppError::Llm(format!("gemini request failed: {e}")))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(AppError::Llm(format!("gemini api error {status}: {text}")));
    }

    let parsed: GeminiResponse = resp
        .json()
        .await
        .map_err(|e| AppError::Llm(format!("gemini response parse failed: {e}")))?;

    let raw_text = parsed
        .candidates
        .and_then(|c| c.into_iter().next())
        .and_then(|c| c.content)
        .and_then(|c| c.parts)
        .and_then(|p| p.into_iter().next())
        .and_then(|p| p.text)
        .unwrap_or_default();

    let cleaned = extract_json_block(&raw_text);

    serde_json::from_str::<LlmVerdict>(cleaned)
        .map_err(|e| AppError::Llm(format!("could not parse gemini verdict ({e}): {raw_text}")))
}

fn extract_json_block(raw: &str) -> &str {
    let text = raw.trim();
    if let Some(start) = text.find('{') {
        if let Some(end) = text.rfind('}') {
            if end >= start {
                return &text[start..=end];
            }
        }
    }
    text
}

// ---------------- Anthropic Claude ----------------
const ANTHROPIC_MODEL: &str = "claude-sonnet-4-6";
const ANTHROPIC_VERSION: &str = "2023-06-01";

#[derive(Serialize)]
struct AnthropicMessage {
    role: String,
    content: String,
}

#[derive(Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: u32,
    messages: Vec<AnthropicMessage>,
}

#[derive(Debug, Deserialize)]
struct AnthropicContentBlock {
    #[serde(rename = "type")]
    kind: String,
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AnthropicResponse {
    content: Vec<AnthropicContentBlock>,
}

pub async fn judge_with_anthropic(
    http: &reqwest::Client,
    api_key: &str,
    tweet_text: &str,
    candidates: &[MatchedSource],
) -> Result<LlmVerdict, AppError> {
    let prompt = build_judge_prompt(tweet_text, candidates);

    let body = AnthropicRequest {
        model: ANTHROPIC_MODEL.to_string(),
        max_tokens: 300,
        messages: vec![AnthropicMessage {
            role: "user".to_string(),
            content: prompt,
        }],
    };

    let resp = http
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", api_key)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| AppError::Llm(format!("anthropic request failed: {e}")))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(AppError::Llm(format!("anthropic api error {status}: {text}")));
    }

    let parsed: AnthropicResponse = resp
        .json()
        .await
        .map_err(|e| AppError::Llm(format!("anthropic response parse failed: {e}")))?;

    let raw_text = parsed
        .content
        .iter()
        .find(|c| c.kind == "text")
        .and_then(|c| c.text.clone())
        .unwrap_or_default();

    let cleaned = extract_json_block(&raw_text);

    serde_json::from_str::<LlmVerdict>(cleaned)
        .map_err(|e| AppError::Llm(format!("could not parse anthropic verdict ({e}): {raw_text}")))
}

// ---------------- Shared Prompt & Dispatcher ----------------

fn build_judge_prompt(tweet_text: &str, candidates: &[MatchedSource]) -> String {
    let candidates_block = if candidates.is_empty() {
        "(no close web matches were found)".to_string()
    } else {
        candidates
            .iter()
            .take(5)
            .enumerate()
            .map(|(i, c)| {
                format!(
                    "{}. [{:.0}% lexical overlap] {}\n   \"{}\"",
                    i + 1,
                    c.similarity * 100.0,
                    c.url,
                    c.snippet.chars().take(300).collect::<String>()
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    };

    format!(
        "You assess whether a social media post is original writing or copied/paraphrased \
         from an existing web source. Quoting with clear attribution should score high; \
         restating someone else's claim/joke/analysis as your own without attribution should \
         score low, even if the wording differs.\n\n\
         POST:\n\"{tweet_text}\"\n\n\
         TOP WEB MATCHES:\n{candidates_block}\n\n\
         Reply with ONLY a JSON object:\n\
         {{\"originality_score\": <integer 0-100, 100 = fully original, 0 = verbatim copy>, \
         \"reasoning\": \"<one short sentence>\"}}"
    )
}

/// Dispatches to the active LLM judge. Prioritizes Gemini Flash for cost and speed,
/// falling back to Anthropic if configured.
pub async fn judge_originality(
    http: &reqwest::Client,
    config: &Config,
    tweet_text: &str,
    candidates: &[MatchedSource],
) -> Result<(LlmVerdict, String), AppError> {
    if let Some(key) = &config.gemini_api_key {
        let verdict = judge_with_gemini(http, key, &config.gemini_model, tweet_text, candidates).await?;
        return Ok((verdict, format!("lexical+{}", config.gemini_model)));
    }

    if let Some(key) = &config.anthropic_api_key {
        let verdict = judge_with_anthropic(http, key, tweet_text, candidates).await?;
        return Ok((verdict, "lexical+claude".to_string()));
    }

    Err(AppError::Config("no LLM API key configured for judge".to_string()))
}

