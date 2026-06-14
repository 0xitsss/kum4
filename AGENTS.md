# Kumquad (kum4) — AGENTS.md

## Project Overview
Kumquad (kum4) — lightweight, high-performance, non-custodial open-source crypto
processing engine in Rust. Accepts USDT (TRC-20 / BEP-20), converts via CEX + DEX,
sends BTC to merchant wallet. No KYC, no third parties, no hidden fees.

## Core Principles

### 1. YAGNI — You Aren't Gonna Need It
- No code "just in case". Every function, struct, module serves an immediate need.
- If it isn't in the current milestone, it doesn't go in.
- Simple solution for today > architecturally pure solution for a future that may never come.

### 2. KISS — Keep It Simple, Stupid
- Simplest working solution wins.
- Flat modules over deep nesting. If a function can't be read in 30s, split it.
- No unnecessary abstractions (extra traits, generics, macros) without concrete duplication.

### 3. TDD — Test-Driven Development
- Red → Green → Refactor. Always.
- Test first, see it fail, implement minimum code, refactor.
- Unit tests for ALL business logic (fee math, UTXO selection, address derivation).
- Deterministic tests — no network calls in unit tests (mock RPC / CEX).
- `cargo test` before every commit.
- No commit that breaks the build or fails tests.

### 4. SOLID
- **SRP:** One module, one concern.
- **OCP:** Extend via config, not modification.
- **DIP:** High-level modules don't depend on low-level implementations (trait objects / generics).
- **LSP:** Replaceable implementations without surprising behavior.
- **ISP:** Small focused traits over large monoliths.

### 5. Error Handling
- Unified `Kum4Error` with `thiserror`.
- All fallible → `Result<T, Kum4Error>`.
- Zero `.unwrap()` / `.expect()` in production. Tests only.
- Panic only at startup for missing critical config.

### 6. Security
- Private keys never logged, never printed, never serialized.
- Seed / mnemonic only in env.
- Validate all external inputs (RPC, CEX API responses).
- Rate-limit outbound requests.

## Development Workflow

1. **Spec** — `docs/superpowers/specs/YYYY-MM-DD-topic-design.md`
2. **Plan** — Implementation plan (writing-plans skill)
3. **Test** — Write tests first
4. **Implement** — Minimum to pass
5. **Refactor** — Clean, retest
6. **Verify** — `cargo clippy && cargo test`
7. **Commit** — conventional commit

## Commit Convention
```
<type>(<scope>): <description>

types: feat, fix, refactor, test, docs, chore, perf
scopes: core, monitor, exchange, btc, wallet, db, config
```

## Performance
- Async via tokio, no sync I/O in async context.
- Sled transactions for atomic DB ops.
- Log every financial op (tracing with structured fields).
- No external shell commands — pure Rust.
