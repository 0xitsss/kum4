use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;

use crate::monitor::DepositEvent;

pub struct RebalanceEngine {
    accumulated_usdt: Arc<Mutex<Accumulator>>,
    pub threshold: f64,
}

struct Accumulator {
    total: f64,
    tx_hashes: Vec<String>,
}

impl RebalanceEngine {
    pub fn new(threshold: f64) -> Self {
        RebalanceEngine {
            accumulated_usdt: Arc::new(Mutex::new(Accumulator { total: 0.0, tx_hashes: vec![] })),
            threshold,
        }
    }

    pub async fn add_deposit(&self, deposit: DepositEvent) -> f64 {
        let mut acc = self.accumulated_usdt.lock().await;
        acc.total += deposit.usdt_amount;
        acc.tx_hashes.push(deposit.tx_hash.clone());
        tracing::info!(
            total = acc.total, threshold = self.threshold,
            "Deposit accumulated"
        );
        if acc.total >= self.threshold {
            tracing::warn!(
                total = acc.total,
                "USDT threshold reached — refill BTC reserve manually"
            );
        }
        acc.total
    }

    pub async fn run(&self) {
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            let total = {
                let acc = self.accumulated_usdt.lock().await;
                acc.total
            };
            if total >= self.threshold {
                tracing::info!(
                    total,
                    "Accumulated USDT above threshold, awaiting operator action"
                );
            }
        }
    }
}
