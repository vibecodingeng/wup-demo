//! Polymarket exchange adapter implementation.

use crate::schema::{NormalizedOrderbook, OrderbookMessageType, OrderbookUpdate, PriceLevel, Side};
use crate::traits::{AdapterConfig, ExchangeAdapter, OrderbookExchange};
use anyhow::Result;
use chrono::Utc;
use serde::Deserialize;
use std::collections::HashMap;

/// Polymarket exchange adapter.
///
/// Handles parsing and transformation of Polymarket WebSocket messages
/// (book and price_change) to normalized orderbook format.
#[derive(Debug, Default, Clone)]
pub struct PolymarketAdapter;

impl PolymarketAdapter {
    /// Create a new Polymarket adapter.
    pub fn new() -> Self {
        Self
    }
}

impl ExchangeAdapter for PolymarketAdapter {
    const NAME: &'static str = "polymarket";
    const FILTER_SUBJECT: &'static str = "market.polymarket.>";

    fn parse_and_transform(&self, payload: &str) -> Result<Vec<NormalizedOrderbook>> {
        let parsed_messages = ParsedMessage::parse_all(payload)?;
        let normalized_at = Utc::now().to_rfc3339();

        let mut results = Vec::new();
        for parsed in parsed_messages {
            match parsed {
                ParsedMessage::Book(book) => {
                    results.push(transform_book(book, &normalized_at));
                }
                ParsedMessage::PriceChange(pc) => {
                    results.extend(transform_price_change(pc, &normalized_at));
                }
                ParsedMessage::Unknown(_) => {}
            }
        }

        Ok(results)
    }

    /// Build output subject: normalized.polymarket.{market_id}.{clob_token_id}
    fn build_output_subject(&self, config: &AdapterConfig, msg: &NormalizedOrderbook) -> String {
        format!(
            "{}.{}.{}",
            config.output_subject_prefix, msg.market_id, msg.clob_token_id
        )
    }
}

impl OrderbookExchange for PolymarketAdapter {
    fn supports_snapshots(&self) -> bool {
        true
    }

    fn supports_deltas(&self) -> bool {
        true
    }
}

// ============================================================================
// Raw Message Types (private)
// ============================================================================

/// Deserialize a value that could be either a string or a number into a String.
fn string_or_number<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    match value {
        serde_json::Value::String(s) => Ok(s),
        serde_json::Value::Number(n) => Ok(n.to_string()),
        serde_json::Value::Null => Ok(String::new()),
        _ => Ok(String::new()),
    }
}

/// Deserialize an optional string or number with default.
fn string_or_number_default<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    match value {
        Some(serde_json::Value::String(s)) => Ok(s),
        Some(serde_json::Value::Number(n)) => Ok(n.to_string()),
        _ => Ok(String::new()),
    }
}

#[derive(Debug, Deserialize)]
struct RawBookMessage {
    #[allow(dead_code)]
    event_type: String,
    asset_id: String,
    market: String,
    bids: Vec<RawPriceLevel>,
    asks: Vec<RawPriceLevel>,
    #[serde(deserialize_with = "string_or_number")]
    timestamp: String,
    #[allow(dead_code)]
    #[serde(default, deserialize_with = "string_or_number_default")]
    hash: String,
    #[serde(default)]
    received_at: i64,
}

#[derive(Debug, Deserialize)]
struct RawPriceLevel {
    #[serde(deserialize_with = "string_or_number")]
    price: String,
    #[serde(deserialize_with = "string_or_number")]
    size: String,
}

#[derive(Debug, Deserialize)]
struct RawPriceChangeMessage {
    #[allow(dead_code)]
    event_type: String,
    market: String,
    price_changes: Vec<RawPriceChange>,
    #[serde(deserialize_with = "string_or_number")]
    timestamp: String,
    #[serde(default)]
    received_at: i64,
}

#[derive(Debug, Clone, Deserialize)]
struct RawPriceChange {
    asset_id: String,
    #[serde(deserialize_with = "string_or_number")]
    price: String,
    #[serde(deserialize_with = "string_or_number")]
    size: String,
    side: String,
    #[allow(dead_code)]
    #[serde(default, deserialize_with = "string_or_number_default")]
    hash: String,
    #[serde(default, deserialize_with = "string_or_number_default")]
    best_bid: String,
    #[serde(default, deserialize_with = "string_or_number_default")]
    best_ask: String,
}

#[derive(Debug)]
enum ParsedMessage {
    Book(RawBookMessage),
    PriceChange(RawPriceChangeMessage),
    #[allow(dead_code)]
    Unknown(String),
}

impl ParsedMessage {
    /// Parse a single JSON value into a ParsedMessage.
    fn parse_value(obj: serde_json::Value) -> Result<Self> {
        // Extract event_type from the object
        let event_type = obj
            .get("event_type")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        match event_type {
            "book" => {
                let msg: RawBookMessage = serde_json::from_value(obj)?;
                Ok(ParsedMessage::Book(msg))
            }
            "price_change" => {
                let msg: RawPriceChangeMessage = serde_json::from_value(obj)?;
                Ok(ParsedMessage::PriceChange(msg))
            }
            other => Ok(ParsedMessage::Unknown(other.to_string())),
        }
    }

    /// Parse JSON that may be a single object or an array of objects.
    fn parse_all(json: &str) -> Result<Vec<Self>> {
        let value: serde_json::Value = serde_json::from_str(json)?;

        if value.is_array() {
            // Handle array - parse each element
            let arr = value.as_array().unwrap();
            let mut results = Vec::with_capacity(arr.len());
            for item in arr {
                results.push(Self::parse_value(item.clone())?);
            }
            Ok(results)
        } else {
            // Single object
            Ok(vec![Self::parse_value(value)?])
        }
    }

}

// ============================================================================
// Transformation Functions (private)
// ============================================================================

fn transform_book(book: RawBookMessage, normalized_at: &str) -> NormalizedOrderbook {
    let platform = PolymarketAdapter::NAME.to_string();

    let bids: Vec<PriceLevel> = book
        .bids
        .into_iter()
        .map(|l| PriceLevel {
            price: l.price,
            size: l.size,
        })
        .collect();

    let asks: Vec<PriceLevel> = book
        .asks
        .into_iter()
        .map(|l| PriceLevel {
            price: l.price,
            size: l.size,
        })
        .collect();

    let best_bid = bids.first().map(|l| l.price.clone());
    let best_ask = asks.first().map(|l| l.price.clone());

    NormalizedOrderbook {
        platform,
        clob_token_id: book.asset_id,
        market_id: book.market,
        message_type: OrderbookMessageType::Snapshot,
        bids: Some(bids),
        asks: Some(asks),
        updates: None,
        best_bid,
        best_ask,
        exchange_timestamp: book.timestamp,
        received_at: book.received_at,
        normalized_at: normalized_at.to_string(),
    }
}

fn transform_price_change(
    msg: RawPriceChangeMessage,
    normalized_at: &str,
) -> Vec<NormalizedOrderbook> {
    let mut grouped: HashMap<String, Vec<RawPriceChange>> = HashMap::new();
    let mut best_prices: HashMap<String, (String, String)> = HashMap::new();

    for change in msg.price_changes {
        grouped
            .entry(change.asset_id.clone())
            .or_default()
            .push(change.clone());

        best_prices.insert(
            change.asset_id.clone(),
            (change.best_bid.clone(), change.best_ask.clone()),
        );
    }

    let received_at = msg.received_at;

    grouped
        .into_iter()
        .map(|(clob_token_id, changes)| {
            let updates: Vec<OrderbookUpdate> = changes
                .into_iter()
                .map(|c| OrderbookUpdate {
                    side: if c.side == "BUY" {
                        Side::Buy
                    } else {
                        Side::Sell
                    },
                    price: c.price,
                    size: c.size,
                })
                .collect();

            let (best_bid, best_ask) = best_prices.get(&clob_token_id).cloned().unwrap_or_default();

            NormalizedOrderbook {
                platform: PolymarketAdapter::NAME.to_string(),
                clob_token_id,
                market_id: msg.market.clone(),
                message_type: OrderbookMessageType::Delta,
                bids: None,
                asks: None,
                updates: Some(updates),
                best_bid: Some(best_bid),
                best_ask: Some(best_ask),
                exchange_timestamp: msg.timestamp.clone(),
                received_at,
                normalized_at: normalized_at.to_string(),
            }
        })
        .collect()
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_and_transform_book() {
        let adapter = PolymarketAdapter::new();
        let json = r#"{
            "event_type": "book",
            "asset_id": "abc123",
            "market": "market-1",
            "bids": [{"price": "0.55", "size": "100"}, {"price": "0.54", "size": "200"}],
            "asks": [{"price": "0.60", "size": "50"}],
            "timestamp": "1704067200000",
            "hash": "xyz"
        }"#;

        let result = adapter.parse_and_transform(json).unwrap();

        assert_eq!(result.len(), 1);
        let normalized = &result[0];
        assert_eq!(normalized.platform, "polymarket");
        assert_eq!(normalized.clob_token_id, "abc123");
        assert_eq!(normalized.message_type, OrderbookMessageType::Snapshot);
        assert!(normalized.bids.is_some());
        assert!(normalized.asks.is_some());
        assert!(normalized.updates.is_none());
        assert_eq!(normalized.best_bid, Some("0.55".to_string()));
        assert_eq!(normalized.best_ask, Some("0.60".to_string()));
        // Verify price levels have platform field
        let bids = normalized.bids.as_ref().unwrap();
    }

    #[test]
    fn test_parse_and_transform_price_change() {
        let adapter = PolymarketAdapter::new();
        let json = r#"{
            "event_type": "price_change",
            "market": "market-1",
            "price_changes": [{
                "asset_id": "abc123",
                "price": "0.56",
                "size": "25",
                "side": "BUY",
                "hash": "xyz",
                "best_bid": "0.56",
                "best_ask": "0.60"
            }],
            "timestamp": "1704067201000"
        }"#;

        let result = adapter.parse_and_transform(json).unwrap();

        assert_eq!(result.len(), 1);
        let normalized = &result[0];
        assert_eq!(normalized.platform, "polymarket");
        assert_eq!(normalized.message_type, OrderbookMessageType::Delta);
        assert!(normalized.updates.is_some());
        assert_eq!(normalized.updates.as_ref().unwrap()[0].side, Side::Buy);
    }

    #[test]
    fn test_parse_and_transform_multiple_assets() {
        let adapter = PolymarketAdapter::new();
        let json = r#"{
            "event_type": "price_change",
            "market": "market-1",
            "price_changes": [
                {"asset_id": "asset1", "price": "0.55", "size": "100", "side": "BUY", "hash": "h1", "best_bid": "0.55", "best_ask": "0.60"},
                {"asset_id": "asset2", "price": "0.45", "size": "50", "side": "SELL", "hash": "h2", "best_bid": "0.40", "best_ask": "0.45"}
            ],
            "timestamp": "1704067201000"
        }"#;

        let result = adapter.parse_and_transform(json).unwrap();
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_parse_unknown_message() {
        let adapter = PolymarketAdapter::new();
        let json = r#"{"event_type": "last_trade_price", "price": "0.55"}"#;

        let result = adapter.parse_and_transform(json).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_numeric_values() {
        let adapter = PolymarketAdapter::new();
        // Test with numeric price/size/timestamp (not strings)
        let json = r#"{
            "event_type": "book",
            "asset_id": "abc123",
            "market": "market-1",
            "bids": [{"price": 0.55, "size": 100}],
            "asks": [{"price": 0.60, "size": 50}],
            "timestamp": 1704067200000,
            "hash": "xyz"
        }"#;

        let result = adapter.parse_and_transform(json).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].best_bid, Some("0.55".to_string()));
    }

    #[test]
    fn test_parse_array_wrapper() {
        let adapter = PolymarketAdapter::new();
        // Test message wrapped in array
        let json = r#"[{
            "event_type": "book",
            "asset_id": "abc123",
            "market": "market-1",
            "bids": [{"price": "0.55", "size": "100"}],
            "asks": [{"price": "0.60", "size": "50"}],
            "timestamp": "1704067200000",
            "hash": "xyz"
        }]"#;

        let result = adapter.parse_and_transform(json).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].clob_token_id, "abc123");
    }

    #[test]
    fn test_parse_multiple_books_in_array() {
        let adapter = PolymarketAdapter::new();
        // Test multiple book messages in a single array (as Polymarket sends them)
        let json = r#"[
            {
                "event_type": "book",
                "asset_id": "asset1",
                "market": "market-1",
                "bids": [{"price": "0.55", "size": "100"}],
                "asks": [{"price": "0.60", "size": "50"}],
                "timestamp": "1704067200000",
                "hash": "xyz1"
            },
            {
                "event_type": "book",
                "asset_id": "asset2",
                "market": "market-1",
                "bids": [{"price": "0.45", "size": "200"}],
                "asks": [{"price": "0.50", "size": "100"}],
                "timestamp": "1704067200000",
                "hash": "xyz2"
            },
            {
                "event_type": "book",
                "asset_id": "asset3",
                "market": "market-2",
                "bids": [{"price": "0.30", "size": "300"}],
                "asks": [{"price": "0.35", "size": "150"}],
                "timestamp": "1704067200000",
                "hash": "xyz3"
            }
        ]"#;

        let result = adapter.parse_and_transform(json).unwrap();
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].clob_token_id, "asset1");
        assert_eq!(result[0].market_id, "market-1");
        assert_eq!(result[1].clob_token_id, "asset2");
        assert_eq!(result[1].market_id, "market-1");
        assert_eq!(result[2].clob_token_id, "asset3");
        assert_eq!(result[2].market_id, "market-2");
    }

    #[test]
    fn test_received_at_timestamp() {
        let adapter = PolymarketAdapter::new();
        let json = r#"{
            "event_type": "book",
            "asset_id": "abc123",
            "market": "market-1",
            "bids": [{"price": "0.55", "size": "100"}],
            "asks": [{"price": "0.60", "size": "50"}],
            "timestamp": "1704067200000",
            "hash": "xyz",
            "received_at": 1704067200123
        }"#;

        let result = adapter.parse_and_transform(json).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].received_at, 1704067200123);
    }
}
