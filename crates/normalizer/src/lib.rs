//! Normalizer service for transforming raw market data to unified schemas.
//!
//! This crate provides a generic, plugin-based architecture for normalizing
//! market data from various exchanges. To add support for a new exchange,
//! implement the `ExchangeAdapter` trait.
//!
//! # Architecture
//!
//! ```text
//! Raw Messages (NATS) --> ExchangeAdapter --> NormalizedOrderbook --> NATS
//!                         (parse & transform)
//! ```
//!
//! # Adding a New Exchange
//!
//! 1. Create a new adapter struct
//! 2. Implement `ExchangeAdapter` trait
//! 3. Optionally implement `OrderbookExchange` or `SportsbookExchange`
//!
//! ```ignore
//! use normalizer::{ExchangeAdapter, NormalizedOrderbook, AdapterConfig};
//!
//! pub struct KalshiAdapter;
//!
//! impl ExchangeAdapter for KalshiAdapter {
//!     const NAME: &'static str = "kalshi";
//!     const FILTER_SUBJECT: &'static str = "market.kalshi.>";
//!
//!     fn parse_and_transform(&self, payload: &str) -> Result<Vec<NormalizedOrderbook>> {
//!         // Parse and transform Kalshi messages
//!     }
//! }
//! ```
//!
//! # Usage
//!
//! ```ignore
//! use normalizer::{NormalizerService, PolymarketAdapter};
//!
//! let adapter = PolymarketAdapter::new();
//! let service = NormalizerService::with_defaults(adapter, nats_client, shutdown_rx);
//! service.run().await?;
//! ```

pub mod polymarket;
pub mod schema;
pub mod service;
pub mod traits;

// Re-export core types
pub use schema::{NormalizedOrderbook, OrderbookMessageType, OrderbookUpdate, PriceLevel, Side};
pub use service::{NormalizerService, NormalizerServiceBuilder};
pub use traits::{AdapterConfig, ExchangeAdapter, OrderbookExchange, SportsbookExchange};

// Re-export exchange adapters
pub use polymarket::PolymarketAdapter;
