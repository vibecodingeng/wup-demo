//! Executor service for smart order routing.
//!
//! This crate provides:
//! - `Executor` trait for platform-specific order execution
//! - `SmartRouter` for routing orders to the best platform
//! - `BboCache` for local BBO cache updated via NATS
//! - `PolymarketExecutor` for Polymarket order execution
//! - HTTP API for order submission and management
//!
//! # Architecture
//!
//! ```text
//!                     ORDER REQUEST
//!                          │
//!                          ▼
//!                    SmartRouter
//!                          │
//!               ┌──────────┴──────────┐
//!               │                     │
//!          BboCache              Executors
//!     (NATS subscription)            │
//!               │              ┌─────┴─────┐
//!               ▼              ▼           ▼
//!         Local DashMap   Polymarket   Kalshi
//!         (~0μs lookup)   Executor     Executor
//! ```
//!
//! # Usage
//!
//! ```ignore
//! use executor::{SmartRouter, PolymarketExecutor, BboCache};
//! use std::sync::Arc;
//!
//! // Create BBO cache
//! let bbo_cache = Arc::new(BboCache::new());
//! bbo_cache.clone().start_subscription(nats_client).await?;
//!
//! // Create router
//! let mut router = SmartRouter::new(bbo_cache);
//!
//! // Register executor
//! let polymarket = Arc::new(PolymarketExecutor::from_env()?);
//! router.register(polymarket);
//!
//! // Submit order with smart routing
//! let response = router.submit_order(order_request).await?;
//! ```

pub mod api;
pub mod bbo_cache;
pub mod error;
pub mod platforms;
pub mod router;
pub mod traits;
mod types;

use crate::types::{OrderRequest, OrderResponse, OrderSide, OrderStatus, OrderType};
