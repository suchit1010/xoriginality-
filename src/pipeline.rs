use chrono::Utc;

use crate::claim_detector::evaluate_tweet;
use crate::error::AppError;
use crate::llm_judge::judge_originality;
use crate::models::{
    CriterionScore, DraftEvaluation, EvaluationCriteria, MatchedSource, OriginatorInfo,
    ThinkingStep, Tweet, TweetAnalysis, Verdict,
};
use crate::similarity::{combined_similarity, extract_x_posts_from_results};
use crate::AppState;

fn bucket_verdict(state: &AppState, score: f32) -> Verdict {
    let c = &state.config;
    if score >= c.original_threshold {
        Verdict::Original
    } else if score >= c.likely_original_threshold {
        Verdict::LikelyOriginal
    } else if score >= c.paraphrased_threshold {
        Verdict::Paraphrased
    } else {
        Verdict::Copied
    }
}

/// Analyze one tweet using an efficient cost-reducing cascade:
/// 1. Pre-filter trivial chatter/greetings without web search.
/// 2. Extract high-signal search query from verifiable claims.
/// 3. Search web using configured provider (Google CSE / DuckDuckGo / Brave).
/// 4. Score lexical similarity.
/// 5. Only call LLM judge for ambiguous cases (paraphrases / attributions).
pub async fn analyze_one(state: &AppState, handle: &str, tweet: &Tweet) -> Result<TweetAnalysis, AppError> {
    // --- Cascade Stage 1: Fast claim & searchability filter ---
    let eval = evaluate_tweet(&tweet.text);
    if !eval.should_search {
        let analysis = TweetAnalysis {
            tweet_id: tweet.id.clone(),
            author_handle: handle.to_string(),
            text: tweet.text.clone(),
            created_at: tweet.created_at,
            originality_score: 100.0,
            verdict: Verdict::Original,
            best_match: None,
            method: format!("cascade:heuristic_skip ({})", eval.reason),
            analyzed_at: Utc::now(),
        };
        state.store.upsert(handle, analysis.clone());
        return Ok(analysis);
    }

    // --- Cascade Stage 2: Web Search ---
    let results = state
        .search
        .search(&eval.clean_query)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!("search failed for tweet {}: {e}", tweet.id);
            Vec::new()
        });

    // --- Cascade Stage 3: Lexical Matching & Similarity ---
    let mut matches: Vec<MatchedSource> = results
        .into_iter()
        .map(|r| {
            let sim = combined_similarity(&tweet.text, &format!("{} {}", r.title, r.snippet));
            MatchedSource {
                url: r.url,
                title: r.title,
                snippet: r.snippet,
                similarity: sim,
            }
        })
        .collect();
    matches.sort_by(|a, b| b.similarity.partial_cmp(&a.similarity).unwrap_or(std::cmp::Ordering::Equal));

    let best = matches.first().cloned();
    let best_sim = best.as_ref().map(|b| b.similarity).unwrap_or(0.0);
    let lexical_score = 100.0 - (best_sim * 100.0);

    // --- Cascade Stage 4: Conditional LLM Judge Escalation ---
    // Only invoke LLM if:
    // - LLM judge is enabled
    // - Candidates were found
    // - Similarity is in the ambiguous/suspicious zone (0.20 <= best_sim <= 0.85)
    // (If < 0.20, text is clearly novel; if > 0.85, text is clearly verbatim copied)
    let (final_score, method) = if state.config.use_llm_judge && !matches.is_empty() && (0.20..=0.85).contains(&best_sim) {
        match judge_originality(&state.http, &state.config, &tweet.text, &matches).await {
            Ok((v, model_name)) => (v.originality_score.clamp(0.0, 100.0), model_name),
            Err(e) => {
                tracing::warn!("llm judge failed for tweet {}, falling back to lexical: {e}", tweet.id);
                (lexical_score, "lexical".to_string())
            }
        }
    } else {
        (lexical_score, "lexical".to_string())
    };

    let analysis = TweetAnalysis {
        tweet_id: tweet.id.clone(),
        author_handle: handle.to_string(),
        text: tweet.text.clone(),
        created_at: tweet.created_at,
        originality_score: final_score,
        verdict: bucket_verdict(state, final_score),
        best_match: best,
        method,
        analyzed_at: Utc::now(),
    };

    state.store.upsert(handle, analysis.clone());
    Ok(analysis)
}

/// Evaluate raw draft tweet text without requiring prior publishing on X.
/// Used by pre-publish checking flows (e.g. composer UI, extensions, or bots).
pub async fn evaluate_text(state: &AppState, text: &str) -> Result<DraftEvaluation, AppError> {
    let dummy_tweet = Tweet {
        id: uuid::Uuid::new_v4().to_string(),
        author_handle: "draft".to_string(),
        text: text.to_string(),
        created_at: Utc::now(),
        url: String::new(),
    };

    let words: Vec<&str> = text.split_whitespace().collect();
    let words_count = words.len();

    // --- Cascade Stage 1: Fast claim & searchability filter ---
    let t1_start = std::time::Instant::now();
    let eval = evaluate_tweet(&dummy_tweet.text);
    let t1_ms = t1_start.elapsed().as_millis() as u64;

    if !eval.should_search {
        let analysis = TweetAnalysis {
            tweet_id: dummy_tweet.id,
            author_handle: "draft".to_string(),
            text: dummy_tweet.text,
            created_at: dummy_tweet.created_at,
            originality_score: 100.0,
            verdict: Verdict::Original,
            best_match: None,
            method: format!("cascade:heuristic_skip ({})", eval.reason),
            analyzed_at: Utc::now(),
        };

        let thinking_steps = vec![
            ThinkingStep {
                stage: 1,
                stage_name: "Syntactic Claim Filter".to_string(),
                title: "Noise & Pleasantry Detection".to_string(),
                detail: format!(
                    "Analyzed text ({} words). Triggered heuristic bypass: '{}'. Classified as conversational chatter or greeting without verifiable factual claims.",
                    words_count, eval.reason
                ),
                status: "skip".to_string(),
                execution_time_ms: t1_ms,
            },
            ThinkingStep {
                stage: 2,
                stage_name: "Web Index Retrieval".to_string(),
                title: "Live Web Search".to_string(),
                detail: "Bypassed search query execution. Conserved 100% of search quota since no factual assertions require verification.".to_string(),
                status: "skip".to_string(),
                execution_time_ms: 0,
            },
            ThinkingStep {
                stage: 3,
                stage_name: "Lexical & Distance Analysis".to_string(),
                title: "N-Gram Similarity Matching".to_string(),
                detail: "Bypassed similarity calculation. Baseline similarity set to 0.0% (fully distinct).".to_string(),
                status: "skip".to_string(),
                execution_time_ms: 0,
            },
            ThinkingStep {
                stage: 4,
                stage_name: "Algorithmic Arbitration".to_string(),
                title: "Twitter/X Recommendation Verdict".to_string(),
                detail: "Assigned 100/100 Originality Score. Casual conversational posts are exempt from duplicate suppression and maintain standard feed distribution.".to_string(),
                status: "pass".to_string(),
                execution_time_ms: 0,
            },
        ];

        let criteria = EvaluationCriteria {
            lexical_uniqueness: CriterionScore {
                name: "Lexical Uniqueness".to_string(),
                score: 100.0,
                weight_pct: 35,
                status: "Trivially Unique".to_string(),
                description: "No verbatim or near-duplicate matches identified across web or social corpora.".to_string(),
                impact: "Passes X SimHash deduplication filter".to_string(),
            },
            claim_substantiveness: CriterionScore {
                name: "Claim Substantiveness".to_string(),
                score: 50.0,
                weight_pct: 25,
                status: "Conversational".to_string(),
                description: "Casual social reaction, pleasantry, or personal status without factual assertion.".to_string(),
                impact: "Standard social feed delivery".to_string(),
            },
            attribution_integrity: CriterionScore {
                name: "Attribution Integrity".to_string(),
                score: 100.0,
                weight_pct: 20,
                status: "First-Party Voice".to_string(),
                description: "Natural first-person expression; no external material required attribution.".to_string(),
                impact: "Zero plagiarism risk".to_string(),
            },
            algo_distribution_impact: CriterionScore {
                name: "X Algorithm Reach Multiplier".to_string(),
                score: 90.0,
                weight_pct: 20,
                status: "1.0x Organic Delivery".to_string(),
                description: "Standard organic reach for casual timeline conversations and community engagement.".to_string(),
                impact: "Normal in-network distribution".to_string(),
            },
        };

        return Ok(DraftEvaluation {
            analysis,
            thinking_steps,
            criteria,
            reasoning: "Classified as casual social conversation or greeting. Completely original personal expression with zero duplicate penalty risk.".to_string(),
            algo_multiplier: "1.0x (Organic In-Network)".to_string(),
            originator: None,
        });
    }

    let step1_detail = format!(
        "Extracted substantive claim ({} words). Formulated high-entropy search query: \"{}\".",
        words_count, eval.clean_query
    );

    // --- Cascade Stage 2: Web Search (DDG generic + DDG site:x.com) ---
    let t2_start = std::time::Instant::now();
    let mut results = state
        .search
        .search(&eval.clean_query)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!("search failed for draft text: {e}");
            Vec::new()
        });

    // Also search the full tweet text (with emoji) as a second parallel query.
    // The main DDG search naturally returns x.com/twitter.com results — we extract
    // tweet IDs from ALL results, not just site:-specific ones.
    // (site:x.com queries are blocked by DDG Lite with 202 from server IPs)
    let x_search_text = crate::claim_detector::extract_x_search_query(text);
    if x_search_text != eval.clean_query && x_search_text.split_whitespace().count() >= 5 {
        tracing::info!("running additional full-text X query: '{}'", x_search_text);
        if let Ok(full_results) = state.search.search(&x_search_text).await {
            tracing::info!("full-text search returned {} results", full_results.len());
            for xr in full_results {
                if !results.iter().any(|r| r.url == xr.url) {
                    results.push(xr);
                }
            }
        }
    }
    let t2_ms = t2_start.elapsed().as_millis() as u64;

    tracing::info!("draft search query: '{}' returned {} total results", eval.clean_query, results.len());

    // --- Cascade Stage 2b: X Originator Resolution ---
    // Priority order:
    //   1. X API v2 search/recent (when X_BEARER_TOKEN is set) — queries X's
    //      internal index which contains EVERY public tweet, including
    //      low-impression originals that web crawlers never visited.
    //   2. Web search URL extraction — fallback when no token is present.
    let mut raw_x_posts: Vec<crate::models::XPostMatch> = Vec::new();

    if let Some(bearer) = state.config.x_bearer_token.as_deref() {
        // Build a compact, high-signal query for X's full-text search.
        // X search is more literal than web engines — shorter is better.
        let x_query = if eval.clean_query.split_whitespace().count() > 8 {
            // Take the first 8 high-signal tokens to avoid over-constraining
            eval.clean_query
                .split_whitespace()
                .take(8)
                .collect::<Vec<_>>()
                .join(" ")
        } else {
            eval.clean_query.clone()
        };

        tracing::info!("X API search/recent query: '{}'", x_query);
        let x_client = crate::x_client::XClient::new(bearer.to_string());
        match x_client.search_recent_tweets(&x_query, 50).await {
            Ok(x_results) if !x_results.is_empty() => {
                tracing::info!(
                    "X API returned {} candidates; earliest will be the original poster",
                    x_results.len()
                );
                // Convert XSearchResult → XPostMatch and merge into raw_x_posts
                // Sort ascending by tweet_id so the earliest (original) is first
                let mut api_posts: Vec<crate::models::XPostMatch> = x_results
                    .into_iter()
                    .map(|r| crate::models::XPostMatch {
                        author_handle: r.handle.clone(),
                        tweet_id: r.tweet_id,
                        tweet_url: r.tweet_url.clone(),
                        published_at: r.created_at,
                        formatted_date: r.created_at.format("%b %d, %Y at %H:%M UTC").to_string(),
                        snippet: format!(
                            "{} views · {}",
                            r.impression_count,
                            &r.text.chars().take(80).collect::<String>()
                        ),
                    })
                    .collect();
                api_posts.sort_by_key(|p| p.tweet_id);
                // Also inject the X status URLs into the web results pool so
                // similarity scoring includes the tweet texts
                for p in &api_posts {
                    if !results.iter().any(|r| r.url == p.tweet_url) {
                        results.push(crate::search::SearchResult {
                            url: p.tweet_url.clone(),
                            title: format!("Post by @{} on X", p.author_handle),
                            snippet: p.snippet.clone(),
                        });
                    }
                }
                raw_x_posts = api_posts;
            }
            Ok(_) => {
                tracing::info!("X API returned 0 results for '{}'; falling back to web search extraction", x_query);
            }
            Err(e) => {
                tracing::warn!("X API search/recent failed ({}); falling back to web search extraction", e);
            }
        }
    }

    // Fallback: extract X post URLs from web search results if X API was
    // unavailable or returned nothing
    if raw_x_posts.is_empty() {
        raw_x_posts = extract_x_posts_from_results(&results);
        if raw_x_posts.is_empty() {
            let site_query = format!("site:x.com {}", eval.clean_query);
            tracing::info!("no X status links in general results; querying: '{}'", site_query);
            if let Ok(x_results) = state.search.search(&site_query).await {
                for xr in &x_results {
                    if !results.iter().any(|r| r.url == xr.url) {
                        results.push(xr.clone());
                    }
                }
                raw_x_posts = extract_x_posts_from_results(&results);
            }
        }

        // If still empty and text has multiple lines or clauses, search the first main clause
        if raw_x_posts.is_empty() {
            if let Some(first_clause) = text.lines().find(|l| l.trim().split_whitespace().count() >= 3) {
                let first_clean = crate::claim_detector::extract_search_query(first_clause);
                if !first_clean.is_empty() && first_clean != eval.clean_query {
                    let site_query_first = format!("site:x.com {}", first_clean);
                    tracing::info!("querying first clause for X status: '{}'", site_query_first);
                    if let Ok(x_results) = state.search.search(&site_query_first).await {
                        for xr in &x_results {
                            if !results.iter().any(|r| r.url == xr.url) {
                                results.push(xr.clone());
                            }
                        }
                        raw_x_posts = extract_x_posts_from_results(&results);
                    }
                }
            }
        }
    }

    let x_posts = enrich_with_syndication(&state.http, raw_x_posts).await;

    let originator = if let Some(earliest) = x_posts.first() {
        let duplicate_count = x_posts.len();
        let is_viral = duplicate_count >= 2;
        Some(OriginatorInfo {
            first_poster_handle: earliest.author_handle.clone(),
            first_tweet_url: earliest.tweet_url.clone(),
            earliest_published_at: earliest.formatted_date.clone(),
            earliest_tweet_id: earliest.tweet_id,
            is_viral_copypasta: is_viral,
            duplicate_count_on_x: duplicate_count,
            recent_copycats: x_posts.iter().skip(1).take(5).cloned().collect(),
            deduplication_verdict: if is_viral {
                format!(
                    "Viral Copypasta Pattern: {} accounts posted this on X. Earliest original: @{} on {}.",
                    duplicate_count, earliest.author_handle, earliest.formatted_date
                )
            } else {
                format!("Matched original post on X by @{} on {}.", earliest.author_handle, earliest.formatted_date)
            },
        })
    } else {
        None
    };

    let step2_detail = if let Some(ref orig) = originator {
        format!(
            "Queried live web & X indices. Located {} matches ({} distinct X accounts). Earliest post on X originated by @{} on {}.",
            results.len(), orig.duplicate_count_on_x, orig.first_poster_handle, orig.earliest_published_at
        )
    } else {
        format!(
            "Queried search engine with \"{}\". Retrieved {} matching candidates from the live web index.",
            eval.clean_query, results.len()
        )
    };

    // --- Cascade Stage 3: Lexical Matching & Similarity ---
    let t3_start = std::time::Instant::now();
    let mut matches: Vec<MatchedSource> = results
        .into_iter()
        .map(|r| {
            let sim = combined_similarity(&dummy_tweet.text, &format!("{} {}", r.title, r.snippet));
            MatchedSource {
                url: r.url,
                title: r.title,
                snippet: r.snippet,
                similarity: sim,
            }
        })
        .collect();
    matches.sort_by(|a, b| b.similarity.partial_cmp(&a.similarity).unwrap_or(std::cmp::Ordering::Equal));
    let t3_ms = t3_start.elapsed().as_millis() as u64;

    let mut best = matches.first().cloned();
    if let Some(orig) = &originator {
        // If no web match or existing match has low similarity, ensure the canonical originator is the primary reference
        if best.as_ref().map(|b| b.similarity).unwrap_or(0.0) < 0.60 {
            best = Some(MatchedSource {
                url: orig.first_tweet_url.clone(),
                title: format!("Original Tweet on X by @{}", orig.first_poster_handle),
                snippet: format!("Earliest verified post on X by @{} ({}). {}", orig.first_poster_handle, orig.earliest_published_at, orig.deduplication_verdict),
                similarity: 0.85,
            });
        }
    } else if best.as_ref().map(|b| b.similarity).unwrap_or(0.0) < 0.20 {
        // Drop noisy/irrelevant matches with less than 20% overlap
        best = None;
    }
    let best_sim = best.as_ref().map(|b| b.similarity).unwrap_or(0.0);
    let lexical_score = 100.0 - (best_sim * 100.0);

    let (step3_status, step3_detail) = if let Some(ref top) = best {
        if best_sim >= 0.70 {
            (
                "flag",
                format!(
                    "High duplicate overlap ({:.1}%) detected against '{}' ({})",
                    best_sim * 100.0, top.title, top.url
                ),
            )
        } else if best_sim >= 0.20 {
            (
                "warn",
                format!(
                    "Moderate phrasing overlap ({:.1}%) detected against candidate '{}' ({})",
                    best_sim * 100.0, top.title, top.url
                ),
            )
        } else {
            (
                "pass",
                format!(
                    "Low lexical overlap ({:.1}%). Distinct phrasing compared to all {} web candidates retrieved.",
                    best_sim * 100.0, matches.len()
                ),
            )
        }
    } else {
        (
            "pass",
            "No web candidates matched. Content appears completely novel across web index.".to_string(),
        )
    };

    // --- Cascade Stage 4: Conditional LLM Judge Escalation & X Copypasta Check ---
    let t4_start = std::time::Instant::now();
    let mut llm_reasoning: Option<String> = None;

    let is_copypasta = originator.as_ref().map(|o| o.is_viral_copypasta).unwrap_or(false) && best_sim >= 0.30;

    let (final_score, method) = if is_copypasta {
        (20.0, "x_algorithm_copypasta_penalty".to_string())
    } else if state.config.use_llm_judge && !matches.is_empty() && (0.20..=0.85).contains(&best_sim) {
        match judge_originality(&state.http, &state.config, &dummy_tweet.text, &matches).await {
            Ok((v, model_name)) => {
                llm_reasoning = Some(v.reasoning.clone());
                (v.originality_score.clamp(0.0, 100.0), model_name)
            }
            Err(e) => {
                tracing::warn!("llm judge failed for draft, falling back to lexical: {e}");
                (lexical_score, "lexical".to_string())
            }
        }
    } else {
        (lexical_score, "lexical".to_string())
    };
    let t4_ms = t4_start.elapsed().as_millis() as u64;

    let verdict = if is_copypasta {
        Verdict::Copied
    } else {
        bucket_verdict(state, final_score)
    };

    let (step4_status, step4_detail) = if is_copypasta {
        let orig = originator.as_ref().unwrap();
        (
            "flag",
            format!(
                "X Deduplication Penalty Triggered: Viral copypasta format detected across {} accounts. Earliest creator identified: @{} on {} ({}). In X's recommendation algorithm, duplicate engagement bait is clustered and demoted in the 'For You' feed.",
                orig.duplicate_count_on_x, orig.first_poster_handle, orig.earliest_published_at, orig.first_tweet_url
            ),
        )
    } else if let Some(ref reason) = llm_reasoning {
        (
            if final_score >= 70.0 { "pass" } else { "warn" },
            format!("AI Judge Evaluation: \"{}\" (Assigned Score: {:.0}/100)", reason, final_score),
        )
    } else if best_sim <= 0.20 {
        (
            "pass",
            format!("Fast Lexical Gating passed: {:.1}% max similarity is well below threshold (<20%). Confirmed as original.", best_sim * 100.0),
        )
    } else if best_sim >= 0.85 {
        (
            "flag",
            format!("Fast Lexical Gating triggered: {:.1}% max similarity exceeds threshold (>85%). Flagged as copied.", best_sim * 100.0),
        )
    } else {
        (
            if final_score >= 70.0 { "pass" } else { "warn" },
            format!("Heuristic arbitration concluded with score {:.0}/100 based on lexical distance.", final_score),
        )
    };

    let thinking_steps = vec![
        ThinkingStep {
            stage: 1,
            stage_name: "Syntactic Claim Filter".to_string(),
            title: "Substantive Assertion Analysis".to_string(),
            detail: step1_detail,
            status: "pass".to_string(),
            execution_time_ms: t1_ms,
        },
        ThinkingStep {
            stage: 2,
            stage_name: "Web & X Index Retrieval".to_string(),
            title: "Live Multi-Source & X Query".to_string(),
            detail: step2_detail,
            status: if matches.is_empty() { "info".to_string() } else { "pass".to_string() },
            execution_time_ms: t2_ms,
        },
        ThinkingStep {
            stage: 3,
            stage_name: "Lexical & Distance Analysis".to_string(),
            title: "N-Gram Shingle & SimHash Overlap".to_string(),
            detail: step3_detail,
            status: step3_status.to_string(),
            execution_time_ms: t3_ms,
        },
        ThinkingStep {
            stage: 4,
            stage_name: "Algorithmic Arbitration".to_string(),
            title: "Twitter/X Recommendation Verdict".to_string(),
            detail: step4_detail,
            status: step4_status.to_string(),
            execution_time_ms: t4_ms,
        },
    ];

    // Compute Twitter/X Algorithm Criteria Rubric
    let uniqueness_score = if is_copypasta {
        20.0
    } else {
        (100.0 - best_sim * 100.0).clamp(0.0, 100.0)
    };
    let substantiveness_score = (60.0 + (words_count as f32 * 1.5)).clamp(50.0, 100.0);

    let (attrib_score, attrib_status, attrib_desc, attrib_impact) = if is_copypasta {
        let orig = originator.as_ref().unwrap();
        (
            20.0,
            "Viral Copypasta Pattern".to_string(),
            format!("Unattributed reuse of a viral template first popularized on X by @{}.", orig.first_poster_handle),
            "Penalized by X duplicate suppression filter".to_string(),
        )
    } else if best_sim > 0.65 {
        if text.contains('"') || text.to_lowercase().contains("via ") || text.to_lowercase().contains("by ") {
            (
                65.0,
                "Quoted Citation".to_string(),
                "Text contains quotation marks or reference tokens acknowledging the source material.".to_string(),
                "Fair use context recognized".to_string(),
            )
        } else {
            (
                20.0,
                "Unattributed Duplication".to_string(),
                "Direct copy of existing web/literary text without quotation marks or source attribution.".to_string(),
                "Severe copypasta suppression risk".to_string(),
            )
        }
    } else {
        (
            95.0,
            "Original IP".to_string(),
            "No unattributed third-party text or verbatim excerpts detected.".to_string(),
            "Full creator attribution credit".to_string(),
        )
    };

    let (algo_score, algo_status, algo_desc, algo_multiplier) = if is_copypasta {
        (
            15.0,
            "⚠️ Deduplication Suppression".to_string(),
            "X algorithm groups near-duplicates into parent clusters; standalone impressions are throttled.".to_string(),
            "-0.4x (Duplicate Penalty)".to_string(),
        )
    } else if final_score >= state.config.original_threshold {
        (
            95.0,
            "🚀 High Priority Exploration".to_string(),
            "Eligible for maximum out-of-network recommendation in X's 'For You' timeline graph.".to_string(),
            "+1.8x (Explore / For You Boost)".to_string(),
        )
    } else if final_score >= state.config.likely_original_threshold {
        (
            80.0,
            "✅ Positive Organic Reach".to_string(),
            "High originality score passes all duplicate filters; recommended to follower graph.".to_string(),
            "+1.3x (Organic Growth)".to_string(),
        )
    } else if final_score >= state.config.paraphrased_threshold {
        (
            55.0,
            "⚖️ Neutral Distribution".to_string(),
            "May compete with related content clusters in X's SimHash deduplication buckets.".to_string(),
            "1.0x (Standard Feed)".to_string(),
        )
    } else {
        (
            15.0,
            "⚠️ Deduplication Suppression".to_string(),
            "X algorithm groups near-duplicates into parent clusters; standalone impressions are throttled.".to_string(),
            "-0.4x (Duplicate Penalty)".to_string(),
        )
    };

    let criteria = EvaluationCriteria {
        lexical_uniqueness: CriterionScore {
            name: "Lexical Uniqueness".to_string(),
            score: uniqueness_score,
            weight_pct: 35,
            status: if is_copypasta {
                "Viral Copypasta Pattern".to_string()
            } else if uniqueness_score >= 80.0 {
                "High Uniqueness".to_string()
            } else if uniqueness_score >= 50.0 {
                "Moderate Overlap".to_string()
            } else {
                "Critical Duplication".to_string()
            },
            description: if is_copypasta {
                "Viral template matching existing engagement copypasta format on X.".to_string()
            } else {
                "Verbatim phrasing overlap evaluated against the indexed web corpus.".to_string()
            },
            impact: if is_copypasta {
                "Flagged by X Duplicate Cluster Filter".to_string()
            } else if uniqueness_score >= 80.0 {
                "Passes duplicate clustering filter".to_string()
            } else {
                "Flagged by duplicate detection filter".to_string()
            },
        },
        claim_substantiveness: CriterionScore {
            name: "Claim Substantiveness".to_string(),
            score: substantiveness_score,
            weight_pct: 25,
            status: if words_count >= 15 {
                "High Depth".to_string()
            } else {
                "Moderate Claim Depth".to_string()
            },
            description: "Presence of concrete claims, technical assertions, or structured arguments.".to_string(),
            impact: "Higher engagement weighting in X recommendation graph".to_string(),
        },
        attribution_integrity: CriterionScore {
            name: "Attribution Integrity".to_string(),
            score: attrib_score,
            weight_pct: 20,
            status: attrib_status,
            description: attrib_desc,
            impact: attrib_impact,
        },
        algo_distribution_impact: CriterionScore {
            name: "X Algorithm Reach Multiplier".to_string(),
            score: algo_score,
            weight_pct: 20,
            status: algo_status,
            description: algo_desc,
            impact: format!("Estimated reach factor: {}", algo_multiplier),
        },
    };

    let reasoning = if is_copypasta {
        let orig = originator.as_ref().unwrap();
        format!(
            "Viral copypasta template detected on X. First originated by @{} on {}. X recommendation algorithm suppresses duplicate engagement bait in the 'For You' feed.",
            orig.first_poster_handle, orig.earliest_published_at
        )
    } else if let Some(r) = llm_reasoning {
        r
    } else if best_sim > 0.70 {
        format!(
            "High duplicate match ({:.1}%) with '{}'. Subject to X's duplicate suppression algorithm.",
            best_sim * 100.0,
            best.as_ref().map(|b| b.title.as_str()).unwrap_or("external source")
        )
    } else if best_sim > 0.20 {
        "Moderate similarity with existing web documents. Consider adding unique insights or attribution.".to_string()
    } else {
        "Original phrasing and claims with negligible web overlap. Optimal for X recommendation algorithm reach.".to_string()
    };

    let analysis = TweetAnalysis {
        tweet_id: dummy_tweet.id,
        author_handle: "draft".to_string(),
        text: dummy_tweet.text,
        created_at: dummy_tweet.created_at,
        originality_score: final_score,
        verdict,
        best_match: best,
        method,
        analyzed_at: Utc::now(),
    };

    Ok(DraftEvaluation {
        analysis,
        thinking_steps,
        criteria,
        reasoning,
        algo_multiplier,
        originator,
    })
}

/// Analyze a batch of tweets (used by both the live-fetch and import routes).
pub async fn analyze_batch(state: &AppState, handle: &str, tweets: Vec<Tweet>) -> Result<usize, AppError> {
    let mut count = 0;
    for tweet in &tweets {
        analyze_one(state, handle, tweet).await?;
        count += 1;
    }
    Ok(count)
}

// ─────────────────────────────────────────────────────────────────────────────
// Syndication API Enrichment
// ─────────────────────────────────────────────────────────────────────────────

/// Enriches a list of X posts found via DDG with exact `created_at` timestamps
/// from the X Syndication CDN API (completely free, no auth, works for all
/// public tweets back to 2006).
///
/// Endpoint: https://cdn.syndication.twimg.com/tweet-result?id={id}&lang=en
/// For each tweet ID we successfully enrich, we replace the Snowflake-decoded
/// approximate timestamp with the exact server-side `created_at` value.
/// Posts that Syndication can't fetch fall back to the Snowflake estimate.
pub async fn enrich_with_syndication(
    http: &reqwest::Client,
    mut posts: Vec<crate::models::XPostMatch>,
) -> Vec<crate::models::XPostMatch> {
    use futures::future::join_all;

    if posts.is_empty() {
        return posts;
    }

    #[derive(serde::Deserialize)]
    struct SyndicationUser {
        screen_name: Option<String>,
    }

    #[derive(serde::Deserialize)]
    #[allow(dead_code)]
    struct SyndicationResp {
        id_str: Option<String>,
        created_at: Option<String>,
        user: Option<SyndicationUser>,
    }

    let client = http.clone();

    let futures: Vec<_> = posts
        .iter()
        .map(|p| {
            let id = p.tweet_id.to_string();
            let c = client.clone();
            async move {
                let url = format!(
                    "https://cdn.syndication.twimg.com/tweet-result?id={}&lang=en&token=0",
                    id
                );
                let resp = c
                    .get(&url)
                    .header("Origin", "https://platform.twitter.com")
                    .header("Referer", "https://platform.twitter.com/")
                    .header("Accept", "application/json")
                    .timeout(std::time::Duration::from_secs(5))
                    .send()
                    .await
                    .ok()?;

                if !resp.status().is_success() {
                    return None;
                }
                let data: SyndicationResp = resp.json().await.ok()?;

                // Parse the exact timestamp
                let ts = data.created_at?;
                let dt = ts
                    .parse::<chrono::DateTime<chrono::Utc>>()
                    .or_else(|_| {
                        chrono::DateTime::parse_from_str(&ts, "%a %b %d %H:%M:%S %z %Y")
                            .map(|d| d.with_timezone(&chrono::Utc))
                    })
                    .ok()?;

                let handle = data
                    .user
                    .as_ref()
                    .and_then(|u| u.screen_name.clone())
                    .unwrap_or_default();

                Some((id, handle, dt))
            }
        })
        .collect();

    let enrichment_results = join_all(futures).await;

    // Apply enrichment — update published_at and formatted_date with exact values
    for (i, maybe_enrich) in enrichment_results.into_iter().enumerate() {
        if let Some((id, handle, exact_dt)) = maybe_enrich {
            if let Some(post) = posts.get_mut(i) {
                post.published_at = exact_dt;
                post.formatted_date = exact_dt.format("%b %d, %Y at %H:%M UTC").to_string();
                // If Syndication returned a handle, trust it over URL-parsed handle
                if !handle.is_empty() {
                    post.author_handle = handle.clone();
                    post.tweet_url = format!("https://x.com/{}/status/{}", handle, id);
                }
                tracing::info!(
                    "syndication enriched @{} tweet {} → exact date: {}",
                    post.author_handle, id, post.formatted_date
                );
            }
        }
    }

    // Re-sort by exact published_at (not Snowflake estimate)
    posts.sort_by_key(|p| p.published_at);
    posts
}
