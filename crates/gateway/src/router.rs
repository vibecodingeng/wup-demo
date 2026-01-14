//! Change router: NATS → WebSocket fan-out.
//!
//! Subscribes to orderbook changes from NATS and routes them to
//! subscribed WebSocket clients.

use crate::client::{ClientRegistry, ClientState};
use crate::error::{GatewayError, Result};
use crate::protocol::{OrderbookHttpResponse, PriceChangeData, ServerMessage};
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
            "Received price change for {}/{}/{} ({} changes)",
            change.aggregate_id,
            change.hashed_market_id,
            change.clob_token_id,
            change.changes.len()
        );

        // Build the client subscription subject (without the orderbook.changes. prefix)
        let client_subject = format!(
            "{}.{}.{}",
            change.aggregate_id, change.hashed_market_id, change.clob_token_id
        );

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
        // Parse subject to get aggregate_id, hashed_market_id, clob_token_id
        // Subject format: {aggregate_id}.{hashed_market_id}.{clob_token_id}
        // or with wildcards which we skip for snapshots
        if subject.contains('*') || subject.contains('>') {
            // Can't fetch snapshot for wildcard subscriptions
            debug!("Skipping snapshot for wildcard subscription: {}", subject);
            return Ok(());
        }

        let parts: Vec<&str> = subject.split('.').collect();
        if parts.len() < 3 {
            return Err(GatewayError::InvalidSubject(format!(
                "Subject must have at least 3 parts: {}",
                subject
            )));
        }

        let aggregate_id = parts[0];
        let hashed_market_id = parts[1];
        let clob_token_id = parts[2];

        // Fetch orderbook from HTTP API
        let url = format!(
            "{}/orderbook/{}/{}/{}",
            self.config.orderbook_service_url, aggregate_id, hashed_market_id, clob_token_id
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
        let orderbook_data = orderbook.to_orderbook_data(
            aggregate_id.to_string(),
            hashed_market_id.to_string(),
            Utc::now().timestamp_micros(),
        );

        // Send snapshot to client (full orderbook on initial subscribe)
        client.send(ServerMessage::Snapshot(orderbook_data))?;

        counter!("gateway_snapshots_sent_total").increment(1);

        Ok(())
    }
}
