//! External exchange API clients library.
//!
//! This library provides unified access to external exchange APIs:
//! - Polymarket: Prediction market exchange
//! - Kalshi: (Future) Event contracts exchange
//!
//! Also provides a shared Redis client for event/market metadata.
//!
//! # Example
//!
//! ```ignore
//! use external_services::polymarket::PolymarketClient;
//! use external_services::SharedRedisClient;
//!
//! let client = PolymarketClient::new();
//! let event = client.fetch_event_by_slug("some-event-slug").await?;
//! let tokens = PolymarketClient::extract_token_mappings(&event);
//!
//! let redis = SharedRedisClient::new("redis://localhost:6379")?;
//! let market = redis.find_market_by_id("polymarket", "market123").await?;
//! ```

pub mod error;
pub mod polymarket;
pub mod redis_client;

pub use error::{Error, Result};
pub use redis_client::{ConditionMapping, MarketSlugMapping, SharedRedisClient};
