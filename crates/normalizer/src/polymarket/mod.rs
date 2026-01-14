//! Polymarket exchange adapter.
//!
//! This module provides the `PolymarketAdapter` which implements
//! the `ExchangeAdapter` trait for normalizing Polymarket WebSocket messages.

mod adapter;

pub use adapter::PolymarketAdapter;
