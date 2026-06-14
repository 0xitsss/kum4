<p align="center">
  <svg width="56" height="56" viewBox="0 0 28 28" fill="none" xmlns="http://www.w3.org/2000/svg">
    <rect x="2" y="2" width="24" height="24" rx="8" stroke="#f7931a" stroke-width="2.5"/>
    <path d="M14 8C10.686 8 8 9.79 8 12.5c0 2.71 2.686 3.5 6 3.5s6 .79 6 3.5c0 2.71-2.686 4.5-6 4.5" stroke="#f7931a" stroke-width="2" stroke-linecap="round"/>
    <path d="M14 6v2M14 20v2" stroke="#f7931a" stroke-width="2" stroke-linecap="round"/>
  </svg>
</p>

# Kumquad (kum4) &nbsp;

<p align="center">
  <a href=""><img src="https://img.shields.io/badge/rust-1.75%2B-blue?style=for-the-badge" alt="Rust"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-MIT-yellow?style=for-the-badge" alt="MIT"></a>
  <a href=""><img src="https://img.shields.io/badge/Tor-supported-7D4698?style=for-the-badge" alt="Tor"></a>
</p>

<p align="center"><b>Non-custodial USDT → BTC processing engine.</b><br>
Accept USDT (TRC-20 / BEP-20), send BTC from own reserve.<br>
No CEX, no DEX, no KYC, no third parties.</p>

---

## What is Kumquad?

Kumquad is a lightweight, high-performance crypto processing engine for
self-hosted payment processing. It runs as a single binary with an embedded
web UI and database.

**The problem it solves:** merchants who want to accept USDT but pay suppliers
in BTC currently need exchange accounts, KYC, or third-party processors that
take a cut and hold funds. Kumquad lets you run your own processing node —
users send USDT, you send BTC from your reserve. The USDT stays yours as profit.

**Key principles:**
- **Non-custodial** — private keys never leave RAM, seed encrypted at rest
- **No external dependencies** — no CEX/DEX API, no database server, no build step
- **Per-exchange unique addresses** — each swap gets its own HD-derived deposit address (BIP32)
- **Self-contained** — embedded DB (sled), embedded web server (axum), single binary

---

### Flow

1. **Operator funds BTC reserve** — set your BTC balance via API/UI
2. **User creates exchange** — enters BTC address, amount, chain (TRC-20/BEP-20)
3. **System generates unique deposit address** — HD-derived BIP32 address, never reused
4. **User sends USDT** — to the generated deposit address
5. **Monitor detects deposit** — queries Trongrid/BSC account API for each pending exchange,
   verifies exact amount match (±0.01 USDT), marks tx processed
6. **Operator manually sends BTC** — from reserve to user's BTC address
7. **Exchange marked completed**

If local BTC reserve is insufficient, the node queries peers via P2P and
can redirect the deposit to a peer with enough reserve.

---

## Quick Start

### Prerequisites
- Rust 1.75+

### 1. Configure

```bash
cp .env.example .env
# Edit .env — see "Configuration" below
```

### 2. Run

```bash
cargo run --release
```

First launch generates a new encrypted wallet (`key.kum4`) and asks for a password.
**Save the seed phrase** — it's the only way to recover funds.

Web UI: `http://127.0.0.1:8080`

---

## Configuration

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `KEY_PATH` | no | `key.kum4` | Path to encrypted seed file |
| `TRON_RPC_URL` | no | `https://api.trongrid.io` | Tron full node |
| `BSC_RPC_URL` | no | `https://bsc-dataseed.binance.org` | BSC RPC |
| `BTC_NETWORK` | no | `mainnet` | `mainnet`, `testnet`, `signet`, `regtest` |
| `MEMPOOL_URL` | no | `https://mempool.space` | Mempool API for fee estimates |
| `MIN_USDT_AMOUNT` | no | `0.0` | Minimum deposit in USDT |
| `PROFIT_FEE_USD` | no | `1.0` | Your fee in USD |
| `REBALANCE_THRESHOLD` | no | `500.0` | USDT accumulated before alert |
| `DB_PATH` | no | `kum4_data` | Sled database path |
| `TRON_USDT_CONTRACT` | no | `TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t` | USDT contract on Tron |
| `BSC_USDT_CONTRACT` | no | `0x55d398326f99059ff775485246999027b3197955` | USDT contract on BSC |
| `TOR_ENABLED` | no | `false` | Route via Tor + enable DHT mesh |
| `TOR_PROXY` | no | `socks5://127.0.0.1:9050` | Tor SOCKS5 proxy |
| `WEB_HOST` | no | `127.0.0.1` | HTTP bind address |
| `NODE_PORT` | no | `8080` | HTTP server port |
| `NODE_ID` | no | `kum4-default` | Identity in mesh network |
| `NODE_VERSION` | no | `0.0.3` | Version reported in health API |

---

## API

| Method | Path | Description |
|--------|------|-------------|
| GET | `/` | Web UI |
| GET | `/api/rate` | BTC/USD price + fee rate |
| POST | `/api/calculate` | USDT↔BTC conversion with fees |
| GET | `/api/health` | Node status, peer ID, BTC reserve, uptime |
| POST | `/api/exchange` | Create new exchange (get unique deposit address) |
| GET | `/api/exchange/:id` | Get exchange status |
| POST | `/api/reserve` | Set BTC reserve balance |
| GET | `/exchange/:id` | Exchange detail page (with auto-refresh) |

---

## Mesh Network (DHT)

In Tor mode (`TOR_ENABLED=true`), nodes discover each other via Kademlia DHT:

```rust
NodeInfo {
    peer_id: String,     // derived from seed phrase
    http_addr: String,   // node's HTTP API address
    fee_usd: f64,        // PROFIT_FEE_USD
    chains: Vec<String>, // ["TRC20", "BEP20"]
    btc_reserve: f64,    // available BTC reserve
    status: String,      // "online" | "busy" | "offline"
}
```

Peers discovered via DHT are queried for BTC reserve when the local node
cannot process a deposit. If a peer has enough reserve, the deposit can be
redirected. Discovery is fully distributed — no central registry.

Without Tor (`TOR_ENABLED=false`), the node runs standalone without DHT.

---

## Security

- **Encrypted seed** — Twofish-256 + Whirlpool, stored in `key.kum4`, unlocked with password at startup
- **Private keys never leave RAM** — not logged, not serialized
- **Per-exchange unique addresses** — BIP32 derivation, no address reuse
- **All RPC via Tor** (optional) — no IP leaks
- **Amount verification** — deposits matched exact (±0.01 USDT), dust rejected
- **Input validation** — all external inputs validated (RPC responses, user input)
- **No shell commands** — pure Rust, no system dependencies at runtime

---

## Development

```bash
cargo test          # unit tests (deterministic, no network)
cargo clippy        # lint
cargo build --release
```

### Testnet

For development on test networks:

```env
TRON_RPC_URL=https://nile.trongrid.io/jsonrpc/
BSC_RPC_URL=https://bsc-testnet-dataseed.bnbchain.org/
BTC_NETWORK=testnet
MEMPOOL_URL=https://mempool.space/testnet
```

---

## License

MIT — do what you want, no warranty.
