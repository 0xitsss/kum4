use std::sync::Arc;
use tokio::sync::mpsc;

use teloxide::dispatching::UpdateFilterExt;
use teloxide::dptree;
use teloxide::prelude::*;
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup, MaybeInaccessibleMessage};

use crate::database::Database;
use crate::wallet::Wallet;

use crate::monitor::{Chain, DepositEvent};
use crate::{config::Config, error::Result};

#[derive(serde::Deserialize, Debug)]
#[allow(dead_code)]
struct UtxoStatus {
    confirmed: bool,
    block_height: Option<u64>,
}

#[derive(serde::Deserialize, Debug)]
#[allow(dead_code)]
struct UtxoJson {
    txid: String,
    vout: u32,
    value: u64,
    status: UtxoStatus,
}

#[allow(dead_code)]
fn sats_to_btc(sats: u64) -> f64 {
    sats as f64 / 100_000_000.0
}

#[allow(dead_code)]
fn reserve_is_low(reserve_btc: f64, pending_btc: f64) -> bool {
    reserve_btc < pending_btc * 1.2
}

#[allow(dead_code)]
fn paginate<T>(items: &[T], page: usize, per_page: usize) -> (&[T], usize) {
    let total_pages = items.len().div_ceil(per_page);
    let page = page.clamp(1, total_pages.max(1));
    let start = (page - 1) * per_page;
    let end = start + per_page.min(items.len().saturating_sub(start));
    (&items[start..end], total_pages)
}

#[allow(dead_code)]
async fn fetch_btc_balance(client: &reqwest::Client, mempool_url: &str, address: &str)
    -> std::result::Result<(Vec<UtxoJson>, u64, u64), String>
{
    let url = format!("{}/api/address/{}/utxo", mempool_url.trim_end_matches('/'), address);
    let resp = client.get(&url).send().await.map_err(|e| format!("HTTP error: {e}"))?;
    let utxos: Vec<UtxoJson> = resp.json().await.map_err(|e| format!("Parse error: {e}"))?;
    let confirmed_value: u64 = utxos.iter().filter(|u| u.status.confirmed).map(|u| u.value).sum();
    let unconfirmed_value: u64 = utxos.iter().filter(|u| !u.status.confirmed).map(|u| u.value).sum();
    Ok((utxos, confirmed_value, unconfirmed_value))
}

#[allow(dead_code)]
fn format_utxo_summary(utxos: &[UtxoJson], confirmed_sats: u64, unconfirmed_sats: u64) -> String {
    let confirmed_count = utxos.iter().filter(|u| u.status.confirmed).count();
    let unconfirmed_count = utxos.iter().filter(|u| !u.status.confirmed).count();
    format!(
        "Balance: `{:.8}` BTC\nUTXOs: {} confirmed, {} unconfirmed",
        sats_to_btc(confirmed_sats + unconfirmed_sats),
        confirmed_count,
        unconfirmed_count,
    )
}

pub struct BotState {
    pub db: Database,
    pub config: Config,
    #[allow(dead_code)]
    pub wallet: Arc<Wallet>,
    #[allow(dead_code)]
    pub http_client: reqwest::Client,
    pub deposit_tx: mpsc::Sender<DepositEvent>,
}

#[allow(dead_code)]
const DATE_FMT: &str = "%b %d %H:%M";

fn fmt_time(ts: u64) -> String {
    let d = chrono::DateTime::from_timestamp(ts as i64, 0)
        .unwrap_or_default();
    d.format(DATE_FMT).to_string()
}

fn main_menu_kb() -> InlineKeyboardMarkup {
    let mut kb: Vec<Vec<InlineKeyboardButton>> = Vec::new();
    let items = [
        ("📊 Dashboard", "menu:dashboard"),
        ("💱 Exchanges", "menu:exchanges"),
        ("🏦 BTC Reserve", "menu:reserve"),
        ("⚙️ System / Health", "menu:system"),
        ("🔍 Manual Reviews", "menu:reviews"),
    ];
    for (text, cb) in &items {
        kb.push(vec![InlineKeyboardButton::callback(*text, cb.to_string())]);
    }
    InlineKeyboardMarkup::new(kb)
}

fn back_kb() -> InlineKeyboardMarkup {
    let kb = vec![
        vec![InlineKeyboardButton::callback("🔙 Main Menu", "menu:back")]
    ];
    InlineKeyboardMarkup::new(kb)
}

fn main_menu_text() -> String {
    "🏠 *Kumquad Admin*\n\nDashboard, exchanges, BTC reserve — all in one place.".into()
}

fn is_admin(msg: &Message, state: &BotState) -> bool {
    let uid = msg.from.as_ref().map(|u| u.id.0 as i64).unwrap_or(0);
    uid == state.config.admin_user_id
}

fn admin_only(msg: Message, state: Arc<BotState>) -> String {
    if !is_admin(&msg, &state) {
        return "⛔ Access denied. You are not the admin.".into();
    }
    String::new()
}

fn build_exchange_kb(exchange: &crate::database::ExchangeRequest) -> InlineKeyboardMarkup {
    let mut kb: Vec<Vec<InlineKeyboardButton>> = Vec::new();
    let mut row = Vec::new();
    row.push(InlineKeyboardButton::callback("📋 Details", format!("exch:{}", exchange.id)));
    if exchange.status == "pending" || exchange.status == "deposit_detected" || exchange.status == "error" {
        row.push(InlineKeyboardButton::callback("🔄 Resolve", format!("resolve:{}", exchange.id)));
    }
    kb.push(row);
    InlineKeyboardMarkup::new(kb)
}

fn exchange_summary(exchange: &crate::database::ExchangeRequest) -> String {
    let usdt = exchange.usdt_amount.map(|v| format!("{:.2}", v)).unwrap_or("?".into());
    let btc = exchange.btc_amount.map(|v| format!("{:.8}", v)).unwrap_or("?".into());
    let status_icon = match exchange.status.as_str() {
        "pending" => "⏳",
        "deposit_detected" => "🔍",
        "sending" => "🔄",
        "sent" => "✅",
        "confirmed" => "✅",
        "completed" => "✅",
        "error" => "❌",
        "expired" => "⌛",
        _ => "❓",
    };
    format!(
        "{icon} `{id}` • {chain}\n{usdt} USDT → {btc} BTC\n{status}",
        icon = status_icon,
        id = exchange.id.chars().take(12).collect::<String>(),
        chain = exchange.chain,
        usdt = usdt,
        btc = btc,
        status = exchange.status,
    )
}

fn exchange_detail(exchange: &crate::database::ExchangeRequest) -> String {
    let usdt = exchange.usdt_amount.map(|v| format!("{:.2}", v)).unwrap_or("—".into());
    let btc = exchange.btc_amount.map(|v| format!("{:.8}", v)).unwrap_or("—".into());
    let status_icon = match exchange.status.as_str() {
        "pending" => "⏳",
        "deposit_detected" => "🔍",
        "sending" => "🔄",
        "sent" | "confirmed" | "completed" => "✅",
        "error" => "❌",
        "expired" => "⌛",
        _ => "❓",
    };
    format!(
        "┌ *Exchange* `{id}`\n\
         ├ Chain: `{chain}`\n\
         ├ Status: {icon} {status}\n\
         ├ USDT: `{usdt}`\n\
         ├ BTC: `{btc}`\n\
         ├ Deposit: `{deposit}`\n\
         ├ Destination: `{dest}`\n\
         ├ Created: {created}\n\
         └ Expires: {expires}",
        id = exchange.id,
        chain = exchange.chain,
        icon = status_icon,
        status = exchange.status,
        usdt = usdt,
        btc = btc,
        deposit = exchange.deposit_address,
        dest = exchange.btc_address,
        created = fmt_time(exchange.created_at),
        expires = fmt_time(exchange.expires_at),
    )
}

fn build_system_text(version: &str, node_id: &str, tron_ok: bool, bsc_ok: bool) -> String {
    let tron_icon = if tron_ok { "✅" } else { "❌" };
    let bsc_icon = if bsc_ok { "✅" } else { "❌" };
    format!(
        "⚙️ *System*\n\n\
         Bot: ✅ running\n\
         Version: `{}`\n\
         Node: `{}`\n\
         TRON RPC: {}\n\
         BSC RPC:  {}",
        version, node_id, tron_icon, bsc_icon,
    )
}

fn build_reserve_text(address: &str, utxos: &[UtxoJson], confirmed_sats: u64, unconfirmed_sats: u64, pending_btc: f64) -> String {
    let balance = sats_to_btc(confirmed_sats + unconfirmed_sats);
    let utxo_total: u64 = utxos.iter().map(|u| u.value).sum();
    let confirmed_count = utxos.iter().filter(|u| u.status.confirmed).count();
    let unconfirmed_count = utxos.iter().filter(|u| !u.status.confirmed).count();
    let mut utxo_lines: Vec<String> = Vec::new();
    let show = utxos.iter().take(10);
    for (i, u) in show.enumerate() {
        let short_txid: String = u.txid.chars().take(12).collect();
        let conf: String = if u.status.confirmed {
            " (confirmed)".into()
        } else {
            " (unconfirmed)".into()
        };
        utxo_lines.push(format!("{}. `{}` — {:.8} BTC{}", i + 1, short_txid, sats_to_btc(u.value), conf));
    }
    if utxos.len() > 10 {
        utxo_lines.push(format!("... and {} more", utxos.len() - 10));
    }
    let utxo_section = if utxo_lines.is_empty() {
        "No UTXOs".into()
    } else {
        utxo_lines.join("\n")
    };
    let low = reserve_is_low(sats_to_btc(utxo_total), pending_btc);
    let warning = if low {
        format!("\n⚠️ *Warning:* Reserve below 1.2× pending ({:.8} BTC needed)!", pending_btc * 1.2)
    } else {
        "\n✅ Reserve adequate.".into()
    };
    format!(
        "🏦 *BTC Reserve*\n\n\
         Address: `{addr}`\n\
         Balance: `{bal:.8}` BTC\n\
         UTXOs: {conf} confirmed, {unconf} unconfirmed\n\
         Pending outgoing: `{pend:.8}` BTC\n\n\
         {utxos}{warn}",
        addr = address, bal = balance,
        conf = confirmed_count, unconf = unconfirmed_count,
        pend = pending_btc, utxos = utxo_section, warn = warning,
    )
}

fn build_reviews_view(reviews: &[serde_json::Value]) -> (String, InlineKeyboardMarkup) {
    if reviews.is_empty() {
        return ("✅ No manual reviews.".into(), back_kb());
    }
    let mut lines: Vec<String> = Vec::new();
    let mut kb_rows: Vec<Vec<InlineKeyboardButton>> = Vec::new();
    lines.push(format!("🔍 *Manual Reviews: {}*", reviews.len()));
    for (i, r) in reviews.iter().enumerate() {
        let tx = r["tx_hash"].as_str().unwrap_or("?");
        let chain = r["chain"].as_str().unwrap_or("?");
        let got = r["got_amount"].as_f64().unwrap_or(0.0);
        let expected = r["expected_amount"].as_f64().unwrap_or(0.0);
        let short_tx: String = tx.chars().take(16).collect();
        lines.push(format!("{}. `{}` {} — got {:.2}, expected {:.2}", i + 1, short_tx, chain, got, expected));
        kb_rows.push(vec![
            InlineKeyboardButton::callback(format!("✅ Resolve: {}", short_tx), format!("resolve_review:{}", tx)),
        ]);
    }
    kb_rows.push(vec![InlineKeyboardButton::callback("🔙 Main Menu", "menu:back")]);
    (lines.join("\n"), InlineKeyboardMarkup::new(kb_rows))
}

fn build_dashboard_text(tron_pending: usize, bsc_pending: usize, n_reviews: usize, btc_balance: f64) -> String {
    let total = tron_pending + bsc_pending;
    let warning = if n_reviews > 0 {
        format!("\n⚠️ {} manual review{} pending", n_reviews, if n_reviews == 1 { "" } else { "s" })
    } else {
        String::new()
    };
    format!(
        "📊 *Dashboard*\n\n\
         💱 Pending exchanges: {total} (TRON: {tron}, BSC: {bsc})\n\
         🏦 BTC Reserve: `{btc:.8}` BTC\n\
         🔍 Manual reviews: {n}{warning}\n\
         🩺 System: ✅ running",
        total = total, tron = tron_pending, bsc = bsc_pending,
        btc = btc_balance, n = n_reviews, warning = warning,
    )
}

fn build_exchanges_page_text(exchanges: &[crate::database::ExchangeRequest], page: usize, total_pages: usize) -> (String, bool, bool) {
    if exchanges.is_empty() {
        return ("✅ No exchanges to show.".into(), false, false);
    }
    let lines: Vec<String> = exchanges.iter().map(|ex| exchange_summary(ex)).collect();
    let text = format!("💱 *Exchanges (стр {}/{})*\n\n{}", page, total_pages, lines.join("\n\n"));
    let has_prev = page > 1;
    let has_next = page < total_pages;
    (text, has_next, has_prev)
}

async fn cmd_dashboard(bot: Bot, msg: Message, state: Arc<BotState>) -> Result<()> {
    let deny = admin_only(msg.clone(), state.clone());
    if !deny.is_empty() {
        bot.send_message(msg.chat.id, &deny).await?;
        return Ok(());
    }

    let tron = state.db.get_pending_exchanges("tron").unwrap_or_default().len();
    let bsc = state.db.get_pending_exchanges("bsc").unwrap_or_default().len();
    let reviews = state.db.get_manual_reviews().unwrap_or_default().len();
    let reserve_addr = state.wallet.btc_address(state.config.btc_reserve_index)?;
    let (_utxos, confirmed, unconfirmed) = fetch_btc_balance(
        &state.http_client, &state.config.mempool_url,
        &reserve_addr.to_string(),
    ).await.unwrap_or((vec![], 0, 0));
    let balance = sats_to_btc(confirmed + unconfirmed);
    let text = build_dashboard_text(tron, bsc, reviews, balance);
    bot.send_message(msg.chat.id, text)
        .parse_mode(teloxide::types::ParseMode::MarkdownV2)
        .reply_markup(back_kb())
        .await?;
    Ok(())
}

async fn cmd_start(bot: Bot, msg: Message, state: Arc<BotState>) -> Result<()> {
    if !is_admin(&msg, &state) {
        bot.send_message(msg.chat.id, "⛔ Access denied.").await?;
        return Ok(());
    }
    bot.send_message(msg.chat.id, main_menu_text())
        .parse_mode(teloxide::types::ParseMode::MarkdownV2)
        .reply_markup(main_menu_kb())
        .await?;
    Ok(())
}

async fn cmd_exchanges(bot: Bot, msg: Message, state: Arc<BotState>) -> Result<()> {
    cmd_exchanges_page(bot, msg, state, 1).await
}

async fn cmd_exchanges_page(bot: Bot, msg: Message, state: Arc<BotState>, page: usize) -> Result<()> {
    let deny = admin_only(msg.clone(), state.clone());
    if !deny.is_empty() {
        bot.send_message(msg.chat.id, &deny).await?;
        return Ok(());
    }
    let mut all: Vec<crate::database::ExchangeRequest> = Vec::new();
    for chain in &["tron", "bsc"] {
        if let Ok(mut exs) = state.db.get_pending_exchanges(chain) {
            all.append(&mut exs);
        }
    }
    let per_page = 5usize;
    let total_pages = (all.len() + per_page - 1) / per_page;
    let page = page.clamp(1, total_pages.max(1));
    let start = (page - 1) * per_page;
    let end = start + per_page.min(all.len().saturating_sub(start));
    let page_items = &all[start..end];
    let (text, has_next, has_prev) = build_exchanges_page_text(page_items, page, total_pages.max(1));

    let mut kb_rows: Vec<Vec<InlineKeyboardButton>> = Vec::new();
    if has_prev || has_next {
        let mut row = Vec::new();
        if has_prev {
            row.push(InlineKeyboardButton::callback("◀️", format!("menu:exchanges:p{}", page - 1)));
        }
        row.push(InlineKeyboardButton::callback(format!("{}/{}", page, total_pages.max(1)), "noop"));
        if has_next {
            row.push(InlineKeyboardButton::callback("▶️", format!("menu:exchanges:p{}", page + 1)));
        }
        kb_rows.push(row);
    }
    kb_rows.push(vec![InlineKeyboardButton::callback("🔙 Main Menu", "menu:back")]);
    let kb = InlineKeyboardMarkup::new(kb_rows);

    bot.send_message(msg.chat.id, text)
        .parse_mode(teloxide::types::ParseMode::MarkdownV2)
        .reply_markup(kb)
        .await?;
    Ok(())
}

async fn cmd_exchange(bot: Bot, msg: Message, state: Arc<BotState>, args: String) -> Result<()> {
    let deny = admin_only(msg.clone(), state.clone());
    if !deny.is_empty() {
        bot.send_message(msg.chat.id, &deny).await?;
        return Ok(());
    }

    let id = args.trim();
    if id.is_empty() {
        bot.send_message(msg.chat.id, "Usage: /exchange <id>").await?;
        return Ok(());
    }

    match state.db.get_exchange(id)? {
        Some(ex) => {
            let kb = build_exchange_kb(&ex);
            bot.send_message(msg.chat.id, exchange_detail(&ex))
                .parse_mode(teloxide::types::ParseMode::MarkdownV2)
                .reply_markup(kb)
                .await?;
        }
        None => {
            bot.send_message(msg.chat.id, "❌ Exchange not found.").await?;
        }
    }
    Ok(())
}

async fn cmd_reserve(bot: Bot, msg: Message, state: Arc<BotState>) -> Result<()> {
    let deny = admin_only(msg.clone(), state.clone());
    if !deny.is_empty() {
        bot.send_message(msg.chat.id, &deny).await?;
        return Ok(());
    }
    let reserve_addr = state.wallet.btc_address(state.config.btc_reserve_index)?;
    let pending_btc = state.db.get_pending_total_btc().unwrap_or(0.0);
    let (utxos, confirmed, unconfirmed) = fetch_btc_balance(
        &state.http_client, &state.config.mempool_url,
        &reserve_addr.to_string(),
    ).await.unwrap_or((vec![], 0, 0));
    let text = build_reserve_text(&reserve_addr.to_string(), &utxos, confirmed, unconfirmed, pending_btc);
    bot.send_message(msg.chat.id, text)
        .parse_mode(teloxide::types::ParseMode::MarkdownV2)
        .reply_markup(back_kb())
        .await?;
    Ok(())
}

async fn cmd_health(bot: Bot, msg: Message, state: Arc<BotState>) -> Result<()> {
    let deny = admin_only(msg.clone(), state.clone());
    if !deny.is_empty() {
        bot.send_message(msg.chat.id, &deny).await?;
        return Ok(());
    }
    let tron_ok = state.http_client
        .post(&state.config.tron_rpc_url)
        .json(&serde_json::json!({"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}))
        .send().await.is_ok();
    let bsc_ok = state.http_client
        .post(&state.config.bsc_rpc_url)
        .json(&serde_json::json!({"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}))
        .send().await.is_ok();
    let text = build_system_text(env!("CARGO_PKG_VERSION"), &state.config.node_id, tron_ok, bsc_ok);
    bot.send_message(msg.chat.id, text)
        .parse_mode(teloxide::types::ParseMode::MarkdownV2)
        .reply_markup(back_kb())
        .await?;
    Ok(())
}

async fn cmd_resolve(bot: Bot, msg: Message, state: Arc<BotState>, args: String) -> Result<()> {
    let deny = admin_only(msg.clone(), state.clone());
    if !deny.is_empty() {
        bot.send_message(msg.chat.id, &deny).await?;
        return Ok(());
    }

    let tx_hash = args.trim();
    if tx_hash.is_empty() {
        bot.send_message(msg.chat.id, "Usage: /resolve <tx_hash>").await?;
        return Ok(());
    }

    let reviews = state.db.get_manual_reviews()?;
    let review = reviews.iter().find(|r| r["tx_hash"].as_str().unwrap_or("") == tx_hash).cloned();

    match review {
        Some(r) => {
            let to_addr = r["to_address"].as_str().unwrap_or("");
            let got = r["got_amount"].as_f64().unwrap_or(0.0);
            let chain_str = r["chain"].as_str().unwrap_or("tron");

            let exchange = state.db.find_exchange_by_address(to_addr)?;
            match exchange {
                Some(ex) => {
                    let btc_price = 100_000.0;
                    let fee = 1.0;
                    let net = got - fee;
                    let btc_amount = if net > 0.0 { net / btc_price } else { 0.0 };

                    let _ = state.db.set_exchange_amounts(&ex.id, got, btc_amount);
                    let _ = state.db.set_exchange_status(&ex.id, "deposit_detected");

                    let chain = if chain_str == "bsc" { Chain::Bsc } else { Chain::Tron };
                    let deposit_event = DepositEvent {
                        chain,
                        tx_hash: tx_hash.to_string(),
                        from_address: r["from_address"].as_str().unwrap_or("").to_string(),
                        to_address: to_addr.to_string(),
                        usdt_amount: got,
                        block_number: 0,
                    };

                    match state.deposit_tx.send(deposit_event).await {
                        Ok(_) => {
                            bot.send_message(msg.chat.id, format!(
                                "✅ *Resolved* `{}`\n\nActual USDT: `{:.2}`\nBTC to send: `{:.8}`\nDeposit event sent for processing.",
                                tx_hash, got, btc_amount
                            )).parse_mode(teloxide::types::ParseMode::MarkdownV2).await?;
                        }
                        Err(e) => {
                            bot.send_message(msg.chat.id, format!("❌ Failed to send deposit event: {e}")).await?;
                        }
                    }
                }
                None => {
                    bot.send_message(msg.chat.id, "❌ No matching exchange found for this deposit address.").await?;
                }
            }
        }
        None => {
            bot.send_message(msg.chat.id, "❌ No unresolved manual review with that tx_hash.").await?;
        }
    }
    Ok(())
}

async fn cmd_reviews(bot: Bot, msg: Message, state: Arc<BotState>) -> Result<()> {
    let deny = admin_only(msg.clone(), state.clone());
    if !deny.is_empty() {
        bot.send_message(msg.chat.id, &deny).await?;
        return Ok(());
    }
    let reviews = state.db.get_manual_reviews().unwrap_or_default();
    let (text, kb) = build_reviews_view(&reviews);
    bot.send_message(msg.chat.id, text)
        .parse_mode(teloxide::types::ParseMode::MarkdownV2)
        .reply_markup(kb)
        .await?;
    Ok(())
}

use teloxide::types::MessageId;

fn msg_chat_id(msg: &MaybeInaccessibleMessage) -> ChatId {
    match msg {
        MaybeInaccessibleMessage::Regular(m) => m.chat.id,
        MaybeInaccessibleMessage::Inaccessible(m) => ChatId(m.chat.id.0),
    }
}

fn msg_id_val(msg: &MaybeInaccessibleMessage) -> Option<MessageId> {
    match msg {
        MaybeInaccessibleMessage::Regular(m) => Some(m.id),
        MaybeInaccessibleMessage::Inaccessible(_) => None,
    }
}

async fn callback_handler(bot: Bot, q: CallbackQuery, state: Arc<BotState>) -> Result<()> {
    if let Some(data) = q.data {
        let chat_id = q.message.as_ref().map(msg_chat_id).unwrap_or(ChatId(0));
        let msg_id = q.message.as_ref().and_then(msg_id_val);

        if data.starts_with("exch:") {
            let id = data.trim_start_matches("exch:");
            if let Ok(Some(ex)) = state.db.get_exchange(id) {
                let kb = build_exchange_kb(&ex);
                if let Some(mid) = msg_id {
                    bot.edit_message_text(chat_id, mid, exchange_detail(&ex))
                        .parse_mode(teloxide::types::ParseMode::MarkdownV2)
                        .reply_markup(kb)
                        .await?;
                }
            } else {
                bot.answer_callback_query(q.id).text("Exchange not found").await?;
            }
        } else if data.starts_with("resolve:") {
            let id = data.trim_start_matches("resolve:");
            let exchange = state.db.get_exchange(id)?;
            match exchange {
                Some(ex) => {
                    let usdt = ex.usdt_amount.unwrap_or(0.0);
                    let btc_price = 100_000.0;
                    let fee = 1.0;
                    let net = usdt - fee;
                    let btc_amount = if net > 0.0 { net / btc_price } else { 0.0 };

                    if usdt > 0.0 {
                        let _ = state.db.set_exchange_amounts(&ex.id, usdt, btc_amount);
                    }
                    let _ = state.db.set_exchange_status(&ex.id, "deposit_detected");

                    let chain = if ex.chain == "bsc" { Chain::Bsc } else { Chain::Tron };
                    let deposit_event = DepositEvent {
                        chain,
                        tx_hash: format!("manual-{}", ex.id),
                        from_address: "admin".into(),
                        to_address: ex.deposit_address.clone(),
                        usdt_amount: usdt.max(0.0),
                        block_number: 0,
                    };

                    match state.deposit_tx.send(deposit_event).await {
                        Ok(_) => {
                            bot.answer_callback_query(q.id)
                                .text(format!("✅ Resolved! BTC: {:.8}", btc_amount))
                                .await?;
                            if let Some(mid) = msg_id {
                                let _ = bot.edit_message_text(chat_id, mid, format!(
                                    "✅ *Resolved*\n\nExchange `{}`\nUSDT: `{:.2}` → BTC: `{:.8}`\n\nProcessing BTC send...",
                                    ex.id, usdt, btc_amount
                                )).parse_mode(teloxide::types::ParseMode::MarkdownV2).await;
                            }
                        }
                        Err(e) => {
                            bot.answer_callback_query(q.id).text(format!("Error: {e}")).await?;
                        }
                    }
                }
                None => {
                    bot.answer_callback_query(q.id).text("Exchange not found").await?;
                }
            }
        }
    }
    Ok(())
}

pub async fn run(state: Arc<BotState>) {
    let token = state.config.bot_token.clone();
    if token.is_empty() {
        tracing::info!("BOT_TOKEN not set, Telegram bot disabled");
        return;
    }

    let bot = Bot::new(token);

    let msg_handler = Update::filter_message().endpoint({
        let state = state.clone();
        move |bot: Bot, msg: Message| {
            let state = state.clone();
            async move {
                let text = msg.text().unwrap_or("").to_string();
                if text.starts_with("/start") || text.starts_with("/help") {
                    cmd_start(bot, msg, state).await
                } else if text.starts_with("/exchanges") || text.starts_with("/list") {
                    cmd_exchanges(bot, msg, state).await
                } else if let Some(args) = text.strip_prefix("/exchange ") {
                    cmd_exchange(bot, msg, state, args.to_string()).await
                } else if text.starts_with("/dashboard") {
                    cmd_dashboard(bot, msg, state).await
                } else if text.starts_with("/reviews") {
                    cmd_reviews(bot, msg, state).await
                } else if text.starts_with("/reserve") {
                    cmd_reserve(bot, msg, state).await
                } else if text.starts_with("/health") {
                    cmd_health(bot, msg, state).await
                } else if let Some(args) = text.strip_prefix("/resolve ") {
                    cmd_resolve(bot, msg, state, args.to_string()).await
                } else {
                    Ok(())
                }
            }
        }
    });

    let cq_handler = Update::filter_callback_query().endpoint({
        let state = state.clone();
        move |bot: Bot, q: CallbackQuery| {
            let state = state.clone();
            async move {
                callback_handler(bot, q, state).await
            }
        }
    });

    let bot_handler = dptree::entry().branch(msg_handler).branch(cq_handler);

    tracing::info!("Telegram bot starting...");
    Dispatcher::builder(bot, bot_handler)
        .default_handler(|upd| async move {
            tracing::warn!("Unhandled update: {:?}", upd);
        })
        .error_handler(LoggingErrorHandler::with_custom_text("Bot error"))
        .build()
        .dispatch()
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::wallet::Wallet;
    use bitcoin::Network;

    #[test]
    fn test_parse_utxo_response() {
        let json = r#"[
            {"txid":"abc","vout":0,"value":5000000,"status":{"confirmed":true,"block_height":800000}}
        ]"#;
        let utxos: Vec<UtxoJson> = serde_json::from_str(json).unwrap();
        assert_eq!(utxos.len(), 1);
        assert_eq!(utxos[0].value, 5000000);
        assert!(utxos[0].status.confirmed);
    }

    #[test]
    fn test_pagination_bounds() {
        let total = 13;
        let per_page = 5;
        let max_page = (total + per_page - 1) / per_page;
        assert_eq!(max_page, 3);
        assert_eq!((1 as usize).clamp(1, max_page), 1);
        assert_eq!((0 as usize).clamp(1, max_page), 1);
        assert_eq!((3 as usize).clamp(1, max_page), 3);
        assert_eq!((4 as usize).clamp(1, max_page), 3);
        assert_eq!((5 as usize).clamp(1, max_page), 3);
    }

    #[test]
    fn test_sats_to_btc() {
        assert_eq!(sats_to_btc(100_000_000), 1.0);
        assert_eq!(sats_to_btc(50_000_000), 0.5);
        assert_eq!(sats_to_btc(1), 0.00000001);
        assert_eq!(sats_to_btc(0), 0.0);
    }

    #[test]
    fn test_reserve_warning() {
        assert!(reserve_is_low(0.5, 1.0));
        assert!(!reserve_is_low(2.0, 1.0));
        assert!(!reserve_is_low(1.2, 1.0));
    }

    #[test]
    fn test_format_utxo_summary() {
        let result = format_utxo_summary(&[], 0, 0);
        assert!(result.contains("0.00000000"));
    }

    #[test]
    fn test_dashboard_text_format() {
        let text = build_dashboard_text(3, 2, 1, 0.12345678);
        assert!(text.contains("Pending"));
        assert!(text.contains("0.12345678"));
        assert!(text.contains("TRON: 3"));
        assert!(text.contains("BSC: 2"));
        assert!(text.contains("Manual reviews: 1"));
    }

    #[test]
    fn test_dashboard_text_empty() {
        let text = build_dashboard_text(0, 0, 0, 0.0);
        assert!(text.contains("Pending exchanges: 0"));
        assert!(text.contains("0.00000000"));
    }

    #[test]
    fn test_main_menu_keyboard() {
        let kb = main_menu_kb();
        let rows = kb.inline_keyboard;
        assert_eq!(rows.len(), 5);
        assert!(rows[0][0].text.contains("Dashboard"));
        assert_eq!(rows[0][0].kind, teloxide::types::InlineKeyboardButtonKind::CallbackData("menu:dashboard".into()));
        assert!(rows[4][0].text.contains("Reviews"));
    }

    #[test]
    fn test_bot_state_fields() {
        let tmp = tempfile::tempdir().unwrap();
        let db = crate::database::Database::open(tmp.path().to_str().unwrap()).unwrap();
        let cfg = Config {
            tron_rpc_url: "".into(), bsc_rpc_url: "".into(), key_path: "".into(),
            min_usdt_amount: 0.0, profit_fee_usd: 0.0, rebalance_threshold: 0.0,
            db_path: "".into(), tron_usdt_contract: "".into(), bsc_usdt_contract: "".into(),
            btc_network: "mainnet".into(), mempool_url: "".into(), node_id: "".into(),
            node_version: "".into(), tor_enabled: false, tor_proxy: "".into(),
            node_port: 0, web_host: "".into(), btc_reserve_index: 0, admin_token: "".into(),
            max_pending_per_chain: 20, tron_confirmations: 19, bsc_confirmations: 6,
            bot_token: "".into(), admin_user_id: 0,
        };
        let bot_state = BotState {
            db,
            config: cfg,
            wallet: Arc::new(Wallet::from_seed_phrase(
                "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
                Network::Bitcoin,
            ).unwrap()),
            http_client: reqwest::Client::new(),
            deposit_tx: mpsc::channel::<DepositEvent>(16).0,
        };
        let _ = bot_state.wallet.btc_address(0);
        let _ = bot_state.config.admin_user_id;
    }

    #[test]
    fn test_exchanges_page_text() {
        let exchanges = vec![
            crate::database::ExchangeRequest {
                id: "aaa".into(), chain: "tron".into(),
                deposit_address: "addr1".into(), btc_address: "btc1".into(),
                status: "pending".into(),
                usdt_amount: Some(100.0), btc_amount: Some(0.001),
                created_at: 1000, expires_at: 2000,
            },
        ];
        let (text, has_next, has_prev) = build_exchanges_page_text(&exchanges, 1, 1);
        assert!(text.contains("aaa"));
        assert!(text.contains("100.00"));
        assert!(text.contains("0.00100000"));
        assert!(!has_next);
        assert!(!has_prev);
    }

    #[test]
    fn test_exchanges_page_text_empty() {
        let (text, has_next, has_prev) = build_exchanges_page_text(&[], 1, 1);
        assert!(text.contains("No exchanges"));
        assert!(!has_next);
        assert!(!has_prev);
    }

    #[test]
    fn test_reserve_text_format() {
        let utxos = vec![
            UtxoJson {
                txid: "abc123".into(), vout: 0, value: 50_000_000,
                status: UtxoStatus { confirmed: true, block_height: Some(800000) },
            },
        ];
        let text = build_reserve_text("bc1q...", &utxos, 50_000_000, 0, 0.001);
        assert!(text.contains("bc1q"));
        assert!(text.contains("0.50000000"));
        assert!(text.contains("abc123"));
    }

    #[test]
    fn test_reserve_warning_text() {
        let text = build_reserve_text("addr", &[], 100_000_000, 0, 0.5);
        assert!(text.contains("⚠️"));
    }

    #[test]
    fn test_reserve_adequate_text() {
        let text = build_reserve_text("addr", &[], 100_000_000, 0, 0.0);
        assert!(text.contains("✅"));
    }

    #[test]
    fn test_system_text_format() {
        let text = build_system_text("1.0.0", "node1", true, true);
        assert!(text.contains("1.0.0"));
        assert!(text.contains("node1"));
        assert!(text.contains("✅"));
    }

    #[test]
    fn test_system_text_rpc_failure() {
        let text = build_system_text("1.0.0", "node1", false, true);
        assert!(text.contains("❌"));
    }

    #[test]
    fn test_reserve_many_utxos() {
        let utxos: Vec<UtxoJson> = (0..15).map(|i| UtxoJson {
            txid: format!("tx{:012}", i), vout: 0, value: 10_000_000,
            status: UtxoStatus { confirmed: true, block_height: Some(800000) },
        }).collect();
        let text = build_reserve_text("addr", &utxos, 150_000_000, 0, 0.0);
        assert!(text.contains("... and 5 more"));
        assert_eq!(text.matches("tx").count(), 10);
    }

    #[test]
    fn test_reviews_text_empty() {
        let (text, _kb) = build_reviews_view(&[]);
        assert!(text.contains("No manual reviews"));
    }

    #[test]
    fn test_reviews_text_with_items() {
        let reviews = vec![
            serde_json::json!({"tx_hash": "tx1", "chain": "tron", "got_amount": 95.0, "expected_amount": 100.0}),
        ];
        let (text, _kb) = build_reviews_view(&reviews);
        assert!(text.contains("tx1"));
        assert!(text.contains("tron"));
        assert!(text.contains("95.00"));
    }

    #[test]
    fn test_reviews_multiple_items() {
        let reviews = vec![
            serde_json::json!({"tx_hash": "tx1", "chain": "tron", "got_amount": 95.0, "expected_amount": 100.0}),
            serde_json::json!({"tx_hash": "tx2", "chain": "bsc", "got_amount": 200.0, "expected_amount": 200.0}),
        ];
        let (text, _kb) = build_reviews_view(&reviews);
        assert!(text.contains("Manual Reviews: 2"));
        assert!(text.contains("200.00"));
    }
}
