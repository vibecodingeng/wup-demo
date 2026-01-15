//! Lock-free in-memory orderbook storage using DashMap.
//!
//! Provides concurrent read/write access without blocking locks.
//! Structure: market_id -> asset_id -> Orderbook
//!
//! Metadata (market questions/slugs) is stored separately in caches
//! and only included in API responses, keeping Orderbook focused on order matching.

use crate::orderbook::{BboResponse, Orderbook, OrderbookResponse, ResponseMetadata, TokenSummary};
use dashmap::DashMap;
use normalizer::schema::NormalizedOrderbook;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Lock-free storage for orderbooks.
///
/// Uses DashMap for high-throughput concurrent access:
/// - No blocking locks
/// - Sharded internally for reduced contention
/// - Safe concurrent reads and writes
///
/// Storage hierarchy: market_id -> asset_id -> Orderbook
/// Metadata is stored in separate caches, not in the Orderbook struct.
#[derive(Debug, Clone)]
pub struct OrderbookStore {
    inner: Arc<OrderbookStoreInner>,
}

#[derive(Debug)]
struct OrderbookStoreInner {
    /// market_id -> (asset_id -> Orderbook)
    books: DashMap<String, DashMap<String, Orderbook>>,
    /// Cache: market_id -> MarketMetadata
    market_metadata_cache: DashMap<String, MarketMetadata>,
    /// Statistics
    total_snapshots: AtomicU64,
    total_deltas: AtomicU64,
}

/// Cached market metadata.
/// Stores platform-specific market details.
#[derive(Debug, Clone, Serialize, Default)]
pub struct MarketMetadata {
    /// Platform-specific questions: {"polymarket": "question", "kalshi": "question"}
    #[serde(default)]
    pub questions: HashMap<String, String>,
    /// Platform-specific slugs: {"polymarket": "slug", "kalshi": "slug"}
    #[serde(default)]
    pub slugs: HashMap<String, String>,
}

impl OrderbookStore {
    /// Create a new empty store.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(OrderbookStoreInner {
                books: DashMap::new(),
                market_metadata_cache: DashMap::new(),
                total_snapshots: AtomicU64::new(0),
                total_deltas: AtomicU64::new(0),
            }),
        }
    }

    /// Cache market metadata by market_id.
    pub fn cache_market_metadata(&self, market_id: &str, metadata: MarketMetadata) {
        self.inner
            .market_metadata_cache
            .insert(market_id.to_string(), metadata);
    }

    /// Get cached market metadata by market_id.
    pub fn get_cached_market_metadata(&self, market_id: &str) -> Option<MarketMetadata> {
        self.inner
            .market_metadata_cache
            .get(market_id)
            .map(|v| v.clone())
    }

    /// Build ResponseMetadata from cached market metadata.
    fn build_response_metadata(&self, market_id: &str) -> ResponseMetadata {
        let market_meta = self.get_cached_market_metadata(market_id);

        ResponseMetadata {
            market_id: market_id.to_string(),
            market_questions: market_meta
                .as_ref()
                .map(|m| m.questions.clone())
                .unwrap_or_default(),
            market_slugs: market_meta.map(|m| m.slugs).unwrap_or_default(),
        }
    }

    /// Apply a normalized orderbook message.
    ///
    /// Creates market and asset entries if they don't exist.
    /// Uses market_id and clob_token_id (asset_id) directly.
    pub fn apply(&self, msg: &NormalizedOrderbook) {
        // Get or create market entry using market_id directly
        let market_books = self.inner.books.entry(msg.market_id.clone()).or_default();

        // Get or create asset entry and apply update
        let mut orderbook = market_books
            .entry(msg.clob_token_id.clone())
            .or_insert_with(|| Orderbook::new(msg.clob_token_id.clone(), msg.market_id.clone()));

        // Apply the message to the orderbook (price level updates only)
        orderbook.apply(msg);

        // Update stats
        match msg.message_type {
            normalizer::schema::OrderbookMessageType::Snapshot => {
                self.inner.total_snapshots.fetch_add(1, Ordering::Relaxed);
            }
            normalizer::schema::OrderbookMessageType::Delta => {
                self.inner.total_deltas.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Get a specific orderbook by market_id and asset_id.
    pub fn get(
        &self,
        market_id: &str,
        asset_id: &str,
        depth: Option<usize>,
    ) -> Option<OrderbookResponse> {
        let metadata = self.build_response_metadata(market_id);
        self.inner.books.get(market_id).and_then(|market_books| {
            market_books
                .get(asset_id)
                .map(|ob| ob.to_response(depth, &metadata))
        })
    }

    /// Get BBO for a specific asset.
    pub fn get_bbo(&self, market_id: &str, asset_id: &str) -> Option<BboResponse> {
        let metadata = self.build_response_metadata(market_id);
        self.inner.books.get(market_id).and_then(|market_books| {
            market_books
                .get(asset_id)
                .map(|ob| ob.to_bbo_response(&metadata))
        })
    }

    /// Get aggregated bid level at a specific price.
    /// Returns None if the orderbook or price level doesn't exist.
    pub fn get_aggregated_bid_level(
        &self,
        market_id: &str,
        asset_id: &str,
        price: &str,
    ) -> Option<crate::orderbook::AggregatedPriceLevel> {
        self.inner.books.get(market_id).and_then(|market_books| {
            market_books
                .get(asset_id)
                .and_then(|ob| ob.get_aggregated_bid_level(price))
        })
    }

    /// Get aggregated ask level at a specific price.
    /// Returns None if the orderbook or price level doesn't exist.
    pub fn get_aggregated_ask_level(
        &self,
        market_id: &str,
        asset_id: &str,
        price: &str,
    ) -> Option<crate::orderbook::AggregatedPriceLevel> {
        self.inner.books.get(market_id).and_then(|market_books| {
            market_books
                .get(asset_id)
                .and_then(|ob| ob.get_aggregated_ask_level(price))
        })
    }

    /// Get all orderbooks for a market.
    pub fn get_market(
        &self,
        market_id: &str,
        depth: Option<usize>,
    ) -> Option<MarketOrderbooksResponse> {
        let metadata = self.build_response_metadata(market_id);

        self.inner.books.get(market_id).map(|market_books| {
            let assets: Vec<OrderbookResponse> = market_books
                .iter()
                .map(|entry| entry.value().to_response(depth, &metadata))
                .collect();

            MarketOrderbooksResponse {
                market_id: market_id.to_string(),
                questions: metadata.market_questions.clone(),
                slugs: metadata.market_slugs.clone(),
                asset_count: assets.len(),
                assets,
            }
        })
    }

    /// Get all BBOs for a market.
    pub fn get_market_bbos(&self, market_id: &str) -> Option<MarketBbosResponse> {
        let metadata = self.build_response_metadata(market_id);

        self.inner.books.get(market_id).map(|market_books| {
            let bbos: Vec<BboResponse> = market_books
                .iter()
                .map(|entry| entry.value().to_bbo_response(&metadata))
                .collect();

            MarketBbosResponse {
                market_id: market_id.to_string(),
                questions: metadata.market_questions.clone(),
                slugs: metadata.market_slugs.clone(),
                asset_count: bbos.len(),
                bbos,
            }
        })
    }

    /// List all markets and their assets (summary only).
    pub fn list_all(&self) -> AllOrderbooksResponse {
        let markets: Vec<MarketSummary> = self
            .inner
            .books
            .iter()
            .map(|market_entry| {
                let market_id = market_entry.key().clone();
                let metadata = self.build_response_metadata(&market_id);

                let asset_summaries: Vec<TokenSummary> = market_entry
                    .value()
                    .iter()
                    .map(|asset_entry| asset_entry.value().to_summary(&metadata))
                    .collect();

                // Get market metadata from cache
                let market_meta = self.get_cached_market_metadata(&market_id);
                let questions = market_meta
                    .as_ref()
                    .map(|m| m.questions.clone())
                    .unwrap_or_default();
                let slugs = market_meta.map(|m| m.slugs).unwrap_or_default();

                MarketSummary {
                    market_id,
                    questions,
                    slugs,
                    asset_count: asset_summaries.len(),
                    assets: asset_summaries,
                }
            })
            .collect();

        let total_assets: usize = markets.iter().map(|m| m.asset_count).sum();

        AllOrderbooksResponse {
            market_count: markets.len(),
            total_assets,
            total_snapshots: self.inner.total_snapshots.load(Ordering::Relaxed),
            total_deltas: self.inner.total_deltas.load(Ordering::Relaxed),
            markets,
        }
    }

    /// Get store statistics.
    pub fn stats(&self) -> StoreStats {
        let mut total_assets = 0;

        for market in self.inner.books.iter() {
            total_assets += market.value().len();
        }

        StoreStats {
            market_count: self.inner.books.len(),
            total_assets,
            total_snapshots: self.inner.total_snapshots.load(Ordering::Relaxed),
            total_deltas: self.inner.total_deltas.load(Ordering::Relaxed),
        }
    }

    /// Check if store is empty.
    pub fn is_empty(&self) -> bool {
        self.inner.books.is_empty()
    }

    /// Get number of markets.
    pub fn market_count(&self) -> usize {
        self.inner.books.len()
    }
}

impl Default for OrderbookStore {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// API Response Types
// ============================================================================

/// All orderbooks for a specific market.
#[derive(Debug, Clone, Serialize)]
pub struct MarketOrderbooksResponse {
    pub market_id: String,
    /// Platform-specific questions: {"polymarket": "question", "kalshi": "question"}
    pub questions: HashMap<String, String>,
    /// Platform-specific slugs: {"polymarket": "slug", "kalshi": "slug"}
    pub slugs: HashMap<String, String>,
    pub asset_count: usize,
    pub assets: Vec<OrderbookResponse>,
}

/// All BBOs for a specific market.
#[derive(Debug, Clone, Serialize)]
pub struct MarketBbosResponse {
    pub market_id: String,
    /// Platform-specific questions: {"polymarket": "question", "kalshi": "question"}
    pub questions: HashMap<String, String>,
    /// Platform-specific slugs: {"polymarket": "slug", "kalshi": "slug"}
    pub slugs: HashMap<String, String>,
    pub asset_count: usize,
    pub bbos: Vec<BboResponse>,
}

/// Market summary for listing.
#[derive(Debug, Clone, Serialize)]
pub struct MarketSummary {
    pub market_id: String,
    /// Platform-specific questions: {"polymarket": "question", "kalshi": "question"}
    pub questions: HashMap<String, String>,
    /// Platform-specific slugs: {"polymarket": "slug", "kalshi": "slug"}
    pub slugs: HashMap<String, String>,
    pub asset_count: usize,
    pub assets: Vec<TokenSummary>,
}

/// Response listing all orderbooks.
#[derive(Debug, Clone, Serialize)]
pub struct AllOrderbooksResponse {
    pub market_count: usize,
    pub total_assets: usize,
    pub total_snapshots: u64,
    pub total_deltas: u64,
    pub markets: Vec<MarketSummary>,
}

/// Store statistics.
#[derive(Debug, Clone, Serialize)]
pub struct StoreStats {
    pub market_count: usize,
    pub total_assets: usize,
    pub total_snapshots: u64,
    pub total_deltas: u64,
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use normalizer::schema::{OrderbookMessageType, OrderbookUpdate, PriceLevel, Side};

    fn make_snapshot(market_id: &str, asset_id: &str, platform: &str) -> NormalizedOrderbook {
        NormalizedOrderbook {
            platform: platform.to_string(),
            clob_token_id: asset_id.to_string(),
            market_id: market_id.to_string(),
            message_type: OrderbookMessageType::Snapshot,
            bids: Some(vec![PriceLevel {
                price: "0.55".to_string(),
                size: "100".to_string(),
            }]),
            asks: Some(vec![PriceLevel {
                price: "0.56".to_string(),
                size: "150".to_string(),
            }]),
            updates: None,
            best_bid: Some("0.55".to_string()),
            best_ask: Some("0.56".to_string()),
            exchange_timestamp: "1704067200000".to_string(),
            received_at: 1704067200123,
            normalized_at: "2024-01-01T00:00:00Z".to_string(),
        }
    }

    fn make_delta(market_id: &str, asset_id: &str, platform: &str) -> NormalizedOrderbook {
        NormalizedOrderbook {
            platform: platform.to_string(),
            clob_token_id: asset_id.to_string(),
            market_id: market_id.to_string(),
            message_type: OrderbookMessageType::Delta,
            bids: None,
            asks: None,
            updates: Some(vec![OrderbookUpdate {
                side: Side::Buy,
                price: "0.54".to_string(),
                size: "50".to_string(),
            }]),
            best_bid: Some("0.55".to_string()),
            best_ask: Some("0.56".to_string()),
            exchange_timestamp: "1704067201000".to_string(),
            received_at: 1704067201123,
            normalized_at: "2024-01-01T00:00:01Z".to_string(),
        }
    }

    #[test]
    fn test_apply() {
        let store = OrderbookStore::new();
        store.apply(&make_snapshot("market1", "asset1", "polymarket"));

        let ob = store.get("market1", "asset1", None).unwrap();
        assert_eq!(ob.asset_id, "asset1");
        assert_eq!(ob.market_id, "market1");
        assert_eq!(ob.system_best_bid, Some("0.55".to_string()));
    }

    #[test]
    fn test_get_nonexistent() {
        let store = OrderbookStore::new();
        assert!(store.get("market1", "asset1", None).is_none());
    }

    #[test]
    fn test_get_bbo() {
        let store = OrderbookStore::new();
        store.apply(&make_snapshot("market1", "asset1", "polymarket"));

        let bbo = store.get_bbo("market1", "asset1").unwrap();
        assert_eq!(
            bbo.exchange_best_bid.get("polymarket"),
            Some(&"0.55".to_string())
        );
        assert_eq!(bbo.system_best_bid, Some("0.55".to_string()));
    }

    #[test]
    fn test_get_market() {
        let store = OrderbookStore::new();
        store.apply(&make_snapshot("market1", "asset1", "polymarket"));
        store.apply(&make_snapshot("market1", "asset2", "polymarket"));

        let market = store.get_market("market1", None).unwrap();
        assert_eq!(market.market_id, "market1");
        assert_eq!(market.asset_count, 2);
        assert_eq!(market.assets.len(), 2);
    }

    #[test]
    fn test_list_all() {
        let store = OrderbookStore::new();
        store.apply(&make_snapshot("market1", "asset1", "polymarket"));
        store.apply(&make_snapshot("market1", "asset2", "polymarket"));
        store.apply(&make_snapshot("market2", "asset3", "polymarket"));

        let all = store.list_all();
        assert_eq!(all.market_count, 2);
        assert_eq!(all.total_assets, 3);
        assert_eq!(all.total_snapshots, 3);
    }

    #[test]
    fn test_stats() {
        let store = OrderbookStore::new();
        store.apply(&make_snapshot("market1", "asset1", "polymarket"));
        store.apply(&make_delta("market1", "asset1", "polymarket"));

        let stats = store.stats();
        assert_eq!(stats.market_count, 1);
        assert_eq!(stats.total_assets, 1);
        assert_eq!(stats.total_snapshots, 1);
        assert_eq!(stats.total_deltas, 1);
    }

    #[test]
    fn test_market_metadata_cache() {
        let store = OrderbookStore::new();

        // Cache metadata with platform-specific questions and slugs
        let mut questions = HashMap::new();
        questions.insert(
            "polymarket".to_string(),
            "Will it rain tomorrow?".to_string(),
        );

        let mut slugs = HashMap::new();
        slugs.insert(
            "polymarket".to_string(),
            "weather-rain-tomorrow".to_string(),
        );

        store.cache_market_metadata(
            "market1",
            MarketMetadata {
                questions: questions.clone(),
                slugs: slugs.clone(),
            },
        );

        // Retrieve it
        let meta = store.get_cached_market_metadata("market1").unwrap();
        assert_eq!(
            meta.questions.get("polymarket"),
            Some(&"Will it rain tomorrow?".to_string())
        );
        assert_eq!(
            meta.slugs.get("polymarket"),
            Some(&"weather-rain-tomorrow".to_string())
        );
    }

    #[test]
    fn test_multi_platform_same_market() {
        let store = OrderbookStore::new();

        // Apply polymarket data
        store.apply(&make_snapshot("market1", "asset1", "polymarket"));

        // Apply kalshi data to same market/asset
        let kalshi_msg = NormalizedOrderbook {
            platform: "kalshi".to_string(),
            clob_token_id: "asset1".to_string(),
            market_id: "market1".to_string(),
            message_type: OrderbookMessageType::Snapshot,
            bids: Some(vec![PriceLevel {
                price: "0.55".to_string(),
                size: "50".to_string(),
            }]),
            asks: Some(vec![PriceLevel {
                price: "0.56".to_string(),
                size: "75".to_string(),
            }]),
            updates: None,
            best_bid: Some("0.55".to_string()),
            best_ask: Some("0.56".to_string()),
            exchange_timestamp: "1704067200000".to_string(),
            received_at: 1704067200123,
            normalized_at: "2024-01-01T00:00:00Z".to_string(),
        };
        store.apply(&kalshi_msg);

        // Verify both platforms are aggregated
        let ob = store.get("market1", "asset1", None).unwrap();
        assert_eq!(ob.platforms.len(), 2);
        assert!(ob.platforms.contains(&"polymarket".to_string()));
        assert!(ob.platforms.contains(&"kalshi".to_string()));

        // Total size at 0.55 should be 150 (100 poly + 50 kalshi)
        let bid_55 = ob.bids.iter().find(|b| b.price == "0.55").unwrap();
        assert_eq!(bid_55.total_size, "150");
    }

    #[test]
    fn test_concurrent_access() {
        use std::thread;

        let store = OrderbookStore::new();
        let store1 = store.clone();
        let store2 = store.clone();

        let h1 = thread::spawn(move || {
            for i in 0..100 {
                store1.apply(&make_snapshot("market1", &format!("asset{}", i), "polymarket"));
            }
        });

        let h2 = thread::spawn(move || {
            for i in 100..200 {
                store2.apply(&make_snapshot("market1", &format!("asset{}", i), "kalshi"));
            }
        });

        h1.join().unwrap();
        h2.join().unwrap();

        let stats = store.stats();
        assert_eq!(stats.total_assets, 200);
    }
}
