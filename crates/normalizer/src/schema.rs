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

/// Platform entry showing a platform's size at a price level.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformEntry {
    /// Platform name (e.g., "polymarket", "kalshi").
    pub platform: String,
    /// Size from this platform at the price level.
    pub size: String,
}

/// Aggregated price level with platform breakdown.
/// This is the format used in WebSocket messages and HTTP API responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregatedPriceLevel {
    /// Price at this level.
    pub price: String,
    /// Total size across all platforms.
    pub total_size: String,
    /// Breakdown by platform.
    pub platforms: Vec<PlatformEntry>,
}

/// Orderbook change event published to NATS for gateway consumption.
/// This is emitted by OrderbookService after applying updates.
/// Contains the full aggregated orderbook state after the change.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderbookChange {
    /// Event aggregate identifier.
    pub aggregate_id: String,
    /// Hashed market identifier (first 16 chars of SHA256).
    pub hashed_market_id: String,
    /// CLOB token identifier.
    pub clob_token_id: String,
    /// Original market identifier.
    pub market_id: String,
    /// List of platforms contributing to this orderbook.
    pub platforms: Vec<String>,
    /// Aggregated bid price levels (sorted by price descending).
    pub bids: Vec<AggregatedPriceLevel>,
    /// Aggregated ask price levels (sorted by price ascending).
    pub asks: Vec<AggregatedPriceLevel>,
    /// System-calculated best bid after this change.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_best_bid: Option<String>,
    /// System-calculated best ask after this change.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_best_ask: Option<String>,
    /// Timestamp in microseconds for latency tracking.
    pub timestamp_us: i64,
}

/// Order side for price change events (lowercase JSON serialization).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum OrderSide {
    /// Buy side (bids).
    Buy,
    /// Sell side (asks).
    Sell,
}

impl From<Side> for OrderSide {
    fn from(side: Side) -> Self {
        match side {
            Side::Buy => OrderSide::Buy,
            Side::Sell => OrderSide::Sell,
        }
    }
}

/// Aggregated price level change with explicit side.
/// Contains the full aggregated state for a price level after a change.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregatedPriceLevelChange {
    /// Price at this level.
    pub price: String,
    /// Order side (buy or sell).
    pub side: OrderSide,
    /// Total size across all platforms.
    pub total_size: String,
    /// Breakdown by platform.
    pub platforms: Vec<PlatformEntry>,
}

/// Aggregated price change event published to NATS for gateway consumption.
/// Contains aggregated state for each changed price level with explicit side.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregatedPriceChangeEvent {
    /// Event aggregate identifier.
    pub aggregate_id: String,
    /// Hashed market identifier (first 16 chars of SHA256).
    pub hashed_market_id: String,
    /// CLOB token identifier.
    pub clob_token_id: String,
    /// Original market identifier.
    pub market_id: String,
    /// All changed price levels with explicit side and platform breakdown.
    pub changes: Vec<AggregatedPriceLevelChange>,
    /// Timestamp in microseconds for latency tracking.
    pub timestamp_us: i64,
}
