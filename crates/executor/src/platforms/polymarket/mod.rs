//! Polymarket CLOB API client.
//!
//! This module provides a client for interacting with Polymarket's Central
//! Limit Order Book (CLOB) API using the official SDK.
//!
//! # Usage
//!
//! The [`PolymarketSdkClient`] uses the official `polymarket-client-sdk` crate
//! and handles all EIP-712 signing correctly:
//!
//! ```ignore
//! use platforms::polymarket::PolymarketSdkClient;
//!
//! let client = PolymarketSdkClient::from_env().await?;
//! let response = client.submit_order(&order_request).await?;
//! ```

// SDK-based client (uses official polymarket-client-sdk)
pub mod client;
pub use client::PolymarketSdkClient;

// Types for order responses and API types
pub mod types;
pub use types::{
    CancelOrdersRequest, CancelOrdersResponse, MarketInfo, OpenOrder, OrderSubmitRequest,
    OrderSubmitResponse, PolymarketSide, RoundConfig, SignedOrder, TickSize, TokenInfo,
};

pub mod executor;
pub use executor::PolymarketExecutor;
