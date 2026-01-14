//! Gateway service for real-time orderbook streaming to WebSocket clients.
//!
//! This service:
//! - Accepts WebSocket connections from trading clients
//! - Manages client subscriptions to specific markets/assets
//! - Subscribes to orderbook changes via NATS
//! - Routes changes to subscribed clients with minimal latency
//!
//! ## Architecture
//!
//! ```text
//! NATS: orderbook.changes.>
//!         ↓
//! ChangeRouter (subscribes to NATS)
//!         ↓
//! ClientRegistry (DashMap-based, lock-free)
//!         ↓
//! WebSocket clients
//! ```
//!
//! ## Low-Latency Design
//!
//! - Lock-free client registry using DashMap
//! - Pre-serialized messages for broadcast
//! - Unbounded channels to avoid backpressure blocking
//! - Single NATS subscription with local filtering

pub mod client;
pub mod error;
pub mod protocol;
pub mod router;
pub mod subscription;
pub mod ws_server;

pub use client::{ClientId, ClientRegistry, ClientState};
pub use error::{GatewayError, Result};
pub use protocol::{ClientMessage, OrderbookData, PriceChangeData, ServerMessage};
pub use router::{ChangeRouter, RouterConfig};
pub use ws_server::{create_router, AppState};
