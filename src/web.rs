use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Html;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::error::{Kum4Error, Result};
use crate::wallet::Wallet;

pub struct AppState {
    pub wallet: Wallet,
    pub config: Config,
    pub mempool_url: String,
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/api/rate", get(api_rate))
        .route("/api/addresses", get(api_addresses))
        .route("/api/calculate", post(api_calculate))
        .with_state(state)
}

async fn index() -> Html<&'static str> {
    Html(include_str!("../templates/index.html"))
}

#[derive(Serialize)]
struct RateResponse {
    btc_usd: f64,
    fee_rate: f64,
    profit_fee_usd: f64,
    min_usdt: f64,
}

async fn api_rate(
    State(state): State<Arc<AppState>>,
) -> Json<RateResponse> {
    let (btc_usd, fee_rate) = fetch_price(&state.mempool_url).await.unwrap_or((0.0, 50.0));
    Json(RateResponse { btc_usd, fee_rate, profit_fee_usd: 1.0, min_usdt: 10.0 })
}

#[derive(Serialize)]
struct AddressesResponse {
    tron: Vec<String>,
    bsc: Vec<String>,
    btc: Vec<String>,
}

async fn api_addresses(
    State(state): State<Arc<AppState>>,
) -> Result<Json<AddressesResponse>> {
    let tron = (0..5)
        .map(|i| state.wallet.tron_address_at_index(i))
        .filter_map(|r| r.ok())
        .collect();
    let bsc = (0..5)
        .map(|i| state.wallet.eth_address_at_index(i))
        .filter_map(|r| r.ok())
        .collect();
    let btc = (0..5)
        .map(|i| state.wallet.btc_address(i).map(|a| a.to_string()))
        .filter_map(|r| r.ok())
        .collect();
    Ok(Json(AddressesResponse { tron, bsc, btc }))
}

#[derive(Deserialize)]
struct CalculateQuery {
    usdt: Option<f64>,
    btc: Option<f64>,
}

#[derive(Serialize)]
struct CalculateResponse {
    usdt_amount: Option<String>,
    btc_amount: Option<String>,
    btc_price: f64,
    fee_usd: f64,
    error: Option<String>,
}

async fn api_calculate(
    State(state): State<Arc<AppState>>,
    Json(query): Json<CalculateQuery>,
) -> Json<CalculateResponse> {
    let (btc_price, fee_rate) = match fetch_price(&state.mempool_url).await {
        Ok(p) => p,
        Err(e) => return Json(CalculateResponse {
            usdt_amount: None, btc_amount: None,
            btc_price: 0.0, fee_usd: 0.0,
            error: Some(e),
        }),
    };

    let profit_fee = state.config.profit_fee_usd;
    let tx_vbytes = 150.0;
    let gas_sats = fee_rate * tx_vbytes;
    let gas_usd = gas_sats * btc_price / 100_000_000.0;
    let total_fee = profit_fee + gas_usd;

    if let Some(usdt) = query.usdt {
        if usdt < state.config.min_usdt_amount {
            return Json(CalculateResponse {
                usdt_amount: None, btc_amount: None,
                btc_price, fee_usd: total_fee,
                error: Some(format!("Minimum {} USDT", state.config.min_usdt_amount)),
            });
        }
        let net = usdt - total_fee;
        let btc_amount = if net > 0.0 { net / btc_price } else { 0.0 };
        Json(CalculateResponse {
            usdt_amount: Some(format!("{:.2}", usdt)),
            btc_amount: Some(format!("{:.8}", btc_amount)),
            btc_price, fee_usd: total_fee, error: None,
        })
    } else if let Some(btc) = query.btc {
        let usdt_needed = btc * btc_price + total_fee;
        Json(CalculateResponse {
            usdt_amount: Some(format!("{:.2}", usdt_needed)),
            btc_amount: Some(format!("{:.8}", btc)),
            btc_price, fee_usd: total_fee, error: None,
        })
    } else {
        Json(CalculateResponse {
            usdt_amount: None, btc_amount: None,
            btc_price: 0.0, fee_usd: 0.0,
            error: Some("Provide `usdt` or `btc` field".into()),
        })
    }
}

async fn fetch_price(mempool_url: &str) -> std::result::Result<(f64, f64), String> {
    let client = reqwest::Client::new();
    let price_resp = client
        .get("https://api.binance.com/api/v3/ticker/price?symbol=BTCUSDT")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let price_data: serde_json::Value = price_resp.json().await.map_err(|e| e.to_string())?;
    let btc_usd = price_data["price"]
        .as_str()
        .and_then(|p| p.parse::<f64>().ok())
        .ok_or("BTC price parse error")?;

    let fee_url = format!("{mempool_url}/api/v1/fees/recommended");
    let fee_resp = client
        .get(&fee_url)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let fee_data: serde_json::Value = fee_resp.json().await.map_err(|e| e.to_string())?;
    let fee_rate = fee_data["fastestFee"].as_f64().unwrap_or(50.0);

    Ok((btc_usd, fee_rate))
}

impl axum::response::IntoResponse for Kum4Error {
    fn into_response(self) -> axum::response::Response {
        let code = match &self {
            Kum4Error::Config(_) => StatusCode::BAD_REQUEST,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (code, Json(serde_json::json!({ "error": self.to_string() }))).into_response()
    }
}
