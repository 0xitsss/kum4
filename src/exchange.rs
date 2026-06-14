use async_trait::async_trait;

use crate::error::Result;

#[async_trait]
pub trait ExchangeProvider: Send + Sync {
    async fn deposit_usdt(&self, amount: f64, from_chain: &str) -> Result<String>;
    async fn market_sell(&self, symbol: &str) -> Result<f64>;
    async fn withdraw_btc(&self, address: &str, amount_sats: u64) -> Result<String>;
}

pub struct CexExchange {
    client: reqwest::Client,
    api_key: String,
    api_secret: String,
    base_url: String,
}

impl CexExchange {
    pub fn new(api_key: String, api_secret: String, base_url: String) -> Self {
        CexExchange { client: reqwest::Client::new(), api_key, api_secret, base_url }
    }
}

#[async_trait]
impl ExchangeProvider for CexExchange {
    async fn deposit_usdt(&self, amount: f64, from_chain: &str) -> Result<String> {
        let url = format!("{}/api/v3/deposit/address", self.base_url);
        let resp = self.client
            .get(&url)
            .header("X-MBX-APIKEY", &self.api_key)
            .query(&[("coin", "USDT"), ("network", from_chain)])
            .send()
            .await?;
        let data: serde_json::Value = resp.json().await?;
        let address = data["address"].as_str().unwrap_or("").to_string();
        tracing::info!(amount, from_chain, address, "Deposited USDT to CEX");
        Ok(address)
    }

    async fn market_sell(&self, symbol: &str) -> Result<f64> {
        let url = format!("{}/api/v3/order", self.base_url);
        let resp = self.client
            .post(&url)
            .header("X-MBX-APIKEY", &self.api_key)
            .json(&serde_json::json!({
                "symbol": symbol,
                "side": "SELL",
                "type": "MARKET",
                "quoteOrderQty": "0"
            }))
            .send()
            .await?;
        let data: serde_json::Value = resp.json().await?;
        let executed = data["cummulativeQuoteQty"].as_str().unwrap_or("0").parse().unwrap_or(0.0);
        tracing::info!(executed_amount = executed, "Market sell executed");
        Ok(executed)
    }

    async fn withdraw_btc(&self, address: &str, amount_sats: u64) -> Result<String> {
        let url = format!("{}/sapi/v1/capital/withdraw/apply", self.base_url);
        let btc_amount = amount_sats as f64 / 100_000_000.0;
        let resp = self.client
            .post(&url)
            .header("X-MBX-APIKEY", &self.api_key)
            .json(&serde_json::json!({
                "coin": "BTC",
                "address": address,
                "amount": btc_amount,
                "network": "BTC"
            }))
            .send()
            .await?;
        let data: serde_json::Value = resp.json().await?;
        let id = data["id"].as_str().unwrap_or("").to_string();
        tracing::info!(address, amount_sats, withdraw_id = id, "BTC withdrawal submitted");
        Ok(id)
    }
}

pub struct DexExchange {
    client: reqwest::Client,
}

impl DexExchange {
    pub fn new() -> Self {
        DexExchange { client: reqwest::Client::new() }
    }
}

#[async_trait]
impl ExchangeProvider for DexExchange {
    async fn deposit_usdt(&self, _amount: f64, _from_chain: &str) -> Result<String> {
        tracing::warn!("DEX deposit not implemented — use direct swap");
        Ok("dex_placeholder".into())
    }

    async fn market_sell(&self, _symbol: &str) -> Result<f64> {
        tracing::warn!("DEX market sell not implemented");
        Ok(0.0)
    }

    async fn withdraw_btc(&self, _address: &str, _amount_sats: u64) -> Result<String> {
        tracing::warn!("DEX BTC withdraw not implemented");
        Ok("dex_placeholder".into())
    }
}
