use std::time::Instant;

use serde::Deserialize;

use crate::error::Result;

#[derive(Debug, Clone)]
pub struct Prices {
    pub btc_usd: f64,
    pub fee_rate_sat_per_vb: f64,
}

pub struct PriceFeed {
    client: reqwest::Client,
    btc_price_cache: Option<(Prices, Instant)>,
    cache_ttl_secs: u64,
}

#[derive(Deserialize)]
struct BinancePrice {
    price: String,
}

#[derive(Deserialize)]
struct MempoolFee {
    fastest_fee: f64,
}

impl PriceFeed {
    pub fn new(client: reqwest::Client) -> Self {
        PriceFeed { client, btc_price_cache: None, cache_ttl_secs: 30 }
    }

    pub async fn get_prices(&mut self) -> Result<Prices> {
        if let Some((cached, time)) = &self.btc_price_cache {
            if time.elapsed().as_secs() < self.cache_ttl_secs {
                return Ok(cached.clone());
            }
        }
        let (btc_usd, fee_rate) = tokio::try_join!(
            self.fetch_btc_price(),
            self.fetch_fee_rate(),
        )?;
        let prices = Prices { btc_usd, fee_rate_sat_per_vb: fee_rate };
        self.btc_price_cache = Some((prices.clone(), Instant::now()));
        Ok(prices)
    }

    async fn fetch_btc_price(&self) -> Result<f64> {
        let resp = self.client
            .get("https://api.binance.com/api/v3/ticker/price?symbol=BTCUSDT")
            .send()
            .await?;
        let data: BinancePrice = resp.json().await?;
        data.price.parse::<f64>().map_err(|e| crate::error::Kum4Error::Parse(e.to_string()))
    }

    async fn fetch_fee_rate(&self) -> Result<f64> {
        let resp = self.client
            .get("https://mempool.space/api/v1/fees/recommended")
            .send()
            .await?;
        let data: MempoolFee = resp.json().await?;
        Ok(data.fastest_fee)
    }
}
