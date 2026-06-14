use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

use crate::error::Result;
use crate::exchange::ExchangeProvider;
use crate::monitor::DepositEvent;

pub struct RebalanceEngine {
    accumulated_usdt: Arc<Mutex<Accumulator>>,
    threshold: f64,
    interval: Duration,
    exchange: Box<dyn ExchangeProvider>,
    merchant_address: String,
    btc_deposit_address: String,
}

struct Accumulator {
    total: f64,
    tx_hashes: Vec<String>,
}

impl RebalanceEngine {
    pub fn new(
        exchange: Box<dyn ExchangeProvider>,
        threshold: f64,
        interval_secs: u64,
        merchant_address: String,
        btc_deposit_address: String,
    ) -> Self {
        RebalanceEngine {
            accumulated_usdt: Arc::new(Mutex::new(Accumulator { total: 0.0, tx_hashes: vec![] })),
            threshold,
            interval: Duration::from_secs(interval_secs),
            exchange,
            merchant_address,
            btc_deposit_address,
        }
    }

    pub async fn add_deposit(&self, deposit: DepositEvent) {
        let mut acc = self.accumulated_usdt.lock().await;
        acc.total += deposit.usdt_amount;
        acc.tx_hashes.push(deposit.tx_hash.clone());
        tracing::info!(
            total = acc.total, threshold = self.threshold,
            "Deposit accumulated"
        );
    }

    pub async fn run(&self) {
        let last_rebalance = Arc::new(Mutex::new(Instant::now()));
        loop {
            tokio::time::sleep(Duration::from_secs(10)).await;
            let should_rebalance = {
                let acc = self.accumulated_usdt.lock().await;
                let elapsed = last_rebalance.lock().await.elapsed();
                acc.total >= self.threshold || elapsed >= self.interval
            };
            if should_rebalance {
                let total = {
                    let mut acc = self.accumulated_usdt.lock().await;
                    let t = acc.total;
                    acc.total = 0.0;
                    acc.tx_hashes.clear();
                    t
                };
                if total > 0.0 {
                    if let Err(e) = self.execute_swap(total).await {
                        tracing::error!(error = %e, total, "Rebalance swap failed");
                        let mut acc = self.accumulated_usdt.lock().await;
                        acc.total += total;
                    }
                }
                *last_rebalance.lock().await = Instant::now();
            }
        }
    }

    async fn execute_swap(&self, usdt_amount: f64) -> Result<()> {
        tracing::info!(usdt_amount, "Starting rebalance swap");
        self.exchange.deposit_usdt(usdt_amount, "BEP20").await?;
        let btc_amount = self.exchange.market_sell("BTCUSDT").await?;
        let sats = (btc_amount * 100_000_000.0) as u64;
        self.exchange.withdraw_btc(&self.merchant_address, sats).await?;
        tracing::info!(usdt_amount, btc_amount, sats, "Rebalance complete");
        Ok(())
    }
}
