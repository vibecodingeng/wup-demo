//! Normalized orderbook schema definitions.

use serde::{Deserialize, Serialize};

/// Default platform value for backwards compatibility.
fn default_platform() -> String {
    "unknown".to_string()
}

/// Price level in the orderbook.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PriceLevel {
    /// Price as decimal string (e.g., "0.55").
    pub price: String,
    /// Size/quantity at this price level.
    pub size: String,
    /// Source platform (e.g., "polymarket", "kalshi").
    #[serde(default = "default_platform")]
    pub platform: String,
}

/// Type of orderbook message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum OrderbookMessageType {
    /// Full orderbook snapshot.
    Snapshot,
    /// Incremental update/delta.
    Delta,
}

/// Order side.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "UPPERCASE")]
pub enum Side {
    Buy,
    Sell,
}

/// Individual update in a delta message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderbookUpdate {
    /// BUY or SELL side.
    pub side: Side,
    /// Price level being updated.
    pub price: String,
    /// New size at this level (0 means remove).
    pub size: String,
}

/// Normalized orderbook message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NormalizedOrderbook {
    /// Source exchange identifier.
    pub exchange: String,
    /// Source platform identifier (e.g., "polymarket", "kalshi").
    #[serde(default = "default_platform")]
    pub platform: String,
    /// CLOB token identifier (the tradeable token/outcome).
    pub clob_token_id: String,
    /// Market identifier (condition_id on Polymarket).
    pub market_id: String,
    /// Message type for downstream consumers.
    pub message_type: OrderbookMessageType,
    /// Full bid side (only populated for Snapshot).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bids: Option<Vec<PriceLevel>>,
    /// Full ask side (only populated for Snapshot).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asks: Option<Vec<PriceLevel>>,
    /// Delta updates (only populated for Delta).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updates: Option<Vec<OrderbookUpdate>>,
    /// Best bid price.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub best_bid: Option<String>,
    /// Best ask price.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub best_ask: Option<String>,
    /// Original exchange timestamp (milliseconds).
    pub exchange_timestamp: String,
    /// Timestamp when message was received from exchange (milliseconds).
    pub received_at: i64,
    /// Normalizer processing timestamp (ISO 8601).
    pub normalized_at: String,
}
