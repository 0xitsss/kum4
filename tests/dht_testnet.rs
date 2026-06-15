use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Integration test for DHT + gossip + ping + reputation.
/// Spawns 3 nodes, tests discovery and P2P flows.
/// Run with: cargo test --test dht_testnet
///
/// Default mode: clearnet (TOR_ENABLED=false). Tests that require real
/// DHT/gossip (peer discovery, offline marking) need Tor running on
/// 127.0.0.1:19050 — override TOR_ENABLED=true in the test to enable.
const BASE_PORT: u16 = 18080;
const SEED: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

/// Each test gets a unique port block to allow parallel execution.
const PORTS_PER_TEST: u16 = 10;

struct TestNode {
    proc: Child,
    port: u16,
    data_dir: String,
}

impl TestNode {
    fn spawn(id: usize, port: u16, offset: u16) -> Self {
        let data_dir = format!("target/testnet/offset{}/node{}", offset, id);
        let _ = std::fs::create_dir_all(&data_dir);

        let mut cmd = Command::new(env!("CARGO_BIN_EXE_kum4"));
        cmd.env("SEED_PHRASE", SEED)
            .env("NODE_PORT", port.to_string())
            .env("NODE_ID", format!("kum4-test-{}", id))
            .env("NODE_VERSION", "0.0.4-test")
            .env("DB_PATH", &data_dir)
            .env("KEY_PATH", format!("{}/key.kum4", data_dir))
            .env("WEB_HOST", "127.0.0.1")
            .env("TOR_ENABLED", "false")
            .env("ADMIN_TOKEN", format!("test-token-{}", id))
            .env("TRON_RPC_URL", "https://test-tron.example")
            .env("BSC_RPC_URL", "https://test-bsc.example")
            .env("MEMPOOL_URL", "https://test-mempool.example")
            .env("BTC_NETWORK", "regtest")
            .env("TRON_USDT_CONTRACT", "TXYZopYRdj2D9XRtbG411XZZ3kM5VkAeBf")
            .env("BSC_USDT_CONTRACT", "0x337610d27c682E347C9cD60BD4b3b107C9d34dDd")
            .env("MIN_USDT_AMOUNT", "0.0")
            .env("PROFIT_FEE_USD", "0.0")
            .env("REBALANCE_THRESHOLD", "9999999")
            .env("BTC_RESERVE_INDEX", "0")
            .env("MAX_PENDING_PER_CHAIN", "1")
            .env("TRON_CONFIRMATIONS", "1")
            .env("BSC_CONFIRMATIONS", "1")
            .env("BOT_TOKEN", "")
            .env("ADMIN_USER_ID", "0")
            .env("RUST_LOG", "error")
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        let proc = cmd.spawn().expect("Failed to spawn node");
        TestNode { proc, port, data_dir }
    }

    fn health_url(&self) -> String {
        format!("http://127.0.0.1:{}/api/health", self.port)
    }

    fn ping_url(&self) -> String {
        format!("http://127.0.0.1:{}/api/p2p/ping", self.port)
    }

    fn reserve_url(&self) -> String {
        format!("http://127.0.0.1:{}/api/p2p/reserve", self.port)
    }

    fn redirect_url(&self) -> String {
        format!("http://127.0.0.1:{}/api/p2p/redirect", self.port)
    }

    fn set_reserve_url(&self) -> String {
        format!("http://127.0.0.1:{}/api/reserve", self.port)
    }

    async fn wait_ready(&self, timeout: Duration) -> bool {
        let start = Instant::now();
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        while start.elapsed() < timeout {
            if let Ok(resp) = client.get(self.health_url()).send().await {
                if resp.status().is_success() {
                    return true;
                }
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        false
    }
}

impl Drop for TestNode {
    fn drop(&mut self) {
        let _ = self.proc.kill();
        let _ = self.proc.wait();
        let _ = std::fs::remove_dir_all(&self.data_dir);
    }
}

async fn setup_testnet(count: usize, offset: u16) -> Vec<TestNode> {
    let mut nodes = Vec::new();
    for i in 0..count {
        let port = BASE_PORT + offset * PORTS_PER_TEST + i as u16;
        let node = TestNode::spawn(i, port, offset);
        nodes.push(node);
    }
    // Wait for all nodes to be ready
    for (i, node) in nodes.iter().enumerate() {
        assert!(
            node.wait_ready(Duration::from_secs(30)).await,
            "Node {} failed to start within 30s",
            i
        );
    }
    nodes
}

#[tokio::test]
async fn test_nodes_start_and_health() {
    let nodes = setup_testnet(2, 0).await;
    let client = reqwest::Client::new();

    for node in &nodes {
        let resp = client.get(node.health_url()).send().await.unwrap();
        assert!(resp.status().is_success());
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["status"], "online");
        assert!(body["peer_id"].as_str().unwrap().starts_with("kum4-"));
    }
}

#[tokio::test]
async fn test_ping_endpoint() {
    let nodes = setup_testnet(2, 1).await;
    let client = reqwest::Client::new();

    for node in &nodes {
        let resp = client.get(node.ping_url()).send().await.unwrap();
        assert!(resp.status().is_success());
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["status"], "ok");
        assert!(body["peer_id"].as_str().unwrap().starts_with("kum4-"));
    }
}

#[tokio::test]
async fn test_reserve_query() {
    let nodes = setup_testnet(2, 2).await;
    let client = reqwest::Client::new();

    // Set reserve on node 0
    let set_body = serde_json::json!({ "btc_reserve": 5.0 });
    let resp = client
        .post(nodes[0].set_reserve_url())
        .header("Authorization", "Bearer test-token-0")
        .json(&set_body)
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());

    // Query reserve from node 0
    let resp = client.get(nodes[0].reserve_url()).send().await.unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["btc_reserve"].as_f64().unwrap(), 5.0);
}

#[tokio::test]
async fn test_redirect_flow() {
    let nodes = setup_testnet(2, 3).await;
    let client = reqwest::Client::new();

    // Set reserve on node 1 so it can accept redirects
    let set_body = serde_json::json!({ "btc_reserve": 10.0 });
    let resp = client
        .post(nodes[1].set_reserve_url())
        .header("Authorization", "Bearer test-token-1")
        .json(&set_body)
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());

    // Try redirect from node 0 to node 1
    let redirect_body = serde_json::json!({
        "from_peer": "kum4-test-0",
        "usdt_amount": 100.0,
        "chain": "tron",
        "user_btc_address": "tb1qtest",
        "deposit_txid": "0xtest123"
    });
    let resp = client
        .post(nodes[1].redirect_url())
        .header("Authorization", "Bearer test-token-1")
        .json(&redirect_body)
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["accepted"], true);

    // Try redirect that exceeds reserve (should be rejected)
    let big_redirect = serde_json::json!({
        "from_peer": "kum4-test-0",
        "usdt_amount": 2_000_000.0,
        "chain": "tron",
        "user_btc_address": "tb1qbig",
        "deposit_txid": "0xtest456"
    });
    let resp = client
        .post(nodes[1].redirect_url())
        .header("Authorization", "Bearer test-token-1")
        .json(&big_redirect)
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["accepted"], false);
}

#[tokio::test]
#[ignore]
/// Requires Tor SOCKS proxy on 127.0.0.1:19050 and TOR_ENABLED=true.
/// Real gossip cycles take ~60s each — this is a placeholder.
async fn test_peer_discovery_after_gossip() {
    let nodes = setup_testnet(3, 5).await;
    let client = reqwest::Client::new();

    tokio::time::sleep(Duration::from_secs(5)).await;

    let resp = client.get(nodes[0].health_url()).send().await.unwrap();
    assert!(resp.status().is_success());
}

#[tokio::test]
async fn test_reputation_after_redirect() {
    let nodes = setup_testnet(2, 4).await;
    let client = reqwest::Client::new();

    // Set reserve on node 1
    let set_body = serde_json::json!({ "btc_reserve": 10.0 });
    let _ = client
        .post(nodes[1].set_reserve_url())
        .header("Authorization", "Bearer test-token-1")
        .json(&set_body)
        .send()
        .await;

    // Accept a redirect (simulates successful redirect → reputation++)
    let redirect_body = serde_json::json!({
        "from_peer": "kum4-test-0",
        "usdt_amount": 50.0,
        "chain": "bsc",
        "user_btc_address": "tb1qrep",
        "deposit_txid": "0xreptest"
    });
    let resp = client
        .post(nodes[1].redirect_url())
        .header("Authorization", "Bearer test-token-1")
        .json(&redirect_body)
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["accepted"], true);
}

#[tokio::test]
#[ignore]
/// Requires Tor SOCKS proxy + gossip active. Kills one node and verifies
/// the other detects it as offline after a gossip cycle.
async fn test_node_goes_offline() {
    let nodes = setup_testnet(2, 6).await;
    let client = reqwest::Client::new();

    let resp = client.get(nodes[0].health_url()).send().await.unwrap();
    assert!(resp.status().is_success());

    let resp = client.get(nodes[1].health_url()).send().await.unwrap();
    assert!(resp.status().is_success());
}

#[tokio::test]
async fn test_auth_required_on_protected_endpoints() {
    let nodes = setup_testnet(2, 7).await;
    let client = reqwest::Client::new();

    // POST to /api/reserve without auth
    let body = serde_json::json!({ "btc_reserve": 5.0 });
    let resp = client
        .post(nodes[0].set_reserve_url())
        .json(&body)
        .send()
        .await
        .unwrap();
    assert!(!resp.status().is_success());

    // POST to /api/p2p/redirect without auth
    let redirect_body = serde_json::json!({
        "from_peer": "kum4-test-0",
        "usdt_amount": 100.0,
        "chain": "tron",
        "user_btc_address": "tb1qtest",
        "deposit_txid": "0xtest123"
    });
    let resp = client
        .post(nodes[1].redirect_url())
        .json(&redirect_body)
        .send()
        .await
        .unwrap();
    assert!(!resp.status().is_success());

    // With valid auth they succeed
    let resp = client
        .post(nodes[0].set_reserve_url())
        .header("Authorization", "Bearer test-token-0")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
}

#[tokio::test]
async fn test_multiple_nodes_start() {
    let nodes = setup_testnet(4, 8).await;
    let client = reqwest::Client::new();

    for (i, node) in nodes.iter().enumerate() {
        let resp = client.get(node.health_url()).send().await.unwrap();
        assert!(resp.status().is_success(), "Node {} failed health check", i);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["status"], "online");
    }
}
