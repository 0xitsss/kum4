use std::fmt;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;

use crate::error::Result;

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
    ) {
        tracing::info!("Starting Tron monitor");
        loop {
            if let Err(e) = Self::scan_tron(&client, &rpc_url, &contract, &tx, &db).await {
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

    async fn scan_tron(
        client: &reqwest::Client,
        rpc_url: &str,
        contract: &str,
        tx: &mpsc::Sender<DepositEvent>,
        db: &crate::database::Database,
    ) -> Result<()> {
        let base = Self::rest_url(rpc_url);
        let exchanges = db.get_pending_exchanges("tron")?;
        for exchange in &exchanges {
            let url = format!(
                "{base}/v1/accounts/{addr}/transactions/trc20?contract_address={contract}&only_to=true&limit=5",
                addr = exchange.deposit_address
            );
            let resp = client.get(&url).send().await?;
            let data: serde_json::Value = resp.json().await?;
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
                        tracing::debug!(
                            tx = %tx_hash, addr = %to,
                            got = usdt_amount, expected = expected,
                            "Amount mismatch, skipping"
                        );
                        db.mark_tx_processed(&tx_hash)?;
                        continue;
                    }
                    let block_number = trx["block_timestamp"].as_u64().unwrap_or(0);
                    let deposit = DepositEvent {
                        chain: Chain::Tron,
                        tx_hash: tx_hash.clone(),
                        from_address: trx["from"].as_str().unwrap_or("").to_string(),
                        to_address: to.to_string(),
                        usdt_amount,
                        block_number,
                    };
                    db.mark_tx_processed(&tx_hash)?;
                    if tx.send(deposit).await.is_err() {
                        tracing::warn!("Tron monitor receiver dropped");
                        break;
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
    ) {
        tracing::info!("Starting BSC monitor");
        loop {
            if let Err(e) = Self::scan_bsc(&client, &rpc_url, &contract, &tx, &db).await {
                tracing::error!("BSC scan error: {e}");
            }
            tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
        }
    }

    async fn scan_bsc(
        client: &reqwest::Client,
        rpc_url: &str,
        contract: &str,
        tx: &mpsc::Sender<DepositEvent>,
        db: &crate::database::Database,
    ) -> Result<()> {
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
        let resp = client.post(rpc_url).json(&body).send().await?;
        let data: serde_json::Value = resp.json().await?;
        if let Some(logs) = data["result"].as_array() {
            for log in logs {
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
                let block_number = log["blockNumber"].as_str()
                    .and_then(|b| u64::from_str_radix(b.trim_start_matches("0x"), 16).ok())
                    .unwrap_or(0);
                let deposit = DepositEvent {
                    chain: Chain::Bsc,
                    tx_hash: tx_hash.clone(),
                    from_address: Self::hex_to_eth_address(&topics[1]),
                    to_address: to_addr.clone(),
                    usdt_amount,
                    block_number,
                };
                db.mark_tx_processed(&tx_hash)?;
                if tx.send(deposit).await.is_err() {
                    tracing::warn!("BSC monitor receiver dropped");
                    break;
                }
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
