//! WebSocket protocol message types.
//!
//! Defines the JSON message format for client-server communication.

use normalizer::schema::{AggregatedPriceChangeEvent, AggregatedPriceLevel, AggregatedPriceLevelChange};
use serde::{Deserialize, Serialize};

// ============================================================================
// Client → Server Messages
// ============================================================================

/// Message sent from client to server.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    /// Subscribe to orderbook changes for specific subjects.
    Subscribe {
        /// Subjects to subscribe to (e.g., "agg123.market456.token789").
        /// Supports wildcards: "*" matches any single segment.
        subjects: Vec<String>,
    },
    /// Unsubscribe from orderbook changes.
    Unsubscribe {
        /// Subjects to unsubscribe from.
        subjects: Vec<String>,
    },
    /// Ping message for keepalive.
    Ping,
}

// ============================================================================
// Server → Client Messages
// ============================================================================

/// Message sent from server to client.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    /// Full orderbook snapshot (sent on initial subscribe).
    Snapshot(OrderbookData),
    /// Price change event (sent on each orderbook update).
    PriceChange(PriceChangeData),
    /// Pong response to ping.
    Pong,
    /// Confirmation of subscription.
    Subscribed {
        /// Subjects successfully subscribed to.
        subjects: Vec<String>,
    },
    /// Confirmation of unsubscription.
    Unsubscribed {
        /// Subjects successfully unsubscribed from.
        subjects: Vec<String>,
    },
    /// Error message.
    Error {
        /// Error message.
        message: String,
        /// Error code.
        code: String,
    },
}

/// Orderbook data with aggregated price levels.
/// This is the format sent via WebSocket for full orderbook snapshots.
#[derive(Debug, Clone, Serialize)]
pub struct OrderbookData {
    /// Market identifier (condition_id from exchange).
    pub market_id: String,
    /// Asset identifier (clob_token_id from exchange).
    pub asset_id: String,
    /// List of platforms contributing to this orderbook.
    pub platforms: Vec<String>,
    /// Aggregated bid price levels (sorted by price descending).
    pub bids: Vec<AggregatedPriceLevel>,
    /// Aggregated ask price levels (sorted by price ascending).
    pub asks: Vec<AggregatedPriceLevel>,
    /// System-calculated best bid.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_best_bid: Option<String>,
    /// System-calculated best ask.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_best_ask: Option<String>,
    /// Timestamp in microseconds.
    pub timestamp_us: i64,
}

/// Price change data with aggregated levels and explicit side.
/// This is the format sent via WebSocket for orderbook updates.
#[derive(Debug, Clone, Serialize)]
pub struct PriceChangeData {
    /// Market identifier (condition_id from exchange).
    pub market_id: String,
    /// Asset identifier (clob_token_id from exchange).
    pub asset_id: String,
    /// Changed price levels with explicit side and platform breakdown.
    /// Size of "0" means the level was removed.
    pub changes: Vec<AggregatedPriceLevelChange>,
    /// Timestamp in microseconds.
    pub timestamp_us: i64,
}

impl From<AggregatedPriceChangeEvent> for PriceChangeData {
    fn from(event: AggregatedPriceChangeEvent) -> Self {
        Self {
            market_id: event.market_id,
            asset_id: event.asset_id,
            changes: event.changes,
            timestamp_us: event.timestamp_us,
        }
    }
}

/// Orderbook response from HTTP API (for initial snapshots).
#[derive(Debug, Clone, Deserialize)]
pub struct OrderbookHttpResponse {
    pub asset_id: String,
    pub market_id: String,
    pub platforms: Vec<String>,
    pub bids: Vec<HttpAggregatedPriceLevel>,
    pub asks: Vec<HttpAggregatedPriceLevel>,
    pub system_best_bid: Option<String>,
    pub system_best_ask: Option<String>,
}

/// Aggregated price level from HTTP API.
#[derive(Debug, Clone, Deserialize)]
pub struct HttpAggregatedPriceLevel {
    pub price: String,
    pub total_size: String,
    pub platforms: Vec<HttpPlatformSize>,
}

/// Platform size breakdown from HTTP API.
#[derive(Debug, Clone, Deserialize)]
pub struct HttpPlatformSize {
    pub platform: String,
    pub size: String,
}

/// Market orderbooks response from HTTP API (for wildcard snapshots).
#[derive(Debug, Clone, Deserialize)]
pub struct MarketOrderbooksHttpResponse {
    pub market_id: String,
    pub asset_count: usize,
    pub assets: Vec<OrderbookHttpResponse>,
}

impl OrderbookHttpResponse {
    /// Convert to orderbook data for WebSocket.
    pub fn to_orderbook_data(self, timestamp_us: i64) -> OrderbookData {
        use normalizer::schema::PlatformEntry;

        let bids: Vec<AggregatedPriceLevel> = self
            .bids
            .into_iter()
            .map(|level| AggregatedPriceLevel {
                price: level.price,
                total_size: level.total_size,
                platforms: level
                    .platforms
                    .into_iter()
                    .map(|p| PlatformEntry {
                        platform: p.platform,
                        size: p.size,
                    })
                    .collect(),
            })
            .collect();

        let asks: Vec<AggregatedPriceLevel> = self
            .asks
            .into_iter()
            .map(|level| AggregatedPriceLevel {
                price: level.price,
                total_size: level.total_size,
                platforms: level
                    .platforms
                    .into_iter()
                    .map(|p| PlatformEntry {
                        platform: p.platform,
                        size: p.size,
                    })
                    .collect(),
            })
            .collect();

        OrderbookData {
            market_id: self.market_id,
            asset_id: self.asset_id,
            platforms: self.platforms,
            bids,
            asks,
            system_best_bid: self.system_best_bid,
            system_best_ask: self.system_best_ask,
            timestamp_us,
        }
    }
}
