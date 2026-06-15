#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReputationEntry {
    pub peer_id: String,
    pub score: f64,
    pub successful_redirects: u64,
    pub failed_redirects: u64,
    pub accepted_redirects: u64,
    pub avg_response_ms: u32,
    pub last_seen: u64,
    pub last_positive_event: u64,
    pub last_negative_event: u64,
    pub balance_trust: f64,
}

impl ReputationEntry {
    fn new(peer_id: &str, now: u64) -> Self {
        ReputationEntry {
            peer_id: peer_id.to_string(),
            score: 0.0,
            successful_redirects: 0,
            failed_redirects: 0,
            accepted_redirects: 0,
            avg_response_ms: 0,
            last_seen: now,
            last_positive_event: 0,
            last_negative_event: 0,
            balance_trust: 0.5,
        }
    }
}

pub struct ReputationTable {
    inner: Arc<RwLock<HashMap<String, ReputationEntry>>>,
}

impl ReputationTable {
    pub fn new() -> Self {
        ReputationTable {
            inner: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn get(&self, peer_id: &str) -> Option<ReputationEntry> {
        self.inner.read().await.get(peer_id).cloned()
    }

    pub async fn score(&self, peer_id: &str) -> f64 {
        self.inner
            .read()
            .await
            .get(peer_id)
            .map(|e| {
                if e.score <= -1.0 {
                    -1.0
                } else {
                    compute_score(e.successful_redirects, e.failed_redirects)
                }
            })
            .unwrap_or(0.0)
    }

    pub async fn record_success(&self, peer_id: &str) {
        let mut map = self.inner.write().await;
        let entry = map.entry(peer_id.to_string()).or_insert_with(|| {
            ReputationEntry::new(peer_id, 0)
        });
        entry.successful_redirects += 1;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        entry.last_positive_event = now;
        entry.last_seen = now;
    }

    pub async fn record_failure(&self, peer_id: &str) {
        let mut map = self.inner.write().await;
        let entry = map.entry(peer_id.to_string()).or_insert_with(|| {
            ReputationEntry::new(peer_id, 0)
        });
        entry.failed_redirects += 1;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        entry.last_negative_event = now;
        entry.last_seen = now;
    }

    pub async fn record_receipt(&self, peer_id: &str) {
        self.record_success(peer_id).await;
    }

    pub async fn record_timeout(&self, peer_id: &str) {
        self.record_failure(peer_id).await;
    }

    pub async fn record_rejection(&self, peer_id: &str, reason: &str) {
        if reason != "insufficient_reserve" {
            self.record_failure(peer_id).await;
        }
    }

    pub async fn report_fraud(&self, peer_id: &str) {
        let mut map = self.inner.write().await;
        let entry = map.entry(peer_id.to_string()).or_insert_with(|| {
            ReputationEntry::new(peer_id, 0)
        });
        entry.score = -1.0;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        entry.last_negative_event = now;
        entry.last_seen = now;
    }

    pub async fn is_blacklisted(&self, peer_id: &str) -> bool {
        self.score(peer_id).await <= -1.0
    }

    pub async fn is_stale(&self, peer_id: &str, now: u64, timeout_secs: u64) -> bool {
        self.inner
            .read()
            .await
            .get(peer_id)
            .map(|e| now.saturating_sub(e.last_seen) > timeout_secs)
            .unwrap_or(true)
    }

    pub async fn best_peers(&self, min_balance_trust: f64, limit: usize) -> Vec<(String, f64)> {
        let map = self.inner.read().await;
        let mut peers: Vec<(String, f64)> = map
            .iter()
            .filter(|(_, e)| {
                let s = compute_score(e.successful_redirects, e.failed_redirects);
                s > 0.0 && e.balance_trust >= min_balance_trust
            })
            .map(|(id, e)| (id.clone(), compute_score(e.successful_redirects, e.failed_redirects)))
            .collect();
        peers.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        peers.truncate(limit);
        peers
    }

    pub async fn cleanup_stale(&self, now: u64, timeout_secs: u64) {
        let mut map = self.inner.write().await;
        map.retain(|_, e| now.saturating_sub(e.last_seen) <= timeout_secs);
    }
}

fn compute_score(successes: u64, failures: u64) -> f64 {
    let total = successes + failures;
    if total == 0 {
        return 0.0;
    }
    let ratio = (successes as f64 + 1.0) / (total as f64 + 2.0);
    ratio * 2.0 - 1.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_reputation_score_initial() {
        let table = ReputationTable::new();
        assert_eq!(table.score("unknown").await, 0.0);
    }

    #[tokio::test]
    async fn test_reputation_score_after_success() {
        let table = ReputationTable::new();
        table.record_success("peer1").await;
        let s = table.score("peer1").await;
        assert!(s > 0.0);
    }

    #[tokio::test]
    async fn test_reputation_blacklist() {
        let table = ReputationTable::new();
        table.report_fraud("peer1").await;
        assert!(table.is_blacklisted("peer1").await);
    }

    #[tokio::test]
    async fn test_reputation_blacklist_on_report() {
        let table = ReputationTable::new();
        table.report_fraud("bad").await;
        assert!(table.score("bad").await <= -1.0);
        assert!(table.is_blacklisted("bad").await);
    }

    #[tokio::test]
    async fn test_best_peers_ranking() {
        let table = ReputationTable::new();
        table.record_success("good").await;
        table.record_success("good").await;
        table.record_failure("bad").await;
        let results = table.best_peers(0.0, 10).await;
        assert!(!results.is_empty());
        assert_eq!(results[0].0, "good");
    }

    #[tokio::test]
    async fn test_is_stale() {
        let table = ReputationTable::new();
        table.record_success("peer1").await;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        assert!(!table.is_stale("peer1", now + 10, 3600).await);
        assert!(table.is_stale("peer1", now + 3600 + 10, 3600).await);
    }

    #[tokio::test]
    async fn test_cleanup_stale() {
        let table = ReputationTable::new();
        table.record_success("peer1").await;
        let far_future = 9999999999;
        table.cleanup_stale(far_future, 1).await;
        assert!(table.is_stale("peer1", far_future, 1).await);
    }

    #[test]
    fn test_compute_score_initial() {
        assert_eq!(compute_score(0, 0), 0.0);
    }

    #[test]
    fn test_compute_score_all_success() {
        let s = compute_score(10, 0);
        assert!(s > 0.8);
    }

    #[test]
    fn test_compute_score_all_failure() {
        let s = compute_score(0, 10);
        assert!(s < -0.8);
    }

    #[test]
    fn test_reputation_entry_new() {
        let e = ReputationEntry::new("test", 1000);
        assert_eq!(e.peer_id, "test");
        assert_eq!(e.score, 0.0);
        assert_eq!(e.balance_trust, 0.5);
    }

    #[tokio::test]
    async fn test_record_failure() {
        let table = ReputationTable::new();
        table.record_failure("p").await;
        let e = table.get("p").await.unwrap();
        assert_eq!(e.failed_redirects, 1);
    }

    #[tokio::test]
    async fn test_record_receipt() {
        let table = ReputationTable::new();
        table.record_receipt("p").await;
        let e = table.get("p").await.unwrap();
        assert_eq!(e.successful_redirects, 1);
    }

    #[tokio::test]
    async fn test_record_rejection_non_insufficient() {
        let table = ReputationTable::new();
        table.record_rejection("p", "busy").await;
        assert_eq!(table.get("p").await.unwrap().failed_redirects, 1);
    }
}
