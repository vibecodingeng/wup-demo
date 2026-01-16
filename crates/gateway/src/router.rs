//! Change router: NATS → WebSocket fan-out.
//!
//! Subscribes to orderbook changes from NATS and routes them to
//! subscribed WebSocket clients.

use crate::client::{ClientRegistry, ClientState};
use crate::error::{GatewayError, Result};
use crate::protocol::{MarketOrderbooksHttpResponse, OrderbookHttpResponse, PriceChangeData, ServerMessage};
use chrono::Utc;
use futures::StreamExt;
use metrics::{counter, histogram};
use nats_client::NatsClient;
use normalizer::schema::AggregatedPriceChangeEvent;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

/// Extract market_id and asset_id from JSON payload without full deserialization.
/// This is much faster than deserializing the entire struct (~1μs vs ~50-70μs).
fn extract_ids_fast(payload: &[u8]) -> Option<(String, String)> {
    // Use simd_json or manual parsing for maximum speed
    // For now, use serde_json::from_slice with a minimal struct
    #[derive(serde::Deserialize)]
    struct IdExtractor {
        market_id: String,
        asset_id: String,
    }

    serde_json::from_slice::<IdExtractor>(payload)
        .ok()
        .map(|e| (e.market_id, e.asset_id))
}

/// Configuration for the change router.
#[derive(Debug, Clone)]
pub struct RouterConfig {
    /// NATS subject pattern to subscribe to.
    pub nats_subject: String,
    /// Base URL for OrderbookService HTTP API.
    pub orderbook_service_url: String,
}

impl Default for RouterConfig {
    fn default() -> Self {
        Self {
            nats_subject: "orderbook.changes.>".to_string(),
            orderbook_service_url: "http://localhost:8080".to_string(),
        }
    }
}

/// Routes orderbook changes from NATS to WebSocket clients.
pub struct ChangeRouter {
    /// Client registry for routing messages.
    registry: Arc<ClientRegistry>,
    /// NATS client for subscribing to changes.
    nats_client: Arc<NatsClient>,
    /// HTTP client for fetching snapshots.
    http_client: reqwest::Client,
    /// Configuration.
    config: RouterConfig,
}

impl ChangeRouter {
    /// Create a new change router.
    pub fn new(
        registry: Arc<ClientRegistry>,
        nats_client: Arc<NatsClient>,
        config: RouterConfig,
    ) -> Self {
        Self {
            registry,
            nats_client,
            http_client: reqwest::Client::new(),
            config,
        }
    }

    /// Run the router (blocking).
    pub async fn run(self: Arc<Self>, mut shutdown_rx: mpsc::Receiver<()>) -> Result<()> {
        info!(
            "Starting ChangeRouter, subscribing to '{}'",
            self.config.nats_subject
        );

        // Subscribe to orderbook changes
        let mut subscriber = self.nats_client.subscribe(&self.config.nats_subject).await?;

        info!("ChangeRouter running");

        loop {
            tokio::select! {
                biased;

                _ = shutdown_rx.recv() => {
                    info!("ChangeRouter received shutdown signal");
                    break;
                }

                msg = subscriber.next() => {
                    match msg {
                        Some(nats_msg) => {
                            if let Err(e) = self.handle_change(&nats_msg.subject, &nats_msg.payload).await {
                                warn!("Failed to handle change: {:?}", e);
                                counter!("gateway_routing_errors_total").increment(1);
                            }
                        }
                        None => {
                            warn!("NATS subscription ended unexpectedly");
                            break;
                        }
                    }
                }
            }
        }

        info!("ChangeRouter stopped");
        Ok(())
    }

    /// Handle a single aggregated price change event from NATS.
    async fn handle_change(&self, _subject: &str, payload: &[u8]) -> Result<()> {
        let start = Instant::now();
        counter!("gateway_changes_received_total").increment(1);

        // PHASE 1 OPTIMIZATION: Extract IDs without full deserialization
        // This is ~50x faster than deserializing the entire struct
        let (market_id, asset_id) = match extract_ids_fast(payload) {
            Some(ids) => ids,
            None => {
                // Fallback to full deserialization if fast extraction fails
                let change: AggregatedPriceChangeEvent = serde_json::from_slice(payload)?;
                (change.market_id, change.asset_id)
            }
        };

        // Build the client subscription subject (without the orderbook.changes. prefix)
        // Format: {market_id}.{asset_id}
        let client_subject = format!("{}.{}", market_id, asset_id);

        // EARLY EXIT: Check if anyone is subscribed BEFORE expensive deserialization
        let routing_start = Instant::now();
        let clients = self.registry.get_matching_subscribers(&client_subject);
        histogram!("gateway_routing_duration_us").record(routing_start.elapsed().as_micros() as f64);

        if clients.is_empty() {
            debug!("No clients subscribed to {}", client_subject);
            histogram!("gateway_handle_change_duration_us").record(start.elapsed().as_micros() as f64);
            return Ok(());
        }

        // Only deserialize fully when we have subscribers
        let deserialize_start = Instant::now();
        let change: AggregatedPriceChangeEvent = serde_json::from_slice(payload)?;
        histogram!("gateway_deserialize_duration_us").record(deserialize_start.elapsed().as_micros() as f64);

        debug!(
            "Received price change for {}/{} ({} changes)",
            change.market_id,
            change.asset_id,
            change.changes.len()
        );

        info!(
            "Routing price change for {} to {} clients ({} changes)",
            client_subject,
            clients.len(),
            change.changes.len()
        );

        // Convert to price change message with aggregated format
        let price_change_msg = ServerMessage::PriceChange(PriceChangeData::from(change));

        // Pre-serialize once
        let json = serde_json::to_string(&price_change_msg)?;

        // Send to all matching clients
        // OPTIMIZATION: Use try_send for non-blocking behavior with bounded channels
        let mut sent_count = 0;
        let mut dropped_count = 0;
        for client in &clients {
            if client.try_send_raw(axum::extract::ws::Message::Text(json.clone().into())) {
                sent_count += 1;
            } else {
                dropped_count += 1;
                debug!("Dropped message for slow client {}", client.id);
            }
        }

        if dropped_count > 0 {
            counter!("gateway_messages_dropped_slow_client").increment(dropped_count);
        }

        counter!("gateway_changes_routed_total").increment(1);
        histogram!("gateway_clients_per_route").record(sent_count as f64);
        histogram!("gateway_handle_change_duration_us").record(start.elapsed().as_micros() as f64);

        Ok(())
    }

    /// Send initial snapshot to a client for a subscription.
    pub async fn send_snapshot(&self, client: &Arc<ClientState>, subject: &str) -> Result<()> {
        // Parse subject to get market_id, asset_id
        // Subject format: {market_id}.{asset_id}
        // or with wildcards like {market_id}.* for all assets in a market

        let parts: Vec<&str> = subject.split('.').collect();

        // Handle market wildcard: {market_id}.* or {market_id}.>
        // This sends snapshots for ALL assets in that market
        if parts.len() == 2 && (parts[1] == "*" || parts[1] == ">") {
            let market_id = parts[0];
            return self.send_market_snapshots(client, market_id).await;
        }

        // Skip other wildcard patterns (e.g., *.* or complex patterns)
        if subject.contains('*') || subject.contains('>') {
            debug!("Skipping snapshot for complex wildcard subscription: {}", subject);
            return Ok(());
        }

        if parts.len() < 2 {
            return Err(GatewayError::InvalidSubject(format!(
                "Subject must have at least 2 parts: {}",
                subject
            )));
        }

        let market_id = parts[0];
        let asset_id = parts[1];

        // Fetch orderbook from HTTP API (2-level hierarchy)
        let url = format!(
            "{}/orderbook/{}/{}",
            self.config.orderbook_service_url, market_id, asset_id
        );

        debug!("Fetching snapshot from: {}", url);

        let response = self.http_client.get(&url).send().await?;

        if !response.status().is_success() {
            warn!(
                "Failed to fetch snapshot for {}: {}",
                subject,
                response.status()
            );
            return Ok(()); // Don't error, just skip snapshot
        }

        let orderbook: OrderbookHttpResponse = response.json().await?;

        // Convert to orderbook snapshot message
        let orderbook_data = orderbook.to_orderbook_data(Utc::now().timestamp_micros());

        // Send snapshot to client (full orderbook on initial subscribe)
        client.send(ServerMessage::Snapshot(orderbook_data))?;

        counter!("gateway_snapshots_sent_total").increment(1);

        Ok(())
    }

    /// Send snapshots for all assets in a market.
    /// Used for wildcard subscriptions like {market_id}.*
    async fn send_market_snapshots(&self, client: &Arc<ClientState>, market_id: &str) -> Result<()> {
        let url = format!(
            "{}/orderbook/{}",
            self.config.orderbook_service_url, market_id
        );

        debug!("Fetching market snapshots from: {}", url);

        let response = self.http_client.get(&url).send().await?;

        if !response.status().is_success() {
            warn!(
                "Failed to fetch market snapshots for {}: {}",
                market_id,
                response.status()
            );
            return Ok(()); // Don't error, just skip snapshots
        }

        let market_response: MarketOrderbooksHttpResponse = response.json().await?;

        info!(
            "Sending {} snapshots for market {} to client {}",
            market_response.asset_count, market_id, client.id
        );

        let timestamp_us = Utc::now().timestamp_micros();

        // Send a snapshot for each asset in the market
        for orderbook in market_response.assets {
            let orderbook_data = orderbook.to_orderbook_data(timestamp_us);
            client.send(ServerMessage::Snapshot(orderbook_data))?;
            counter!("gateway_snapshots_sent_total").increment(1);
        }

        Ok(())
    }
}
