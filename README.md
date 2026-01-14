# Low Latency Betting Aggregator

A high-performance, low-latency aggregator for betting market data. Connects to multiple exchanges via WebSocket and publishes raw events to NATS JetStream.

## Features

- **Multi-Platform Support**: Adapter pattern for easy integration of new betting platforms (Polymarket, Kalshi, 4Casters)
- **Dynamic Subscriptions**: Add/remove market subscriptions at runtime
- **Connection Supervisor**: Manages WebSocket worker pools with automatic failover
- **Prometheus Metrics**: Built-in observability for production monitoring
- **Low Latency**: Direct push architecture minimizes processing overhead

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                        Aggregator Service                        │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│   ┌──────────────┐     ┌──────────────┐     ┌──────────────┐   │
│   │  Supervisor  │────▶│  WS Worker 1 │────▶│    NATS      │   │
│   │              │     └──────────────┘     │  JetStream   │   │
│   │              │     ┌──────────────┐     │              │   │
│   │              │────▶│  WS Worker 2 │────▶│              │   │
│   └──────────────┘     └──────────────┘     └──────────────┘   │
│                                                                  │
│   ┌──────────────┐                                              │
│   │  Prometheus  │  :9090/metrics                               │
│   │   Exporter   │                                              │
│   └──────────────┘                                              │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

## Prerequisites

- Rust 1.75+ (2021 edition)
- NATS Server with JetStream enabled
- Docker (optional, for running NATS)

## Quick Start

### 1. Start NATS JetStream

```bash
docker run -d --name nats -p 4222:4222 -p 8222:8222 nats:latest -js
```

### 2. Build the Project

```bash
cargo build --release
```

### 3. Run the Aggregator

```bash
# Set environment variables
export NATS_URL="nats://localhost:4222"
export METRICS_PORT="9090"
export TOKEN_IDS="token1,token2,token3"
export RUST_LOG="info"

# Run
cargo run --release --bin aggregator
```

## Configuration

| Environment Variable | Description                                    | Default                 |
| -------------------- | ---------------------------------------------- | ----------------------- |
| `NATS_URL`           | NATS server URL                                | `nats://localhost:4222` |
| `METRICS_PORT`       | Prometheus metrics port                        | `9090`                  |
| `TOKEN_IDS`          | Comma-separated list of asset IDs to subscribe | (empty)                 |
| `RUST_LOG`           | Log level                                      | `info`                  |

## Project Structure

```
crates/
├── common/         # Shared traits, WsHandler, WsManager
├── nats_client/    # NATS JetStream wrapper
├── polymarket/     # Polymarket adapter implementation
└── aggregator/     # Main binary, Supervisor
```

## Metrics

The aggregator exposes Prometheus metrics at `http://localhost:9090/metrics`:

| Metric                                | Type    | Description                            |
| ------------------------------------- | ------- | -------------------------------------- |
| `aggregator_active_connections`       | Gauge   | Number of active WebSocket connections |
| `aggregator_messages_received_total`  | Counter | Total raw messages received            |
| `aggregator_messages_published_total` | Counter | Total messages published to NATS       |
| `aggregator_errors_total`             | Counter | Total errors by type                   |
| `aggregator_active_subscriptions`     | Gauge   | Total markets being tracked            |
| `aggregator_worker_count`             | Gauge   | Number of active workers               |

## Adding a New Platform

1. Create a new crate in `crates/` (e.g., `crates/kalshi/`)
2. Implement the `WsHandler` trait from `common`
3. Register the adapter in the Supervisor
4. Update the main binary to support the new platform

## License

MIT
