//! Local BBO cache updated via NATS subscription.
//!
//! The BboCache subscribes to `orderbook.changes.>` from the orderbook_service
//! and maintains a local cache of best bid/ask prices per market/asset/platform.
//!
//! This provides ~0μs lookup latency for routing decisions.

use dashmap::DashMap;
use futures::StreamExt;
use rust_decimal::Decimal;
use serde::Deserialize;
use std::str::FromStr;
use std::sync::Arc;
use tracing::{debug, warn};

use crate::error::{Error, Result};

/// Cached BBO per market/asset.
#[derive(Debug, Clone, Default)]
pub struct CachedBbo {
    /// Platform -> best bid price.
    pub best_bids: DashMap<String, Decimal>,
    /// Platform -> best ask price.
    pub best_asks: DashMap<String, Decimal>,
}

/// Local BBO cache updated via NATS subscription.
///
/// Zero network latency for routing decisions.
pub struct BboCache {
    /// (market_id, asset_id) -> CachedBbo
    cache: DashMap<(String, String), CachedBbo>,
}

impl BboCache {
    /// Create a new empty cache.
    pub fn new() -> Self {
        Self {
            cache: DashMap::new(),
        }
    }

    /// Start subscription to orderbook changes.
    ///
    /// This spawns a background task that processes incoming messages
    /// and updates the local cache.
    pub async fn start_subscription(
        self: Arc<Self>,
        nats_client: nats_client::NatsClient,
    ) -> Result<()> {
        let subscriber = nats_client
            .subscribe("orderbook.changes.>")
            .await
            .map_err(|e| Error::Nats(format!("Failed to subscribe: {}", e)))?;

        let cache = self.clone();
        tokio::spawn(async move {
            let mut subscriber = subscriber;
            while let Some(msg) = subscriber.next().await {
                if let Err(e) = cache.handle_change(&msg.payload) {
                    debug!("Failed to parse orderbook change: {}", e);
                }
            }
            warn!("BBO cache subscription ended");
        });

        Ok(())
    }

    /// Handle incoming orderbook change message.
    fn handle_change(&self, payload: &[u8]) -> Result<()> {
        let change: OrderbookChange = serde_json::from_slice(payload)?;

        let key = (change.market_id.clone(), change.asset_id.clone());
        let entry = self.cache.entry(key).or_default();

        // Process each price level change
        for price_change in &change.changes {
            let price = Decimal::from_str(&price_change.price)
                .map_err(Error::DecimalParse)?;

            // Update BBO for each platform at this price level
            for platform_size in &price_change.platforms {
                let size = Decimal::from_str(&platform_size.size)
                    .map_err(Error::DecimalParse)?;

                match price_change.side.as_str() {
                    "buy" => {
                        if size.is_zero() {
                            // Level removed - need to check if this was the best
                            if let Some(current_best) = entry.best_bids.get(&platform_size.platform) {
                                if *current_best == price {
                                    // This was the best bid, remove it
                                    // Note: In a production system, we'd need to track all levels
                                    // to find the new best. For simplicity, we just remove it.
                                    entry.best_bids.remove(&platform_size.platform);
                                }
                            }
                        } else {
                            // Update best bid if this is higher
                            entry
                                .best_bids
                                .entry(platform_size.platform.clone())
                                .and_modify(|current| {
                                    if price > *current {
                                        *current = price;
                                    }
                                })
                                .or_insert(price);
                        }
                    }
                    "sell" => {
                        if size.is_zero() {
                            // Level removed
                            if let Some(current_best) = entry.best_asks.get(&platform_size.platform) {
                                if *current_best == price {
                                    entry.best_asks.remove(&platform_size.platform);
                                }
                            }
                        } else {
                            // Update best ask if this is lower
                            entry
                                .best_asks
                                .entry(platform_size.platform.clone())
                                .and_modify(|current| {
                                    if price < *current {
                                        *current = price;
                                    }
                                })
                                .or_insert(price);
                        }
                    }
                    _ => {}
                }
            }
        }

        Ok(())
    }

    /// Get best bid across all platforms (highest).
    ///
    /// Returns the platform name and price.
    pub fn best_bid(&self, market_id: &str, asset_id: &str) -> Option<(String, Decimal)> {
        let key = (market_id.to_string(), asset_id.to_string());
        self.cache.get(&key).and_then(|entry| {
            entry
                .best_bids
                .iter()
                .max_by_key(|e| *e.value())
                .map(|e| (e.key().clone(), *e.value()))
        })
    }

    /// Get best ask across all platforms (lowest).
    ///
    /// Returns the platform name and price.
    pub fn best_ask(&self, market_id: &str, asset_id: &str) -> Option<(String, Decimal)> {
        let key = (market_id.to_string(), asset_id.to_string());
        self.cache.get(&key).and_then(|entry| {
            entry
                .best_asks
                .iter()
                .min_by_key(|e| *e.value())
                .map(|e| (e.key().clone(), *e.value()))
        })
    }

    /// Get BBO for a specific platform.
    pub fn get_platform_bbo(
        &self,
        market_id: &str,
        asset_id: &str,
        platform: &str,
    ) -> Option<(Option<Decimal>, Option<Decimal>)> {
        let key = (market_id.to_string(), asset_id.to_string());
        self.cache.get(&key).map(|entry| {
            let bid = entry.best_bids.get(platform).map(|v| *v);
            let ask = entry.best_asks.get(platform).map(|v| *v);
            (bid, ask)
        })
    }

    /// Get number of cached markets.
    pub fn market_count(&self) -> usize {
        self.cache.len()
    }
}

impl Default for BboCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Orderbook change message from NATS.
#[derive(Debug, Deserialize)]
struct OrderbookChange {
    market_id: String,
    asset_id: String,
    changes: Vec<PriceChange>,
}

/// Price level change.
#[derive(Debug, Deserialize)]
struct PriceChange {
    side: String,
    price: String,
    #[allow(dead_code)]
    total_size: String,
    platforms: Vec<PlatformSize>,
}

/// Platform size at a price level.
#[derive(Debug, Deserialize)]
struct PlatformSize {
    platform: String,
    size: String,
}
