//! Main entry point for the betting aggregator.

mod supervisor;

use anyhow::Result;
use external_services::polymarket::{EventData, TokenMapping};
use metrics_exporter_prometheus::PrometheusBuilder;
use nats_client::NatsClient;
use normalizer::{AdapterConfig, NormalizerService, PolymarketAdapter};
use redis::AsyncCommands;
use supervisor::Supervisor;
use tokio::sync::mpsc;
use tracing::{error, info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

/// Service name for Polymarket.
const SERVICE_NAME: &str = "polymarket";

/// Redis key prefix for events.
const EVENT_KEY_PREFIX: &str = "event:";

/// Fetch token mappings from Redis for a platform event slug.
async fn fetch_token_mappings_from_redis(
    redis_url: &str,
    platform: &str,
    event_slug: &str,
) -> Result<Vec<TokenMapping>> {
    let client = redis::Client::open(redis_url)?;
    let mut conn = client.get_multiplexed_async_connection().await?;

    // Key format: event:{platform}:{slug}
    let key = format!("{}{}:{}", EVENT_KEY_PREFIX, platform, event_slug);
    let json: Option<String> = conn.get(&key).await?;

    match json {
        Some(j) => {
            let data: EventData = serde_json::from_str(&j)?;
            info!(
                "Fetched {} token mappings from Redis for event '{}:{}'",
                data.tokens.len(),
                platform,
                event_slug
            );
            Ok(data.tokens)
        }
        None => {
            warn!("Event '{}:{}' not found in Redis", platform, event_slug);
            Ok(vec![])
        }
    }
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

    // Get Redis URL
    let redis_url =
        std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://localhost:6379".into());

    // Check for EVENT_SLUG (new mode) or TOKEN_IDS (legacy mode)
    let event_slug = std::env::var("EVENT_SLUG").ok();
    let token_ids_legacy: Vec<String> = std::env::var("TOKEN_IDS")
        .unwrap_or_else(|_| "".into())
        .split(',')
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect();

    // Create the supervisor for raw message ingestion
    let mut supervisor = Supervisor::new(nats_client.clone(), 50);

    // Subscribe based on configuration
    if let Some(slug) = &event_slug {
        // New mode: fetch token mappings from Redis
        info!("Using EVENT_SLUG mode: {}:{}", SERVICE_NAME, slug);
        let mappings = fetch_token_mappings_from_redis(&redis_url, SERVICE_NAME, slug).await?;

        if mappings.is_empty() {
            warn!(
                "No token mappings found for event '{}:{}'. Make sure event_service has fetched the event first.",
                SERVICE_NAME, slug
            );
        } else {
            supervisor.subscribe_with_mappings(mappings).await?;
        }
    } else if !token_ids_legacy.is_empty() {
        // Legacy mode: use TOKEN_IDS directly
        info!(
            "Using legacy TOKEN_IDS mode with {} tokens",
            token_ids_legacy.len()
        );
        supervisor.subscribe(token_ids_legacy).await?;
    } else {
        warn!("No EVENT_SLUG or TOKEN_IDS provided. Starting without subscriptions.");
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
