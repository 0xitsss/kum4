# Kumquad (kum4)

[![Rust](https://img.shields.io/badge/rust-1.75%2B-blue?style=for-the-badge)]()
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow?style=for-the-badge)](LICENSE)
[![Tor](https://img.shields.io/badge/Tor-supported-7D4698?style=for-the-badge)]()

**Non-custodial USDT→BTC processing engine with reserve model.**

Lightweight, high-performance, fully open-source. Accepts USDT (TRC-20 / BEP-20),
sends BTC from the node's own reserve. No CEX, no DEX, no KYC.

---

## Architecture

```
User ──> kum4 node
            │
            ├── USDT deposit (TRC-20 / BEP-20) ──> accumulated as profit
            ├── BTC payout from node reserve
            ├── P2P redirect to peer if reserve low
            └── Mesh discovery via DHT (Tor) or standalone (clearnet)
```

### Current
- **HD Wallet** — BIP32 seed → BTC (P2WPKH), ETH, Tron addresses
- **Monitor** — scans Tron/BSC blocks for incoming USDT
- **Reserve model** — operator funds BTC reserve, USDT stays as profit
- **P2P Redirect** — deposits forwarded to peers with sufficient BTC reserve
- **Web UI** — one-pager (axum), calculator, deposit addresses
- **DB** — sled (embedded, atomic transactions)
- **Tor / Clearnet** — DHT mesh (Tor) or standalone (clearnet)

### Roadmap

| # | What | Why |
|---|------|-----|
| 1 | BTC reserve API | Operator sets reserve balance |
| 2 | P2P redirect | Auto-forward deposits when reserve low |
| 3 | Mesh peer list in UI | Choose node with best reserve/fee |
| 4 | Decentralized reputation | Proof of successful swaps |
| 5 | Onion service | Native Tor hidden service |

---

## Quick Start

### Prerequisites
- Rust 1.75+

### 1. Configure

```bash
cp .env.example .env
# Edit .env — see "Configuration" section below
```

### 2. Run

```bash
cargo run --release
```

Web UI opens on `http://127.0.0.1:8080`.

---

## Configuration

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `SEED_PHRASE` | **yes** | — | 12/24-word BIP39 mnemonic |
| `TRON_RPC_URL` | no | `https://api.trongrid.io` | Tron full node |
| `BSC_RPC_URL` | no | `https://bsc-dataseed.binance.org` | BSC RPC |
| `BTC_NETWORK` | no | `mainnet` | `mainnet`, `testnet`, `signet`, `regtest` |
| `MEMPOOL_URL` | no | `https://mempool.space` | Mempool API for fee estimates |
| `MIN_USDT_AMOUNT` | no | `10.0` | Minimum deposit in USDT |
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

---

## Mesh Network (DHT)

In Tor mode, each kum4 node announces itself via Kademlia DHT:

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

Discovery is fully distributed — no central registry.
Peers discovered via DHT are queried for BTC reserve when the local node
cannot process a deposit.

---

## API

| Method | Path | Description |
|--------|------|-------------|
| GET | `/` | Web UI |
| GET | `/api/rate` | BTC/USD price, fee rate |
| GET | `/api/addresses` | Deposit addresses (TRC-20, BEP-20, BTC) |
| POST | `/api/calculate` | USDT↔BTC conversion calculator |
| GET | `/api/health` | Node status, peer ID, BTC reserve |
| POST | `/api/reserve` | Set BTC reserve balance |
| GET | `/api/p2p/reserve` | P2P: query this node's reserve |
| POST | `/api/p2p/redirect` | P2P: accept incoming deposit redirect |

---

## Security

- **Private keys never leave RAM** — not logged, not serialized
- **Seed phrase only in `.env`** — file permissions `600`
- **All RPC via Tor** (optional) — no IP leaks
- **Input validation** — RPC responses, user input
- **Rate limiting** — outbound requests bounded
- **No shell commands** — pure Rust

---

## Development

```bash
cargo test          # unit tests (deterministic, no network)
cargo clippy        # lint
cargo build --release
```

## License

MIT — do what you want, no warranty.
