//! NATS subscription service for consuming normalized orderbook messages.

use crate::store::{MarketMetadata, OrderbookStore};
use anyhow::Result;
use chrono::Utc;
use external_services::SharedRedisClient;
use futures::StreamExt;
use metrics::{counter, gauge};
use nats_client::NatsClient;
use normalizer::schema::NormalizedOrderbook;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

/// Configuration for the orderbook service.
#[derive(Debug, Clone)]
pub struct OrderbookServiceConfig {
    /// NATS subject to subscribe to (e.g., "normalized.polymarket.>").
    pub subject: String,
    /// Metrics update interval in seconds.
    pub metrics_interval_secs: u64,
    /// Redis URL for fetching market metadata.
    pub redis_url: Option<String>,
    /// Enable publishing orderbook changes to NATS for gateway consumption.
    pub publish_changes: bool,
}

impl Default for OrderbookServiceConfig {
    fn default() -> Self {
        Self {
            subject: "normalized.polymarket.>".to_string(),
            metrics_interval_secs: 5,
            redis_url: None,
            publish_changes: false,
        }
    }
}

impl OrderbookServiceConfig {
    /// Create config with custom subject.
    pub fn with_subject(subject: impl Into<String>) -> Self {
        Self {
            subject: subject.into(),
            ..Default::default()
        }
    }
}

/// Service that subscribes to normalized orderbook messages and maintains state.
///
/// Uses NATS Core subscription for low-latency push delivery.
pub struct OrderbookService {
    /// Shared orderbook store.
    store: OrderbookStore,
    /// NATS client for subscription.
    nats_client: Arc<NatsClient>,
    /// Shared Redis client for fetching market metadata.
    redis_client: Option<SharedRedisClient>,
    /// Service configuration.
    config: OrderbookServiceConfig,
    /// Shutdown signal receiver.
    shutdown_rx: mpsc::Receiver<()>,
}

impl OrderbookService {
    /// Create a new orderbook service.
    pub fn new(
        store: OrderbookStore,
        nats_client: NatsClient,
        config: OrderbookServiceConfig,
        shutdown_rx: mpsc::Receiver<()>,
    ) -> Self {
        // Try to create Redis client if URL is provided
        let redis_client = config.redis_url.as_ref().and_then(|url| {
            match SharedRedisClient::new(url) {
                Ok(client) => {
                    info!("Connected to Redis for market metadata");
                    Some(client)
                }
                Err(e) => {
                    warn!(
                        "Failed to connect to Redis: {:?}. Market metadata will not be available.",
                        e
                    );
                    None
                }
            }
        });

        Self {
            store,
            nats_client: Arc::new(nats_client),
            redis_client,
            config,
            shutdown_rx,
        }
    }

    /// Create with default configuration.
    pub fn with_defaults(
        store: OrderbookStore,
        nats_client: NatsClient,
        shutdown_rx: mpsc::Receiver<()>,
    ) -> Self {
        Self::new(
            store,
            nats_client,
            OrderbookServiceConfig::default(),
            shutdown_rx,
        )
    }

    /// Get a reference to the store.
    pub fn store(&self) -> &OrderbookStore {
        &self.store
    }

    /// Run the service (blocking).
    pub async fn run(mut self) -> Result<()> {
        info!(
            "Starting OrderbookService, subscribing to '{}'",
            self.config.subject
        );

        // Subscribe using NATS Core for low-latency push delivery
        let mut subscriber = self.nats_client.subscribe(&self.config.subject).await?;

        info!("OrderbookService running (lock-free mode)");

        // Metrics update ticker
        let mut metrics_interval =
            tokio::time::interval(Duration::from_secs(self.config.metrics_interval_secs));

        loop {
            tokio::select! {
                biased;  // Prioritize shutdown signal

                _ = self.shutdown_rx.recv() => {
                    info!("OrderbookService received shutdown signal");
                    break;
                }

                _ = metrics_interval.tick() => {
                    self.update_metrics();
                }

                msg = subscriber.next() => {
                    match msg {
                        Some(nats_msg) => {
                            counter!("orderbook_service_messages_received_total").increment(1);

                            if let Err(e) = self.process_message(&nats_msg.payload).await {
                                error!("Failed to process message: {:?}", e);
                                counter!(
                                    "orderbook_service_errors_total",
                                    "error_type" => "processing"
                                ).increment(1);
                            }
                        }
                        None => {
                            warn!("Subscription ended unexpectedly");
                            break;
                        }
                    }
                }
            }
        }

        info!("OrderbookService stopped");
        Ok(())
    }

    /// Process a single message.
    async fn process_message(&self, payload: &[u8]) -> Result<()> {
        let payload_str = std::str::from_utf8(payload)?;
        let normalized: NormalizedOrderbook = serde_json::from_str(payload_str)?;

        debug!(
            "Received {} for {}/{}",
            match normalized.message_type {
                normalizer::schema::OrderbookMessageType::Snapshot => "snapshot",
                normalizer::schema::OrderbookMessageType::Delta => "delta",
            },
            normalized.market_id,
            normalized.clob_token_id
        );

        // Try to fetch and cache market metadata if not already cached
        // Use hashed_market_id for caching
        let hashed_market_id = crate::hash_market_id(&normalized.market_id);
        if self
            .store
            .get_cached_market_metadata(&hashed_market_id)
            .is_none()
        {
            if let Err(e) = self
                .fetch_and_cache_market_metadata(
                    &normalized.platform,
                    &normalized.market_id,
                    &normalized.clob_token_id,
                    &hashed_market_id,
                )
                .await
            {
                debug!("Could not fetch market metadata: {:?}", e);
            }
        }

        // Apply to store (lock-free)
        self.store.apply(&normalized);

        // Publish change to NATS for gateway consumption (if enabled)
        if self.config.publish_changes {
            if let Err(e) = self.publish_change(&normalized, &hashed_market_id).await {
                warn!("Failed to publish change: {:?}", e);
            }
        }

        // Update message type counter
        let message_type = match normalized.message_type {
            normalizer::schema::OrderbookMessageType::Snapshot => "snapshot",
            normalizer::schema::OrderbookMessageType::Delta => "delta",
        };
        counter!(
            "orderbook_service_messages_processed_total",
            "message_type" => message_type
        )
        .increment(1);

        Ok(())
    }

    /// Publish aggregated price change event to NATS for gateway consumption.
    /// Fetches the current aggregated state for each changed price level.
    async fn publish_change(
        &self,
        msg: &NormalizedOrderbook,
        hashed_market_id: &str,
    ) -> Result<()> {
        use normalizer::schema::{
            AggregatedPriceChangeEvent, AggregatedPriceLevelChange, OrderSide, PlatformEntry,
        };

        // The aggregate_id is the hashed_market_id when using legacy apply
        let aggregate_id = hashed_market_id;

        // Collect affected prices with their side
        let mut affected_prices: Vec<(String, OrderSide)> = Vec::new();

        match msg.message_type {
            normalizer::schema::OrderbookMessageType::Snapshot => {
                // For snapshots, all bid/ask prices in the message are affected
                if let Some(bids) = &msg.bids {
                    for level in bids {
                        affected_prices.push((level.price.clone(), OrderSide::Buy));
                    }
                }
                if let Some(asks) = &msg.asks {
                    for level in asks {
                        affected_prices.push((level.price.clone(), OrderSide::Sell));
                    }
                }
            }
            normalizer::schema::OrderbookMessageType::Delta => {
                // For deltas, only the updated prices are affected
                if let Some(updates) = &msg.updates {
                    for update in updates {
                        let side = match update.side {
                            normalizer::schema::Side::Buy => OrderSide::Buy,
                            normalizer::schema::Side::Sell => OrderSide::Sell,
                        };
                        affected_prices.push((update.price.clone(), side));
                    }
                }
            }
        }

        // Build aggregated changes by fetching current state for each affected price
        let mut changes: Vec<AggregatedPriceLevelChange> = Vec::new();

        for (price, side) in affected_prices {
            // Fetch the current aggregated state at this price
            let level = match side {
                OrderSide::Buy => self.store.get_aggregated_bid_level(
                    aggregate_id,
                    hashed_market_id,
                    &msg.clob_token_id,
                    &price,
                ),
                OrderSide::Sell => self.store.get_aggregated_ask_level(
                    aggregate_id,
                    hashed_market_id,
                    &msg.clob_token_id,
                    &price,
                ),
            };

            // If level exists (has liquidity), include it in changes
            // If level doesn't exist (was removed), send empty with zero total_size
            let change = match level {
                Some(agg_level) => AggregatedPriceLevelChange {
                    price: agg_level.price,
                    side,
                    total_size: agg_level.total_size,
                    platforms: agg_level
                        .platforms
                        .into_iter()
                        .map(|p| PlatformEntry {
                            platform: p.platform,
                            size: p.size,
                        })
                        .collect(),
                },
                None => {
                    // Price level was removed (all orders cleared)
                    AggregatedPriceLevelChange {
                        price,
                        side,
                        total_size: "0".to_string(),
                        platforms: vec![],
                    }
                }
            };

            changes.push(change);
        }

        // Construct aggregated price change event
        let event = AggregatedPriceChangeEvent {
            aggregate_id: aggregate_id.to_string(),
            hashed_market_id: hashed_market_id.to_string(),
            clob_token_id: msg.clob_token_id.clone(),
            market_id: msg.market_id.clone(),
            changes,
            timestamp_us: Utc::now().timestamp_micros(),
        };

        // Publish to NATS Core (fire-and-forget for lowest latency)
        let subject = format!(
            "orderbook.changes.{}.{}.{}",
            aggregate_id, hashed_market_id, msg.clob_token_id
        );
        let change_bytes = serde_json::to_vec(&event)?;

        debug!(
            "Publishing aggregated price change to {} ({} changes)",
            subject,
            event.changes.len()
        );

        self.nats_client
            .publish_fast(&subject, bytes::Bytes::from(change_bytes))
            .await?;

        counter!("orderbook_service_changes_published_total").increment(1);

        Ok(())
    }

    /// Fetch market metadata from Redis and cache it.
    /// Uses clob_token_id to look up market data since the orderbook's market_id
    /// may be a condition_id that doesn't match the market's internal ID in Redis.
    async fn fetch_and_cache_market_metadata(
        &self,
        platform: &str,
        market_id: &str,
        clob_token_id: &str,
        hashed_market_id: &str,
    ) -> Result<()> {
        use std::collections::HashMap;

        let redis_client = match &self.redis_client {
            Some(client) => client,
            None => {
                debug!("No Redis client, skipping market metadata fetch");
                return Ok(());
            }
        };

        // Try to find market by clob_token_id - this is the most reliable method
        // since the orderbook's clob_token_id matches the token's clobTokenIds in Redis
        if let Ok(Some((slug, market))) = redis_client
            .find_market_by_token_id(platform, clob_token_id)
            .await
        {
            // Build platform-specific questions and slugs
            let mut questions = HashMap::new();
            let mut slugs = HashMap::new();

            if let Some(ref question) = market.question {
                questions.insert(platform.to_string(), question.clone());
            }
            slugs.insert(platform.to_string(), slug.clone());

            let metadata = MarketMetadata {
                questions,
                slugs,
                event_aggregate_id: market_id.to_string(),
                market_aggregate_id: None,
            };

            self.store.cache_market_metadata(hashed_market_id, metadata);
            info!(
                "Cached market metadata for {} (via token {}): question={:?}, slug={}",
                hashed_market_id, clob_token_id, market.question, slug
            );
            return Ok(());
        }

        // Fallback: try to find by market_id directly (in case IDs match)
        if let Ok(Some((slug, market))) = redis_client.find_market_by_id(platform, market_id).await
        {
            // Build platform-specific questions and slugs
            let mut questions = HashMap::new();
            let mut slugs = HashMap::new();

            if let Some(ref question) = market.question {
                questions.insert(platform.to_string(), question.clone());
            }
            slugs.insert(platform.to_string(), slug.clone());

            let metadata = MarketMetadata {
                questions,
                slugs,
                event_aggregate_id: market_id.to_string(),
                market_aggregate_id: None,
            };

            self.store.cache_market_metadata(hashed_market_id, metadata);
            info!(
                "Cached market metadata for {}: question={:?}, slug={}",
                hashed_market_id, market.question, slug
            );
        }

        Ok(())
    }

    /// Update Prometheus metrics.
    fn update_metrics(&self) {
        let stats = self.store.stats();
        gauge!("orderbook_service_markets_total").set(stats.market_count as f64);
        gauge!("orderbook_service_tokens_total").set(stats.total_tokens as f64);
        gauge!("orderbook_service_snapshots_total").set(stats.total_snapshots as f64);
        gauge!("orderbook_service_deltas_total").set(stats.total_deltas as f64);
    }
}

/// Builder for OrderbookService.
pub struct OrderbookServiceBuilder {
    config: OrderbookServiceConfig,
}

impl OrderbookServiceBuilder {
    /// Create a new builder with default config.
    pub fn new() -> Self {
        Self {
            config: OrderbookServiceConfig::default(),
        }
    }

    /// Set the NATS subject to subscribe to.
    pub fn subject(mut self, subject: impl Into<String>) -> Self {
        self.config.subject = subject.into();
        self
    }

    /// Set the metrics update interval.
    pub fn metrics_interval_secs(mut self, secs: u64) -> Self {
        self.config.metrics_interval_secs = secs;
        self
    }

    /// Enable publishing orderbook changes to NATS for gateway consumption.
    pub fn publish_changes(mut self, enabled: bool) -> Self {
        self.config.publish_changes = enabled;
        self
    }

    /// Build the service.
    pub fn build(
        self,
        store: OrderbookStore,
        nats_client: NatsClient,
        shutdown_rx: mpsc::Receiver<()>,
    ) -> OrderbookService {
        OrderbookService::new(store, nats_client, self.config, shutdown_rx)
    }
}

impl Default for OrderbookServiceBuilder {
    fn default() -> Self {
        Self::new()
    }
}
