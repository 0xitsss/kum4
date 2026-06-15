#![allow(dead_code)]

use std::net::SocketAddr;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::dht::NodeInfo;
use crate::error::Result;
use crate::p2p::PeerRegistry;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GossipMessage {
    pub sender: NodeInfo,
    pub peers: Vec<NodeInfo>,
    pub reputation_deltas: Vec<ReputationDelta>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReputationDelta {
    pub peer_id: String,
    pub delta_score: f64,
    pub reason: String,
}

pub async fn gossip_task(
    registry: Arc<PeerRegistry>,
    local_info: NodeInfo,
    gossip_addr: SocketAddr,
) {
    use tokio::time::{interval, Duration};
    let mut ticker = interval(Duration::from_secs(60));
    loop {
        ticker.tick().await;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        let peers = registry.all_active(now).await;
        if peers.is_empty() {
            continue;
        }
        let selected = choose_random(&peers, 3);
        for target in selected {
            let msg = GossipMessage {
                sender: local_info.clone(),
                peers: registry.all_active(now).await,
                reputation_deltas: Vec::new(),
            };
            if let Ok(reply) = udp_exchange(target.http_addr.parse().unwrap_or(gossip_addr), &msg).await {
                merge_gossip(&registry, &reply).await;
            }
        }
        registry.remove_stale(now).await;
    }
}

fn choose_random<T>(items: &[T], n: usize) -> Vec<&T> {
    use rand::seq::SliceRandom;
    let mut rng = rand::thread_rng();
    items.choose_multiple(&mut rng, n).collect()
}

async fn udp_exchange(addr: SocketAddr, msg: &GossipMessage) -> Result<GossipMessage> {
    use tokio::net::UdpSocket;
    let socket = UdpSocket::bind("0.0.0.0:0").await
        .map_err(|e| crate::error::Kum4Error::Network(e.to_string()))?;
    let data = serde_json::to_vec(msg)
        .map_err(|e| crate::error::Kum4Error::Network(e.to_string()))?;
    socket.send_to(&data, addr).await
        .map_err(|e| crate::error::Kum4Error::Network(e.to_string()))?;
    let mut buf = vec![0u8; 65535];
    let len = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        socket.recv_from(&mut buf),
    ).await
        .map_err(|_| crate::error::Kum4Error::Network("UDP timeout".into()))?
        .map_err(|e| crate::error::Kum4Error::Network(e.to_string()))?
        .0;
    buf.truncate(len);
    serde_json::from_slice(&buf)
        .map_err(|e| crate::error::Kum4Error::Network(e.to_string()))
}

async fn merge_gossip(registry: &Arc<PeerRegistry>, msg: &GossipMessage) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    for incoming in &msg.peers {
        let existing = registry.get(&incoming.peer_id).await;
        match existing {
            Some(e) if e.last_seen > incoming.last_seen => {}
            _ => {
                registry.update(incoming.clone()).await;
            }
        }
    }
    for delta in &msg.reputation_deltas {
        if delta.delta_score > 0.0 {
            registry.reputation().record_success(&delta.peer_id).await;
        } else if delta.delta_score < 0.0 {
            registry.reputation().record_failure(&delta.peer_id).await;
        }
    }
    registry.update(msg.sender.clone()).await;
    registry.remove_stale(now).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_choose_random_returns_n() {
        let items = vec![1, 2, 3, 4, 5];
        let chosen = choose_random(&items, 3);
        assert_eq!(chosen.len(), 3);
    }

    #[test]
    fn test_choose_random_limits_to_len() {
        let items = vec![1, 2, 3];
        let chosen = choose_random(&items, 10);
        assert_eq!(chosen.len(), 3);
    }

    #[test]
    fn test_gossip_message_serde() {
        let msg = GossipMessage {
            sender: NodeInfo {
                peer_id: "sender".into(),
                http_addr: "127.0.0.1:8080".into(),
                fee_usd: 0.5,
                chains: vec!["tron".into()],
                btc_reserve: 1.0,
                status: "online".into(),
                version: "1.0".into(),
                reserve_updated: 1000,
                last_seen: 1000,
            },
            peers: vec![],
            reputation_deltas: vec![],
        };
        let json = serde_json::to_string(&msg).unwrap();
        let deserialized: GossipMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.sender.peer_id, "sender");
    }

    #[test]
    fn test_reputation_delta_serde() {
        let delta = ReputationDelta {
            peer_id: "peer1".into(),
            delta_score: 1.5,
            reason: "successful redirect".into(),
        };
        let json = serde_json::to_string(&delta).unwrap();
        let deserialized: ReputationDelta = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.peer_id, "peer1");
        assert!((deserialized.delta_score - 1.5).abs() < 1e-10);
        assert_eq!(deserialized.reason, "successful redirect");
    }
}
