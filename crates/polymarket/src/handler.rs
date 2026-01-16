//! Polymarket WebSocket handler implementation.
//!
//! This handler receives raw WebSocket messages from Polymarket and:
//! 1. Injects received_at timestamp
//! 2. Normalizes inline using PolymarketAdapter
//! 3. Publishes normalized messages directly to NATS
//!
//! This eliminates the intermediate raw stream and NormalizerService hop.

use async_trait::async_trait;
use chrono::Utc;
use common::error::Result;
use common::{ControlCommand, WsHandler};
use external_services::polymarket::{
    build_subscription_message, build_unsubscription_message, TokenMapping, MAX_SUBSCRIPTIONS,
    WS_URL,
};
use metrics::counter;
use nats_client::NatsClient;
use normalizer::{AdapterConfig, ExchangeAdapter, PolymarketAdapter};
use std::collections::HashMap;
use std::sync::RwLock;
use tracing::{debug, info, warn};

/// Re-export MAX_SUBSCRIPTIONS for backwards compatibility.
pub const MAX_SUBSCRIPTIONS_PER_CONNECTION: usize = MAX_SUBSCRIPTIONS;

/// Polymarket WebSocket handler.
/// Implements the WsHandler trait for processing market data.
///
/// Performs inline normalization - no separate NormalizerService needed.
pub struct PolymarketHandler {
    /// Token ID to mapping (event_slug, market_id).
    token_mappings: RwLock<HashMap<String, (String, String)>>,
    /// Currently subscribed asset IDs.
    subscribed_ids: RwLock<Vec<String>>,
    /// NATS client for publishing events.
    nats_client: NatsClient,
    /// Worker ID for logging.
    worker_id: String,
    /// Normalizer adapter for inline transformation.
    adapter: PolymarketAdapter,
    /// Adapter configuration for output subjects.
    adapter_config: AdapterConfig,
}

impl PolymarketHandler {
    /// Create a new Polymarket handler with token mappings.
    pub fn new_with_mappings(
        mappings: Vec<TokenMapping>,
        nats_client: NatsClient,
        worker_id: String,
    ) -> Self {
        let mut token_map = HashMap::new();
        let mut ids = Vec::new();

        for mapping in mappings {
            ids.push(mapping.clob_token_id.clone());
            token_map.insert(
                mapping.clob_token_id,
                (mapping.event_slug, mapping.market_id),
            );
        }

        // Initialize normalizer adapter for inline transformation
        let adapter = PolymarketAdapter::new();
        let adapter_config = AdapterConfig {
            source_stream: String::new(), // Not used for inline normalization
            filter_subject: String::new(),
            dest_stream: "POLYMARKET_NORMALIZED".to_string(),
            output_subject_prefix: "normalized.polymarket".to_string(),
        };

        Self {
            token_mappings: RwLock::new(token_map),
            subscribed_ids: RwLock::new(ids),
            nats_client,
            worker_id,
            adapter,
            adapter_config,
        }
    }

    /// Create a new Polymarket handler (legacy - uses default subject format).
    pub fn new(initial_ids: Vec<String>, nats_client: NatsClient, worker_id: String) -> Self {
        let adapter = PolymarketAdapter::new();
        let adapter_config = AdapterConfig {
            source_stream: String::new(),
            filter_subject: String::new(),
            dest_stream: "POLYMARKET_NORMALIZED".to_string(),
            output_subject_prefix: "normalized.polymarket".to_string(),
        };

        Self {
            token_mappings: RwLock::new(HashMap::new()),
            subscribed_ids: RwLock::new(initial_ids),
            nats_client,
            worker_id,
            adapter,
            adapter_config,
        }
    }

    /// Add token mappings.
    pub fn add_mappings(&self, mappings: Vec<TokenMapping>) {
        let mut token_map = self.token_mappings.write().unwrap();
        let mut subscribed = self.subscribed_ids.write().unwrap();

        for mapping in mappings {
            if !subscribed.contains(&mapping.clob_token_id) {
                subscribed.push(mapping.clob_token_id.clone());
            }
            token_map.insert(
                mapping.clob_token_id,
                (mapping.event_slug, mapping.market_id),
            );
        }
    }

    /// Add asset IDs to the subscription list (legacy).
    fn add_subscriptions(&self, ids: Vec<String>) {
        let mut subscribed = self.subscribed_ids.write().unwrap();
        for id in ids {
            if !subscribed.contains(&id) {
                subscribed.push(id);
            }
        }
    }

    /// Remove asset IDs from the subscription list.
    fn remove_subscriptions(&self, ids: &[String]) {
        let mut subscribed = self.subscribed_ids.write().unwrap();
        let mut token_map = self.token_mappings.write().unwrap();

        subscribed.retain(|id| !ids.contains(id));
        for id in ids {
            token_map.remove(id);
        }
    }

    /// Inject received_at timestamp into raw JSON string.
    /// This is faster than parsing + modifying + serializing.
    ///
    /// For objects: `{"event_type":...}` → `{"received_at":123,"event_type":...}`
    /// For arrays: `[{...}]` → `[{"received_at":123,...}]` (injects into each object)
    fn inject_received_at(msg: &str, received_at: i64) -> String {
        let timestamp_field = format!("\"received_at\":{},", received_at);

        if msg.trim_start().starts_with('[') {
            // Array: inject into each object
            msg.replace("{\"", &format!("{{\"received_at\":{},\"", received_at))
        } else {
            // Single object: inject at start
            if let Some(pos) = msg.find('{') {
                let mut result = String::with_capacity(msg.len() + timestamp_field.len());
                result.push_str(&msg[..=pos]);
                result.push_str(&timestamp_field);
                result.push_str(&msg[pos + 1..]);
                result
            } else {
                msg.to_string()
            }
        }
    }
}

#[async_trait]
impl WsHandler for PolymarketHandler {
    fn url(&self) -> &str {
        WS_URL
    }

    fn on_connect_message(&self) -> Option<String> {
        let ids = self.subscribed_ids.read().unwrap();
        if ids.is_empty() {
            None
        } else {
            Some(build_subscription_message(&ids))
        }
    }

    async fn on_message(&self, msg: &str) -> Result<()> {
        // Capture receive timestamp immediately
        let received_at = Utc::now().timestamp_millis();

        debug!("[{}] Received message: {}", self.worker_id, msg);

        // Inject received_at into raw JSON (fast string manipulation, no parse)
        let payload_with_timestamp = Self::inject_received_at(msg, received_at);

        // INLINE NORMALIZATION: Parse and transform in one step
        // This eliminates the separate NormalizerService and NATS hop
        let normalized = match self.adapter.parse_and_transform(&payload_with_timestamp) {
            Ok(results) => results,
            Err(e) => {
                debug!("[{}] Failed to normalize message: {}", self.worker_id, e);
                counter!("aggregator_normalize_errors_total", "platform" => "polymarket").increment(1);
                return Ok(());
            }
        };

        if normalized.is_empty() {
            debug!("[{}] No normalized messages produced (unknown event type)", self.worker_id);
            return Ok(());
        }

        // Publish each normalized message directly to NATS
        for orderbook in normalized {
            let subject = self.adapter.build_output_subject(&self.adapter_config, &orderbook);

            // Serialize normalized message
            let payload = match serde_json::to_vec(&orderbook) {
                Ok(p) => p,
                Err(e) => {
                    warn!("[{}] Failed to serialize normalized message: {}", self.worker_id, e);
                    continue;
                }
            };

            // Publish to normalized subject (skips the raw stream entirely)
            self.nats_client
                .publish_fast(&subject, bytes::Bytes::from(payload))
                .await
                .map_err(|e| common::error::Error::Generic(e.to_string()))?;

            counter!("aggregator_messages_normalized_total", "platform" => "polymarket").increment(1);
        }

        Ok(())
    }

    async fn on_disconnect(&self) {
        warn!("[{}] Polymarket connection lost", self.worker_id);
    }

    async fn on_reconnect(&self) {
        info!("[{}] Polymarket reconnected", self.worker_id);
    }

    async fn handle_command(&self, cmd: ControlCommand) -> Option<String> {
        match cmd {
            ControlCommand::Subscribe(ids) => {
                info!(
                    "[{}] Subscribing to {} new assets",
                    self.worker_id,
                    ids.len()
                );
                self.add_subscriptions(ids.clone());
                Some(build_subscription_message(&ids))
            }
            ControlCommand::Unsubscribe(ids) => {
                info!(
                    "[{}] Unsubscribing from {} assets",
                    self.worker_id,
                    ids.len()
                );
                self.remove_subscriptions(&ids);
                Some(build_unsubscription_message(&ids))
            }
            ControlCommand::Shutdown => None,
        }
    }

    fn subscribed_ids(&self) -> Vec<String> {
        self.subscribed_ids.read().unwrap().clone()
    }
}
