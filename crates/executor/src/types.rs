//! Common types for all trading platforms.

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Order side (buy or sell).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum OrderSide {
    Buy,
    Sell,
}

impl fmt::Display for OrderSide {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OrderSide::Buy => write!(f, "buy"),
            OrderSide::Sell => write!(f, "sell"),
        }
    }
}

/// Order type (time in force).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum OrderType {
    /// Good-Till-Cancelled (limit order)
    GTC,
    /// Good-Till-Date (limit order)
    GTD,
    /// Fill-Or-Kill (limit order)
    FOK,
    /// Market order - immediate execution at best price (DEFAULT)
    #[default]
    Market,
}

impl fmt::Display for OrderType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OrderType::GTC => write!(f, "GTC"),
            OrderType::GTD => write!(f, "GTD"),
            OrderType::FOK => write!(f, "FOK"),
            OrderType::Market => write!(f, "Market"),
        }
    }
}

/// Unified order request - supports both limit and market orders.
///
/// # Order Types
///
/// - **Market orders** (default): Set `order_type: Market` (or omit, as it's the default),
///   and provide `amount`. For BUY orders, amount is in USDC. For SELL orders, amount is in shares.
///
/// - **Limit orders**: Set `order_type` to GTC, GTD, or FOK, and provide `price` and `size`.
///
/// # Examples
///
/// Market buy (spend 50 USDC):
/// ```json
/// { "market_id": "...", "token_id": "...", "side": "buy", "amount": "50.00" }
/// ```
///
/// Market sell (sell 100 shares):
/// ```json
/// { "market_id": "...", "token_id": "...", "side": "sell", "amount": "100" }
/// ```
///
/// Limit buy:
/// ```json
/// { "market_id": "...", "token_id": "...", "side": "buy", "order_type": "GTC", "price": "0.55", "size": "100" }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderRequest {
    /// Market identifier (condition_id for Polymarket).
    pub market_id: String,
    /// Token/asset identifier (clob_token_id for Polymarket).
    pub token_id: String,
    /// Order side (buy or sell).
    pub side: OrderSide,

    /// Order type - defaults to Market when not specified.
    /// Use GTC, GTD, or FOK for limit orders.
    #[serde(default)]
    pub order_type: OrderType,

    /// Target platform (None for smart routing).
    pub platform: Option<String>,

    // === Limit order fields (required when order_type is GTC/GTD/FOK) ===
    /// Limit price (0.0 - 1.0 for prediction markets).
    /// Required for limit orders.
    #[serde(default)]
    pub price: Option<Decimal>,
    /// Order size in shares.
    /// Required for limit orders.
    #[serde(default)]
    pub size: Option<Decimal>,

    // === Market order field (required when order_type is Market) ===
    /// Amount for market orders.
    /// - BUY: Amount in USDC to spend
    /// - SELL: Amount in shares to sell
    #[serde(default)]
    pub amount: Option<Decimal>,

    // === Optional fields ===
    /// Expiration timestamp in seconds (for GTD orders).
    pub expiration: Option<u64>,
    /// Post-only flag (maker orders only, for limit orders).
    pub post_only: Option<bool>,
    /// Tick size for amount rounding (e.g., "0.001", "0.01", "0.0001").
    /// If not specified, defaults to "0.001" (most common for Polymarket).
    #[serde(default)]
    pub tick_size: Option<String>,
}

/// Order response (platform-agnostic).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderResponse {
    /// Order ID from the platform.
    pub order_id: String,
    /// Platform that received the order.
    pub platform: String,
    /// Order status.
    pub status: OrderStatus,
    /// Filled size (if partially/fully filled).
    pub filled_size: Option<Decimal>,
    /// Remaining size.
    pub remaining_size: Option<Decimal>,
    /// Price at time of routing (for smart routing).
    pub routed_price: Option<Decimal>,
}

/// Order status (platform-agnostic).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum OrderStatus {
    /// Order submitted, awaiting confirmation.
    Pending,
    /// Order is open on the book.
    Open,
    /// Order fully filled.
    Filled,
    /// Order partially filled.
    PartiallyFilled,
    /// Order cancelled.
    Cancelled,
    /// Order rejected.
    Rejected,
    /// Order matched.
    Matched,
    /// Order unknown.
    Unknown,
}

impl fmt::Display for OrderStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OrderStatus::Pending => write!(f, "pending"),
            OrderStatus::Open => write!(f, "open"),
            OrderStatus::Filled => write!(f, "filled"),
            OrderStatus::PartiallyFilled => write!(f, "partially_filled"),
            OrderStatus::Cancelled => write!(f, "cancelled"),
            OrderStatus::Rejected => write!(f, "rejected"),
            OrderStatus::Matched => write!(f, "matched"),
            OrderStatus::Unknown => write!(f, "unknown"),
        }
    }
}
