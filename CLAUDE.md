# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**WUP** is a high-frequency trading (HFT) platform for prediction market aggregation. The system prioritizes **ultra-low latency** and **high throughput** above all else.

### Core Principles

1. **Latency is King**: Every microsecond counts. Avoid allocations in hot paths, prefer stack over heap, use lock-free data structures.

2. **Zero-Copy Where Possible**: Minimize data copying. Use references, slices, and `Cow` types. Avoid unnecessary serialization/deserialization.

3. **Lock-Free Concurrency**: Use `DashMap`, atomics, and channels instead of `Mutex`/`RwLock`. Never block the hot path.

4. **Fail Fast, Recover Faster**: Use circuit breakers, graceful degradation, and automatic reconnection.

5. **Measure Everything**: Prometheus metrics on all critical paths. Profile before optimizing.

## Build and Test Commands

```bash
# Build optimized release (ALWAYS use for benchmarks/production)
cargo build --release

# Run all tests
cargo test

# Run tests for a specific crate
cargo test -p orderbook_service
cargo test -p normalizer

# Run a single test
cargo test -p orderbook_service test_name

# Run with logs
RUST_LOG=debug cargo test -p normalizer -- --nocapture

# Lint (fix all warnings before committing)
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

## Running Services

```bash
# Using docker-compose (recommended)
docker compose up -d

# Or start infrastructure manually
docker run -d --name nats -p 4222:4222 -p 8222:8222 nats:latest -js
docker run -d --name redis -p 6379:6379 redis:latest

# Run services (always use --release for performance)
cargo run --release --bin aggregator
PUBLISH_CHANGES=true cargo run --release --bin orderbook_service
cargo run --release --bin event_service
cargo run --release --bin gateway
```

## Architecture

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                     HFT Prediction Market Aggregator                        │
│                    Target: <1ms end-to-end latency                          │
└─────────────────────────────────────────────────────────────────────────────┘

┌──────────────┐  ┌──────────────┐  ┌──────────────┐
│  Polymarket  │  │    Kalshi    │  │   Future     │
│   WebSocket  │  │   WebSocket  │  │  Platforms   │
└──────┬───────┘  └──────┬───────┘  └──────┬───────┘
       │                 │                 │
       ▼                 ▼                 ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                         Aggregator Service                                  │
│   • Persistent WebSocket connections with auto-reconnect                    │
│   • Zero-copy message forwarding to NATS                                    │
│   • Supervisor pattern for connection management                            │
└───────────────────────────────────┬─────────────────────────────────────────┘
                                    │ NATS (push-based, <100μs)
                                    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                         Normalizer Service                                  │
│   • Exchange-specific adapters (trait-based)                                │
│   • Schema normalization with minimal allocations                           │
│   • Inline with aggregator or standalone                                    │
└───────────────────────────────────┬─────────────────────────────────────────┘
                                    │ NATS
                                    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                      Orderbook Service (Hot Path)                           │
│   • Lock-free DashMap storage                                               │
│   • BTreeMap for O(log n) price level operations                            │
│   • Platform-aware aggregation (PlatformSizes)                              │
│   • In-memory only - no disk I/O on hot path                                │
│   • Publishes aggregated price changes to NATS                              │
└───────────────────────────────────┬─────────────────────────────────────────┘
                                    │ NATS (price changes) / HTTP
                                    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                           Gateway Service                                   │
│   • WebSocket server for real-time streaming (port 8082)                    │
│   • Per-client subscription management with wildcards                       │
│   • Snapshot on subscribe + delta streaming                                 │
│   • Pre-serialized JSON fan-out to all matching clients                     │
└───────────────────────────────────┬─────────────────────────────────────────┘
                                    │ WebSocket / HTTP
                                    ▼
                              Trading Systems
```

### Data Flow

| Stage | Service          | Latency Target | Key Optimizations                     |
| ----- | ---------------- | -------------- | ------------------------------------- |
| 1     | Aggregator       | <10ms          | Persistent WS, zero-copy publish      |
| 2     | Normalizer       | <100μs         | Pre-allocated buffers, trait dispatch |
| 3     | OrderbookService | <50μs          | Lock-free DashMap, BTreeMap           |
| 4     | Gateway          | <200μs         | Pre-serialized JSON, parallel fan-out |
| 5     | HTTP API         | <1ms           | Connection pooling, JSON streaming    |

### Critical Design Decisions

**Lock-Free Storage**: `OrderbookStore` uses `DashMap` (sharded concurrent hashmap) instead of `RwLock<HashMap>`. This eliminates lock contention under high load.

**BTreeMap for Orderbooks**: Price levels stored in `BTreeMap<Decimal, PlatformSizes>` for O(log n) insertion and automatic sorting. Best bid/ask retrieval is O(1) via iterators.

**Platform-Aware Aggregation**: Each price level tracks sizes per platform (`PlatformSizes`). Snapshots only replace the originating platform's data, preserving other platforms.

**NATS Core over JetStream**: Hot path uses NATS Core (push-based, no persistence) for lowest latency. JetStream only for replay/recovery.

## Key Patterns

### Adding a New Exchange

1. Create adapter in `crates/normalizer/src/{exchange}/adapter.rs`
2. Implement `ExchangeAdapter` trait:

```rust
pub trait ExchangeAdapter: Send + Sync {
    fn exchange_name(&self) -> &'static str;
    fn parse_message(&self, raw: &[u8]) -> Result<Option<NormalizedOrderbook>>;
}
```

3. Add WebSocket handler in `crates/{exchange}/` or reuse `common::WsHandler`

### Multi-Platform Aggregation

Different platforms use different slugs for the same event. Redis mappings unify them:

```
polymarket:event:us-election  →  AGG123
kalshi:event:2024-president   →  AGG123
```

Storage hierarchy: `aggregate_id` → `market_id` → `asset_id` → `Orderbook`

## NATS Subject Conventions

| Pattern                                                    | Purpose                 | Example                                  |
| ---------------------------------------------------------- | ----------------------- | ---------------------------------------- |
| `market.{platform}.{asset_id}`                             | Raw exchange messages   | `market.polymarket.12345`                |
| `normalized.{platform}.{market_id}.{asset_id}`             | Normalized orderbooks   | `normalized.polymarket.abc.12345`        |
| `orderbook.changes.{agg_id}.{hashed_market_id}.{token_id}` | Aggregated price deltas | `orderbook.changes.abc123.def456.token1` |

## Redis Key Conventions

| Pattern                   | Purpose                           |
| ------------------------- | --------------------------------- |
| `event:{platform}:{slug}` | Full event data (markets, tokens) |
| `{platform}:event:{slug}` | Maps to aggregate_id              |
| `aggregate:{id}`          | Aggregate metadata                |

## Crate Dependencies

```
common            ← polymarket, aggregator (WsHandler, WsManager)
nats_client       ← aggregator, normalizer, orderbook_service, gateway
normalizer        ← aggregator (inline), orderbook_service, gateway
external_services ← event_service, aggregator, orderbook_service (SharedRedisClient)
gateway           ← depends on normalizer (for schema types), nats_client
```

## Environment Variables

| Variable                | Service                                      | Default                   | Description                            |
| ----------------------- | -------------------------------------------- | ------------------------- | -------------------------------------- |
| `NATS_URL`              | all                                          | `nats://localhost:4222`   | NATS server                            |
| `REDIS_URL`             | aggregator, event_service, orderbook_service | `redis://localhost:6379`  | Redis server                           |
| `METRICS_PORT`          | all                                          | 9090/9091/9092/9093       | Prometheus metrics                     |
| `HTTP_PORT`             | orderbook_service, event_service             | 8080/8081                 | HTTP API                               |
| `REFRESH_INTERVAL_SECS` | event_service                                | `60`                      | Event refresh interval                 |
| `EVENT_SLUG`            | aggregator                                   | -                         | Event to subscribe                     |
| `NATS_SUBJECT`          | orderbook_service                            | `normalized.polymarket.>` | NATS subscription                      |
| `PUBLISH_CHANGES`       | orderbook_service                            | `false`                   | Publish changes to NATS for gateway    |
| `WS_PORT`               | gateway                                      | `8082`                    | WebSocket server port                  |
| `ORDERBOOK_SERVICE_URL` | gateway                                      | `http://localhost:8080`   | Orderbook HTTP API URL (for snapshots) |
| `RUST_LOG`              | all                                          | `info`                    | Log level                              |

## Performance Guidelines

### DO

- Use `#[inline]` for small, hot functions
- Prefer `&str` over `String` in function signatures
- Use `Cow<'_, str>` when ownership is conditional
- Pre-allocate vectors with `Vec::with_capacity()`
- Use `Decimal` for price arithmetic (no floating point errors)
- Profile with `cargo flamegraph` before optimizing

### DON'T

- Allocate in hot loops
- Use `Mutex` or `RwLock` on the hot path
- Clone large structures unnecessarily
- Log at DEBUG level in production hot paths
- Use `unwrap()` - prefer `?` or explicit error handling
- Block async tasks with synchronous I/O
