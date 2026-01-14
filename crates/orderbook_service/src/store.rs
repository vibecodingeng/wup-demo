//! Lock-free in-memory orderbook storage using DashMap.
//!
//! Provides concurrent read/write access without blocking locks.
//! Structure: event_aggregate_id -> hashed_market_id -> clob_token_id -> Orderbook
//!
//! Metadata (event details, market questions/slugs) is stored separately in caches
//! and only included in API responses, keeping Orderbook focused on order matching.

use crate::hash_market_id;
use crate::orderbook::{BboResponse, EventInfo, Orderbook, OrderbookResponse, ResponseMetadata, TokenSummary};
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
/// Storage hierarchy: event_aggregate_id -> hashed_market_id -> clob_token_id -> Orderbook
/// Metadata is stored in separate caches, not in the Orderbook struct.
#[derive(Debug, Clone)]
pub struct OrderbookStore {
    inner: Arc<OrderbookStoreInner>,
}

#[derive(Debug)]
struct OrderbookStoreInner {
    /// event_aggregate_id -> (hashed_market_id -> (clob_token_id -> Orderbook))
    books: DashMap<String, DashMap<String, DashMap<String, Orderbook>>>,
    /// Cache: platform:slug -> event_aggregate_id
    aggregate_mapping_cache: DashMap<String, String>,
    /// Cache: event_aggregate_id -> EventMetadata
    event_metadata_cache: DashMap<String, EventMetadata>,
    /// Cache: hashed_market_id -> MarketMetadata
    market_metadata_cache: DashMap<String, MarketMetadata>,
    /// Statistics
    total_snapshots: AtomicU64,
    total_deltas: AtomicU64,
}

/// Cached event metadata.
/// Stores platform-specific event details.
#[derive(Debug, Clone, Serialize, Default)]
pub struct EventMetadata {
    /// Event aggregate ID.
    pub event_aggregate_id: String,
    /// Platform-specific event titles: {"polymarket": "title", "kalshi": "title"}
    #[serde(default)]
    pub titles: HashMap<String, String>,
    /// Platform-specific event slugs: {"polymarket": "slug", "kalshi": "slug"}
    #[serde(default)]
    pub slugs: HashMap<String, String>,
    /// Platform-specific event descriptions: {"polymarket": "desc", "kalshi": "desc"}
    #[serde(default)]
    pub descriptions: HashMap<String, String>,
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
    /// Event aggregate ID (for linking market to event).
    pub event_aggregate_id: String,
    /// Market aggregate ID (for multi-platform market aggregation).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub market_aggregate_id: Option<String>,
}

impl OrderbookStore {
    /// Create a new empty store.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(OrderbookStoreInner {
                books: DashMap::new(),
                aggregate_mapping_cache: DashMap::new(),
                event_metadata_cache: DashMap::new(),
                market_metadata_cache: DashMap::new(),
                total_snapshots: AtomicU64::new(0),
                total_deltas: AtomicU64::new(0),
            }),
        }
    }

    /// Cache an aggregate mapping (platform:slug -> event_aggregate_id).
    pub fn cache_aggregate_mapping(&self, platform: &str, slug: &str, event_aggregate_id: &str) {
        let cache_key = format!("{}:{}", platform, slug);
        self.inner.aggregate_mapping_cache.insert(cache_key, event_aggregate_id.to_string());
    }

    /// Get cached event aggregate ID for a platform slug.
    pub fn get_cached_aggregate_id(&self, platform: &str, slug: &str) -> Option<String> {
        let cache_key = format!("{}:{}", platform, slug);
        self.inner.aggregate_mapping_cache.get(&cache_key).map(|v| v.clone())
    }

    /// Cache event metadata by event_aggregate_id.
    pub fn cache_event_metadata(&self, event_aggregate_id: &str, metadata: EventMetadata) {
        self.inner.event_metadata_cache.insert(event_aggregate_id.to_string(), metadata);
    }

    /// Get cached event metadata by event_aggregate_id.
    pub fn get_cached_event_metadata(&self, event_aggregate_id: &str) -> Option<EventMetadata> {
        self.inner.event_metadata_cache.get(event_aggregate_id).map(|v| v.clone())
    }

    /// Cache market metadata by hashed_market_id.
    pub fn cache_market_metadata(&self, hashed_market_id: &str, metadata: MarketMetadata) {
        self.inner.market_metadata_cache.insert(hashed_market_id.to_string(), metadata);
    }

    /// Get cached market metadata by hashed_market_id.
    pub fn get_cached_market_metadata(&self, hashed_market_id: &str) -> Option<MarketMetadata> {
        self.inner.market_metadata_cache.get(hashed_market_id).map(|v| v.clone())
    }

    /// Build ResponseMetadata from cached event and market metadata.
    fn build_response_metadata(&self, event_aggregate_id: &str, hashed_market_id: &str) -> ResponseMetadata {
        let event_meta = self.get_cached_event_metadata(event_aggregate_id);
        let market_meta = self.get_cached_market_metadata(hashed_market_id);

        ResponseMetadata {
            event: event_meta.map(|e| EventInfo {
                event_aggregate_id: e.event_aggregate_id,
                titles: e.titles,
                slugs: e.slugs,
                descriptions: e.descriptions,
            }),
            hashed_market_id: hashed_market_id.to_string(),
            market_questions: market_meta.as_ref().map(|m| m.questions.clone()).unwrap_or_default(),
            market_slugs: market_meta.map(|m| m.slugs).unwrap_or_default(),
        }
    }

    /// Apply a normalized orderbook message to the specified event aggregate.
    ///
    /// Creates event, market, and token entries if they don't exist.
    /// Uses hashed_market_id (hash of condition_id/market_id) for storage.
    /// Metadata is stored separately in caches, not in the Orderbook struct.
    pub fn apply_with_aggregate(&self, msg: &NormalizedOrderbook, event_aggregate_id: &str) {
        // Hash the market_id (condition_id) to get a shorter key
        let hashed_market_id = hash_market_id(&msg.market_id);

        // Get or create event aggregate entry
        let event_books = self.inner.books
            .entry(event_aggregate_id.to_string())
            .or_insert_with(DashMap::new);

        // Get or create market entry using hashed_market_id
        let market_books = event_books
            .entry(hashed_market_id.clone())
            .or_insert_with(DashMap::new);

        // Get or create token entry and apply update
        let mut orderbook = market_books
            .entry(msg.clob_token_id.clone())
            .or_insert_with(|| {
                Orderbook::new(
                    msg.clob_token_id.clone(),
                    msg.market_id.clone(),
                )
            });

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

    /// Apply a normalized orderbook message (legacy - uses hashed market_id as aggregate).
    ///
    /// For backwards compatibility. Creates entries if they don't exist.
    pub fn apply(&self, msg: &NormalizedOrderbook) {
        // Use hashed market_id as the aggregate_id for backwards compatibility
        let hashed = hash_market_id(&msg.market_id);
        self.apply_with_aggregate(msg, &hashed);
    }

    /// Get a specific orderbook by event_aggregate_id, hashed_market_id and clob_token_id.
    pub fn get(&self, event_aggregate_id: &str, hashed_market_id: &str, clob_token_id: &str, depth: Option<usize>) -> Option<OrderbookResponse> {
        let metadata = self.build_response_metadata(event_aggregate_id, hashed_market_id);
        self.inner.books
            .get(event_aggregate_id)
            .and_then(|aggregate_books| {
                aggregate_books
                    .get(hashed_market_id)
                    .and_then(|market_books| {
                        market_books
                            .get(clob_token_id)
                            .map(|ob| ob.to_response(depth, &metadata))
                    })
            })
    }

    /// Get a specific orderbook by market_id and clob_token_id (legacy - hashes market_id).
    pub fn get_legacy(&self, market_id: &str, clob_token_id: &str, depth: Option<usize>) -> Option<OrderbookResponse> {
        let hashed = hash_market_id(market_id);
        self.get(&hashed, &hashed, clob_token_id, depth)
    }

    /// Get BBO for a specific token.
    pub fn get_bbo(&self, event_aggregate_id: &str, hashed_market_id: &str, clob_token_id: &str) -> Option<BboResponse> {
        let metadata = self.build_response_metadata(event_aggregate_id, hashed_market_id);
        self.inner.books
            .get(event_aggregate_id)
            .and_then(|aggregate_books| {
                aggregate_books
                    .get(hashed_market_id)
                    .and_then(|market_books| {
                        market_books
                            .get(clob_token_id)
                            .map(|ob| ob.to_bbo_response(&metadata))
                    })
            })
    }

    /// Get all orderbooks for an event aggregate.
    pub fn get_aggregate(&self, event_aggregate_id: &str, depth: Option<usize>) -> Option<EventOrderbooksResponse> {
        // Get event metadata for the response
        let event_meta = self.get_cached_event_metadata(event_aggregate_id);

        self.inner.books.get(event_aggregate_id).map(|aggregate_books| {
            let mut markets = Vec::new();

            for market_entry in aggregate_books.iter() {
                let hashed_market_id = market_entry.key().clone();
                let metadata = self.build_response_metadata(event_aggregate_id, &hashed_market_id);

                let tokens: Vec<OrderbookResponse> = market_entry.value()
                    .iter()
                    .map(|entry| entry.value().to_response(depth, &metadata))
                    .collect();

                // Get market metadata for aggregate response
                let market_meta = self.get_cached_market_metadata(&hashed_market_id);
                let questions = market_meta.as_ref().map(|m| m.questions.clone()).unwrap_or_default();
                let slugs = market_meta.map(|m| m.slugs).unwrap_or_default();

                markets.push(MarketOrderbooksResponse {
                    hashed_market_id,
                    questions,
                    slugs,
                    token_count: tokens.len(),
                    tokens,
                });
            }

            EventOrderbooksResponse {
                event_aggregate_id: event_aggregate_id.to_string(),
                event: event_meta.map(|e| EventInfo {
                    event_aggregate_id: e.event_aggregate_id,
                    titles: e.titles,
                    slugs: e.slugs,
                    descriptions: e.descriptions,
                }),
                market_count: markets.len(),
                markets,
            }
        })
    }

    /// Get all orderbooks for a market within an aggregate.
    pub fn get_market(&self, event_aggregate_id: &str, hashed_market_id: &str, depth: Option<usize>) -> Option<MarketOrderbooksResponse> {
        let metadata = self.build_response_metadata(event_aggregate_id, hashed_market_id);

        self.inner.books
            .get(event_aggregate_id)
            .and_then(|aggregate_books| {
                aggregate_books.get(hashed_market_id).map(|market_books| {
                    let tokens: Vec<OrderbookResponse> = market_books
                        .iter()
                        .map(|entry| entry.value().to_response(depth, &metadata))
                        .collect();

                    MarketOrderbooksResponse {
                        hashed_market_id: hashed_market_id.to_string(),
                        questions: metadata.market_questions.clone(),
                        slugs: metadata.market_slugs.clone(),
                        token_count: tokens.len(),
                        tokens,
                    }
                })
            })
    }

    /// Get all BBOs for a market within an aggregate.
    pub fn get_market_bbos(&self, event_aggregate_id: &str, hashed_market_id: &str) -> Option<MarketBbosResponse> {
        let metadata = self.build_response_metadata(event_aggregate_id, hashed_market_id);

        self.inner.books
            .get(event_aggregate_id)
            .and_then(|aggregate_books| {
                aggregate_books.get(hashed_market_id).map(|market_books| {
                    let bbos: Vec<BboResponse> = market_books
                        .iter()
                        .map(|entry| entry.value().to_bbo_response(&metadata))
                        .collect();

                    MarketBbosResponse {
                        hashed_market_id: hashed_market_id.to_string(),
                        questions: metadata.market_questions.clone(),
                        slugs: metadata.market_slugs.clone(),
                        token_count: bbos.len(),
                        bbos,
                    }
                })
            })
    }

    /// List all events, markets, and their tokens (summary only).
    pub fn list_all(&self) -> AllOrderbooksResponse {
        let events: Vec<EventSummary> = self.inner.books
            .iter()
            .map(|event_entry| {
                let event_aggregate_id = event_entry.key().clone();
                let event_meta = self.get_cached_event_metadata(&event_aggregate_id);

                let markets: Vec<MarketSummary> = event_entry.value()
                    .iter()
                    .map(|market_entry| {
                        let hashed_market_id = market_entry.key().clone();
                        let metadata = self.build_response_metadata(&event_aggregate_id, &hashed_market_id);

                        let token_summaries: Vec<TokenSummary> = market_entry.value()
                            .iter()
                            .map(|token_entry| token_entry.value().to_summary(&metadata))
                            .collect();

                        // Get market metadata from cache
                        let market_meta = self.get_cached_market_metadata(&hashed_market_id);
                        let questions = market_meta.as_ref().map(|m| m.questions.clone()).unwrap_or_default();
                        let slugs = market_meta.map(|m| m.slugs).unwrap_or_default();

                        MarketSummary {
                            hashed_market_id,
                            questions,
                            slugs,
                            token_count: token_summaries.len(),
                            tokens: token_summaries,
                        }
                    })
                    .collect();

                EventSummary {
                    event_aggregate_id: event_aggregate_id.clone(),
                    event: event_meta.map(|e| EventInfo {
                        event_aggregate_id: e.event_aggregate_id,
                        titles: e.titles,
                        slugs: e.slugs,
                        descriptions: e.descriptions,
                    }),
                    market_count: markets.len(),
                    token_count: markets.iter().map(|m| m.token_count).sum(),
                    markets,
                }
            })
            .collect();

        let total_tokens: usize = events.iter().map(|e| e.token_count).sum();
        let total_markets: usize = events.iter().map(|e| e.market_count).sum();

        AllOrderbooksResponse {
            event_count: events.len(),
            market_count: total_markets,
            total_tokens,
            total_snapshots: self.inner.total_snapshots.load(Ordering::Relaxed),
            total_deltas: self.inner.total_deltas.load(Ordering::Relaxed),
            events,
        }
    }

    /// Get store statistics.
    pub fn stats(&self) -> StoreStats {
        let mut total_markets = 0;
        let mut total_tokens = 0;

        for event in self.inner.books.iter() {
            total_markets += event.value().len();
            for market in event.value().iter() {
                total_tokens += market.value().len();
            }
        }

        StoreStats {
            event_count: self.inner.books.len(),
            market_count: total_markets,
            total_tokens,
            total_snapshots: self.inner.total_snapshots.load(Ordering::Relaxed),
            total_deltas: self.inner.total_deltas.load(Ordering::Relaxed),
        }
    }

    /// Check if store is empty.
    pub fn is_empty(&self) -> bool {
        self.inner.books.is_empty()
    }

    /// Get number of aggregates.
    pub fn aggregate_count(&self) -> usize {
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
    pub hashed_market_id: String,
    /// Platform-specific questions: {"polymarket": "question", "kalshi": "question"}
    pub questions: HashMap<String, String>,
    /// Platform-specific slugs: {"polymarket": "slug", "kalshi": "slug"}
    pub slugs: HashMap<String, String>,
    pub token_count: usize,
    pub tokens: Vec<OrderbookResponse>,
}

/// All BBOs for a specific market.
#[derive(Debug, Clone, Serialize)]
pub struct MarketBbosResponse {
    pub hashed_market_id: String,
    /// Platform-specific questions: {"polymarket": "question", "kalshi": "question"}
    pub questions: HashMap<String, String>,
    /// Platform-specific slugs: {"polymarket": "slug", "kalshi": "slug"}
    pub slugs: HashMap<String, String>,
    pub token_count: usize,
    pub bbos: Vec<BboResponse>,
}

/// All orderbooks for a specific event aggregate.
#[derive(Debug, Clone, Serialize)]
pub struct EventOrderbooksResponse {
    pub event_aggregate_id: String,
    /// Event details (titles, slugs, descriptions per platform).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event: Option<EventInfo>,
    pub market_count: usize,
    pub markets: Vec<MarketOrderbooksResponse>,
}

/// Market summary for listing.
#[derive(Debug, Clone, Serialize)]
pub struct MarketSummary {
    pub hashed_market_id: String,
    /// Platform-specific questions: {"polymarket": "question", "kalshi": "question"}
    pub questions: HashMap<String, String>,
    /// Platform-specific slugs: {"polymarket": "slug", "kalshi": "slug"}
    pub slugs: HashMap<String, String>,
    pub token_count: usize,
    pub tokens: Vec<TokenSummary>,
}

/// Event summary for listing.
#[derive(Debug, Clone, Serialize)]
pub struct EventSummary {
    pub event_aggregate_id: String,
    /// Event details (titles, slugs, descriptions per platform).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event: Option<EventInfo>,
    pub market_count: usize,
    pub token_count: usize,
    pub markets: Vec<MarketSummary>,
}

/// Response listing all orderbooks.
#[derive(Debug, Clone, Serialize)]
pub struct AllOrderbooksResponse {
    pub event_count: usize,
    pub market_count: usize,
    pub total_tokens: usize,
    pub total_snapshots: u64,
    pub total_deltas: u64,
    pub events: Vec<EventSummary>,
}

/// Store statistics.
#[derive(Debug, Clone, Serialize)]
pub struct StoreStats {
    pub event_count: usize,
    pub market_count: usize,
    pub total_tokens: usize,
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

    fn make_snapshot(market_id: &str, clob_token_id: &str, platform: &str) -> NormalizedOrderbook {
        NormalizedOrderbook {
            exchange: platform.to_string(),
            platform: platform.to_string(),
            clob_token_id: clob_token_id.to_string(),
            market_id: market_id.to_string(),
            message_type: OrderbookMessageType::Snapshot,
            bids: Some(vec![
                PriceLevel { price: "0.55".to_string(), size: "100".to_string(), platform: platform.to_string() },
            ]),
            asks: Some(vec![
                PriceLevel { price: "0.56".to_string(), size: "150".to_string(), platform: platform.to_string() },
            ]),
            updates: None,
            best_bid: Some("0.55".to_string()),
            best_ask: Some("0.56".to_string()),
            exchange_timestamp: "1704067200000".to_string(),
            received_at: 1704067200123,
            normalized_at: "2024-01-01T00:00:00Z".to_string(),
        }
    }

    fn make_delta(market_id: &str, clob_token_id: &str, platform: &str) -> NormalizedOrderbook {
        NormalizedOrderbook {
            exchange: platform.to_string(),
            platform: platform.to_string(),
            clob_token_id: clob_token_id.to_string(),
            market_id: market_id.to_string(),
            message_type: OrderbookMessageType::Delta,
            bids: None,
            asks: None,
            updates: Some(vec![
                OrderbookUpdate { side: Side::Buy, price: "0.54".to_string(), size: "50".to_string() },
            ]),
            best_bid: Some("0.55".to_string()),
            best_ask: Some("0.56".to_string()),
            exchange_timestamp: "1704067201000".to_string(),
            received_at: 1704067201123,
            normalized_at: "2024-01-01T00:00:01Z".to_string(),
        }
    }

    #[test]
    fn test_apply_with_aggregate() {
        let store = OrderbookStore::new();
        store.apply_with_aggregate(&make_snapshot("market1", "token1", "polymarket"), "AGG123");

        let hashed = hash_market_id("market1");
        let ob = store.get("AGG123", &hashed, "token1", None).unwrap();
        assert_eq!(ob.clob_token_id, "token1");
        assert_eq!(ob.market_id, "market1");
        // Event info is now in the `event` field (if cached)
        assert_eq!(ob.market.hashed_market_id, hashed);
        assert_eq!(ob.system_best_bid, Some("0.55".to_string()));
    }

    #[test]
    fn test_apply_legacy() {
        let store = OrderbookStore::new();
        store.apply(&make_snapshot("market1", "token1", "polymarket"));

        // Legacy uses hashed market_id as aggregate_id
        let ob = store.get_legacy("market1", "token1", None).unwrap();
        assert_eq!(ob.clob_token_id, "token1");
        assert_eq!(ob.market_id, "market1");
        assert_eq!(ob.system_best_bid, Some("0.55".to_string()));
    }

    #[test]
    fn test_get_nonexistent() {
        let store = OrderbookStore::new();
        assert!(store.get("AGG123", "somehash", "token1", None).is_none());
    }

    #[test]
    fn test_get_bbo() {
        let store = OrderbookStore::new();
        store.apply_with_aggregate(&make_snapshot("market1", "token1", "polymarket"), "AGG123");

        let hashed = hash_market_id("market1");
        let bbo = store.get_bbo("AGG123", &hashed, "token1").unwrap();
        assert_eq!(bbo.exchange_best_bid.get("polymarket"), Some(&"0.55".to_string()));
        assert_eq!(bbo.system_best_bid, Some("0.55".to_string()));
    }

    #[test]
    fn test_get_aggregate() {
        let store = OrderbookStore::new();
        store.apply_with_aggregate(&make_snapshot("market1", "token1", "polymarket"), "AGG123");
        store.apply_with_aggregate(&make_snapshot("market1", "token2", "polymarket"), "AGG123");
        store.apply_with_aggregate(&make_snapshot("market2", "token3", "polymarket"), "AGG123");

        let event = store.get_aggregate("AGG123", None).unwrap();
        assert_eq!(event.event_aggregate_id, "AGG123");
        assert_eq!(event.market_count, 2);
    }

    #[test]
    fn test_get_market() {
        let store = OrderbookStore::new();
        store.apply_with_aggregate(&make_snapshot("market1", "token1", "polymarket"), "AGG123");
        store.apply_with_aggregate(&make_snapshot("market1", "token2", "polymarket"), "AGG123");

        let hashed = hash_market_id("market1");
        let market = store.get_market("AGG123", &hashed, None).unwrap();
        assert_eq!(market.hashed_market_id, hashed);
        assert_eq!(market.token_count, 2);
        assert_eq!(market.tokens.len(), 2);
    }

    #[test]
    fn test_list_all() {
        let store = OrderbookStore::new();
        store.apply_with_aggregate(&make_snapshot("market1", "token1", "polymarket"), "AGG123");
        store.apply_with_aggregate(&make_snapshot("market1", "token2", "polymarket"), "AGG123");
        store.apply_with_aggregate(&make_snapshot("market1", "token3", "polymarket"), "AGG456");

        let all = store.list_all();
        assert_eq!(all.event_count, 2);
        assert_eq!(all.total_tokens, 3);
        assert_eq!(all.total_snapshots, 3);
    }

    #[test]
    fn test_stats() {
        let store = OrderbookStore::new();
        store.apply_with_aggregate(&make_snapshot("market1", "token1", "polymarket"), "AGG123");
        store.apply_with_aggregate(&make_delta("market1", "token1", "polymarket"), "AGG123");

        let stats = store.stats();
        assert_eq!(stats.event_count, 1);
        assert_eq!(stats.market_count, 1);
        assert_eq!(stats.total_tokens, 1);
        assert_eq!(stats.total_snapshots, 1);
        assert_eq!(stats.total_deltas, 1);
    }

    #[test]
    fn test_aggregate_mapping_cache() {
        let store = OrderbookStore::new();

        // Cache a mapping
        store.cache_aggregate_mapping("polymarket", "us-election", "AGG123");

        // Retrieve it
        let agg_id = store.get_cached_aggregate_id("polymarket", "us-election");
        assert_eq!(agg_id, Some("AGG123".to_string()));

        // Non-existent mapping
        let none = store.get_cached_aggregate_id("kalshi", "unknown");
        assert!(none.is_none());
    }

    #[test]
    fn test_market_metadata_cache() {
        let store = OrderbookStore::new();

        // Cache metadata with platform-specific questions and slugs
        let mut questions = HashMap::new();
        questions.insert("polymarket".to_string(), "Will it rain tomorrow?".to_string());

        let mut slugs = HashMap::new();
        slugs.insert("polymarket".to_string(), "weather-rain-tomorrow".to_string());

        store.cache_market_metadata("market1", MarketMetadata {
            questions: questions.clone(),
            slugs: slugs.clone(),
            event_aggregate_id: "AGG123".to_string(),
            market_aggregate_id: None,
        });

        // Retrieve it
        let meta = store.get_cached_market_metadata("market1").unwrap();
        assert_eq!(meta.questions.get("polymarket"), Some(&"Will it rain tomorrow?".to_string()));
        assert_eq!(meta.slugs.get("polymarket"), Some(&"weather-rain-tomorrow".to_string()));
        assert_eq!(meta.event_aggregate_id, "AGG123");
    }

    #[test]
    fn test_multi_platform_in_same_aggregate() {
        let store = OrderbookStore::new();

        // Apply polymarket data
        store.apply_with_aggregate(&make_snapshot("market1", "token1", "polymarket"), "AGG123");

        // Apply kalshi data to same aggregate/market/token
        let kalshi_msg = NormalizedOrderbook {
            exchange: "kalshi".to_string(),
            platform: "kalshi".to_string(),
            clob_token_id: "token1".to_string(),
            market_id: "market1".to_string(),
            message_type: OrderbookMessageType::Snapshot,
            bids: Some(vec![
                PriceLevel { price: "0.55".to_string(), size: "50".to_string(), platform: "kalshi".to_string() },
            ]),
            asks: Some(vec![
                PriceLevel { price: "0.56".to_string(), size: "75".to_string(), platform: "kalshi".to_string() },
            ]),
            updates: None,
            best_bid: Some("0.55".to_string()),
            best_ask: Some("0.56".to_string()),
            exchange_timestamp: "1704067200000".to_string(),
            received_at: 1704067200123,
            normalized_at: "2024-01-01T00:00:00Z".to_string(),
        };
        store.apply_with_aggregate(&kalshi_msg, "AGG123");

        // Verify both platforms are aggregated
        let hashed = hash_market_id("market1");
        let ob = store.get("AGG123", &hashed, "token1", None).unwrap();
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
                store1.apply_with_aggregate(
                    &make_snapshot("market1", &format!("token{}", i), "polymarket"),
                    "AGG123"
                );
            }
        });

        let h2 = thread::spawn(move || {
            for i in 100..200 {
                store2.apply_with_aggregate(
                    &make_snapshot("market1", &format!("token{}", i), "kalshi"),
                    "AGG123"
                );
            }
        });

        h1.join().unwrap();
        h2.join().unwrap();

        let stats = store.stats();
        assert_eq!(stats.total_tokens, 200);
    }
}
