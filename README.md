# 🛡️ X (Twitter) Originality Checker & Pre-Publish Verifier

A high-performance Rust microservice and interactive web dashboard that analyzes draft and published posts for **originality vs. web-matched content** according to the **Twitter/X Recommendation Algorithm**.

It provides:
- ✍️ **Pre-Publish Verifier Web UI**: Write or paste a tweet draft to test its originality score and see predicted Twitter/X algorithm reach impact *before* posting.
- ⚡ **Zero-Cost Out of the Box**: Ships with a built-in **DuckDuckGo Lite** search engine that requires **$0, zero API keys, and zero signups**.
- 🧠 **Multi-Stage Cost-Reduction Cascade**: Heuristic filtering + lexical gating skips up to **60%** of external searches and LLM calls.
- 📉 **Ultra-Cheap AI Judging**: Optional integration with **Google Gemini Flash** (free tier available with up to 1,500 requests/day).
- 📜 **Full Long-Form Post Support**: Automatically extracts complete `note_tweet` content (>280 chars) and resolves `@handles` directly via X API v2.

---

## 🖥️ Interactive Web Dashboard (`GET /`)

Run the microservice and navigate to **[http://localhost:8080/](http://localhost:8080/)** in any browser.

### Features
1. **Interactive Tweet & Note Tweet Composer**:
   - Live character counter with Twitter-styled 280-character circular ring.
   - Dynamic badge detection: switches seamlessly between `Standard Tweet (280 chars)` and `Long-Form Post / Note Tweet (>280 chars)`.
   - Optional `@handle` input for account-specific context.
2. **One-Click Quick Presets**:
   - 🎭 **Shakespeare (Copied)**: Tests duplicate quote detection against web sources.
   - 🚀 **Novel Tech Launch (Original)**: Tests novel announcement claims.
   - ☕ **Casual Greeting**: Tests trivial social chatter bypass (0 API cost).
   - 💰 **Crypto Shilling**: Tests repetitive copypasta detection.
3. **Instant Algorithm Audit**:
   - **Originality Gauge (0–100)**: Color-coded radial score meter (`ORIGINAL`, `PARAPHRASED`, or `COPIED`).
   - **Safe to Publish Banner**: Green checkmark for high originality or red warning banner for duplicates.
   - **Twitter/X Algorithm Impact Breakdown**: Actionable advice on reach boost vs. shadow suppression in the recommendation feed.
   - **Matching Web Source Card**: Displays the matched web page title, URL, snippet, and similarity percentage.
4. **Recent Audits History**:
   - Maintains an interactive session history table of all tested drafts with timestamp, verdict, score, and inspect action.

---

## ⚙️ How the Originality Algorithm Works

In the open-source **Twitter/X recommendation algorithm**, posts undergo duplicate detection and clustering. Copied content and repetitive spam are heavily downranked in the "For You" timeline, whereas original claims, insights, and media receive engagement multipliers.

To emulate this without incurring massive search engine and LLM fees, this microservice runs an intelligent **4-stage cascade**:

```text
                             [ Draft or Ingested Tweet ]
                                          │
                 ┌────────────────────────┴────────────────────────┐
                 ▼                                                 ▼
      [ Stage 1: Heuristic Filter ]                      [ Substantive Claim ]
    (Short chatter, greetings, "gm",                               │
     subjective feelings, < 4 words)                               │
                 │                                                 │
          Score: 100/100                                           ▼
          Verdict: ORIGINAL                             [ Stage 2: Web Search ]
          (0 API calls used)                            (DuckDuckGo Lite / Google CSE / Brave)
                                                                   │
                                                                   ▼
                                                     [ Stage 3: Lexical Gating ]
                                                 (Shingle Jaccard + Levenshtein)
                                                                   │
                            ┌──────────────────────────────────────┼──────────────────────────────────────┐
                            ▼                                      ▼                                      ▼
                   [ Similarity < 20% ]                 [ 20% <= Sim <= 85% ]                   [ Similarity > 85% ]
                    Score: 90–100/100                  [ Stage 4: LLM Judge ]                    Score: 10–30/100
                    Verdict: ORIGINAL                  (Google Gemini Flash /                   Verdict: COPIED
                    (No LLM needed)                      Anthropic Claude)                      (No LLM needed)
```

1. **Stage 1: Claim Detection (`src/claim_detector.rs`)**:
   - Trivial chatter like *"Good morning everyone! Have an awesome day"* or *"lol so true"* bypasses external searches. Saves **40%–60%** of search quota.
2. **Stage 2: Zero-Cost / Free-Tier Web Search (`src/search/`)**:
   - **DuckDuckGo Lite (Default)**: Zero cost, zero keys. Built-in HTML scraper with anti-bot resistance.
   - **Google Custom Search Engine (CSE)**: 100 free queries/day.
   - **Brave Search API**: 2,000 free queries/month.
3. **Stage 3: Lexical Gating (`src/similarity.rs`)**:
   - Compares the text against web snippets. If similarity is clearly near-zero (<20%) or near-identical (>85%), it assigns the score immediately without LLM costs.
4. **Stage 4: LLM Originality Judge (`src/llm_judge.rs`)**:
   - For ambiguous cases (20%–85%), an LLM differentiates legitimate citations and commentary from covert paraphrasing.
   - Supported backends: **Google Gemini Flash** (Free tier on Google AI Studio) and **Anthropic Claude**.
5. **Stage 5: Aggregation & History (`src/aggregate.rs`)**:
   - Rolls up per-tweet scores into 30-day or 90-day account health reports.

---

## 📡 API Endpoints

| Method | Path | Description |
|---|---|---|
| `GET` | `/` | **Interactive Web Dashboard** (Tweet composer & algorithm verifier) |
| `GET` | `/health` | Service liveness & health check |
| `POST` | `/verify` or `/check` | **Pre-publish draft audit**: check a draft *before* posting to X |
| `POST` | `/accounts/:handle/analyze` | Live-fetch recent tweets from X API v2 (auto-resolves username & `note_tweet`) |
| `POST` | `/accounts/:handle/import` | Bulk-import tweets from official X data export archives (0 X API cost) |
| `GET` | `/accounts/:handle/analyses` | Get raw per-tweet originality results for an account |
| `GET` | `/accounts/:handle/report?days=30` | Rolling-window account originality distribution and score |

---

### Example Requests

#### 1. Verify Draft Tweet (Pre-Publish Check)
```bash
curl -X POST http://localhost:8080/verify \
  -H "Content-Type: application/json" \
  -d '{
    "text": "We just open-sourced our custom asynchronous zero-allocation ring buffer written in Rust.",
    "handle": "my_handle"
  }'
```

**Response:**
```json
{
  "text": "We just open-sourced our custom asynchronous zero-allocation ring buffer written in Rust.",
  "originality_score": 90.98,
  "verdict": "original",
  "safe_to_publish": true,
  "advice": "High originality: Safe to publish with strong potential algorithmic reach.",
  "best_match": null,
  "method": "lexical",
  "analyzed_at": "2026-09-18T06:40:11.717Z"
}
```

#### 2. Analyze Account via X API
```bash
curl -X POST http://localhost:8080/accounts/elonmusk/analyze \
  -H "Content-Type: application/json" \
  -d '{"lookback_days": 30}'
```

#### 3. Bulk Import from Twitter Archive JSON
```bash
curl -X POST http://localhost:8080/accounts/myuser/import \
  -H "Content-Type: application/json" \
  -d '{
    "tweets": [
      {
        "id": "1800000000000000001",
        "text": "First tweet text here...",
        "created_at": "2026-08-01T12:00:00Z"
      }
    ]
  }'
```

#### 4. Get 30-Day Aggregated Report
```bash
curl http://localhost:8080/accounts/myuser/report?days=30
```

---

## ⚙️ Configuration (`.env`)

Create a `.env` file in the root directory (or copy `.env.example`). **No keys are required to get started** — DuckDuckGo search is active by default.

```env
PORT=8080

# --- X (Twitter) API v2 (Optional: needed for POST /accounts/:handle/analyze) ---
# Automatically resolves usernames and long-form note_tweets
X_BEARER_TOKEN=

# --- Search Backend (Default: DuckDuckGo Lite, 100% FREE with 0 API keys) ---
# Optional: Google Programmable Search Engine (100 free queries/day)
GOOGLE_CSE_API_KEY=
GOOGLE_CSE_CX=

# Optional: Brave Search API (2,000 free queries/month)
BRAVE_SEARCH_API_KEY=

# Optional: SerpApi
SERPAPI_API_KEY=

# Optional: Disable web searching entirely (scores all tweets 100% original)
# DISABLE_SEARCH=1

# --- LLM Originality Judge ---
# Google Gemini Flash (RECOMMENDED: 1,500 free queries/day via Google AI Studio)
GEMINI_API_KEY=
GEMINI_MODEL=gemini-1.5-flash

# Anthropic Claude (Alternative fallback)
ANTHROPIC_API_KEY=

# Optional: Fast lexical scoring only (disables LLM stage)
# DISABLE_LLM_JUDGE=1

RUST_LOG=originality_checker=info
```

---

## 🚀 Running the Project

### Local Execution
```bash
# Run the microservice and UI
cargo run

# Visit in your browser
open http://localhost:8080
```

### Running Tests
```bash
cargo test
```

### Docker
```bash
# Build Docker image
docker build -t originality-checker .

# Run container with environment file
docker run -p 8080:8080 --env-file .env originality-checker
```

---

## 🔒 Privacy & Safety
- **No Draft Storage**: The pre-publish endpoints (`/verify` and `/check`) analyze drafts ephemerally in-memory and do not store drafts in the account database.
- **Quota Safeguards**: The 4-stage cascade automatically minimizes external requests to keep API utilization well within free tier limits.
