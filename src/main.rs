mod bitcoin_tx;
mod config;
mod database;
mod error;
mod exchange;
mod monitor;
mod price;
mod rebalance;
mod wallet;
mod web;

use std::sync::Arc;

use tokio::sync::mpsc;

use crate::config::Config;
use crate::database::Database;
use crate::exchange::CexExchange;
use crate::monitor::Monitor;
use crate::rebalance::RebalanceEngine;
use crate::wallet::Wallet;

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
    let _btc_addresses: Vec<String> = wallet
        .btc_addresses()?
        .into_iter()
        .map(|(addr, _)| addr.to_string())
        .collect();

    let (deposit_tx, mut deposit_rx) = mpsc::channel::<monitor::DepositEvent>(1024);
    let client = reqwest::Client::new();

    let exchange = Box::new(CexExchange::new(
        std::env::var("CEX_API_KEY").unwrap_or_default(),
        std::env::var("CEX_API_SECRET").unwrap_or_default(),
        std::env::var("CEX_BASE_URL")
            .unwrap_or_else(|_| "https://api.binance.com".into()),
    ));

    let tron_rpc = config.tron_rpc_url.clone();
    let tron_contract = config.tron_usdt_contract.clone();
    let bsc_rpc = config.bsc_rpc_url.clone();
    let bsc_contract = config.bsc_usdt_contract.clone();
    let db_path = config.db_path.clone();

    let rebalance = Arc::new(RebalanceEngine::new(
        exchange,
        config.rebalance_threshold,
        config.rebalance_interval_secs,
        config.merchant_btc_address.clone(),
        String::new(),
    ));

    let rebalance_loop = rebalance.clone();
    tokio::spawn(async move { rebalance_loop.run().await; });

    let rebalance_handler = rebalance.clone();
    tokio::spawn(async move {
        while let Some(deposit) = deposit_rx.recv().await {
            tracing::info!(
                chain = %deposit.chain, tx = %deposit.tx_hash,
                amount = %deposit.usdt_amount, from = %deposit.from_address,
                "Deposit detected"
            );
            rebalance_handler.add_deposit(deposit).await;
        }
    });

    let db = Database::open(&db_path)?;
    let client_tron = client.clone();
    let client_bsc = client.clone();
    let tx_tron = deposit_tx.clone();
    let tx_bsc = deposit_tx.clone();
    let db_tron = db.clone();
    let db_bsc = db;
    tokio::spawn(async move {
        Monitor::start_tron(client_tron, tron_rpc, tron_contract, vec![], tx_tron, db_tron).await;
    });
    tokio::spawn(async move {
        Monitor::start_bsc(client_bsc, bsc_rpc, bsc_contract, vec![], tx_bsc, db_bsc).await;
    });

    drop(deposit_tx);

    let app_state = Arc::new(web::AppState {
        wallet,
        config: config.clone(),
        mempool_url: config.mempool_url.clone(),
    });
    let web_port = std::env::var("WEB_PORT").unwrap_or_else(|_| "3000".into());
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{web_port}")).await?;
    tracing::info!("Web UI: http://0.0.0.0:{web_port}");

    tokio::spawn(async move {
        axum::serve(listener, web::router(app_state))
            .await
            .unwrap();
    });

    tracing::info!("Kumquad started");
    tokio::signal::ctrl_c().await?;
    tracing::info!("Shutting down");
    Ok(())
}
