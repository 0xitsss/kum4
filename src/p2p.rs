use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, RwLock};

use crate::dht::NodeInfo;

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

pub type PeerRegistry = Arc<RwLock<HashMap<String, NodeInfo>>>;

pub fn new_peer_registry() -> PeerRegistry {
    Arc::new(RwLock::new(HashMap::new()))
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
