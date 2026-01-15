//! Event service entry point.
//!
//! Fetches event data from Polymarket and stores in Redis.
//! Exposes HTTP API for querying event data and token mappings.
//! Periodically refreshes cached events from exchange APIs.

use anyhow::Result;
use event_service::{create_router, AppState, SharedRedisClient};
use external_services::polymarket::PolymarketClient;
use metrics_exporter_prometheus::PrometheusBuilder;
use std::sync::Arc;
use std::time::Duration;
use tracing::{error, info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

/// Refresh all events stored in Redis.
async fn refresh_all_events(redis: &SharedRedisClient, polymarket: &PolymarketClient) {
    let events = match redis.list_events().await {
        Ok(e) => e,
        Err(e) => {
            error!("Failed to list events for refresh: {:?}", e);
            return;
        }
    };

    if events.is_empty() {
        return;
    }

    info!("Refreshing {} events...", events.len());
    let mut success_count = 0;
    let mut error_count = 0;

    for event_key in events {
        // Parse platform:slug format
        let parts: Vec<&str> = event_key.splitn(2, ':').collect();
        if parts.len() != 2 {
            warn!("Invalid event key format: {}", event_key);
            continue;
        }

        let platform = parts[0];
        let slug = parts[1];

        // Currently only polymarket is supported
        if platform != "polymarket" {
            continue;
        }

        match polymarket.fetch_event_data(slug).await {
            Ok(event_data) => {
                if let Err(e) = redis.store_event(platform, slug, &event_data).await {
                    error!("Failed to update event {}:{}: {:?}", platform, slug, e);
                    error_count += 1;
                } else {
                    success_count += 1;
                }
            }
            Err(e) => {
                error!("Failed to fetch event {}:{}: {:?}", platform, slug, e);
                error_count += 1;
            }
        }
    }

    info!(
        "Refresh complete: {} updated, {} errors",
        success_count, error_count
    );
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

    info!("Starting event service...");

    // Initialize Prometheus metrics
    let metrics_port: u16 = std::env::var("METRICS_PORT")
        .unwrap_or_else(|_| "9092".into())
        .parse()
        .unwrap_or(9092);

    PrometheusBuilder::new()
        .with_http_listener(([0, 0, 0, 0], metrics_port))
        .install()?;

    info!(
        "Prometheus metrics available at http://0.0.0.0:{}/metrics",
        metrics_port
    );

    // Configuration from environment
    let redis_url = std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://localhost:6379".into());
    let http_port: u16 = std::env::var("HTTP_PORT")
        .unwrap_or_else(|_| "8081".into())
        .parse()
        .unwrap_or(8081);
    let event_slug = std::env::var("EVENT_SLUG").ok();
    let refresh_interval_secs: u64 = std::env::var("REFRESH_INTERVAL_SECS")
        .unwrap_or_else(|_| "60".into())
        .parse()
        .unwrap_or(60);

    // Connect to Redis
    info!("Connecting to Redis at {}...", redis_url);
    let redis_client = SharedRedisClient::new(&redis_url)?;
    info!("Connected to Redis");

    // Create Polymarket client
    let polymarket_client = PolymarketClient::new();

    // If EVENT_SLUG is provided, fetch and store on startup
    if let Some(slug) = &event_slug {
        info!("Fetching initial event: polymarket:{}", slug);
        match polymarket_client.fetch_event_data(slug).await {
            Ok(event_data) => {
                // Store with platform prefix
                if let Err(e) = redis_client
                    .store_event("polymarket", slug, &event_data)
                    .await
                {
                    error!("Failed to store initial event: {:?}", e);
                } else {
                    info!(
                        "Stored initial event 'polymarket:{}' with {} tokens",
                        slug,
                        event_data.tokens.len()
                    );
                }
            }
            Err(e) => {
                error!("Failed to fetch initial event '{}': {:?}", slug, e);
            }
        }
    }

    // Create shared state
    let redis_client = Arc::new(redis_client);
    let polymarket_client = Arc::new(polymarket_client);

    // Spawn background refresh task
    if refresh_interval_secs > 0 {
        let refresh_redis = redis_client.clone();
        let refresh_polymarket = polymarket_client.clone();
        let interval = Duration::from_secs(refresh_interval_secs);

        tokio::spawn(async move {
            info!(
                "Background refresh task started (interval: {}s)",
                refresh_interval_secs
            );
            loop {
                tokio::time::sleep(interval).await;
                refresh_all_events(&refresh_redis, &refresh_polymarket).await;
            }
        });
    } else {
        info!("Background refresh disabled (REFRESH_INTERVAL_SECS=0)");
    }

    // Create HTTP server
    let app_state = AppState {
        redis: (*redis_client).clone(),
        polymarket: (*polymarket_client).clone(),
    };
    let router = create_router(app_state);

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", http_port)).await?;
    info!("HTTP API listening on http://0.0.0.0:{}", http_port);
    info!("Available endpoints:");
    info!("  GET  /health                              - Health check");
    info!("  GET  /events                              - List all stored events");
    info!("  GET  /event/{{platform}}/{{slug}}            - Get event data");
    info!("  GET  /event/{{platform}}/{{slug}}/tokens     - Get token mappings");
    info!("  POST /event/{{platform}}/{{slug}}/refresh    - Refresh event from API");

    // Run HTTP server
    axum::serve(listener, router).await?;

    info!("Event service stopped");
    Ok(())
}
