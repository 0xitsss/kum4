# Kumquad (kum4)

**Non-custodial USDT→BTC processing engine for the darknet.**

Lightweight, high-performance, fully open-source. Runs as a Tor hidden service.
USDT (TRC-20 / BEP-20) → swap → BTC to merchant wallet.
No KYC, no third parties, no hidden fees.

---

## Architecture

```
User ──Tor──> kum4 node (.onion)
                │
                ├── USDT deposit (TRC-20 / BEP-20)
                ├── Swap (DEX integration)
                ├── BTC payout (P2WPKH)
                └── Status via DHT (mesh discovery)
```

### Current
- **HD Wallet** — BIP32 seed → BTC (P2WPKH), ETH, Tron addresses
- **Monitor** — scans Tron/BSC blocks for incoming USDT
- **Exchange** — CEX (Binance), DEX scaffold
- **Rebalance** — auto-convert USDT → BTC when threshold hit
- **Web UI** — one-pager (axum + Tera), exchange rate, deposit addresses
- **DB** — sled (embedded, atomic transactions)
- **Tor** — all outbound traffic via Tor proxy

### Roadmap

| # | What | Why |
|---|------|-----|
| 1 | Tor hidden service (`.onion`) | Darknet native |
| 2 | DHT node discovery (Kademlia) | No central aggregator |
| 3 | Peer-to-peer swap relay | Nodes help each other find liquidity |
| 4 | Decentralized reputation | Proof of successful swaps |
| 5 | Mesh network | Full darknet availability |

---

## Quick Start

### Prerequisites
- Rust 1.75+
- Tor (for outbound connections)

### 1. Configure

```bash
cp .env.example .env
# Edit .env — see "Configuration" section below
```

### 2. Generate a wallet

```bash
# Seed phrase (12 words, BIP39)
cargo run --bin kum4 -- generate-seed
```

### 3. Run

```bash
cargo run --release
```

Opens HTTP server on `127.0.0.1:8080` (bind Tor to this).

---

## Configuration

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `SEED_PHRASE` | **yes** | — | 12/24-word BIP39 mnemonic |
| `BTC_PRIVATE_KEY_WIF` | **yes** | — | BTC private key (WIF) for merchant wallet |
| `MERCHANT_BTC_ADDRESS` | **yes** | — | Final BTC payout address |
| `TRON_RPC_URL` | no | `https://api.trongrid.io` | Tron full node (via Tor) |
| `BSC_RPC_URL` | no | `https://bsc-dataseed.binance.org` | BSC RPC (via Tor) |
| `BTC_NETWORK` | no | `mainnet` | `mainnet`, `testnet`, `signet`, `regtest` |
| `MEMPOOL_URL` | no | `https://mempool.space` | Mempool API (via Tor) |
| `MIN_USDT_AMOUNT` | no | `10.0` | Minimum swap in USDT |
| `PROFIT_FEE_USD` | no | `1.0` | Your fee in USD |
| `REBALANCE_THRESHOLD` | no | `500.0` | USDT balance before auto-swap |
| `REBALANCE_INTERVAL_SECS` | no | `3600` | Rebalance check interval |
| `DB_PATH` | no | `kum4_data` | Sled database path |
| `TRON_USDT_CONTRACT` | no | `TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t` | USDT contract on Tron |
| `BSC_USDT_CONTRACT` | no | `0x55d398326f99059ff775485246999027b3197955` | USDT contract on BSC |
| `WEB_HOST` | no | `127.0.0.1` | HTTP bind address |
| `WEB_PORT` | no | `8080` | HTTP bind port |
| `NODE_ID` | no | `kum4-default` | Identity in mesh network |
| `NODE_PORT` | no | `8080` | P2P port for DHT |

---

## Mesh Network (DHT)

Each kum4 node announces itself via Kademlia DHT:

```rust
// Every node:
NodeInfo {
    id: NodeId,        // derived from seed phrase
    addr: SocketAddr,  // .onion:port
    fee: f64,          // PROFIT_FEE_USD
    version: String,   // semver
    chain: Vec<String>,// ["TRC20", "BEP20"]
    status: Status,    // Online | Busy | Offline
}
```

Discovery is fully distributed — no central registry.
Clients query the DHT for active nodes, choose one by fee/uptime.

---

## Security

- **Private keys never leave RAM** — not logged, not serialized
- **Seed phrase only in `.env`** — file permissions `600`
- **All RPC via Tor** — no IP leaks
- **Input validation** — RPC responses, user input, CEX API
- **Rate limiting** — outbound requests bounded
- **No shell commands** — pure Rust

---

## Development

```bash
cargo test          # unit tests only (deterministic)
cargo clippy        # lint
cargo build --release
```

## License

MIT — do what you want, no warranty.
