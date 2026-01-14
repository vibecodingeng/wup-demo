//! Polymarket data types.

use serde::{Deserialize, Serialize};

/// Event from Polymarket Gamma API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub id: String,
    pub slug: String,
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub active: bool,
    #[serde(default)]
    pub closed: bool,
    #[serde(default)]
    pub markets: Vec<Market>,
}

/// Market within an event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Market {
    pub id: String,
    #[serde(default)]
    pub question: Option<String>,
    #[serde(default)]
    pub slug: Option<String>,
    /// CLOB token IDs as JSON string: "[\"token1\", \"token2\"]"
    #[serde(rename = "clobTokenIds", default)]
    pub clob_token_ids_raw: Option<String>,
    #[serde(default)]
    pub outcomes: Option<String>,
    #[serde(rename = "outcomePrices", default)]
    pub outcome_prices: Option<String>,
    #[serde(rename = "bestBid", default)]
    pub best_bid: Option<f64>,
    #[serde(rename = "bestAsk", default)]
    pub best_ask: Option<f64>,
    #[serde(default)]
    pub volume: Option<String>,
    #[serde(default)]
    pub liquidity: Option<String>,
    #[serde(default)]
    pub active: bool,
    #[serde(default)]
    pub closed: bool,
}

impl Market {
    /// Parse clobTokenIds from JSON string to Vec<String>.
    pub fn clob_token_ids(&self) -> Vec<String> {
        self.clob_token_ids_raw
            .as_ref()
            .and_then(|raw| serde_json::from_str::<Vec<String>>(raw).ok())
            .unwrap_or_default()
    }
}

/// Token mapping for subscription management.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenMapping {
    pub clob_token_id: String,
    pub market_id: String,
    pub event_slug: String,
}

/// Market data stored in Redis (full market information).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketData {
    pub id: String,
    pub question: Option<String>,
    pub slug: Option<String>,
    pub outcomes: Option<String>,
    pub outcome_prices: Option<String>,
    pub best_bid: Option<f64>,
    pub best_ask: Option<f64>,
    pub volume: Option<String>,
    pub liquidity: Option<String>,
    pub active: bool,
    pub closed: bool,
    pub clob_token_ids: Vec<String>,
}

impl MarketData {
    /// Create MarketData from a Market.
    pub fn from_market(market: &Market) -> Self {
        Self {
            id: market.id.clone(),
            question: market.question.clone(),
            slug: market.slug.clone(),
            outcomes: market.outcomes.clone(),
            outcome_prices: market.outcome_prices.clone(),
            best_bid: market.best_bid,
            best_ask: market.best_ask,
            volume: market.volume.clone(),
            liquidity: market.liquidity.clone(),
            active: market.active,
            closed: market.closed,
            clob_token_ids: market.clob_token_ids(),
        }
    }
}

/// Cached event data stored in Redis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventData {
    pub event_id: String,
    pub slug: String,
    pub title: String,
    pub description: Option<String>,
    pub active: bool,
    pub closed: bool,
    pub markets: Vec<MarketData>,
    pub market_count: usize,
    pub tokens: Vec<TokenMapping>,
    pub fetched_at: i64,
}

impl EventData {
    /// Create EventData from an Event.
    pub fn from_event(event: &Event) -> Self {
        let tokens = extract_token_mappings(event);
        let markets = event
            .markets
            .iter()
            .map(MarketData::from_market)
            .collect();
        Self {
            event_id: event.id.clone(),
            slug: event.slug.clone(),
            title: event.title.clone(),
            description: event.description.clone(),
            active: event.active,
            closed: event.closed,
            markets,
            market_count: event.markets.len(),
            tokens,
            fetched_at: chrono::Utc::now().timestamp_millis(),
        }
    }
}

/// Extract token mappings from an event.
pub fn extract_token_mappings(event: &Event) -> Vec<TokenMapping> {
    let mut mappings = Vec::new();

    for market in &event.markets {
        for token_id in market.clob_token_ids() {
            mappings.push(TokenMapping {
                clob_token_id: token_id,
                market_id: market.id.clone(),
                event_slug: event.slug.clone(),
            });
        }
    }

    mappings
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_clob_token_ids() {
        let market = Market {
            id: "123".to_string(),
            question: Some("Test?".to_string()),
            slug: None,
            clob_token_ids_raw: Some(r#"["token1", "token2"]"#.to_string()),
            outcomes: None,
            outcome_prices: None,
            best_bid: None,
            best_ask: None,
            volume: None,
            liquidity: None,
            active: true,
            closed: false,
        };

        let tokens = market.clob_token_ids();
        assert_eq!(tokens, vec!["token1", "token2"]);
    }

    #[test]
    fn test_extract_token_mappings() {
        let event = Event {
            id: "event1".to_string(),
            slug: "test-event".to_string(),
            title: "Test Event".to_string(),
            description: None,
            active: true,
            closed: false,
            markets: vec![
                Market {
                    id: "market1".to_string(),
                    question: Some("Q1?".to_string()),
                    slug: None,
                    clob_token_ids_raw: Some(r#"["token1", "token2"]"#.to_string()),
                    outcomes: None,
                    outcome_prices: None,
                    best_bid: None,
                    best_ask: None,
                    volume: None,
                    liquidity: None,
                    active: true,
                    closed: false,
                },
                Market {
                    id: "market2".to_string(),
                    question: Some("Q2?".to_string()),
                    slug: None,
                    clob_token_ids_raw: Some(r#"["token3"]"#.to_string()),
                    outcomes: None,
                    outcome_prices: None,
                    best_bid: None,
                    best_ask: None,
                    volume: None,
                    liquidity: None,
                    active: true,
                    closed: false,
                },
            ],
        };

        let mappings = extract_token_mappings(&event);
        assert_eq!(mappings.len(), 3);
        assert_eq!(mappings[0].clob_token_id, "token1");
        assert_eq!(mappings[0].market_id, "market1");
        assert_eq!(mappings[0].event_slug, "test-event");
        assert_eq!(mappings[2].clob_token_id, "token3");
        assert_eq!(mappings[2].market_id, "market2");
    }
}
