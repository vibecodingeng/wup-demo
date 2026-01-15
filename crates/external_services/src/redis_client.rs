//! Shared Redis client for accessing event data and market metadata.
//!
//! This module provides a reusable Redis client that can be shared across
//! multiple services (event_service, orderbook_service, aggregator, etc.).

use crate::error::Result;
use crate::polymarket::{EventData, MarketData, TokenMapping};
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{debug, info};

/// Redis key prefix for events: event:{platform}:{slug}
pub const EVENT_KEY_PREFIX: &str = "event:";

/// Redis key prefix for condition mappings: condition:{platform}:{condition_id}
pub const CONDITION_KEY_PREFIX: &str = "condition:";

/// Redis key prefix for market slug mappings: market:{platform}:{market_slug}
pub const MARKET_KEY_PREFIX: &str = "market:";

/// Mapping from condition_id to hashed_market_id.
/// Stored when aggregator receives first WebSocket message for a market.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConditionMapping {
    /// The original condition_id (hex string like "0xe93c89...")
    pub condition_id: String,
    /// SHA256 hash of condition_id, truncated to 16 chars
    pub hashed_market_id: String,
    /// List of clob_token_ids associated with this market
    pub clob_token_ids: Vec<String>,
}

/// Mapping from market slug to hashed_market_id.
/// Used to look up markets by their human-readable slug.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketSlugMapping {
    /// The market's slug (e.g., "fed-decreases-interest-rates")
    pub market_slug: String,
    /// SHA256 hash of condition_id, truncated to 16 chars
    pub hashed_market_id: String,
    /// The original condition_id
    pub condition_id: String,
    /// Numeric market ID from platform API (e.g., "601697")
    pub market_id: String,
}

/// Shared Redis client wrapper for event and market data operations.
#[derive(Clone)]
pub struct SharedRedisClient {
    client: Arc<redis::Client>,
}

impl SharedRedisClient {
    /// Create a new shared Redis client.
    pub fn new(redis_url: &str) -> Result<Self> {
        let client = redis::Client::open(redis_url)?;
        Ok(Self {
            client: Arc::new(client),
        })
    }

    /// Get an async connection.
    pub async fn get_connection(&self) -> Result<redis::aio::MultiplexedConnection> {
        let conn = self.client.get_multiplexed_async_connection().await?;
        Ok(conn)
    }

    // =========================================================================
    // Event Operations
    // =========================================================================

    /// Store event data in Redis with platform prefix.
    /// Key format: event:{platform}:{slug}
    pub async fn store_event(
        &self,
        platform: &str,
        event_slug: &str,
        data: &EventData,
    ) -> Result<()> {
        let mut conn = self.get_connection().await?;
        let key = format!("{}{}:{}", EVENT_KEY_PREFIX, platform, event_slug);
        let json = serde_json::to_string(data)?;

        conn.set::<_, _, ()>(&key, &json).await?;
        info!(
            "Stored event '{}:{}' in Redis with {} tokens",
            platform,
            event_slug,
            data.tokens.len()
        );

        Ok(())
    }

    /// Get event data from Redis with platform prefix.
    /// Key format: event:{platform}:{slug}
    pub async fn get_event(&self, platform: &str, event_slug: &str) -> Result<Option<EventData>> {
        let mut conn = self.get_connection().await?;
        let key = format!("{}{}:{}", EVENT_KEY_PREFIX, platform, event_slug);

        let json: Option<String> = conn.get(&key).await?;

        match json {
            Some(j) => {
                let data: EventData = serde_json::from_str(&j)?;
                debug!("Retrieved event '{}:{}' from Redis", platform, event_slug);
                Ok(Some(data))
            }
            None => Ok(None),
        }
    }

    /// Get token mappings for an event.
    pub async fn get_token_mappings(
        &self,
        platform: &str,
        event_slug: &str,
    ) -> Result<Vec<TokenMapping>> {
        let event = self.get_event(platform, event_slug).await?;
        Ok(event.map(|e| e.tokens).unwrap_or_default())
    }

    /// Check if event exists in Redis.
    pub async fn event_exists(&self, platform: &str, event_slug: &str) -> Result<bool> {
        let mut conn = self.get_connection().await?;
        let key = format!("{}{}:{}", EVENT_KEY_PREFIX, platform, event_slug);

        let exists: bool = conn.exists(&key).await?;
        Ok(exists)
    }

    /// List all stored events (returns platform:slug pairs).
    pub async fn list_events(&self) -> Result<Vec<String>> {
        let mut conn = self.get_connection().await?;
        let pattern = format!("{}*", EVENT_KEY_PREFIX);

        let keys: Vec<String> = conn.keys(&pattern).await?;
        let slugs: Vec<String> = keys
            .into_iter()
            .map(|k| k.strip_prefix(EVENT_KEY_PREFIX).unwrap_or(&k).to_string())
            .collect();

        Ok(slugs)
    }

    // =========================================================================
    // Market Metadata Operations
    // =========================================================================

    /// Find market data by market_id across all events for a platform.
    /// Returns (event_slug, market_data) if found.
    pub async fn find_market_by_id(
        &self,
        platform: &str,
        market_id: &str,
    ) -> Result<Option<(String, MarketData)>> {
        let mut conn = self.get_connection().await?;
        let pattern = format!("{}{}:*", EVENT_KEY_PREFIX, platform);

        let keys: Vec<String> = conn.keys(&pattern).await.unwrap_or_default();

        for key in keys {
            let json: Option<String> = conn.get(&key).await.ok().flatten();
            if let Some(json_str) = json {
                if let Ok(event_data) = serde_json::from_str::<EventData>(&json_str) {
                    for market in &event_data.markets {
                        if market.id == market_id {
                            let slug = key
                                .strip_prefix(&format!("{}{}:", EVENT_KEY_PREFIX, platform))
                                .map(|s| s.to_string())
                                .unwrap_or_default();
                            return Ok(Some((slug, market.clone())));
                        }
                    }
                }
            }
        }

        Ok(None)
    }

    /// Find market data by token ID (asset_id/clob_token_id) across all events for a platform.
    /// Returns (event_slug, market_data) if found.
    pub async fn find_market_by_token_id(
        &self,
        platform: &str,
        token_id: &str,
    ) -> Result<Option<(String, MarketData)>> {
        let mut conn = self.get_connection().await?;
        let pattern = format!("{}{}:*", EVENT_KEY_PREFIX, platform);

        let keys: Vec<String> = conn.keys(&pattern).await.unwrap_or_default();

        for key in keys {
            let json: Option<String> = conn.get(&key).await.ok().flatten();
            if let Some(json_str) = json {
                if let Ok(event_data) = serde_json::from_str::<EventData>(&json_str) {
                    // Find the token mapping for this token_id
                    let token_mapping = event_data.tokens.iter().find(|t| t.clob_token_id == token_id);

                    if let Some(mapping) = token_mapping {
                        // Find the market with this market_id
                        if let Some(market) = event_data.markets.iter().find(|m| m.id == mapping.market_id) {
                            let slug = key
                                .strip_prefix(&format!("{}{}:", EVENT_KEY_PREFIX, platform))
                                .map(|s| s.to_string())
                                .unwrap_or_default();
                            return Ok(Some((slug, market.clone())));
                        }
                    }
                }
            }
        }

        Ok(None)
    }

    // =========================================================================
    // Condition ID Mapping Operations
    // =========================================================================

    /// Store condition_id to hashed_market_id mapping.
    /// Key format: condition:{platform}:{condition_id}
    /// This is populated by the aggregator when it receives the first WS message for a market.
    pub async fn store_condition_mapping(
        &self,
        platform: &str,
        condition_id: &str,
        mapping: &ConditionMapping,
    ) -> Result<()> {
        let mut conn = self.get_connection().await?;
        let key = format!("condition:{}:{}", platform, condition_id);
        let json = serde_json::to_string(mapping)?;

        conn.set::<_, _, ()>(&key, &json).await?;
        debug!(
            "Stored condition mapping {}:{} -> hashed_market_id={}",
            platform, condition_id, mapping.hashed_market_id
        );

        Ok(())
    }

    /// Get condition mapping by condition_id.
    pub async fn get_condition_mapping(
        &self,
        platform: &str,
        condition_id: &str,
    ) -> Result<Option<ConditionMapping>> {
        let mut conn = self.get_connection().await?;
        let key = format!("condition:{}:{}", platform, condition_id);

        let json: Option<String> = conn.get(&key).await?;

        match json {
            Some(j) => {
                let data: ConditionMapping = serde_json::from_str(&j)?;
                Ok(Some(data))
            }
            None => Ok(None),
        }
    }

    /// Check if condition mapping exists.
    pub async fn condition_mapping_exists(
        &self,
        platform: &str,
        condition_id: &str,
    ) -> Result<bool> {
        let mut conn = self.get_connection().await?;
        let key = format!("condition:{}:{}", platform, condition_id);

        let exists: bool = conn.exists(&key).await?;
        Ok(exists)
    }

    // =========================================================================
    // Market Slug Mapping Operations
    // =========================================================================

    /// Store market slug to hashed_market_id mapping.
    /// Key format: market:{platform}:{market_slug}
    pub async fn store_market_slug_mapping(
        &self,
        platform: &str,
        market_slug: &str,
        mapping: &MarketSlugMapping,
    ) -> Result<()> {
        let mut conn = self.get_connection().await?;
        let key = format!("market:{}:{}", platform, market_slug);
        let json = serde_json::to_string(mapping)?;

        conn.set::<_, _, ()>(&key, &json).await?;
        debug!(
            "Stored market slug mapping {}:{} -> hashed_market_id={}",
            platform, market_slug, mapping.hashed_market_id
        );

        Ok(())
    }

    /// Get market slug mapping.
    pub async fn get_market_by_slug(
        &self,
        platform: &str,
        market_slug: &str,
    ) -> Result<Option<MarketSlugMapping>> {
        let mut conn = self.get_connection().await?;
        let key = format!("market:{}:{}", platform, market_slug);

        let json: Option<String> = conn.get(&key).await?;

        match json {
            Some(j) => {
                let data: MarketSlugMapping = serde_json::from_str(&j)?;
                Ok(Some(data))
            }
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_prefixes() {
        assert_eq!(EVENT_KEY_PREFIX, "event:");
        assert_eq!(CONDITION_KEY_PREFIX, "condition:");
        assert_eq!(MARKET_KEY_PREFIX, "market:");
    }
}
