use std::collections::HashMap;
use std::sync::Arc;

#[allow(unused_imports)]
use rand::seq::SliceRandom;
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, RwLock};

use crate::dht::NodeInfo;
use crate::reputation::ReputationTable;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReserveInfo {
    pub btc_reserve: f64,
    pub fee_usd: f64,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedirectRequest {
    pub from_peer: String,
    pub usdt_amount: f64,
    pub chain: String,
    pub user_btc_address: String,
    pub deposit_txid: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedirectResponse {
    pub accepted: bool,
    pub message: String,
}

pub struct P2pState {
    pub peer_id: String,
    pub reserve: Arc<Mutex<ReserveInfo>>,
}

impl P2pState {
    pub fn new(peer_id: String, btc_reserve: f64, fee_usd: f64) -> Self {
        P2pState {
            peer_id,
            reserve: Arc::new(Mutex::new(ReserveInfo {
                btc_reserve,
                fee_usd,
                status: "online".into(),
            })),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReserveResponse {
    pub peer_id: String,
    pub btc_reserve: f64,
    pub fee_usd: f64,
    pub status: String,
}

pub async fn call_node_reserve(
    client: &reqwest::Client,
    node_http_addr: &str,
) -> std::result::Result<ReserveResponse, String> {
    let url = format!("{}/api/p2p/reserve", node_http_addr.trim_end_matches('/'));
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("Reserve request failed: {e}"))?;
    resp.json::<ReserveResponse>()
        .await
        .map_err(|e| format!("Reserve parse failed: {e}"))
}

#[derive(Clone)]
#[allow(dead_code)]
pub struct PeerRegistry {
    inner: Arc<RwLock<HashMap<String, NodeInfo>>>,
    reputation: Arc<ReputationTable>,
    stale_timeout_secs: u64,
}

impl PeerRegistry {
    pub fn new() -> Self {
        PeerRegistry {
            inner: Arc::new(RwLock::new(HashMap::new())),
            reputation: Arc::new(ReputationTable::new()),
            stale_timeout_secs: 3600,
        }
    }

    pub async fn read(&self) -> tokio::sync::RwLockReadGuard<'_, HashMap<String, NodeInfo>> {
        self.inner.read().await
    }

    pub async fn write(&self) -> tokio::sync::RwLockWriteGuard<'_, HashMap<String, NodeInfo>> {
        self.inner.write().await
    }
}

#[allow(dead_code)]
impl PeerRegistry {
    pub fn new_with(stale_timeout_secs: u64) -> Self {
        PeerRegistry {
            inner: Arc::new(RwLock::new(HashMap::new())),
            reputation: Arc::new(ReputationTable::new()),
            stale_timeout_secs,
        }
    }

    pub fn reputation(&self) -> &Arc<ReputationTable> {
        &self.reputation
    }

    pub async fn update(&self, info: NodeInfo) {
        let mut map = self.inner.write().await;
        map.insert(info.peer_id.clone(), info);
    }

    pub async fn get(&self, peer_id: &str) -> Option<NodeInfo> {
        let map = self.inner.read().await;
        map.get(peer_id).cloned()
    }

    pub async fn all_active(&self, now: u64) -> Vec<NodeInfo> {
        let map = self.inner.read().await;
        map.values()
            .filter(|node| {
                node.status == "online"
                    && now.saturating_sub(node.last_seen) <= self.stale_timeout_secs
            })
            .cloned()
            .collect()
    }

    pub async fn remove_stale(&self, now: u64) -> Vec<String> {
        let mut map = self.inner.write().await;
        let mut removed = Vec::new();
        map.retain(|peer_id, node| {
            if now.saturating_sub(node.last_seen) > self.stale_timeout_secs {
                removed.push(peer_id.clone());
                false
            } else {
                true
            }
        });
        removed
    }

    pub async fn best_for_redirect(&self, needed_reserve: f64, now: u64) -> Option<NodeInfo> {
        let candidates: Vec<String> = {
            let map = self.inner.read().await;
            map.iter()
                .filter(|(_, node)| {
                    node.status == "online"
                        && node.btc_reserve >= needed_reserve
                        && now.saturating_sub(node.last_seen) <= self.stale_timeout_secs
                })
                .map(|(peer_id, _)| peer_id.clone())
                .collect()
        };

        let mut scored: Vec<(f64, String)> = Vec::new();
        for peer_id in candidates {
            if !self.reputation.is_blacklisted(&peer_id).await {
                let score = self.reputation.score(&peer_id).await;
                scored.push((score, peer_id));
            }
        }

        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

        let best_peer_id = scored.into_iter().next().map(|(_, peer_id)| peer_id)?;

        let map = self.inner.read().await;
        map.get(&best_peer_id).cloned()
    }
}

pub fn new_peer_registry() -> PeerRegistry {
    PeerRegistry::new()
}

pub async fn call_node_redirect(
    client: &reqwest::Client,
    node_http_addr: &str,
    req: &RedirectRequest,
) -> std::result::Result<RedirectResponse, String> {
    let url = format!("{}/api/p2p/redirect", node_http_addr.trim_end_matches('/'));
    let resp = client
        .post(&url)
        .json(req)
        .send()
        .await
        .map_err(|e| format!("Redirect request failed: {e}"))?;
    resp.json::<RedirectResponse>()
        .await
        .map_err(|e| format!("Redirect parse failed: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_peer_registry_update() {
        let registry = PeerRegistry::new();
        let info = NodeInfo {
            peer_id: "test-peer".into(),
            http_addr: "127.0.0.1:8080".into(),
            fee_usd: 0.5,
            chains: vec!["tron".into()],
            btc_reserve: 1.0,
            status: "online".into(),
            version: "1.0".into(),
            reserve_updated: 1000,
            last_seen: 1000,
        };
        registry.update(info.clone()).await;
        let got = registry.get("test-peer").await;
        assert!(got.is_some());
        assert_eq!(got.unwrap().peer_id, "test-peer");
    }

    #[tokio::test]
    async fn test_peer_registry_stale() {
        let registry = PeerRegistry::new();
        let info = NodeInfo {
            peer_id: "stale-peer".into(),
            http_addr: "127.0.0.1:8081".into(),
            fee_usd: 0.5,
            chains: vec!["tron".into()],
            btc_reserve: 1.0,
            status: "online".into(),
            version: "1.0".into(),
            reserve_updated: 100,
            last_seen: 0,
        };
        registry.update(info).await;
        let removed = registry.remove_stale(3601).await;
        assert_eq!(removed, vec!["stale-peer".to_string()]);
        assert!(registry.get("stale-peer").await.is_none());
    }

    #[tokio::test]
    async fn test_best_for_redirect() {
        let registry = PeerRegistry::new();
        registry.reputation().record_success("good-peer").await;

        let good = NodeInfo {
            peer_id: "good-peer".into(),
            http_addr: "127.0.0.1:8082".into(),
            fee_usd: 0.5,
            chains: vec!["tron".into()],
            btc_reserve: 5.0,
            status: "online".into(),
            version: "1.0".into(),
            reserve_updated: 1000,
            last_seen: 1000,
        };
        let poor = NodeInfo {
            peer_id: "poor-peer".into(),
            http_addr: "127.0.0.1:8083".into(),
            fee_usd: 0.3,
            chains: vec!["tron".into()],
            btc_reserve: 0.5,
            status: "online".into(),
            version: "1.0".into(),
            reserve_updated: 1000,
            last_seen: 1000,
        };
        registry.update(good).await;
        registry.update(poor).await;
        let best = registry.best_for_redirect(1.0, 2000).await;
        assert!(best.is_some());
        assert_eq!(best.unwrap().peer_id, "good-peer");
    }

    #[tokio::test]
    async fn test_best_for_redirect_no_candidates() {
        let registry = PeerRegistry::new();
        let poor = NodeInfo {
            peer_id: "poor-peer".into(),
            http_addr: "127.0.0.1:8083".into(),
            fee_usd: 0.3,
            chains: vec!["tron".into()],
            btc_reserve: 0.1,
            status: "online".into(),
            version: "1.0".into(),
            reserve_updated: 1000,
            last_seen: 1000,
        };
        registry.update(poor).await;
        let best = registry.best_for_redirect(1.0, 2000).await;
        assert!(best.is_none());
    }
}
