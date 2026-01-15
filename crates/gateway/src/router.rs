//! Change router: NATS → WebSocket fan-out.
//!
//! Subscribes to orderbook changes from NATS and routes them to
//! subscribed WebSocket clients.

use crate::client::{ClientRegistry, ClientState};
use crate::error::{GatewayError, Result};
use crate::protocol::{MarketOrderbooksHttpResponse, OrderbookHttpResponse, PriceChangeData, ServerMessage};
use chrono::Utc;
use futures::StreamExt;
use metrics::counter;
use nats_client::NatsClient;
use normalizer::schema::AggregatedPriceChangeEvent;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

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
        // Parse the aggregated price change event
        let change: AggregatedPriceChangeEvent = serde_json::from_slice(payload)?;

        counter!("gateway_changes_received_total").increment(1);

        debug!(
            "Received price change for {}/{} ({} changes)",
            change.market_id,
            change.asset_id,
            change.changes.len()
        );

        // Build the client subscription subject (without the orderbook.changes. prefix)
        // Format: {market_id}.{asset_id}
        let client_subject = format!("{}.{}", change.market_id, change.asset_id);

        // Find matching clients
        let clients = self.registry.get_matching_subscribers(&client_subject);
        if clients.is_empty() {
            debug!("No clients subscribed to {}", client_subject);
            return Ok(());
        }

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
        for client in clients {
            if let Err(e) = client
                .tx
                .send(axum::extract::ws::Message::Text(json.clone().into()))
            {
                debug!("Failed to send to client {}: {}", client.id, e);
            }
        }

        counter!("gateway_changes_routed_total").increment(1);

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
