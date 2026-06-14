use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Html;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::database::Database;
use crate::error::{Kum4Error, Result};
use crate::p2p::P2pState;
use crate::wallet::Wallet;

pub struct AppState {
    pub wallet: Arc<Wallet>,
    pub db: Database,
    pub config: Config,
    pub mempool_url: String,
    pub peer_id: String,
    pub uptime_start: tokio::time::Instant,
    pub p2p_state: Arc<P2pState>,
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/api/rate", get(api_rate))
        // .route("/api/addresses", get(api_addresses))
        .route("/api/calculate", post(api_calculate))
        .route("/api/health", get(api_health))
        .route("/api/p2p/reserve", get(p2p_reserve_handler))
        .route("/api/p2p/redirect", post(p2p_redirect_handler))
        .route("/api/reserve", post(api_set_reserve))
        .route("/api/exchange", post(api_create_exchange))
        .route("/api/exchange/:id", get(api_get_exchange))
        .route("/exchange/:id", get(exchange_page))
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
}

async fn api_rate(State(state): State<Arc<AppState>>) -> Json<RateResponse> {
    let (btc_usd, fee_rate) = fetch_price(&state.mempool_url).await.unwrap_or((0.0, 50.0));
    Json(RateResponse {
        btc_usd,
        fee_rate,
        profit_fee_usd: state.config.profit_fee_usd,
    })
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
        Err(e) => {
            return Json(CalculateResponse {
                usdt_amount: None,
                btc_amount: None,
                btc_price: 0.0,
                fee_usd: 0.0,
                error: Some(e),
            })
        }
    };

    let profit_fee = state.config.profit_fee_usd;
    let tx_vbytes = 150.0;
    let gas_sats = fee_rate * tx_vbytes;
    let gas_usd = gas_sats * btc_price / 100_000_000.0;
    let total_fee = profit_fee + gas_usd;

    if let Some(usdt) = query.usdt {
        if usdt < state.config.min_usdt_amount {
            return Json(CalculateResponse {
                usdt_amount: None,
                btc_amount: None,
                btc_price,
                fee_usd: total_fee,
                error: Some(format!("Minimum {} USDT", state.config.min_usdt_amount)),
            });
        }
        let net = usdt - total_fee;
        let btc_amount = if net > 0.0 { net / btc_price } else { 0.0 };
        Json(CalculateResponse {
            usdt_amount: Some(format!("{:.2}", usdt)),
            btc_amount: Some(format!("{:.8}", btc_amount)),
            btc_price,
            fee_usd: total_fee,
            error: None,
        })
    } else if let Some(btc) = query.btc {
        let usdt_needed = btc * btc_price + total_fee;
        Json(CalculateResponse {
            usdt_amount: Some(format!("{:.2}", usdt_needed)),
            btc_amount: Some(format!("{:.8}", btc)),
            btc_price,
            fee_usd: total_fee,
            error: None,
        })
    } else {
        Json(CalculateResponse {
            usdt_amount: None,
            btc_amount: None,
            btc_price: 0.0,
            fee_usd: 0.0,
            error: Some("Provide `usdt` or `btc` field".into()),
        })
    }
}

#[derive(Serialize)]
struct HealthResponse {
    node_id: String,
    version: String,
    peer_id: String,
    status: String,
    fee_usd: f64,
    chains: Vec<String>,
    uptime_secs: u64,
    btc_reserve: f64,
}

async fn api_health(State(state): State<Arc<AppState>>) -> Json<HealthResponse> {
    let reserve = state.p2p_state.reserve.lock().await;
    Json(HealthResponse {
        node_id: state.config.node_id.clone(),
        version: state.config.node_version.clone(),
        peer_id: state.peer_id.clone(),
        status: reserve.status.clone(),
        fee_usd: state.config.profit_fee_usd,
        chains: vec!["TRC20".into(), "BEP20".into()],
        uptime_secs: state.uptime_start.elapsed().as_secs(),
        btc_reserve: reserve.btc_reserve,
    })
}

#[derive(Deserialize)]
struct SetReserveBody {
    btc_reserve: f64,
}

#[derive(Serialize)]
struct SetReserveResponse {
    btc_reserve: f64,
    message: String,
}

async fn api_set_reserve(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SetReserveBody>,
) -> Json<SetReserveResponse> {
    let mut reserve = state.p2p_state.reserve.lock().await;
    reserve.btc_reserve = body.btc_reserve;
    tracing::info!(btc_reserve = body.btc_reserve, "BTC reserve updated");
    Json(SetReserveResponse {
        btc_reserve: body.btc_reserve,
        message: "BTC reserve updated".into(),
    })
}

#[derive(Serialize)]
struct P2pReserveResponse {
    peer_id: String,
    btc_reserve: f64,
    fee_usd: f64,
    status: String,
}

async fn p2p_reserve_handler(State(state): State<Arc<AppState>>) -> Json<P2pReserveResponse> {
    let r = state.p2p_state.reserve.lock().await;
    Json(P2pReserveResponse {
        peer_id: state.p2p_state.peer_id.clone(),
        btc_reserve: r.btc_reserve,
        fee_usd: r.fee_usd,
        status: r.status.clone(),
    })
}

#[derive(Deserialize)]
struct P2pRedirectBody {
    from_peer: String,
    usdt_amount: f64,
    chain: String,
    user_btc_address: String,
    #[allow(dead_code)]
    deposit_txid: String,
}

#[derive(Serialize)]
struct P2pRedirectResponse {
    accepted: bool,
    message: String,
}

async fn p2p_redirect_handler(
    State(state): State<Arc<AppState>>,
    Json(body): Json<P2pRedirectBody>,
) -> Json<P2pRedirectResponse> {
    let reserve = state.p2p_state.reserve.lock().await;
    let required_btc = body.usdt_amount / 100_000.0;

    if reserve.btc_reserve >= required_btc {
        tracing::info!(
            from = %body.from_peer, usdt = %body.usdt_amount,
            chain = %body.chain, to = %body.user_btc_address,
            "Accepting redirect"
        );
        Json(P2pRedirectResponse {
            accepted: true,
            message: "Redirect accepted, processing swap".into(),
        })
    } else {
        Json(P2pRedirectResponse {
            accepted: false,
            message: format!(
                "Insufficient reserve: have {:.8} BTC, need {:.8} BTC",
                reserve.btc_reserve, required_btc
            ),
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

#[derive(Deserialize)]
struct CreateExchangeBody {
    chain: String,
    btc_address: String,
    usdt_amount: f64,
}

#[derive(Serialize)]
struct CreateExchangeResponse {
    id: String,
    chain: String,
    deposit_address: String,
    btc_address: String,
    status: String,
    created_at: u64,
    expires_at: u64,
}

async fn api_create_exchange(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateExchangeBody>,
) -> Result<Json<CreateExchangeResponse>> {
    if body.btc_address.trim().is_empty() {
        return Err(Kum4Error::Config("BTC address is required".into()));
    }
    if body.usdt_amount < state.config.min_usdt_amount {
        return Err(Kum4Error::Config(format!(
            "Minimum {} USDT", state.config.min_usdt_amount
        )));
    }
    let chain = if body.chain == "bsc" { "bsc" } else { "tron" };

    // Fetch price to calculate expected BTC amount
    let (btc_price, fee_rate) = fetch_price(&state.mempool_url).await
        .unwrap_or((0.0, 50.0));

    let profit_fee = state.config.profit_fee_usd;
    let tx_vbytes = 150.0;
    let gas_sats = fee_rate * tx_vbytes;
    let gas_usd = gas_sats * btc_price / 100_000_000.0;
    let total_fee = profit_fee + gas_usd;
    let net = body.usdt_amount - total_fee;
    let expected_btc = if btc_price > 0.0 && net > 0.0 { net / btc_price } else { 0.0 };

    // Allocate next unique address for this exchange
    let idx = state.db.addr_index(chain)?;
    let deposit_address = match chain {
        "tron" => state.wallet.tron_address_at_index(idx)?,
        "bsc" => state.wallet.eth_address_at_index(idx)?,
        _ => return Err(Kum4Error::Config("Invalid chain".into())),
    };
    state.db.advance_addr_index(chain)?;

    let req = state.db.create_exchange(chain, &deposit_address, &body.btc_address, body.usdt_amount, expected_btc)?;

    tracing::info!(
        exchange_id = %req.id, chain = %chain,
        deposit = %deposit_address, btc = %body.btc_address,
        "Exchange created"
    );

    Ok(Json(CreateExchangeResponse {
        id: req.id,
        chain: req.chain,
        deposit_address: req.deposit_address,
        btc_address: req.btc_address,
        status: req.status,
        created_at: req.created_at,
        expires_at: req.expires_at,
    }))
}

#[derive(Serialize)]
struct ExchangeStatusResponse {
    id: String,
    chain: String,
    deposit_address: String,
    btc_address: String,
    status: String,
    usdt_amount: Option<f64>,
    btc_amount: Option<f64>,
    created_at: u64,
    expires_at: u64,
}

async fn api_get_exchange(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<ExchangeStatusResponse>> {
    let req = state.db.get_exchange(&id)?
        .ok_or_else(|| Kum4Error::Internal("Exchange not found".into()))?;
    Ok(Json(ExchangeStatusResponse {
        id: req.id,
        chain: req.chain,
        deposit_address: req.deposit_address,
        btc_address: req.btc_address,
        status: req.status,
        usdt_amount: req.usdt_amount,
        btc_amount: req.btc_amount,
        created_at: req.created_at,
        expires_at: req.expires_at,
    }))
}

async fn exchange_page() -> Html<&'static str> {
    Html(include_str!("../templates/exchange.html"))
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
