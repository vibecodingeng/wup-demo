//! Main entry point for the betting aggregator.
//!
//! The aggregator connects to Polymarket WebSocket, normalizes messages inline,
//! and publishes directly to normalized.polymarket.* subjects.
//!
//! Normalization is done inline in the PolymarketHandler - no separate
//! NormalizerService needed.

mod supervisor;

use anyhow::Result;
use external_services::polymarket::TokenMapping;
use external_services::SharedRedisClient;
use metrics_exporter_prometheus::PrometheusBuilder;
use nats_client::NatsClient;
use supervisor::Supervisor;
use tracing::{info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

/// Service name for Polymarket.
const SERVICE_NAME: &str = "polymarket";

/// Fetch all token mappings from Redis for all events of a platform.
async fn fetch_all_token_mappings_from_redis(
    redis: &SharedRedisClient,
    platform: &str,
) -> Result<Vec<TokenMapping>> {
    // List all events from Redis
    let events = redis.list_events().await?;

    // Filter for this platform's events
    let platform_prefix = format!("{}:", platform);
    let platform_events: Vec<&str> = events
        .iter()
        .filter(|e| e.starts_with(&platform_prefix))
        .map(|e| e.strip_prefix(&platform_prefix).unwrap_or(e))
        .collect();

    if platform_events.is_empty() {
        info!("No events found for platform '{}' in Redis", platform);
        return Ok(vec![]);
    }

    info!(
        "Found {} events for platform '{}': {:?}",
        platform_events.len(),
        platform,
        platform_events
    );

    // Collect token mappings from all events
    let event_count = platform_events.len();
    let mut all_mappings = Vec::new();
    for event_slug in platform_events {
        match redis.get_token_mappings(platform, event_slug).await {
            Ok(tokens) => {
                info!(
                    "Fetched {} token mappings from event '{}:{}'",
                    tokens.len(),
                    platform,
                    event_slug
                );
                all_mappings.extend(tokens);
            }
            Err(e) => {
                warn!(
                    "Failed to fetch tokens for event '{}:{}': {:?}",
                    platform, event_slug, e
                );
            }
        }
    }

    info!(
        "Total: {} token mappings from {} events",
        all_mappings.len(),
        event_count
    );

    Ok(all_mappings)
}

#[tokio::main]
async fn main() -> Result<()> {
    // Load .env file if present
    dotenvy::dotenv().ok();

    // Initialize tracing
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into()),
        ))
        .with(tracing_subscriber::fmt::layer())
        .init();

    info!("Starting betting aggregator...");

    // Initialize Prometheus metrics exporter
    let metrics_port: u16 = std::env::var("METRICS_PORT")
        .unwrap_or_else(|_| "9090".into())
        .parse()
        .unwrap_or(9090);

    PrometheusBuilder::new()
        .with_http_listener(([0, 0, 0, 0], metrics_port))
        .install()?;

    info!(
        "Prometheus metrics available at http://0.0.0.0:{}/metrics",
        metrics_port
    );

    // Connect to NATS
    let nats_url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://localhost:4222".into());
    let nats_client = NatsClient::connect(&nats_url).await?;

    // Create normalized stream (needed by OrderbookService)
    // Note: market stream no longer needed - we publish normalized directly
    nats_client.ensure_normalized_stream(SERVICE_NAME).await?;

    // Get Redis URL and connect
    let redis_url =
        std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://localhost:6379".into());
    let redis_client = SharedRedisClient::new(&redis_url)?;

    // Create the supervisor for WebSocket connections
    // Handler now normalizes inline and publishes to normalized.polymarket.*
    let mut supervisor = Supervisor::new(nats_client.clone(), 50);

    // Fetch all token mappings from all events in Redis for this platform
    info!("Fetching all events for platform '{}' from Redis...", SERVICE_NAME);
    let mappings = fetch_all_token_mappings_from_redis(&redis_client, SERVICE_NAME).await?;

    if mappings.is_empty() {
        warn!(
            "No token mappings found for platform '{}'. Make sure event_service has fetched events first.",
            SERVICE_NAME
        );
    } else {
        supervisor.subscribe_with_mappings(mappings).await?;
    }

    info!("Aggregator running with inline normalization (no separate NormalizerService)");

    // Run the supervisor (this blocks until shutdown)
    supervisor.run().await?;

    Ok(())
}
