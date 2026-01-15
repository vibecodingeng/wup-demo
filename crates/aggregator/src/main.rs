//! Main entry point for the betting aggregator.

mod supervisor;

use anyhow::Result;
use external_services::polymarket::TokenMapping;
use external_services::SharedRedisClient;
use metrics_exporter_prometheus::PrometheusBuilder;
use nats_client::NatsClient;
use normalizer::{AdapterConfig, NormalizerService, PolymarketAdapter};
use supervisor::Supervisor;
use tokio::sync::mpsc;
use tracing::{error, info, warn};
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

    // Create service-specific streams
    nats_client.ensure_market_stream(SERVICE_NAME).await?;
    nats_client.ensure_normalized_stream(SERVICE_NAME).await?;

    // Get Redis URL and connect
    let redis_url =
        std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://localhost:6379".into());
    let redis_client = SharedRedisClient::new(&redis_url)?;

    // Create the supervisor for raw message ingestion
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

    // Create the normalizer service with Polymarket adapter
    let (normalizer_shutdown_tx, normalizer_shutdown_rx) = mpsc::channel(1);

    let normalizer_nats = NatsClient::connect(&nats_url).await?;
    let polymarket_adapter = PolymarketAdapter::new();

    // Normalizer config:
    // - Subscribes to: market.polymarket.>
    // - Publishes to: normalized.polymarket.{event_slug}.{market_id}.{token_id}
    let config = AdapterConfig {
        source_stream: format!("{}_MARKET", SERVICE_NAME.to_uppercase()),
        filter_subject: format!("market.{}.>", SERVICE_NAME),
        dest_stream: format!("{}_NORMALIZED", SERVICE_NAME.to_uppercase()),
        output_subject_prefix: format!("normalized.{}", SERVICE_NAME),
    };

    let normalizer = NormalizerService::new(
        polymarket_adapter,
        normalizer_nats,
        config,
        normalizer_shutdown_rx,
    );

    // Spawn normalizer task
    let normalizer_handle = tokio::spawn(async move {
        if let Err(e) = normalizer.run().await {
            error!("Normalizer failed: {:?}", e);
        }
    });

    info!("Normalizer service spawned");

    // Run the supervisor (this blocks until shutdown)
    supervisor.run().await?;

    // Shutdown normalizer
    info!("Shutting down normalizer...");
    let _ = normalizer_shutdown_tx.send(()).await;
    let _ = normalizer_handle.await;

    Ok(())
}
