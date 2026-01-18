//! Platform-specific API interactions library.
//!
//! This crate provides clients for interacting with various prediction market
//! platforms like Polymarket and Kalshi. Each platform module contains:
//!
//! - Authentication (L1/L2 auth for Polymarket)
//! - HTTP client for API calls
//! - Order signing (EIP-712 for Polymarket)
//! - Platform-specific types
//!
//! # Example
//!
//! ```ignore
//! use platforms::polymarket::{PolymarketClient, L2Auth};
//!
//! let l2_auth = L2Auth::from_env()?;
//! let client = PolymarketClient::with_l2(l2_auth);
//! ```

pub mod error;
pub mod polymarket;

pub use error::{Error, Result};

pub use polymarket::{PolymarketExecutor, PolymarketSdkClient};
