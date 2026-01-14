# WUP System Specification

## Overview

**WUP** is a **High-Frequency Trading (HFT) platform** for prediction market aggregation. The system aggregates real-time orderbook data from multiple prediction market exchanges (Polymarket, Kalshi, etc.) into a unified, ultra-low-latency feed.

### Design Philosophy

> **"Every microsecond is a competitive advantage."**

This platform is built for professional traders and market makers who require:

- Sub-millisecond orderbook updates
- Multi-platform price aggregation
- Reliable, fault-tolerant operation
- Horizontal scalability

### Performance Targets

| Metric             | Target                 | Rationale                     |
| ------------------ | ---------------------- | ----------------------------- |
| End-to-end latency | <1ms p99               | Competitive edge in HFT       |
| Message throughput | >100K msgs/sec         | Handle peak market volatility |
| Availability       | 99.99% uptime          | Trading systems depend on us  |
| Recovery time      | <5 seconds             | Minimize missed opportunities |
| Memory overhead    | <100 bytes/price level | Dense orderbook storage       |

---

## Architecture

### System Overview

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                     WUP HFT Platform - Target: <1ms Latency                 │
└─────────────────────────────────────────────────────────────────────────────┘

                    ┌─────────────────────────────────────┐
                    │         Trading Systems             │
                    │  (Market Makers, Arbitrage Bots)    │
                    └──────────────────┬──────────────────┘
                                       │ HTTP REST / WebSocket
                                       ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                           Gateway Service                                   │
│  ┌─────────────────────────────────────────────────────────────────────┐    │
│  │  WebSocket server for real-time streaming                          │    │
│  │  Subscription management per client                                │    │
│  │  Snapshot on subscribe + delta streaming                           │    │
│  └─────────────────────────────────────────────────────────────────────┘    │
│                                       ▲                                     │
│                   NATS (price changes)│                                     │
└───────────────────────────────────────┼─────────────────────────────────────┘
                                        │
┌───────────────────────────────────────┼─────────────────────────────────────┐
│                          Orderbook Service                                  │
│  ┌─────────────────────────────────────────────────────────────────────┐    │
│  │  Lock-free DashMap Storage (no mutex contention)                    │    │
│  │  BTreeMap<Decimal, PlatformSizes> for O(log n) price operations     │    │
│  │  Platform-aware aggregation: polymarket + kalshi at same price      │    │
│  └─────────────────────────────────────────────────────────────────────┘    │
│                                       ▲                                     │
│                                       │ NATS Core (<100μs)                  │
└───────────────────────────────────────┼─────────────────────────────────────┘
                                        │
┌───────────────────────────────────────┼─────────────────────────────────────┐
│                          Normalizer Service                                 │
│  ┌─────────────────────────────────────────────────────────────────────┐    │
│  │  ExchangeAdapter trait for plugin architecture                      │    │
│  │  Zero-allocation message parsing where possible                     │    │
│  │  Unified NormalizedOrderbook schema                                 │    │
│  └─────────────────────────────────────────────────────────────────────┘    │
│                                       ▲                                     │
│                                       │ NATS Core                           │
└───────────────────────────────────────┼─────────────────────────────────────┘
                                        │
┌───────────────────────────────────────┼─────────────────────────────────────┐
│                          Aggregator Service                                 │
│  ┌─────────────────────────────────────────────────────────────────────┐    │
│  │  Supervisor pattern for connection management                       │    │
│  │  Persistent WebSocket with auto-reconnect + exponential backoff     │    │
│  │  Zero-copy publish to NATS                                          │    │
│  └─────────────────────────────────────────────────────────────────────┘    │
│                                       ▲                                     │
│               WebSocket (persistent)  │                                     │
└───────────────────────────────────────┼─────────────────────────────────────┘
                                        │
        ┌───────────────────────────────┼───────────────────────────────┐
        │                               │                               │
┌───────┴───────┐              ┌────────┴────────┐             ┌────────┴────────┐
│  Polymarket   │              │     Kalshi      │             │    Future       │
│   Exchange    │              │    Exchange     │             │   Exchanges     │
└───────────────┘              └─────────────────┘             └─────────────────┘


┌─────────────────────────────────────────────────────────────────────────────┐
│                          Supporting Services                                │
│                                                                             │
│  ┌─────────────────────┐    ┌─────────────────────┐    ┌─────────────────┐  │
│  │   Event Service     │    │       Redis         │    │      NATS       │  │
│  │  - Metadata cache   │◀──▶│  - Event data       │    │  - Message bus  │  │
│  │  - Token mappings   │    │  - Aggregate maps   │    │  - JetStream    │  │
│  │  - Background sync  │    │  - Token mappings   │    │  - Clustering   │  │
│  └─────────────────────┘    └─────────────────────┘    └─────────────────┘  │
└─────────────────────────────────────────────────────────────────────────────┘
```

### Latency Budget

| Stage     | Component              | Target       | Optimization                                  |
| --------- | ---------------------- | ------------ | --------------------------------------------- |
| 1         | WebSocket receive      | <5ms         | Persistent connection, kernel bypass (future) |
| 2         | Aggregator → NATS      | <100μs       | Zero-copy publish, batch when possible        |
| 3         | Normalizer parse       | <50μs        | Pre-compiled patterns, avoid allocations      |
| 4         | Normalizer → NATS      | <100μs       | NATS Core (no persistence overhead)           |
| 5         | OrderbookService apply | <50μs        | Lock-free DashMap, BTreeMap                   |
| 6         | Publish change to NATS | <100μs       | NATS Core fire-and-forget                     |
| 7         | Gateway fan-out        | <200μs       | Pre-serialized JSON, parallel sends           |
| 8         | HTTP API response      | <500μs       | Connection pooling, JSON streaming            |
| **Total** | End-to-end             | **<1ms p99** |                                               |

---

## Data Model

### Multi-Platform Orderbook

The core innovation is **platform-aware price aggregation**:

```rust
/// Each price level tracks sizes from multiple platforms
struct PlatformSizes {
    sizes: HashMap<String, Decimal>,  // platform -> size
}

/// Orderbook with multi-platform support
struct Orderbook {
    asset_id: String,
    market_id: String,
    aggregate_id: Option<String>,

    // Market metadata (from Redis)
    market_question: Option<String>,
    market_slug: Option<String>,

    // Price -> PlatformSizes (multiple platforms at same price)
    bids: BTreeMap<Decimal, PlatformSizes>,
    asks: BTreeMap<Decimal, PlatformSizes>,

    // Per-platform BBO (exchange-reported)
    exchange_best_bid: HashMap<String, String>,
    exchange_best_ask: HashMap<String, String>,

    // System-calculated BBO (from our aggregated book)
    system_best_bid: Option<Decimal>,
    system_best_ask: Option<Decimal>,
}
```

**Why BTreeMap?**

- O(log n) insert/update
- O(1) best bid (last) / best ask (first) via iterators
- Automatic price ordering
- No rehashing overhead

**Why PlatformSizes?**

- Snapshot from Polymarket only replaces Polymarket's data
- Kalshi data at same price remains intact
- Total size = sum of all platforms
- Easy arbitrage detection (price differences across platforms)

### Normalized Schema

All exchanges normalize to a common schema:

```rust
struct NormalizedOrderbook {
    exchange: String,           // Source exchange name
    platform: String,           // Platform identifier (for routing)
    asset_id: String,           // Token/contract ID
    market_id: String,          // Market/event ID
    message_type: MessageType,  // Snapshot | Delta

    // Snapshot data
    bids: Option<Vec<PriceLevel>>,
    asks: Option<Vec<PriceLevel>>,

    // Delta data
    updates: Option<Vec<OrderbookUpdate>>,

    // Exchange-reported BBO
    best_bid: Option<String>,
    best_ask: Option<String>,

    // Timestamps for latency tracking
    exchange_timestamp: String, // Exchange's timestamp
    received_at: i64,           // Our receive time (epoch ms)
    normalized_at: String,      // Normalization time (ISO 8601)
}

struct PriceLevel {
    price: String,    // Decimal as string (precision preserved)
    size: String,
    platform: String, // Source platform
}
```

### Aggregate ID Mapping

Different platforms use different identifiers for the same event:

```
┌─────────────────────────────────────────────────────────────────┐
│                    Aggregate ID Mapping                         │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  Polymarket: "us-election-2024"     ─┐                          │
│                                      ├──▶ AGG_US_PRES_2024     │
│  Kalshi: "PRES-2024-DEM"            ─┘                          │
│                                                                 │
│  Redis Keys:                                                    │
│    polymarket:event:us-election-2024  →  AGG_US_PRES_2024       │
│    kalshi:event:PRES-2024-DEM         →  AGG_US_PRES_2024       │
│    event:polymarket:us-election-2024  →  {full event JSON}      │
│    aggregate:AGG_US_PRES_2024        →  {metadata JSON}         │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

---

## Component Deep Dive

### Aggregator Service

**Responsibilities:**

- Maintain persistent WebSocket connections to exchanges
- Handle reconnection with exponential backoff
- Forward raw messages to NATS with minimal processing

**Key Design:**

```rust
struct Supervisor {
    workers: HashMap<String, JoinHandle<()>>,
    nats_client: NatsClient,
}

// Supervisor pattern for fault tolerance
impl Supervisor {
    async fn subscribe_with_mappings(&mut self, mappings: Vec<TokenMapping>);
    async fn unsubscribe(&mut self, ids: Vec<String>);
    fn worker_count(&self) -> usize;
}
```

**Performance Optimizations:**

- Single WebSocket per exchange (multiplexed subscriptions)
- Zero-copy message forwarding (pass payload slice directly)
- Keep-alive pings to detect dead connections early

### Normalizer Service

**Responsibilities:**

- Parse exchange-specific message formats
- Transform to unified `NormalizedOrderbook` schema
- Publish to normalized NATS subjects

**Plugin Architecture:**

```rust
pub trait ExchangeAdapter: Send + Sync {
    fn exchange_name(&self) -> &'static str;
    fn parse_message(&self, raw: &[u8]) -> Result<Option<NormalizedOrderbook>>;
}

// Adding new exchange:
struct KalshiAdapter;
impl ExchangeAdapter for KalshiAdapter { ... }
```

**Performance Optimizations:**

- Pre-compiled regex patterns
- Avoid string allocations (use `&str` slices)
- Reuse message buffers

### Orderbook Service

**Responsibilities:**

- Subscribe to normalized NATS messages
- Maintain in-memory orderbook state
- Expose HTTP API for queries
- Fetch market metadata from Redis

**Lock-Free Architecture:**

```rust
struct OrderbookStore {
    // DashMap: sharded concurrent hashmap (no global lock)
    books: DashMap<String, DashMap<String, DashMap<String, Orderbook>>>,
    //       aggregate_id     market_id     asset_id

    // Caches (also DashMap for concurrent access)
    aggregate_mapping_cache: DashMap<String, String>,
    market_metadata_cache: DashMap<String, MarketMetadata>,
}
```

**Why DashMap over RwLock<HashMap>?**

- 16 internal shards reduce contention
- Readers don't block writers (and vice versa)
- No priority inversion issues
- Better cache locality per shard

### Gateway Service

**Responsibilities:**

- Accept WebSocket connections from trading clients
- Manage subscriptions per client
- Route orderbook changes from NATS to subscribed clients
- Send initial snapshot on subscription

**Key Design:**

```rust
struct ChangeRouter {
    registry: Arc<ClientRegistry>,
    nats_client: Arc<NatsClient>,
    http_client: reqwest::Client,
}

// Routes NATS messages to WebSocket clients
impl ChangeRouter {
    async fn handle_change(&self, payload: &[u8]) -> Result<()>;
    async fn send_snapshot(&self, client: &ClientState, subject: &str) -> Result<()>;
}
```

**Performance Optimizations:**

- Pre-serialize JSON once, send to all matching clients
- DashMap for lock-free client registry
- Wildcard subscription matching with pattern cache

### Event Service

**Responsibilities:**

- Fetch event metadata from exchange APIs
- Cache in Redis with configurable TTL
- Provide HTTP API for event queries
- Background refresh of cached events

**Background Refresh:**

```rust
// Periodic refresh of all cached events
tokio::spawn(async move {
    loop {
        tokio::time::sleep(refresh_interval).await;
        refresh_all_events(&redis, &polymarket).await;
    }
});
```

---

## NATS Configuration

### Subject Hierarchy

```
{type}.{platform}.{market_id}.{asset_id}
```

| Pattern                                                    | Purpose                 | Example                                  |
| ---------------------------------------------------------- | ----------------------- | ---------------------------------------- |
| `market.{platform}.{asset_id}`                             | Raw exchange data       | `market.polymarket.12345...`             |
| `normalized.{platform}.{market_id}.{asset_id}`             | Normalized data         | `normalized.polymarket.abc.12345...`     |
| `orderbook.changes.{agg_id}.{hashed_market_id}.{token_id}` | Aggregated price deltas | `orderbook.changes.abc123.def456.token1` |

### Stream Configuration

| Stream                  | Subject                   | Retention | Purpose            |
| ----------------------- | ------------------------- | --------- | ------------------ |
| `POLYMARKET_MARKET`     | `market.polymarket.>`     | 5 min     | Raw message replay |
| `POLYMARKET_NORMALIZED` | `normalized.polymarket.>` | 5 min     | Normalized replay  |

**Why 5-minute retention?**

- Sufficient for reconnection/catchup
- Keeps memory footprint small
- Not designed for historical analysis

---

## HTTP API Reference

### Orderbook Service (`localhost:8080`)

| Endpoint                                     | Method | Description                  |
| -------------------------------------------- | ------ | ---------------------------- |
| `/health`                                    | GET    | Health check                 |
| `/stats`                                     | GET    | Service statistics           |
| `/orderbooks`                                | GET    | List all aggregates/markets  |
| `/orderbook/{agg_id}`                        | GET    | All orderbooks for aggregate |
| `/orderbook/{agg_id}/{market_id}`            | GET    | Orderbooks for market        |
| `/orderbook/{agg_id}/{market_id}/{asset_id}` | GET    | Specific orderbook           |
| `/bbo/{agg_id}/{market_id}`                  | GET    | BBOs for market              |
| `/bbo/{agg_id}/{market_id}/{asset_id}`       | GET    | Specific BBO                 |

**Query Parameters:**

- `depth` - Number of price levels (default: 10)

**Response Example:**

```json
{
  "aggregate_id": "AGG123",
  "asset_id": "token456",
  "market_id": "market789",
  "market_question": "Will the Fed raise rates?",
  "market_slug": "fed-decision-january",
  "platforms": ["polymarket", "kalshi"],
  "bids": [
    {
      "price": "0.55",
      "total_size": "1500",
      "platforms": [
        {"platform": "polymarket", "size": "1000"},
        {"platform": "kalshi", "size": "500"}
      ]
    }
  ],
  "asks": [...],
  "polymarket_best_bid": "0.55",
  "polymarket_best_ask": "0.57",
  "exchange_best_bid": {"polymarket": "0.55", "kalshi": "0.54"},
  "exchange_best_ask": {"polymarket": "0.57", "kalshi": "0.58"},
  "system_best_bid": "0.55",
  "system_best_ask": "0.57",
  "mid_price": "0.56",
  "spread": "0.02"
}
```

### Event Service (`localhost:8081`)

| Endpoint                           | Method     | Description          |
| ---------------------------------- | ---------- | -------------------- |
| `/health`                          | GET        | Health check         |
| `/events`                          | GET        | List all events      |
| `/event/{platform}/{slug}`         | GET        | Get event data       |
| `/event/{platform}/{slug}/tokens`  | GET        | Get token mappings   |
| `/event/{platform}/{slug}/refresh` | POST       | Refresh from API     |
| `/aggregates`                      | GET        | List aggregates      |
| `/aggregate`                       | POST       | Create aggregate     |
| `/aggregate/{id}`                  | GET/DELETE | Get/delete aggregate |
| `/aggregate/{id}/map`              | POST       | Map platform:slug    |
| `/mapping/{platform}/{slug}`       | GET        | Get aggregate ID     |

### Gateway Service - WebSocket API (`localhost:8082`)

**Endpoint:** `ws://localhost:8082/ws`

#### Client → Server Messages

**Subscribe:**
```json
{
  "type": "subscribe",
  "subjects": ["agg123.market456.token789", "agg123.*.token789"]
}
```

**Unsubscribe:**
```json
{
  "type": "unsubscribe",
  "subjects": ["agg123.market456.token789"]
}
```

**Ping:**
```json
{"type": "ping"}
```

#### Server → Client Messages

**Snapshot** (sent on initial subscribe):
```json
{
  "type": "snapshot",
  "aggregate_id": "agg123",
  "hashed_market_id": "market456",
  "clob_token_id": "token789",
  "market_id": "original-market-id",
  "platforms": ["polymarket", "kalshi"],
  "bids": [
    {
      "price": "0.55",
      "total_size": "1500",
      "platforms": [
        {"platform": "polymarket", "size": "1000"},
        {"platform": "kalshi", "size": "500"}
      ]
    }
  ],
  "asks": [...],
  "system_best_bid": "0.55",
  "system_best_ask": "0.57",
  "timestamp_us": 1234567890123456
}
```

**Price Change** (sent on orderbook updates):
```json
{
  "type": "price_change",
  "aggregate_id": "agg123",
  "hashed_market_id": "market456",
  "clob_token_id": "token789",
  "market_id": "original-market-id",
  "changes": [
    {
      "price": "0.55",
      "side": "buy",
      "total_size": "1600",
      "platforms": [
        {"platform": "polymarket", "size": "1100"},
        {"platform": "kalshi", "size": "500"}
      ]
    },
    {
      "price": "0.56",
      "side": "sell",
      "total_size": "0",
      "platforms": []
    }
  ],
  "timestamp_us": 1234567890123456
}
```

**Note:** A `total_size` of `"0"` with empty `platforms` indicates the price level was removed.

**Subscribed:**
```json
{
  "type": "subscribed",
  "subjects": ["agg123.market456.token789"]
}
```

**Error:**
```json
{
  "type": "error",
  "message": "Invalid subject format",
  "code": "INVALID_SUBJECT"
}
```

#### Subject Format

Subjects follow the pattern: `{aggregate_id}.{hashed_market_id}.{clob_token_id}`

Wildcards supported:
- `*` matches any single segment (e.g., `agg123.*.token789`)
- Wildcard subscriptions do not receive initial snapshots

---

## Configuration

### Environment Variables

| Variable                  | Service          | Default                   | Description                             |
| ------------------------- | ---------------- | ------------------------- | --------------------------------------- |
| `NATS_URL`                | all              | `nats://localhost:4222`   | NATS server                             |
| `REDIS_URL`               | all              | `redis://localhost:6379`  | Redis server                            |
| `RUST_LOG`                | all              | `info`                    | Log level                               |
| `HTTP_PORT`               | orderbook, event | 8080/8081                 | HTTP API port                           |
| `METRICS_PORT`            | all              | 9090/9091/9092/9093       | Prometheus port                         |
| `EVENT_SLUG`              | aggregator       | -                         | Event to subscribe                      |
| `NATS_SUBJECT`            | orderbook        | `normalized.polymarket.>` | NATS subscription                       |
| `REFRESH_INTERVAL_SECS`   | event            | `60`                      | Event refresh interval                  |
| `PUBLISH_CHANGES`         | orderbook        | `false`                   | Publish changes to NATS for gateway     |
| `WS_PORT`                 | gateway          | `8082`                    | WebSocket server port                   |
| `ORDERBOOK_SERVICE_URL`   | gateway          | `http://localhost:8080`   | Orderbook HTTP API URL (for snapshots)  |
| `GATEWAY_NATS_SUBJECT`    | gateway          | `orderbook.changes.>`     | NATS subject for price changes          |

---

## Reliability & Fault Tolerance

### Failure Modes and Recovery

| Failure              | Detection            | Recovery                     | Impact              |
| -------------------- | -------------------- | ---------------------------- | ------------------- |
| WebSocket disconnect | Connection closed    | Auto-reconnect (exp backoff) | <5s gap             |
| NATS unavailable     | Publish timeout      | Retry + alert                | Messages buffered   |
| Redis unavailable    | Connection error     | Graceful degradation         | No metadata         |
| Parse error          | Deserialization fail | Log + skip                   | Single message lost |

### Graceful Degradation

```
Level 0: Full functionality (all systems operational)
    ↓ Redis fails
Level 1: No market metadata (question/slug unavailable)
    ↓ One exchange disconnects
Level 2: Partial platform coverage (remaining platforms continue)
    ↓ NATS fails
Level 3: Stale data (last known orderbook state)
    ↓ All exchanges fail
Level 4: Service unavailable (return 503)
```

---

## Performance Best Practices

### Hot Path Rules

1. **No allocations** - Reuse buffers, use stack allocation
2. **No locks** - DashMap, atomics, channels only
3. **No disk I/O** - Everything in-memory
4. **No logging** - DEBUG/TRACE disabled in production
5. **No unnecessary serialization** - Parse once, cache result

### Code Patterns

```rust
// GOOD: Zero-copy parsing
fn parse_price(s: &str) -> Option<Decimal> {
    Decimal::from_str(s).ok()
}

// BAD: Unnecessary allocation
fn parse_price(s: String) -> Option<Decimal> {
    Decimal::from_str(&s).ok()
}

// GOOD: Pre-allocated vector
let mut prices = Vec::with_capacity(expected_count);

// BAD: Growing vector
let mut prices = Vec::new();

// GOOD: Reference in signature
fn process(data: &NormalizedOrderbook) { }

// BAD: Owned type when reference suffices
fn process(data: NormalizedOrderbook) { }
```

---

## Future Roadmap

### Phase 1: Foundation (Current)

- [x] Polymarket WebSocket integration
- [x] Normalizer with adapter pattern
- [x] Lock-free orderbook storage
- [x] Multi-platform aggregation
- [x] Redis metadata caching
- [x] HTTP REST API

### Phase 2: Multi-Exchange

- [ ] Kalshi integration
- [ ] Cross-exchange arbitrage detection
- [ ] Unified order routing

### Phase 3: Real-Time Streaming (Complete)

- [x] WebSocket API for clients (Gateway service)
- [x] Delta-only updates (aggregated price changes with explicit side)
- [x] Subscription management (per-client wildcard support)

### Phase 4: Order Execution

- [ ] Smart order routing
- [ ] Best execution algorithm
- [ ] Position tracking

### Phase 5: Analytics

- [ ] Historical data storage
- [ ] Latency percentile tracking
- [ ] Market microstructure analysis
