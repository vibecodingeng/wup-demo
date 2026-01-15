//! In-memory orderbook with sorted price levels and dual BBO tracking.
//!
//! Supports multi-platform aggregation: multiple platforms can have orders
//! at the same price level, and each platform's data is tracked separately.

use normalizer::schema::{NormalizedOrderbook, OrderbookMessageType, OrderbookUpdate, Side};
use rust_decimal::Decimal;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use std::str::FromStr;

// ============================================================================
// Platform-aware Size Tracking
// ============================================================================

/// Tracks order sizes from multiple platforms at a single price level.
///
/// When multiple platforms (e.g., polymarket, kalshi) have orders at the same
/// price, each platform's size is stored separately. The total size at the
/// price level is the sum of all platform sizes.
#[derive(Debug, Clone, Default)]
pub struct PlatformSizes {
    /// Map of platform name to size at this price level.
    sizes: HashMap<String, Decimal>,
}

impl PlatformSizes {
    /// Create new empty platform sizes.
    pub fn new() -> Self {
        Self {
            sizes: HashMap::new(),
        }
    }

    /// Set the size for a specific platform.
    ///
    /// - If size is zero, removes the platform from this price level.
    /// - Otherwise, sets/updates the platform's size.
    pub fn set(&mut self, platform: &str, size: Decimal) {
        if size.is_zero() {
            self.sizes.remove(platform);
        } else {
            self.sizes.insert(platform.to_string(), size);
        }
    }

    /// Get the size for a specific platform.
    pub fn get(&self, platform: &str) -> Option<Decimal> {
        self.sizes.get(platform).copied()
    }

    /// Get total size across all platforms.
    pub fn total_size(&self) -> Decimal {
        self.sizes.values().copied().sum()
    }

    /// Check if this price level has no orders from any platform.
    pub fn is_empty(&self) -> bool {
        self.sizes.is_empty()
    }

    /// Get all platforms with orders at this price level.
    pub fn platforms(&self) -> Vec<&String> {
        self.sizes.keys().collect()
    }

    /// Iterate over all (platform, size) pairs.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &Decimal)> {
        self.sizes.iter()
    }

    /// Clear all sizes for a specific platform across this level.
    pub fn clear_platform(&mut self, platform: &str) {
        self.sizes.remove(platform);
    }
}

/// Represents a single orderbook for one asset.
///
/// Uses BTreeMap for O(log n) price level operations with automatic sorting.
/// - Bids: Sorted descending (highest bid first)
/// - Asks: Sorted ascending (lowest ask first)
///
/// Supports multi-platform aggregation: multiple platforms can have orders
/// at the same price level. When a snapshot is applied, only that platform's
/// orders are replaced; other platforms' orders remain intact.
///
/// Tracks dual BBO (Best Bid/Offer):
/// - Exchange-reported: from normalized message (per platform)
/// - System-calculated: derived from orderbook after every change
///
/// **Note**: This struct is designed for order matching and execution.
/// Metadata (questions, slugs, event details) are stored separately in caches
/// and only included in API responses.
#[derive(Debug, Clone, Default)]
pub struct Orderbook {
    /// CLOB token identifier (the tradeable token/outcome).
    pub clob_token_id: String,
    /// Market identifier (condition_id on Polymarket).
    pub market_id: String,

    /// Bid side: price -> platform_sizes (sorted by price).
    bids: BTreeMap<Decimal, PlatformSizes>,
    /// Ask side: price -> platform_sizes (sorted by price).
    asks: BTreeMap<Decimal, PlatformSizes>,

    /// Platforms that have contributed data to this orderbook.
    pub platforms: Vec<String>,

    /// Exchange-reported best bid per platform.
    pub exchange_best_bid: HashMap<String, String>,
    /// Exchange-reported best ask per platform.
    pub exchange_best_ask: HashMap<String, String>,

    /// System-calculated best bid (highest bid in orderbook).
    pub system_best_bid: Option<Decimal>,
    /// System-calculated best ask (lowest ask in orderbook).
    pub system_best_ask: Option<Decimal>,

    /// Last update timestamp from exchange.
    pub last_exchange_timestamp: String,
    /// Last update received_at timestamp.
    pub last_received_at: i64,
    /// Count of updates applied.
    pub update_count: u64,
}

impl Orderbook {
    /// Create a new empty orderbook.
    pub fn new(clob_token_id: String, market_id: String) -> Self {
        Self {
            clob_token_id,
            market_id,
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
            platforms: Vec::new(),
            exchange_best_bid: HashMap::new(),
            exchange_best_ask: HashMap::new(),
            system_best_bid: None,
            system_best_ask: None,
            last_exchange_timestamp: String::new(),
            last_received_at: 0,
            update_count: 0,
        }
    }

    /// Apply a normalized orderbook message (snapshot or delta).
    ///
    /// Platform-aware: only affects the platform specified in the message.
    pub fn apply(&mut self, msg: &NormalizedOrderbook) {
        let platform = &msg.platform;

        // Track this platform if not already known
        if !self.platforms.contains(platform) {
            self.platforms.push(platform.clone());
        }

        // Update exchange-reported BBO for this platform
        if let Some(ref bid) = msg.best_bid {
            self.exchange_best_bid.insert(platform.clone(), bid.clone());
        }
        if let Some(ref ask) = msg.best_ask {
            self.exchange_best_ask.insert(platform.clone(), ask.clone());
        }

        // Apply price level changes
        match msg.message_type {
            OrderbookMessageType::Snapshot => self.apply_snapshot_platform(msg, platform),
            OrderbookMessageType::Delta => self.apply_delta_platform(msg, platform),
        }

        // Recalculate system BBO after changes
        self.recalculate_system_bbo();

        // Clean up empty price levels
        self.cleanup_empty_levels();

        // Update metadata
        self.last_exchange_timestamp = msg.exchange_timestamp.clone();
        self.last_received_at = msg.received_at;
        self.update_count += 1;
    }

    /// Replace this platform's data with snapshot data.
    ///
    /// Only clears and replaces data for the specified platform.
    /// Other platforms' orders remain intact.
    fn apply_snapshot_platform(&mut self, msg: &NormalizedOrderbook, platform: &str) {
        // Clear only this platform's orders from bids
        for sizes in self.bids.values_mut() {
            sizes.clear_platform(platform);
        }
        // Clear only this platform's orders from asks
        for sizes in self.asks.values_mut() {
            sizes.clear_platform(platform);
        }

        // Add new entries for this platform
        if let Some(ref bids) = msg.bids {
            for level in bids {
                self.insert_level_platform(&level.price, &level.size, true, platform);
            }
        }

        if let Some(ref asks) = msg.asks {
            for level in asks {
                self.insert_level_platform(&level.price, &level.size, false, platform);
            }
        }
    }

    /// Apply incremental delta updates for a specific platform.
    fn apply_delta_platform(&mut self, msg: &NormalizedOrderbook, platform: &str) {
        if let Some(ref updates) = msg.updates {
            for update in updates {
                self.apply_update_platform(update, platform);
            }
        }
    }

    /// Apply a single update for a specific platform.
    ///
    /// Delta updates replace the size at the given price level for this platform.
    /// - Size > 0: set or update the platform's size at this price
    /// - Size = 0: remove the platform from this price level
    fn apply_update_platform(&mut self, update: &OrderbookUpdate, platform: &str) {
        let is_bid = matches!(update.side, Side::Buy);

        let price = match Decimal::from_str(&update.price) {
            Ok(p) => p,
            Err(_) => return,
        };
        let size = match Decimal::from_str(&update.size) {
            Ok(s) => s,
            Err(_) => return,
        };

        let book = if is_bid { &mut self.bids } else { &mut self.asks };

        // Get or create PlatformSizes at this price level
        let platform_sizes = book.entry(price).or_insert_with(PlatformSizes::new);
        platform_sizes.set(platform, size);
    }

    /// Insert a price level for a specific platform.
    fn insert_level_platform(&mut self, price_str: &str, size_str: &str, is_bid: bool, platform: &str) {
        let price = match Decimal::from_str(price_str) {
            Ok(p) => p,
            Err(_) => return,
        };
        let size = match Decimal::from_str(size_str) {
            Ok(s) => s,
            Err(_) => return,
        };

        if !size.is_zero() {
            let book = if is_bid { &mut self.bids } else { &mut self.asks };
            let platform_sizes = book.entry(price).or_insert_with(PlatformSizes::new);
            platform_sizes.set(platform, size);
        }
    }

    /// Remove empty price levels (no orders from any platform).
    fn cleanup_empty_levels(&mut self) {
        self.bids.retain(|_, sizes| !sizes.is_empty());
        self.asks.retain(|_, sizes| !sizes.is_empty());
    }

    /// Recalculate system best bid/ask from the orderbook.
    fn recalculate_system_bbo(&mut self) {
        // Best bid = highest price in bids with non-empty sizes (last in BTreeMap)
        self.system_best_bid = self.bids.iter().rev()
            .find(|(_, sizes)| !sizes.is_empty())
            .map(|(price, _)| *price);
        // Best ask = lowest price in asks with non-empty sizes (first in BTreeMap)
        self.system_best_ask = self.asks.iter()
            .find(|(_, sizes)| !sizes.is_empty())
            .map(|(price, _)| *price);
    }

    /// Get mid price if both sides have liquidity.
    pub fn mid_price(&self) -> Option<Decimal> {
        match (self.system_best_bid, self.system_best_ask) {
            (Some(bid), Some(ask)) => Some((bid + ask) / Decimal::from(2)),
            _ => None,
        }
    }

    /// Get spread if both sides have liquidity.
    pub fn spread(&self) -> Option<Decimal> {
        match (self.system_best_bid, self.system_best_ask) {
            (Some(bid), Some(ask)) => Some(ask - bid),
            _ => None,
        }
    }

    /// Get top N bid levels with aggregated sizes (sorted descending by price).
    pub fn top_bids(&self, n: usize) -> Vec<(Decimal, Decimal)> {
        self.bids.iter().rev()
            .filter(|(_, sizes)| !sizes.is_empty())
            .take(n)
            .map(|(p, s)| (*p, s.total_size()))
            .collect()
    }

    /// Get top N ask levels with aggregated sizes (sorted ascending by price).
    pub fn top_asks(&self, n: usize) -> Vec<(Decimal, Decimal)> {
        self.asks.iter()
            .filter(|(_, sizes)| !sizes.is_empty())
            .take(n)
            .map(|(p, s)| (*p, s.total_size()))
            .collect()
    }

    /// Get top N bid levels with platform breakdown (sorted descending by price).
    pub fn top_bids_with_platforms(&self, n: usize) -> Vec<AggregatedPriceLevel> {
        self.bids.iter().rev()
            .filter(|(_, sizes)| !sizes.is_empty())
            .take(n)
            .map(|(price, sizes)| AggregatedPriceLevel {
                price: price.to_string(),
                total_size: sizes.total_size().to_string(),
                platforms: sizes.iter()
                    .map(|(platform, size)| PlatformEntry {
                        platform: platform.clone(),
                        size: size.to_string(),
                    })
                    .collect(),
            })
            .collect()
    }

    /// Get top N ask levels with platform breakdown (sorted ascending by price).
    pub fn top_asks_with_platforms(&self, n: usize) -> Vec<AggregatedPriceLevel> {
        self.asks.iter()
            .filter(|(_, sizes)| !sizes.is_empty())
            .take(n)
            .map(|(price, sizes)| AggregatedPriceLevel {
                price: price.to_string(),
                total_size: sizes.total_size().to_string(),
                platforms: sizes.iter()
                    .map(|(platform, size)| PlatformEntry {
                        platform: platform.clone(),
                        size: size.to_string(),
                    })
                    .collect(),
            })
            .collect()
    }

    /// Get aggregated bid level at a specific price.
    /// Returns None if no orders at that price.
    pub fn get_aggregated_bid_level(&self, price: &str) -> Option<AggregatedPriceLevel> {
        let price_decimal = Decimal::from_str(price).ok()?;
        self.bids.get(&price_decimal).and_then(|sizes| {
            if sizes.is_empty() {
                None
            } else {
                Some(AggregatedPriceLevel {
                    price: price_decimal.to_string(),
                    total_size: sizes.total_size().to_string(),
                    platforms: sizes.iter()
                        .map(|(platform, size)| PlatformEntry {
                            platform: platform.clone(),
                            size: size.to_string(),
                        })
                        .collect(),
                })
            }
        })
    }

    /// Get aggregated ask level at a specific price.
    /// Returns None if no orders at that price.
    pub fn get_aggregated_ask_level(&self, price: &str) -> Option<AggregatedPriceLevel> {
        let price_decimal = Decimal::from_str(price).ok()?;
        self.asks.get(&price_decimal).and_then(|sizes| {
            if sizes.is_empty() {
                None
            } else {
                Some(AggregatedPriceLevel {
                    price: price_decimal.to_string(),
                    total_size: sizes.total_size().to_string(),
                    platforms: sizes.iter()
                        .map(|(platform, size)| PlatformEntry {
                            platform: platform.clone(),
                            size: size.to_string(),
                        })
                        .collect(),
                })
            }
        })
    }

    /// Total bid depth (sum of all bid sizes across all platforms).
    pub fn bid_depth(&self) -> Decimal {
        self.bids.values().map(|s| s.total_size()).sum()
    }

    /// Total ask depth (sum of all ask sizes across all platforms).
    pub fn ask_depth(&self) -> Decimal {
        self.asks.values().map(|s| s.total_size()).sum()
    }

    /// Number of bid price levels.
    pub fn bid_levels(&self) -> usize {
        self.bids.iter().filter(|(_, s)| !s.is_empty()).count()
    }

    /// Number of ask price levels.
    pub fn ask_levels(&self) -> usize {
        self.asks.iter().filter(|(_, s)| !s.is_empty()).count()
    }

    /// Check if orderbook has any data.
    pub fn is_empty(&self) -> bool {
        self.bids.iter().all(|(_, s)| s.is_empty()) &&
        self.asks.iter().all(|(_, s)| s.is_empty())
    }

    /// Convert to full orderbook API response with platform breakdown.
    ///
    /// If depth is None, returns ALL price levels (no limit).
    /// If depth is Some(n), returns top n price levels.
    ///
    /// Metadata is provided separately from cached stores, keeping the internal
    /// Orderbook struct focused on order matching.
    pub fn to_response(&self, depth: Option<usize>, metadata: &ResponseMetadata) -> OrderbookResponse {
        // When depth is None, return all levels (use usize::MAX as "unlimited")
        let depth = depth.unwrap_or(usize::MAX);
        OrderbookResponse {
            market: MarketInfo {
                market_id: metadata.market_id.clone(),
                questions: metadata.market_questions.clone(),
                slugs: metadata.market_slugs.clone(),
            },
            asset_id: self.clob_token_id.clone(),
            market_id: self.market_id.clone(),
            platforms: self.platforms.clone(),
            bids: self.top_bids_with_platforms(depth),
            asks: self.top_asks_with_platforms(depth),
            exchange_best_bid: self.exchange_best_bid.clone(),
            exchange_best_ask: self.exchange_best_ask.clone(),
            system_best_bid: self.system_best_bid.map(|d| d.to_string()),
            system_best_ask: self.system_best_ask.map(|d| d.to_string()),
            mid_price: self.mid_price().map(|d| d.to_string()),
            spread: self.spread().map(|d| d.to_string()),
            bid_depth: self.bid_depth().to_string(),
            ask_depth: self.ask_depth().to_string(),
            bid_levels: self.bid_levels(),
            ask_levels: self.ask_levels(),
            last_exchange_timestamp: self.last_exchange_timestamp.clone(),
            last_received_at: self.last_received_at,
            update_count: self.update_count,
        }
    }

    /// Convert to BBO (Best Bid/Offer) response.
    pub fn to_bbo_response(&self, metadata: &ResponseMetadata) -> BboResponse {
        BboResponse {
            asset_id: self.clob_token_id.clone(),
            market_id: self.market_id.clone(),
            market: MarketInfo {
                market_id: metadata.market_id.clone(),
                questions: metadata.market_questions.clone(),
                slugs: metadata.market_slugs.clone(),
            },
            exchange_best_bid: self.exchange_best_bid.clone(),
            exchange_best_ask: self.exchange_best_ask.clone(),
            system_best_bid: self.system_best_bid.map(|d| d.to_string()),
            system_best_ask: self.system_best_ask.map(|d| d.to_string()),
            mid_price: self.mid_price().map(|d| d.to_string()),
            spread: self.spread().map(|d| d.to_string()),
            last_received_at: self.last_received_at,
        }
    }

    /// Convert to summary response (for listing).
    /// Metadata is passed for consistency but not currently used in summary.
    pub fn to_summary(&self, _metadata: &ResponseMetadata) -> TokenSummary {
        TokenSummary {
            asset_id: self.clob_token_id.clone(),
            platforms: self.platforms.clone(),
            system_best_bid: self.system_best_bid.map(|d| d.to_string()),
            system_best_ask: self.system_best_ask.map(|d| d.to_string()),
            bid_levels: self.bid_levels(),
            ask_levels: self.ask_levels(),
            update_count: self.update_count,
        }
    }
}

// ============================================================================
// Metadata Types (for API responses only - not stored in Orderbook)
// ============================================================================

/// Event information for API responses.
/// Platform-specific titles/slugs for multi-platform support.
#[derive(Debug, Clone, Default, Serialize)]
pub struct EventInfo {
    /// Platform-specific event titles: {"polymarket": "title", "kalshi": "title"}
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub titles: HashMap<String, String>,
    /// Platform-specific event slugs: {"polymarket": "slug", "kalshi": "slug"}
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub slugs: HashMap<String, String>,
    /// Platform-specific event descriptions: {"polymarket": "desc", "kalshi": "desc"}
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub descriptions: HashMap<String, String>,
}

/// Market information for API responses.
#[derive(Debug, Clone, Default, Serialize)]
pub struct MarketInfo {
    /// Market ID (condition_id from exchange).
    pub market_id: String,
    /// Platform-specific market questions: {"polymarket": "question", "kalshi": "question"}
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub questions: HashMap<String, String>,
    /// Platform-specific market slugs: {"polymarket": "slug", "kalshi": "slug"}
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub slugs: HashMap<String, String>,
}

/// Metadata passed to response generation methods.
/// Separates API metadata from the internal Orderbook struct.
#[derive(Debug, Clone, Default)]
pub struct ResponseMetadata {
    /// Market ID (condition_id from exchange).
    pub market_id: String,
    /// Platform-specific market questions.
    pub market_questions: HashMap<String, String>,
    /// Platform-specific market slugs.
    pub market_slugs: HashMap<String, String>,
}

// ============================================================================
// API Response Types
// ============================================================================

/// Platform entry showing a platform's size at a price level.
#[derive(Debug, Clone, Serialize)]
pub struct PlatformEntry {
    /// Platform name (e.g., "polymarket", "kalshi").
    pub platform: String,
    /// Size from this platform at the price level.
    pub size: String,
}

/// Aggregated price level with platform breakdown.
#[derive(Debug, Clone, Serialize)]
pub struct AggregatedPriceLevel {
    /// Price at this level.
    pub price: String,
    /// Total size across all platforms.
    pub total_size: String,
    /// Breakdown by platform.
    pub platforms: Vec<PlatformEntry>,
}

/// Full orderbook API response with multi-platform support.
#[derive(Debug, Clone, Serialize)]
pub struct OrderbookResponse {
    /// Market details (question, slug per platform).
    pub market: MarketInfo,
    /// Asset identifier (clob_token_id from exchange).
    pub asset_id: String,
    /// Market identifier (condition_id from exchange).
    pub market_id: String,
    /// List of platforms contributing to this orderbook.
    pub platforms: Vec<String>,
    /// Bid levels with platform breakdown.
    pub bids: Vec<AggregatedPriceLevel>,
    /// Ask levels with platform breakdown.
    pub asks: Vec<AggregatedPriceLevel>,
    /// Exchange-reported best bid per platform.
    pub exchange_best_bid: HashMap<String, String>,
    /// Exchange-reported best ask per platform.
    pub exchange_best_ask: HashMap<String, String>,
    /// System-calculated best bid.
    pub system_best_bid: Option<String>,
    /// System-calculated best ask.
    pub system_best_ask: Option<String>,
    /// Mid price between best bid and ask.
    pub mid_price: Option<String>,
    /// Spread between best ask and bid.
    pub spread: Option<String>,
    /// Total bid depth.
    pub bid_depth: String,
    /// Total ask depth.
    pub ask_depth: String,
    /// Number of bid price levels.
    pub bid_levels: usize,
    /// Number of ask price levels.
    pub ask_levels: usize,
    /// Last exchange timestamp.
    pub last_exchange_timestamp: String,
    /// Last received timestamp.
    pub last_received_at: i64,
    /// Total update count.
    pub update_count: u64,
}

/// BBO (Best Bid/Offer) API response.
#[derive(Debug, Clone, Serialize)]
pub struct BboResponse {
    /// Asset identifier (clob_token_id from exchange).
    pub asset_id: String,
    /// Market identifier (condition_id from exchange).
    pub market_id: String,
    /// Market details (question, slug per platform).
    pub market: MarketInfo,
    /// Exchange-reported best bid per platform.
    pub exchange_best_bid: HashMap<String, String>,
    /// Exchange-reported best ask per platform.
    pub exchange_best_ask: HashMap<String, String>,
    pub system_best_bid: Option<String>,
    pub system_best_ask: Option<String>,
    pub mid_price: Option<String>,
    pub spread: Option<String>,
    pub last_received_at: i64,
}

/// Token summary for listing.
#[derive(Debug, Clone, Serialize)]
pub struct TokenSummary {
    /// Asset identifier (clob_token_id from exchange).
    pub asset_id: String,
    /// List of platforms with data for this token.
    pub platforms: Vec<String>,
    pub system_best_bid: Option<String>,
    pub system_best_ask: Option<String>,
    pub bid_levels: usize,
    pub ask_levels: usize,
    pub update_count: u64,
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use normalizer::schema::PriceLevel;

    /// Create a default ResponseMetadata for testing
    fn make_metadata() -> ResponseMetadata {
        ResponseMetadata {
            market_id: "market1".to_string(),
            market_questions: HashMap::new(),
            market_slugs: HashMap::new(),
        }
    }

    fn make_snapshot(platform: &str) -> NormalizedOrderbook {
        NormalizedOrderbook {
            platform: platform.to_string(),
            clob_token_id: "token1".to_string(),
            market_id: "market1".to_string(),
            message_type: OrderbookMessageType::Snapshot,
            bids: Some(vec![
                PriceLevel { price: "0.55".to_string(), size: "100".to_string() },
                PriceLevel { price: "0.54".to_string(), size: "200".to_string() },
            ]),
            asks: Some(vec![
                PriceLevel { price: "0.56".to_string(), size: "150".to_string() },
                PriceLevel { price: "0.57".to_string(), size: "50".to_string() },
            ]),
            updates: None,
            best_bid: Some("0.55".to_string()),
            best_ask: Some("0.56".to_string()),
            exchange_timestamp: "1704067200000".to_string(),
            received_at: 1704067200123,
            normalized_at: "2024-01-01T00:00:00Z".to_string(),
        }
    }

    fn make_delta(platform: &str) -> NormalizedOrderbook {
        NormalizedOrderbook {
            platform: platform.to_string(),
            clob_token_id: "token1".to_string(),
            market_id: "market1".to_string(),
            message_type: OrderbookMessageType::Delta,
            bids: None,
            asks: None,
            updates: Some(vec![
                OrderbookUpdate { side: Side::Buy, price: "0.55".to_string(), size: "150".to_string() },
                OrderbookUpdate { side: Side::Sell, price: "0.58".to_string(), size: "75".to_string() },
            ]),
            best_bid: Some("0.55".to_string()),
            best_ask: Some("0.56".to_string()),
            exchange_timestamp: "1704067201000".to_string(),
            received_at: 1704067201123,
            normalized_at: "2024-01-01T00:00:01Z".to_string(),
        }
    }

    #[test]
    fn test_apply_snapshot() {
        let mut ob = Orderbook::new("token1".to_string(), "market1".to_string());
        ob.apply(&make_snapshot("polymarket"));

        assert_eq!(ob.bid_levels(), 2);
        assert_eq!(ob.ask_levels(), 2);
        assert_eq!(ob.system_best_bid, Some(Decimal::from_str("0.55").unwrap()));
        assert_eq!(ob.system_best_ask, Some(Decimal::from_str("0.56").unwrap()));
        assert_eq!(ob.exchange_best_bid.get("polymarket"), Some(&"0.55".to_string()));
        assert_eq!(ob.exchange_best_ask.get("polymarket"), Some(&"0.56".to_string()));
        assert_eq!(ob.update_count, 1);
        assert_eq!(ob.platforms, vec!["polymarket"]);
    }

    #[test]
    fn test_apply_delta() {
        let mut ob = Orderbook::new("token1".to_string(), "market1".to_string());
        ob.apply(&make_snapshot("polymarket"));
        ob.apply(&make_delta("polymarket"));

        // Bid at 0.55 should now have size 150 (replaced)
        let top_bids = ob.top_bids(1);
        assert_eq!(top_bids[0].1, Decimal::from_str("150").unwrap());

        // New ask at 0.58 should be added
        assert_eq!(ob.ask_levels(), 3);
        assert_eq!(ob.update_count, 2);
    }

    #[test]
    fn test_delta_remove_level() {
        let mut ob = Orderbook::new("token1".to_string(), "market1".to_string());
        ob.apply(&make_snapshot("polymarket"));

        // Remove bid at 0.54 by setting size to 0
        let remove_delta = NormalizedOrderbook {
            platform: "polymarket".to_string(),
            clob_token_id: "token1".to_string(),
            market_id: "market1".to_string(),
            message_type: OrderbookMessageType::Delta,
            bids: None,
            asks: None,
            updates: Some(vec![
                OrderbookUpdate { side: Side::Buy, price: "0.54".to_string(), size: "0".to_string() },
            ]),
            best_bid: Some("0.55".to_string()),
            best_ask: Some("0.56".to_string()),
            exchange_timestamp: "1704067201000".to_string(),
            received_at: 1704067201123,
            normalized_at: "2024-01-01T00:00:01Z".to_string(),
        };

        ob.apply(&remove_delta);
        assert_eq!(ob.bid_levels(), 1);
    }

    #[test]
    fn test_system_bbo_recalculation() {
        let mut ob = Orderbook::new("token1".to_string(), "market1".to_string());
        ob.apply(&make_snapshot("polymarket"));

        // Add a better bid
        let better_bid = NormalizedOrderbook {
            platform: "polymarket".to_string(),
            clob_token_id: "token1".to_string(),
            market_id: "market1".to_string(),
            message_type: OrderbookMessageType::Delta,
            bids: None,
            asks: None,
            updates: Some(vec![
                OrderbookUpdate { side: Side::Buy, price: "0.555".to_string(), size: "50".to_string() },
            ]),
            best_bid: Some("0.55".to_string()), // Exchange still reports old BBO
            best_ask: Some("0.56".to_string()),
            exchange_timestamp: "1704067201000".to_string(),
            received_at: 1704067201123,
            normalized_at: "2024-01-01T00:00:01Z".to_string(),
        };

        ob.apply(&better_bid);

        // System BBO should be updated
        assert_eq!(ob.system_best_bid, Some(Decimal::from_str("0.555").unwrap()));
        // Exchange BBO stays the same
        assert_eq!(ob.exchange_best_bid.get("polymarket"), Some(&"0.55".to_string()));
    }

    #[test]
    fn test_mid_price_and_spread() {
        let mut ob = Orderbook::new("token1".to_string(), "market1".to_string());
        ob.apply(&make_snapshot("polymarket"));

        let mid = ob.mid_price().unwrap();
        let spread = ob.spread().unwrap();

        // Mid = (0.55 + 0.56) / 2 = 0.555
        assert_eq!(mid, Decimal::from_str("0.555").unwrap());
        // Spread = 0.56 - 0.55 = 0.01
        assert_eq!(spread, Decimal::from_str("0.01").unwrap());
    }

    #[test]
    fn test_to_response_with_platforms() {
        let mut ob = Orderbook::new("token1".to_string(), "market1".to_string());
        ob.apply(&make_snapshot("polymarket"));

        let metadata = make_metadata();
        let response = ob.to_response(Some(5), &metadata);
        assert_eq!(response.asset_id, "token1");
        assert_eq!(response.bids.len(), 2);
        assert_eq!(response.asks.len(), 2);
        assert_eq!(response.system_best_bid, Some("0.55".to_string()));
        assert!(response.exchange_best_bid.contains_key("polymarket"));
        assert_eq!(response.platforms, vec!["polymarket"]);
        // Check platform breakdown
        assert_eq!(response.bids[0].platforms.len(), 1);
        assert_eq!(response.bids[0].platforms[0].platform, "polymarket");
    }

    #[test]
    fn test_to_bbo_response() {
        let mut ob = Orderbook::new("token1".to_string(), "market1".to_string());
        ob.apply(&make_snapshot("polymarket"));

        let metadata = make_metadata();
        let bbo = ob.to_bbo_response(&metadata);
        assert_eq!(bbo.exchange_best_bid.get("polymarket"), Some(&"0.55".to_string()));
        assert_eq!(bbo.system_best_bid, Some("0.55".to_string()));
        // Mid price = (0.55 + 0.56) / 2 = 0.555 (Decimal may format with trailing zeros)
        assert!(bbo.mid_price.as_ref().unwrap().starts_with("0.555"));
        // Spread = 0.56 - 0.55 = 0.01
        assert!(bbo.spread.as_ref().unwrap().starts_with("0.01"));
    }

    #[test]
    fn test_multi_platform_aggregation() {
        let mut ob = Orderbook::new("token1".to_string(), "market1".to_string());

        // Apply polymarket snapshot
        ob.apply(&make_snapshot("polymarket"));
        assert_eq!(ob.platforms, vec!["polymarket"]);

        // Apply kalshi snapshot at same prices
        let kalshi_snapshot = NormalizedOrderbook {
            platform: "kalshi".to_string(),
            clob_token_id: "token1".to_string(),
            market_id: "market1".to_string(),
            message_type: OrderbookMessageType::Snapshot,
            bids: Some(vec![
                PriceLevel { price: "0.55".to_string(), size: "50".to_string() },
            ]),
            asks: Some(vec![
                PriceLevel { price: "0.56".to_string(), size: "75".to_string() },
            ]),
            updates: None,
            best_bid: Some("0.55".to_string()),
            best_ask: Some("0.56".to_string()),
            exchange_timestamp: "1704067200000".to_string(),
            received_at: 1704067200123,
            normalized_at: "2024-01-01T00:00:00Z".to_string(),
        };
        ob.apply(&kalshi_snapshot);

        // Should have both platforms
        assert_eq!(ob.platforms.len(), 2);
        assert!(ob.platforms.contains(&"polymarket".to_string()));
        assert!(ob.platforms.contains(&"kalshi".to_string()));

        // Price 0.55 should have total size 150 (100 poly + 50 kalshi)
        let top_bids = ob.top_bids(1);
        assert_eq!(top_bids[0].1, Decimal::from_str("150").unwrap());

        // Check platform breakdown
        let metadata = make_metadata();
        let response = ob.to_response(Some(5), &metadata);
        let bid_55 = &response.bids[0];
        assert_eq!(bid_55.total_size, "150");
        assert_eq!(bid_55.platforms.len(), 2);
    }

    #[test]
    fn test_platform_snapshot_only_replaces_own_data() {
        let mut ob = Orderbook::new("token1".to_string(), "market1".to_string());

        // Apply polymarket snapshot
        ob.apply(&make_snapshot("polymarket"));

        // Apply kalshi snapshot
        let kalshi_snapshot = NormalizedOrderbook {
            platform: "kalshi".to_string(),
            clob_token_id: "token1".to_string(),
            market_id: "market1".to_string(),
            message_type: OrderbookMessageType::Snapshot,
            bids: Some(vec![
                PriceLevel { price: "0.55".to_string(), size: "50".to_string() },
            ]),
            asks: Some(vec![
                PriceLevel { price: "0.56".to_string(), size: "75".to_string() },
            ]),
            updates: None,
            best_bid: Some("0.55".to_string()),
            best_ask: Some("0.56".to_string()),
            exchange_timestamp: "1704067200000".to_string(),
            received_at: 1704067200123,
            normalized_at: "2024-01-01T00:00:00Z".to_string(),
        };
        ob.apply(&kalshi_snapshot);

        // Now apply a NEW polymarket snapshot (should only replace polymarket's data)
        let poly_new_snapshot = NormalizedOrderbook {
            platform: "polymarket".to_string(),
            clob_token_id: "token1".to_string(),
            market_id: "market1".to_string(),
            message_type: OrderbookMessageType::Snapshot,
            bids: Some(vec![
                PriceLevel { price: "0.53".to_string(), size: "300".to_string() },
            ]),
            asks: Some(vec![
                PriceLevel { price: "0.58".to_string(), size: "200".to_string() },
            ]),
            updates: None,
            best_bid: Some("0.53".to_string()),
            best_ask: Some("0.58".to_string()),
            exchange_timestamp: "1704067202000".to_string(),
            received_at: 1704067202123,
            normalized_at: "2024-01-01T00:00:02Z".to_string(),
        };
        ob.apply(&poly_new_snapshot);

        // Kalshi's data at 0.55 should still exist
        let metadata = make_metadata();
        let response = ob.to_response(Some(10), &metadata);

        // Best bid should now be 0.55 (kalshi) since polymarket moved to 0.53
        assert_eq!(ob.system_best_bid, Some(Decimal::from_str("0.55").unwrap()));

        // Total of 2 bid levels: 0.55 (kalshi) and 0.53 (polymarket)
        assert_eq!(ob.bid_levels(), 2);

        // Kalshi still has 50 at 0.55
        let bid_55 = response.bids.iter().find(|b| b.price == "0.55").unwrap();
        assert_eq!(bid_55.total_size, "50");
        assert_eq!(bid_55.platforms.len(), 1);
        assert_eq!(bid_55.platforms[0].platform, "kalshi");
    }
}
