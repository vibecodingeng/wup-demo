//! Polymarket WebSocket handler implementation.

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
use std::collections::HashMap;
use std::sync::RwLock;
use tracing::{debug, info, warn};

/// Re-export MAX_SUBSCRIPTIONS for backwards compatibility.
pub const MAX_SUBSCRIPTIONS_PER_CONNECTION: usize = MAX_SUBSCRIPTIONS;

/// Polymarket WebSocket handler.
/// Implements the WsHandler trait for processing market data.
pub struct PolymarketHandler {
    /// Token ID to mapping (event_slug, market_id).
    token_mappings: RwLock<HashMap<String, (String, String)>>,
    /// Currently subscribed asset IDs.
    subscribed_ids: RwLock<Vec<String>>,
    /// NATS client for publishing events.
    nats_client: NatsClient,
    /// Worker ID for logging.
    worker_id: String,
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

        Self {
            token_mappings: RwLock::new(token_map),
            subscribed_ids: RwLock::new(ids),
            nats_client,
            worker_id,
        }
    }

    /// Create a new Polymarket handler (legacy - uses default subject format).
    pub fn new(initial_ids: Vec<String>, nats_client: NatsClient, worker_id: String) -> Self {
        Self {
            token_mappings: RwLock::new(HashMap::new()),
            subscribed_ids: RwLock::new(initial_ids),
            nats_client,
            worker_id,
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

    /// Extract asset_id from a market event message.
    fn extract_asset_id(msg: &str) -> Option<String> {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(msg) {
            value.get("asset_id").and_then(|v| v.as_str()).map(String::from)
        } else {
            None
        }
    }

    /// Build NATS subject for the message.
    /// Format: market.polymarket.{event_slug}.{market_id}.{clob_token_id}
    fn build_subject(&self, asset_id: &str) -> String {
        let token_map = self.token_mappings.read().unwrap();

        if let Some((event_slug, market_id)) = token_map.get(asset_id) {
            format!("market.polymarket.{}.{}.{}", event_slug, market_id, asset_id)
        } else {
            // Fallback without event context
            format!("market.polymarket.unknown.unknown.{}", asset_id)
        }
    }

    /// Truncate an ID to max 16 characters for NATS subject (legacy).
    fn truncate_id(id: &str) -> &str {
        if id.len() > 16 { &id[..16] } else { id }
    }

    /// Add received_at timestamp to the JSON message.
    fn add_received_timestamp(msg: &str, received_at: i64) -> String {
        if let Ok(mut value) = serde_json::from_str::<serde_json::Value>(msg) {
            if let Some(obj) = value.as_object_mut() {
                obj.insert(
                    "received_at".to_string(),
                    serde_json::Value::Number(received_at.into()),
                );
                return serde_json::to_string(&value).unwrap_or_else(|_| msg.to_string());
            }
        }
        // If parsing fails, return original message
        msg.to_string()
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

        // Extract asset_id for subject routing
        let asset_id = Self::extract_asset_id(msg);

        // Build subject based on token mapping
        let subject = match &asset_id {
            Some(aid) => self.build_subject(aid),
            None => "market.polymarket.raw".to_string(),
        };

        // Add received_at timestamp to the message
        let payload = Self::add_received_timestamp(msg, received_at);

        // Publish to NATS (fast/fire-and-forget for low latency)
        self.nats_client
            .publish_fast(&subject, bytes::Bytes::from(payload))
            .await
            .map_err(|e| common::error::Error::Generic(e.to_string()))?;

        counter!("aggregator_messages_published_total", "platform" => "polymarket").increment(1);

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
