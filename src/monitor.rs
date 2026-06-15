use std::fmt;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;

use crate::error::{Kum4Error, Result};

const MAX_RETRIES: u32 = 3;
const RETRY_DELAY_MS: u64 = 1000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Chain {
    Tron,
    Bsc,
}

impl fmt::Display for Chain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Chain::Tron => write!(f, "tron"),
            Chain::Bsc => write!(f, "bsc"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DepositEvent {
    pub chain: Chain,
    pub tx_hash: String,
    pub from_address: String,
    pub to_address: String,
    pub usdt_amount: f64,
    pub block_number: u64,
}

#[allow(dead_code)]
pub struct Monitor {
    tx: mpsc::Sender<DepositEvent>,
}

async fn retry_http<F, Fut, T>(f: F) -> Result<T>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = std::result::Result<T, reqwest::Error>>,
{
    let mut last_err = None;
    for attempt in 0..MAX_RETRIES {
        match f().await {
            Ok(val) => return Ok(val),
            Err(e) => {
                tracing::warn!("HTTP attempt {}/{} failed: {e}", attempt + 1, MAX_RETRIES);
                last_err = Some(e);
                if attempt + 1 < MAX_RETRIES {
                    tokio::time::sleep(tokio::time::Duration::from_millis(RETRY_DELAY_MS)).await;
                }
            }
        }
    }
    Err(Kum4Error::Network(format!("HTTP retry exhausted: {}", last_err.unwrap())))
}

#[allow(dead_code)]
impl Monitor {
    pub fn new(tx: mpsc::Sender<DepositEvent>) -> Self {
        Monitor { tx }
    }

    pub async fn start_tron(
        client: reqwest::Client,
        rpc_url: String,
        contract: String,
        tx: mpsc::Sender<DepositEvent>,
        db: crate::database::Database,
        _confirmations: u64,
        max_pending: usize,
    ) {
        tracing::info!("Starting Tron monitor (max_pending={max_pending})");
        loop {
            if let Err(e) = Self::scan_tron(&client, &rpc_url, &contract, &tx, &db, max_pending).await {
                tracing::error!("Tron scan error: {e}");
            }
            tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
        }
    }

    fn rest_url(rpc_url: &str) -> &str {
        // Strip path from RPC URL (e.g. "https://nile.trongrid.io/jsonrpc/" -> "https://nile.trongrid.io")
        rpc_url.trim_end_matches('/')
            .trim_end_matches("/jsonrpc")
            .trim_end_matches("/wallet")
    }

    fn tron_hex_to_base58(hex_addr: &str) -> String {
        let raw = hex_addr.trim_start_matches("0x");
        let addr_bytes = hex::decode(raw).unwrap_or_default();
        let mut tron_bytes = vec![0x41u8];
        tron_bytes.extend_from_slice(&addr_bytes);
        let hash = Sha256::digest(&tron_bytes);
        let hash2 = Sha256::digest(hash);
        let mut buf = Vec::with_capacity(25);
        buf.extend_from_slice(&tron_bytes);
        buf.extend_from_slice(&hash2[..4]);
        bs58::encode(&buf).into_string()
    }

    async fn tron_block_height(client: &reqwest::Client, rpc_url: &str) -> Result<u64> {
        let base = Self::rest_url(rpc_url);
        let url = format!("{}/wallet/getnowblock", base.trim_end_matches("/wallet"));
        let resp = retry_http(|| client.post(&url).send()).await?;
        let data: serde_json::Value = resp.json().await.map_err(|e| Kum4Error::Network(e.to_string()))?;
        Ok(data["block_header"]["raw_data"]["number"].as_u64().unwrap_or(0))
    }

    async fn emit_ready_pending_txs_time(
        db: &crate::database::Database,
        tx: &mpsc::Sender<DepositEvent>,
        chain: &Chain,
        current_time_secs: u64,
        stability_window_secs: u64,
    ) -> Result<()> {
        let pending = db.get_pending_txs()?;
        let chain_str = chain.to_string();
        for p in &pending {
            let tx_chain = p["chain"].as_str().unwrap_or("");
            if tx_chain != chain_str { continue; }
            if p["confirmed"] == serde_json::json!(true) { continue; }
            let tx_hash = p["tx_hash"].as_str().unwrap_or("").to_string();
            let stored_ts = p["block_number"].as_u64().unwrap_or(0);
            let age = current_time_secs.saturating_sub(stored_ts);
            if age >= stability_window_secs {
                let deposit = DepositEvent {
                    chain: chain.clone(),
                    tx_hash: tx_hash.clone(),
                    from_address: p["from_address"].as_str().unwrap_or("").to_string(),
                    to_address: p["to_address"].as_str().unwrap_or("").to_string(),
                    usdt_amount: p["usdt_amount"].as_f64().unwrap_or(0.0),
                    block_number: stored_ts,
                };
                db.mark_pending_tx_confirmed(&tx_hash)?;
                db.mark_tx_processed(&tx_hash)?;
                if tx.send(deposit).await.is_err() {
                    tracing::warn!("{chain_str} monitor receiver dropped");
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    async fn emit_ready_pending_txs(
        db: &crate::database::Database,
        tx: &mpsc::Sender<DepositEvent>,
        chain: &Chain,
        current_block: u64,
        required_confirmations: u64,
    ) -> Result<()> {
        let pending = db.get_pending_txs()?;
        for p in &pending {
            let tx_chain = p["chain"].as_str().unwrap_or("");
            let chain_str = chain.to_string();
            if tx_chain != chain_str { continue; }
            if p["confirmed"] == serde_json::json!(true) { continue; }
            let tx_hash = p["tx_hash"].as_str().unwrap_or("").to_string();
            let stored_height = p["block_number"].as_u64().unwrap_or(0);
            let blocks_passed = current_block.saturating_sub(stored_height);
            if blocks_passed >= required_confirmations {
                let deposit = DepositEvent {
                    chain: chain.clone(),
                    tx_hash: tx_hash.clone(),
                    from_address: p["from_address"].as_str().unwrap_or("").to_string(),
                    to_address: p["to_address"].as_str().unwrap_or("").to_string(),
                    usdt_amount: p["usdt_amount"].as_f64().unwrap_or(0.0),
                    block_number: stored_height,
                };
                db.mark_pending_tx_confirmed(&tx_hash)?;
                db.mark_tx_processed(&tx_hash)?;
                if tx.send(deposit).await.is_err() {
                    tracing::warn!("{chain_str} monitor receiver dropped");
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    async fn scan_tron(
        client: &reqwest::Client,
        rpc_url: &str,
        contract: &str,
        tx: &mpsc::Sender<DepositEvent>,
        db: &crate::database::Database,
        max_pending: usize,
    ) -> Result<()> {
        let base = Self::rest_url(rpc_url);

        // Emit confirmed pending txs (time-based: 1 block ≈ 3s, so required_height = current_time - 3*confirmations)
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        Self::emit_ready_pending_txs_time(db, tx, &Chain::Tron, now, 60).await?;

        // Check pending exchanges for new deposits
        let mut exchanges = db.get_pending_exchanges("tron")?;
        if exchanges.len() > max_pending {
            tracing::warn!(
                "Tron pending exchanges ({}) exceeds limit ({}), scanning only first {}",
                exchanges.len(), max_pending, max_pending
            );
            exchanges.truncate(max_pending);
        }
        for exchange in &exchanges {
            let url = format!(
                "{base}/v1/accounts/{addr}/transactions/trc20?contract_address={contract}&only_to=true&limit=5",
                addr = exchange.deposit_address
            );
            let resp = retry_http(|| client.get(&url).send()).await?;
            let data: serde_json::Value = resp.json().await.map_err(|e| Kum4Error::Network(e.to_string()))?;
            if let Some(trxs) = data["data"].as_array() {
                for trx in trxs {
                    let tx_hash = trx["transaction_id"].as_str().unwrap_or("").to_string();
                    if tx_hash.is_empty() || db.is_tx_processed(&tx_hash)? {
                        continue;
                    }
                    let to = trx["to"].as_str().unwrap_or("");
                    if to != exchange.deposit_address {
                        continue;
                    }
                    let value_str = trx["value"].as_str().unwrap_or("0");
                    let value: f64 = value_str.parse().unwrap_or(0.0);
                    let usdt_amount = value / 1_000_000.0;
                    let expected = exchange.usdt_amount.unwrap_or(0.0);
                    if (usdt_amount - expected).abs() > 0.01 {
                        tracing::warn!(
                            tx = %tx_hash, addr = %to,
                            got = usdt_amount, expected = expected,
                            "Amount mismatch — adding to manual review"
                        );
                        db.add_manual_review(&tx_hash, "tron", "", to, usdt_amount, expected)?;
                        db.mark_tx_processed(&tx_hash)?;
                        continue;
                    }
                    let block_timestamp = trx["block_timestamp"].as_u64().unwrap_or(0);
                    let ts_secs = block_timestamp / 1000;
                    let from_addr = trx["from"].as_str().unwrap_or("").to_string();

                    db.add_pending_tx(&tx_hash, "tron", ts_secs, to, &from_addr, usdt_amount)?;

                    let emit_now = now.saturating_sub(ts_secs) >= 60;
                    if emit_now {
                        let deposit = DepositEvent {
                            chain: Chain::Tron,
                            tx_hash: tx_hash.clone(),
                            from_address: from_addr,
                            to_address: to.to_string(),
                            usdt_amount,
                            block_number: ts_secs,
                        };
                        db.mark_pending_tx_confirmed(&tx_hash)?;
                        db.mark_tx_processed(&tx_hash)?;
                        if tx.send(deposit).await.is_err() {
                            tracing::warn!("Tron monitor receiver dropped");
                            break;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    pub async fn start_bsc(
        client: reqwest::Client,
        rpc_url: String,
        contract: String,
        tx: mpsc::Sender<DepositEvent>,
        db: crate::database::Database,
        confirmations: u64,
        max_pending: usize,
    ) {
        tracing::info!("Starting BSC monitor (max_pending={max_pending}, confirmations={confirmations})");
        loop {
            if let Err(e) = Self::scan_bsc(&client, &rpc_url, &contract, &tx, &db, confirmations, max_pending).await {
                tracing::error!("BSC scan error: {e}");
            }
            tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
        }
    }

    async fn bsc_block_height(client: &reqwest::Client, rpc_url: &str) -> Result<u64> {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "eth_blockNumber",
            "params": [],
            "id": 1
        });
        let resp = retry_http(|| client.post(rpc_url).json(&body).send()).await?;
        let data: serde_json::Value = resp.json().await.map_err(|e| Kum4Error::Network(e.to_string()))?;
        let hex = data["result"].as_str().unwrap_or("0x0");
        Ok(u64::from_str_radix(hex.trim_start_matches("0x"), 16).unwrap_or(0))
    }

    async fn scan_bsc(
        client: &reqwest::Client,
        rpc_url: &str,
        contract: &str,
        tx: &mpsc::Sender<DepositEvent>,
        db: &crate::database::Database,
        confirmations: u64,
        max_pending: usize,
    ) -> Result<()> {
        // Emit pending_txs that have reached enough confirmations
        if let Ok(current_height) = Self::bsc_block_height(client, rpc_url).await {
            Self::emit_ready_pending_txs(db, tx, &Chain::Bsc, current_height, confirmations).await?;
        }

        let transfer_topic = "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef";
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "eth_getLogs",
            "params": [{
                "address": contract,
                "topics": [transfer_topic],
                "fromBlock": "earliest",
                "toBlock": "latest"
            }],
            "id": 1
        });
        let resp = retry_http(|| client.post(rpc_url).json(&body).send()).await?;
        let data: serde_json::Value = resp.json().await.map_err(|e| Kum4Error::Network(e.to_string()))?;
        if let Some(logs) = data["result"].as_array() {
            let mut matched = 0usize;
            for log in logs {
                if matched >= max_pending {
                    tracing::warn!("BSC scan hit max_pending={max_pending}, stopping early");
                    break;
                }
                let topics = log["topics"].as_array().cloned().unwrap_or_default();
                if topics.len() < 3 { continue; }
                let to_hex = &topics[2];
                let to_addr = Self::hex_to_eth_address(to_hex);
                if db.find_exchange_by_address(&to_addr)?.is_none() { continue; }
                let tx_hash = log["transactionHash"].as_str().unwrap_or("").to_string();
                if tx_hash.is_empty() || db.is_tx_processed(&tx_hash)? { continue; }
                let data_hex = log["data"].as_str().unwrap_or("0x");
                let value = Self::hex_to_value(data_hex);
                let usdt_amount = value / 1_000_000.0;

                // Check amount match
                let exchange = db.find_exchange_by_address(&to_addr)?;
                if let Some(ref ex) = exchange {
                    let expected = ex.usdt_amount.unwrap_or(0.0);
                    if (usdt_amount - expected).abs() > 0.01 {
                        tracing::warn!(
                            tx = %tx_hash, addr = %to_addr,
                            got = usdt_amount, expected = expected,
                            "BSC amount mismatch — adding to manual review"
                        );
                        db.add_manual_review(&tx_hash, "bsc", &Self::hex_to_eth_address(&topics[1]), &to_addr, usdt_amount, expected)?;
                        db.mark_tx_processed(&tx_hash)?;
                        continue;
                    }
                }

                let block_number = log["blockNumber"].as_str()
                    .and_then(|b| u64::from_str_radix(b.trim_start_matches("0x"), 16).ok())
                    .unwrap_or(0);

                let from_addr = Self::hex_to_eth_address(&topics[1]);

                db.add_pending_tx(&tx_hash, "bsc", block_number, &to_addr, &from_addr, usdt_amount)?;

                // Emit immediately if enough confirmations already
                if let Ok(current_height) = Self::bsc_block_height(client, rpc_url).await {
                    if current_height.saturating_sub(block_number) >= confirmations {
                        let deposit = DepositEvent {
                            chain: Chain::Bsc,
                            tx_hash: tx_hash.clone(),
                            from_address: from_addr,
                            to_address: to_addr.clone(),
                            usdt_amount,
                            block_number,
                        };
                        db.mark_pending_tx_confirmed(&tx_hash)?;
                        db.mark_tx_processed(&tx_hash)?;
                        if tx.send(deposit).await.is_err() {
                            tracing::warn!("BSC monitor receiver dropped");
                            break;
                        }
                    }
                }
                matched += 1;
            }
        }
        Ok(())
    }

    fn hex_to_eth_address(hex_val: &serde_json::Value) -> String {
        let s = hex_val.as_str().unwrap_or("0x0000000000000000000000000000000000000000");
        let s = s.trim_start_matches("0x");
        if s.len() > 40 {
            format!("0x{}", &s[s.len()-40..])
        } else {
            format!("0x{}", s)
        }
    }

    fn hex_to_value(data_hex: &str) -> f64 {
        let s = data_hex.trim_start_matches("0x");
        u128::from_str_radix(s, 16).unwrap_or(0) as f64
    }
}
