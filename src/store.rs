use std::collections::HashMap;
use std::sync::RwLock;

use crate::models::TweetAnalysis;

/// In-memory store keyed by account handle. Good enough for an MVP /
/// single-instance deployment; swap for Postgres/SQLite behind this same
/// interface once you need multi-instance or durable storage - nothing
/// upstream (handlers, pipeline) needs to change.
#[derive(Default)]
pub struct Store {
    inner: RwLock<HashMap<String, Vec<TweetAnalysis>>>,
}

impl Store {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert or replace the analysis for a given tweet id under `handle`.
    pub fn upsert(&self, handle: &str, analysis: TweetAnalysis) {
        let mut guard = self.inner.write().expect("store lock poisoned");
        let entry = guard.entry(handle.to_string()).or_default();
        entry.retain(|a| a.tweet_id != analysis.tweet_id);
        entry.push(analysis);
    }

    pub fn get_all(&self, handle: &str) -> Vec<TweetAnalysis> {
        self.inner
            .read()
            .expect("store lock poisoned")
            .get(handle)
            .cloned()
            .unwrap_or_default()
    }
}
