use std::fmt;

use serde::{Deserialize, Serialize};
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

pub struct Monitor {
    tx: mpsc::Sender<DepositEvent>,
}

impl Monitor {
    pub fn new(tx: mpsc::Sender<DepositEvent>) -> Self {
        Monitor { tx }
    }

    pub async fn start_tron(
        client: reqwest::Client,
        rpc_url: String,
        contract: String,
        addresses: Vec<String>,
        tx: mpsc::Sender<DepositEvent>,
        db: crate::database::Database,
    ) {
        tracing::info!("Starting Tron monitor");
        loop {
            if let Err(e) = Self::scan_tron(&client, &rpc_url, &contract, &addresses, &tx, &db).await {
                tracing::error!("Tron scan error: {e}");
            }
            tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
        }
    }

    async fn scan_tron(
        client: &reqwest::Client,
        rpc_url: &str,
        contract: &str,
        addresses: &[String],
        tx: &mpsc::Sender<DepositEvent>,
        db: &crate::database::Database,
    ) -> Result<()> {
        let url = format!("{rpc_url}/v1/contracts/{contract}/events");
        let resp = client.get(&url).send().await?;
        let data: serde_json::Value = resp.json().await?;
        if let Some(events) = data["data"].as_array() {
            for event in events {
                if event["event_name"].as_str() != Some("Transfer") {
                    continue;
                }
                let tx_hash = event["transaction_id"].as_str().unwrap_or("").to_string();
                if tx_hash.is_empty() || db.is_tx_processed(&tx_hash)? {
                    continue;
                }
                let to = event["result"]["to"].as_str().unwrap_or("").to_string();
                if !addresses.contains(&to) {
                    continue;
                }
                let value_str = event["result"]["value"].as_str().unwrap_or("0");
                let value: f64 = value_str.parse().unwrap_or(0.0);
                let usdt_amount = value / 1_000_000.0;
                let block_number = event["block_number"].as_u64().unwrap_or(0);
                let deposit = DepositEvent {
                    chain: Chain::Tron,
                    tx_hash: tx_hash.clone(),
                    from_address: event["result"]["from"].as_str().unwrap_or("").to_string(),
                    to_address: to.clone(),
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
        Ok(())
    }

    pub async fn start_bsc(
        client: reqwest::Client,
        rpc_url: String,
        contract: String,
        addresses: Vec<String>,
        tx: mpsc::Sender<DepositEvent>,
        db: crate::database::Database,
    ) {
        tracing::info!("Starting BSC monitor");
        loop {
            if let Err(e) = Self::scan_bsc(&client, &rpc_url, &contract, &addresses, &tx, &db).await {
                tracing::error!("BSC scan error: {e}");
            }
            tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
        }
    }

    async fn scan_bsc(
        client: &reqwest::Client,
        rpc_url: &str,
        contract: &str,
        addresses: &[String],
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
                let topics = log["topics"].as_array().map(|t| t.clone()).unwrap_or_default();
                if topics.len() < 3 { continue; }
                let to_hex = &topics[2];
                let to_addr = Self::hex_to_eth_address(to_hex);
                if !addresses.contains(&to_addr) { continue; }
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
