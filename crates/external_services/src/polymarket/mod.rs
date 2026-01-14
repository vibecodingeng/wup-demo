//! Polymarket exchange API client.
//!
//! Provides REST API client and WebSocket utilities for Polymarket.

pub mod client;
pub mod types;
pub mod websocket;

pub use client::PolymarketClient;
pub use types::{Event, EventData, Market, MarketData, TokenMapping};
pub use websocket::{
    build_subscription_message, build_unsubscription_message, MAX_SUBSCRIPTIONS, WS_URL,
};
