use std::env;

use crate::error::{Kum4Error, Result};

#[derive(Debug, Clone)]
pub struct Config {
    pub tron_rpc_url: String,
    pub bsc_rpc_url: String,
    pub seed_phrase: String,
    pub min_usdt_amount: f64,
    pub profit_fee_usd: f64,
    pub rebalance_threshold: f64,
    pub db_path: String,
    pub tron_usdt_contract: String,
    pub bsc_usdt_contract: String,
    pub btc_network: String,
    pub mempool_url: String,
    pub node_id: String,
    pub node_version: String,
    pub tor_enabled: bool,
    pub tor_proxy: String,
    pub node_port: u16,
    pub web_host: String,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        dotenvy::dotenv().ok();

        let cfg = Config {
            tron_rpc_url: env::var("TRON_RPC_URL")
                .unwrap_or_else(|_| "https://api.trongrid.io".into()),
            bsc_rpc_url: env::var("BSC_RPC_URL")
                .unwrap_or_else(|_| "https://bsc-dataseed.binance.org".into()),
            seed_phrase: Self::require("SEED_PHRASE")?,
            min_usdt_amount: env::var("MIN_USDT_AMOUNT")
                .unwrap_or_else(|_| "10.0".into())
                .parse()
                .map_err(|e| Kum4Error::Config(format!("MIN_USDT_AMOUNT parse: {e}")))?,
            profit_fee_usd: env::var("PROFIT_FEE_USD")
                .unwrap_or_else(|_| "1.0".into())
                .parse()
                .map_err(|e| Kum4Error::Config(format!("PROFIT_FEE_USD parse: {e}")))?,
            rebalance_threshold: env::var("REBALANCE_THRESHOLD")
                .unwrap_or_else(|_| "500.0".into())
                .parse()
                .map_err(|e| Kum4Error::Config(format!("REBALANCE_THRESHOLD parse: {e}")))?,
            db_path: env::var("DB_PATH").unwrap_or_else(|_| "kum4_data".into()),
            tron_usdt_contract: env::var("TRON_USDT_CONTRACT")
                .unwrap_or_else(|_| "TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t".into()),
            bsc_usdt_contract: env::var("BSC_USDT_CONTRACT")
                .unwrap_or_else(|_| "0x55d398326f99059ff775485246999027b3197955".into()),
            btc_network: env::var("BTC_NETWORK").unwrap_or_else(|_| "mainnet".into()),
            mempool_url: env::var("MEMPOOL_URL").unwrap_or_else(|_| "https://mempool.space".into()),
            node_id: env::var("NODE_ID").unwrap_or_else(|_| "kum4-default".into()),
            node_version: env::var("NODE_VERSION")
                .unwrap_or_else(|_| env!("CARGO_PKG_VERSION").into()),
            tor_enabled: env::var("TOR_ENABLED")
                .unwrap_or_else(|_| "false".into())
                .eq_ignore_ascii_case("true"),
            tor_proxy: env::var("TOR_PROXY").unwrap_or_else(|_| "socks5://127.0.0.1:9050".into()),
            node_port: env::var("NODE_PORT")
                .unwrap_or_else(|_| "8080".into())
                .parse()
                .map_err(|e| Kum4Error::Config(format!("NODE_PORT parse: {e}")))?,
            web_host: env::var("WEB_HOST").unwrap_or_else(|_| "127.0.0.1".into()),
        };

        Ok(cfg)
    }

    fn require(key: &str) -> Result<String> {
        env::var(key).map_err(|_| Kum4Error::Config(format!("Missing required env var: {key}")))
    }

    pub fn btc_network_bitcoin(&self) -> bitcoin::Network {
        match self.btc_network.as_str() {
            "testnet" | "testnet3" => bitcoin::Network::Testnet,
            "signet" => bitcoin::Network::Signet,
            "regtest" => bitcoin::Network::Regtest,
            _ => bitcoin::Network::Bitcoin,
        }
    }
}
