//! Orderbook service library.
//!
//! Provides lock-free in-memory orderbook state management with NATS subscription
//! and HTTP API access.
//!
//! # Architecture
//!
//! - **Lock-free storage**: Uses DashMap for concurrent read/write access
//! - **Dual BBO tracking**: Tracks both exchange-reported and system-calculated best bid/ask
//! - **Low-latency**: NATS Core subscription for push-based message delivery
//!
//! # Example
//!
//! ```ignore
//! use orderbook_service::{OrderbookStore, OrderbookService, OrderbookServiceConfig};
//!
//! let store = OrderbookStore::new();
//! let service = OrderbookService::new(store.clone(), nats_client, config, shutdown_rx);
//!
//! // Spawn service
//! tokio::spawn(service.run());
//!
//! // Query orderbook
//! let orderbook = store.get("market_id", "asset_id", Some(10));
//! ```

pub mod api;
pub mod error;
pub mod orderbook;
pub mod service;
pub mod store;

pub use api::{create_router, AppState};
pub use orderbook::{
    AggregatedPriceLevel, BboResponse, EventInfo, MarketInfo, Orderbook, OrderbookResponse,
    PlatformEntry, PlatformSizes, ResponseMetadata, TokenSummary,
};
pub use service::{OrderbookService, OrderbookServiceBuilder, OrderbookServiceConfig};
pub use store::{
    AllOrderbooksResponse, MarketBbosResponse, MarketMetadata, MarketOrderbooksResponse,
    MarketSummary, OrderbookStore, StoreStats,
};
