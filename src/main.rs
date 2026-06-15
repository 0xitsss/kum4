mod bitcoin_tx;
mod config;
mod crypto;
mod database;
mod dht;
mod error;
mod gossip;
mod monitor;
mod p2p;
mod price;
mod rebalance;
mod reputation;
mod telegram_bot;
mod tor_client;
mod wallet;
mod web;

use std::net::SocketAddr;
use std::sync::Arc;

use secp256k1::Secp256k1;
use tokio::sync::mpsc;

use crate::bitcoin_tx::BitcoinTxBuilder;
use crate::config::Config;
use crate::database::Database;
use crate::dht::{DhtCmd, DhtEvent, DhtNode, NodeInfo};
use crate::gossip::{gossip_task, ping_task};
use crate::monitor::Monitor;
use crate::p2p::{new_peer_registry, call_node_redirect, call_node_reserve, PeerRegistry, P2pState, RedirectRequest};
use crate::rebalance::RebalanceEngine;
use crate::wallet::Wallet;
use sha2::{Digest, Sha256};

async fn send_btc_to_user(
    client: &reqwest::Client,
    mempool_url: &str,
    wallet: &Wallet,
    reserve_index: u32,
    network: bitcoin::Network,
    exchange: &database::ExchangeRequest,
) -> error::Result<String> {
    let payout_sats = match exchange.btc_amount {
        Some(btc) if btc > 0.0 => (btc * 100_000_000.0) as u64,
        _ => return Err(error::Kum4Error::Config("Invalid or zero BTC amount".into())),
    };
    let btc_address = wallet.btc_address(reserve_index)?;
    let utxos = BitcoinTxBuilder::fetch_utxos(client, mempool_url, &btc_address.to_string()).await?;
    let confirmed_utxos: Vec<bitcoin_tx::UtxoEntry> = utxos.into_iter().filter(|u| u.confirmed).collect();
    if confirmed_utxos.is_empty() {
        return Err(error::Kum4Error::Bitcoin("No confirmed UTXOs available".into()));
    }

    let fee_url = format!("{}/api/v1/fees/recommended", mempool_url.trim_end_matches('/'));
    let fee_resp = client.get(&fee_url).send().await
        .map_err(|e| error::Kum4Error::Network(format!("Fee fetch: {e}")))?;
    let fee_data: serde_json::Value = fee_resp.json().await
        .map_err(|e| error::Kum4Error::Network(format!("Fee parse: {e}")))?;
    let fee_rate_sat_per_vb = fee_data["fastestFee"].as_f64().unwrap_or(50.0);

    let tx_vbytes = BitcoinTxBuilder::estimate_tx_vbytes(confirmed_utxos.len(), 2);
    let fee_sats = (fee_rate_sat_per_vb * tx_vbytes as f64) as u64;

    let target = payout_sats + fee_sats;
    let (selected, total_selected) = BitcoinTxBuilder::select_utxos(&confirmed_utxos, target);
    if total_selected < target {
        return Err(error::Kum4Error::Bitcoin(format!(
            "Insufficient UTXOs: have {} sats, need {} sats", total_selected, target
        )));
    }

    use bitcoin::address::{NetworkUnchecked};
    let unchecked: bitcoin::Address<NetworkUnchecked> = exchange.btc_address.parse()
        .map_err(error::Kum4Error::BitcoinAddress)?;
    let merchant_address = unchecked.require_network(network)
        .map_err(|e| error::Kum4Error::Bitcoin(format!("Network mismatch: {e}")))?;

    let mut unsigned_tx = BitcoinTxBuilder::build_unsigned_tx(&selected, &merchant_address, payout_sats, fee_sats)?;

    let priv_key = wallet.btc_private_key_at_index(reserve_index)?;
    let secp = Secp256k1::new();
    use bitcoin::CompressedPublicKey;
    let compressed = CompressedPublicKey::from_private_key(&secp, &priv_key)?;
    BitcoinTxBuilder::sign_p2wpkh(&mut unsigned_tx, &selected, &priv_key, &compressed)?;

    let tx_hex = bitcoin::consensus::encode::serialize_hex(&unsigned_tx);
    let txid = BitcoinTxBuilder::broadcast_tx_with_client(client, mempool_url, tx_hex).await?;
    Ok(txid)
}

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
    let seed_phrase = crypto::load_or_generate_key(&config.key_path)?;
    let wallet = Wallet::from_seed_phrase(&seed_phrase, config.btc_network_bitcoin())?;
    let wallet = Arc::new(wallet);

    let db = Database::open(&config.db_path)?;

    // --- Derive & store first monitored address per chain ---
    let _ = db.addr_index("tron")?;
    let _ = db.addr_index("bsc")?;

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
    let peer_registry: Arc<PeerRegistry> = Arc::new(new_peer_registry());

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

        let dht_node = DhtNode::new(&seed_phrase, listen_addr, dht_cmd_rx, dht_event_tx)?;
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

        // Build local NodeInfo for gossip
        let local_info = NodeInfo {
            peer_id: peer_id.clone(),
            http_addr: format!("http://127.0.0.1:{}", config.node_port),
            fee_usd: config.profit_fee_usd,
            chains: vec!["tron".into(), "bsc".into()],
            btc_reserve: 0.0,
            status: "online".into(),
            version: config.node_version.clone(),
            reserve_updated: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs(),
            last_seen: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs(),
        };

        let gossip_addr: SocketAddr = format!("0.0.0.0:{}", config.node_port + 2)
            .parse().expect("Invalid gossip address");

        let gossip_registry = peer_registry.clone();
        tokio::spawn(async move {
            gossip_task(gossip_registry, local_info, gossip_addr).await;
        });

        let ping_registry = peer_registry.clone();
        let ping_http = http_client.clone();
        tokio::spawn(async move {
            ping_task(ping_registry, ping_http).await;
        });
    } else {
        peer_id = peer_id_from_seed(&seed_phrase);
        p2p_state = Arc::new(P2pState::new(peer_id.clone(), 0.0, config.profit_fee_usd));
    }

    // --- Monitor, Rebalance ---
    let (deposit_tx, mut deposit_rx) = mpsc::channel::<monitor::DepositEvent>(1024);

    let tron_rpc = config.tron_rpc_url.clone();
    let tron_contract = config.tron_usdt_contract.clone();
    let bsc_rpc = config.bsc_rpc_url.clone();
    let bsc_contract = config.bsc_usdt_contract.clone();

    let rebalance = Arc::new(RebalanceEngine::new(
        config.rebalance_threshold,
    ));

    let rebalance_loop = rebalance.clone();
    tokio::spawn(async move {
        rebalance_loop.run().await;
    });

    let rebalance_handler = rebalance.clone();
    let deposit_config = config.clone();
    let deposit_db = db.clone();
    let deposit_p2p = p2p_state.clone();
    let deposit_http = http_client.clone();
    let deposit_registry = peer_registry.clone();
    let deposit_wallet = wallet.clone();
    let deposit_mempool = config.mempool_url.clone();
    let deposit_btc_network = config.btc_network_bitcoin();
    let deposit_reserve_index = config.btc_reserve_index;
    tokio::spawn(async move {
        while let Some(deposit) = deposit_rx.recv().await {
            tracing::info!(
                chain = %deposit.chain, tx = %deposit.tx_hash,
                amount = %deposit.usdt_amount, from = %deposit.from_address,
                "Deposit detected"
            );

            if deposit.usdt_amount < deposit_config.min_usdt_amount {
                tracing::warn!(amount = deposit.usdt_amount, "Deposit below minimum, skipping");
                continue;
            }

            if let Ok(Some(exchange)) = deposit_db.find_exchange_by_address(&deposit.to_address) {
                let _ = deposit_db.set_exchange_status(&exchange.id, "deposit_detected");
                tracing::info!(exchange_id = %exchange.id, "Exchange matched to deposit");

                // --- Auto-send BTC (Bug #1 fix) ---
                let _ = deposit_db.set_exchange_status(&exchange.id, "sending");

                match send_btc_to_user(
                    &deposit_http,
                    &deposit_mempool,
                    &deposit_wallet,
                    deposit_reserve_index,
                    deposit_btc_network,
                    &exchange,
                ).await {
                    Ok(btc_txid) => {
                        let _ = deposit_db.set_exchange_status(&exchange.id, "sent");
                        tracing::info!(
                            exchange_id = %exchange.id,
                            btc_txid = %btc_txid,
                            "BTC sent successfully"
                        );
                    }
                    Err(e) => {
                        let _ = deposit_db.set_exchange_status(&exchange.id, "error");
                        tracing::error!(
                            exchange_id = %exchange.id,
                            error = %e,
                            "Failed to send BTC"
                        );
                    }
                }
            }

            let total = rebalance_handler.add_deposit(deposit.clone()).await;
            if total >= rebalance_handler.threshold {
                let estimated_btc = deposit.usdt_amount / 100_000.0;
                let local_reserve = deposit_p2p.reserve.lock().await;
                if local_reserve.btc_reserve >= estimated_btc {
                    tracing::info!(btc_reserve = local_reserve.btc_reserve, "Reserve sufficient");
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
                                        user_btc_address: deposit.from_address.clone(),
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

    let db_for_web = db.clone();
    let db_bot = db.clone();
    let client_trx = http_client.clone();
    let client_bsc = http_client.clone();
    let tx_tron = deposit_tx.clone();
    let tx_bsc = deposit_tx.clone();
    let db_tron = db.clone();
    let db_bsc = db;
    let tron_confirmations = config.tron_confirmations;
    let bsc_confirmations = config.bsc_confirmations;
    let max_pending = config.max_pending_per_chain;
    tokio::spawn(async move {
        Monitor::start_tron(client_trx, tron_rpc, tron_contract, tx_tron, db_tron, tron_confirmations, max_pending).await;
    });
    tokio::spawn(async move {
        Monitor::start_bsc(client_bsc, bsc_rpc, bsc_contract, tx_bsc, db_bsc, bsc_confirmations, max_pending).await;
    });

    let bot_deposit_tx = deposit_tx.clone();

    let bot_state = Arc::new(telegram_bot::BotState {
        db: db_bot,
        config: config.clone(),
        wallet: wallet.clone(),
        http_client: http_client.clone(),
        deposit_tx: bot_deposit_tx,
    });
    tokio::spawn(async move {
        telegram_bot::run(bot_state).await;
    });

    drop(deposit_tx);

    // --- Web server ---
    let uptime_start = tokio::time::Instant::now();
    let cleanup_db = db_for_web.clone();
    let app_state = Arc::new(web::AppState {
        db: db_for_web,
        wallet,
        config: config.clone(),
        mempool_url: config.mempool_url.clone(),
        peer_id,
        uptime_start,
        p2p_state,
        peer_registry: peer_registry.clone(),
    });

    let addr = format!("{}:{}", config.web_host, config.node_port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("Web UI: http://{addr}");

    tokio::spawn(async move {
        axum::serve(listener, web::router(app_state))
            .await
            .unwrap();
    });

    // Background cleanup of expired exchanges (Bug #8 fix)
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(1800)).await;
            match cleanup_db.delete_expired_exchanges(86400) {
                Ok(count) => {
                    if count > 0 {
                        tracing::info!("Cleaned up {count} expired exchanges");
                    }
                }
                Err(e) => tracing::error!("Cleanup error: {e}"),
            }
        }
    });

    let mode = if config.tor_enabled { "tor+mesh" } else { "clearnet" };
    tracing::info!("Kumquad started (mode: {mode})");
    tokio::signal::ctrl_c().await?;
    tracing::info!("Shutting down");
    Ok(())
}
