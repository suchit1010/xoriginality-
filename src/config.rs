use std::env;

/// All runtime configuration is pulled from environment variables (or a
/// local `.env` file - see `.env.example`). Nothing here is hardcoded so the
/// service is safe to run in any environment / container.
#[derive(Debug, Clone)]
pub struct Config {
    pub port: u16,

    /// X API v2 bearer token, used for the *live* fetch path
    /// (`POST /accounts/:handle/analyze`). Optional: if absent, that route
    /// returns a config error but `/import` still works.
    pub x_bearer_token: Option<String>,

    /// Google Programmable Search Engine credentials.
    pub google_cse_api_key: Option<String>,
    pub google_cse_cx: Option<String>,

    /// Brave Search API key (2,000 free queries/month).
    pub brave_search_api_key: Option<String>,

    /// SerpApi credentials (alternative search backend).
    pub serpapi_api_key: Option<String>,

    /// URL of the x-originator-svc microservice (port 8081 by default).
    /// When set the pipeline calls this service for accurate X originator resolution
    /// instead of the DDG site:x.com fallback.
    pub x_originator_svc_url: Option<String>,

    /// Set to 1/true to disable web search entirely (all tweets score 100% original).
    pub disable_search: bool,

    /// Google Gemini API key (Google AI Studio - free tier available, generous limits).
    /// Used as the primary, cost-effective LLM judge.
    pub gemini_api_key: Option<String>,
    pub gemini_model: String,

    /// Anthropic API key (alternative/fallback LLM judge).
    pub anthropic_api_key: Option<String>,
    pub use_llm_judge: bool,

    /// Similarity thresholds (0.0-100.0 originality score) used to bucket a
    /// tweet into Original / LikelyOriginal / Paraphrased / Copied.
    pub original_threshold: f32,
    pub likely_original_threshold: f32,
    pub paraphrased_threshold: f32,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let port = env::var("PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(8080);

        let gemini_api_key = env::var("GEMINI_API_KEY").ok();
        let gemini_model = env::var("GEMINI_MODEL").unwrap_or_else(|_| "gemini-1.5-flash".to_string());
        let anthropic_api_key = env::var("ANTHROPIC_API_KEY").ok();
        let use_llm_judge =
            (gemini_api_key.is_some() || anthropic_api_key.is_some())
                && env::var("DISABLE_LLM_JUDGE").is_err();

        let disable_search = env::var("DISABLE_SEARCH").is_ok();

        Ok(Self {
            port,
            x_bearer_token: env::var("X_BEARER_TOKEN").ok(),
            google_cse_api_key: env::var("GOOGLE_CSE_API_KEY").ok(),
            google_cse_cx: env::var("GOOGLE_CSE_CX").ok(),
            serpapi_api_key: env::var("SERPAPI_API_KEY").ok(),
            brave_search_api_key: env::var("BRAVE_SEARCH_API_KEY").ok(),
            x_originator_svc_url: env::var("X_ORIGINATOR_SVC_URL")
                .ok()
                .or_else(|| Some("http://localhost:8081".to_string())),
            disable_search,
            gemini_api_key,
            gemini_model,
            anthropic_api_key,
            use_llm_judge,
            original_threshold: 80.0,
            likely_original_threshold: 60.0,
            paraphrased_threshold: 35.0,
        })
    }
}
