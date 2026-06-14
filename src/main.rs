mod bitcoin_tx;
mod config;
mod database;
mod dht;
mod error;
mod monitor;
mod p2p;
mod price;
mod rebalance;
mod tor_client;
mod wallet;
mod web;

use std::sync::Arc;

use tokio::sync::mpsc;

use crate::config::Config;
use crate::database::Database;
use crate::dht::{DhtCmd, DhtEvent, DhtNode, NodeInfo};
use crate::monitor::Monitor;
use crate::p2p::{new_peer_registry, call_node_redirect, call_node_reserve, PeerRegistry, P2pState, RedirectRequest};
use crate::rebalance::RebalanceEngine;
use crate::wallet::Wallet;
use sha2::{Digest, Sha256};

fn peer_id_from_seed(seed: &str) -> String {
    let hash = Sha256::digest(seed.as_bytes());
    format!("kum4-{}", hex::encode(&hash[..8]))
}

#[tokio::main]
async fn main() -> error::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let config = Config::from_env()?;
    let wallet = Wallet::from_seed_phrase(&config.seed_phrase, config.btc_network_bitcoin())?;

    // --- HTTP client: Tor or clearnet ---
    let http_client: reqwest::Client = if config.tor_enabled {
        let client = tor_client::new(&config.tor_proxy)?;
        tracing::info!("Tor mode: HTTP via {}", config.tor_proxy);
        client
    } else {
        tracing::info!("Clearnet mode: direct HTTP");
        reqwest::Client::new()
    };

    // --- Peer registry (shared across DHT + deposit handler) ---
    let peer_registry: PeerRegistry = new_peer_registry();

    // --- DHT & peer identity: Tor mode only ---
    let peer_id: String;
    let p2p_state: Arc<P2pState>;
    if config.tor_enabled {
        let listen_addr: libp2p::Multiaddr =
            format!("/ip4/0.0.0.0/tcp/{}", config.node_port + 1)
                .parse()
                .map_err(|e| error::Kum4Error::Dht(format!("Invalid listen addr: {e}")))?;

        let (dht_cmd_tx, dht_cmd_rx) = mpsc::channel::<DhtCmd>(256);
        let (dht_event_tx, mut dht_event_rx) = mpsc::channel::<DhtEvent>(256);

        let dht_node = DhtNode::new(&config.seed_phrase, listen_addr, dht_cmd_rx, dht_event_tx)?;
        let pid = dht_node.peer_id().to_string();
        peer_id = pid;
        p2p_state = Arc::new(P2pState::new(peer_id.clone(), 0.0, config.profit_fee_usd));

        tokio::spawn(async move {
            dht_node.run().await;
        });

        let cmd_tx = dht_cmd_tx;
        let registry = peer_registry.clone();
        tokio::spawn(async move {
            while let Some(event) = dht_event_rx.recv().await {
                match event {
                    DhtEvent::Ready {
                        peer_id: pid,
                        listen_addrs,
                    } => {
                        tracing::info!("DHT ready: peer_id={pid}, addrs={listen_addrs:?}");
                        let _ = cmd_tx.send(DhtCmd::Bootstrap).await;
                    }
                    DhtEvent::Bootstrapped { num_peers } => {
                        tracing::info!("DHT bootstrapped with {num_peers} peers");
                    }
                    DhtEvent::Announced => {
                        tracing::info!("Node info announced to DHT");
                    }
                    DhtEvent::ValueFound { key: _, value } => {
                        if let Some(data) = value {
                            if let Some(node) = NodeInfo::from_record(&data) {
                                registry.write().await.insert(node.peer_id.clone(), node);
                                tracing::info!("Peer discovered via DHT");
                            }
                        }
                    }
                    DhtEvent::Error(e) => {
                        tracing::warn!("DHT error: {e}");
                    }
                }
            }
        });
    } else {
        peer_id = peer_id_from_seed(&config.seed_phrase);
        p2p_state = Arc::new(P2pState::new(peer_id.clone(), 0.0, config.profit_fee_usd));
    }

    // --- Monitor, Rebalance ---
    let (deposit_tx, mut deposit_rx) = mpsc::channel::<monitor::DepositEvent>(1024);

    let tron_rpc = config.tron_rpc_url.clone();
    let tron_contract = config.tron_usdt_contract.clone();
    let bsc_rpc = config.bsc_rpc_url.clone();
    let bsc_contract = config.bsc_usdt_contract.clone();
    let db_path = config.db_path.clone();

    let rebalance = Arc::new(RebalanceEngine::new(
        config.rebalance_threshold,
    ));

    let rebalance_loop = rebalance.clone();
    tokio::spawn(async move {
        rebalance_loop.run().await;
    });

    let rebalance_handler = rebalance.clone();
    let deposit_p2p = p2p_state.clone();
    let deposit_http = http_client.clone();
    let deposit_registry = peer_registry.clone();
    tokio::spawn(async move {
        while let Some(deposit) = deposit_rx.recv().await {
            tracing::info!(
                chain = %deposit.chain, tx = %deposit.tx_hash,
                amount = %deposit.usdt_amount, from = %deposit.from_address,
                "Deposit detected"
            );
            let total = rebalance_handler.add_deposit(deposit.clone()).await;

            if total >= rebalance_handler.threshold {
                let estimated_btc = deposit.usdt_amount / 100_000.0;
                let local_reserve = deposit_p2p.reserve.lock().await;
                if local_reserve.btc_reserve >= estimated_btc {
                    tracing::info!(btc_reserve = local_reserve.btc_reserve, "Reserve sufficient — operator to process");
                } else {
                    tracing::info!("Local reserve insufficient — checking peers");
                    let peers = deposit_registry.read().await;
                    for (pid, info) in peers.iter() {
                        if *pid == deposit_p2p.peer_id { continue; }
                        match call_node_reserve(&deposit_http, &info.http_addr).await {
                            Ok(resp) => {
                                if resp.btc_reserve >= estimated_btc {
                                    let req = RedirectRequest {
                                        from_peer: deposit_p2p.peer_id.clone(),
                                        usdt_amount: deposit.usdt_amount,
                                        chain: deposit.chain.to_string(),
                                        user_btc_address: String::new(),
                                        deposit_txid: deposit.tx_hash.clone(),
                                    };
                                    match call_node_redirect(&deposit_http, &info.http_addr, &req).await {
                                        Ok(r) if r.accepted => {
                                            tracing::info!("Redirected to peer {pid}");
                                        }
                                        Ok(r) => tracing::warn!("Peer {pid} declined: {}", r.message),
                                        Err(e) => tracing::warn!("Redirect to {pid} failed: {e}"),
                                    }
                                }
                            }
                            Err(e) => tracing::warn!("Reserve check {pid} failed: {e}"),
                        }
                    }
                }
            }
        }
    });

    let db = Database::open(&db_path)?;
    let client_trx = http_client.clone();
    let client_bsc = http_client.clone();
    let tx_tron = deposit_tx.clone();
    let tx_bsc = deposit_tx.clone();
    let db_tron = db.clone();
    let db_bsc = db;
    tokio::spawn(async move {
        Monitor::start_tron(client_trx, tron_rpc, tron_contract, vec![], tx_tron, db_tron).await;
    });
    tokio::spawn(async move {
        Monitor::start_bsc(client_bsc, bsc_rpc, bsc_contract, vec![], tx_bsc, db_bsc).await;
    });

    drop(deposit_tx);

    // --- Web server ---
    let uptime_start = tokio::time::Instant::now();
    let app_state = Arc::new(web::AppState {
        wallet,
        config: config.clone(),
        mempool_url: config.mempool_url.clone(),
        peer_id,
        uptime_start,
        p2p_state,
    });

    let addr = format!("{}:{}", config.web_host, config.node_port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("Web UI: http://{addr}");

    tokio::spawn(async move {
        axum::serve(listener, web::router(app_state))
            .await
            .unwrap();
    });

    let mode = if config.tor_enabled { "tor+mesh" } else { "clearnet" };
    tracing::info!("Kumquad started (mode: {mode})");
    tokio::signal::ctrl_c().await?;
    tracing::info!("Shutting down");
    Ok(())
}
