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
//! let orderbook = store.get("market_id", "clob_token_id", Some(10));
//! ```

use sha2::{Digest, Sha256};

/// Create a short, deterministic market ID from condition_id.
///
/// Uses first 16 chars of SHA256 hex for uniqueness + brevity.
/// TODO: For multi-platform, may need to include platform in hash.
///
/// # Example
/// ```
/// use orderbook_service::hash_market_id;
/// let hashed = hash_market_id("0xe93c89c41d1bb08d3bb40066d8565df301a696563b2542256e6e8bbbb1ec490d");
/// assert_eq!(hashed.len(), 16);
/// ```
pub fn hash_market_id(condition_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(condition_id.as_bytes());
    let result = hasher.finalize();
    format!("{:x}", result)[..16].to_string()
}

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
    AllOrderbooksResponse, EventMetadata, EventOrderbooksResponse, EventSummary,
    MarketBbosResponse, MarketMetadata, MarketOrderbooksResponse, MarketSummary,
    OrderbookStore, StoreStats,
};
