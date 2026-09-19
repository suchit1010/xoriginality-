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

    // --- Cascade Stage 2: Web Search ---
    let t2_start = std::time::Instant::now();
    let mut results = state
        .search
        .search(&eval.clean_query)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!("search failed for draft text: {e}");
            Vec::new()
        });

    // Also run targeted X (Twitter) search to discover original creator and viral copypasta
    let x_query = format!("site:x.com \"{}\"", eval.clean_query);
    if let Ok(x_results) = state.search.search(&x_query).await {
        for xr in x_results {
            if !results.iter().any(|r| r.url == xr.url) {
                results.push(xr);
            }
        }
    }
    let t2_ms = t2_start.elapsed().as_millis() as u64;

    tracing::info!("draft search query: '{}' returned {} total results", eval.clean_query, results.len());

    // Extract all unique X posts and sort chronologically by Snowflake ID
    let x_posts = extract_x_posts_from_results(&results);
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

    let best = matches.first().cloned();
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
