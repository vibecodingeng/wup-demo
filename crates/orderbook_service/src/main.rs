//! Orderbook service entry point.
//!
//! Subscribes to normalized orderbook messages from NATS and maintains
//! in-memory orderbook state with HTTP API access.

use anyhow::Result;
use metrics_exporter_prometheus::PrometheusBuilder;
use nats_client::NatsClient;
use orderbook_service::{
    create_router, AppState, OrderbookService, OrderbookServiceConfig, OrderbookStore,
};
use tokio::sync::mpsc;
use tracing::{error, info};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into()),
        ))
        .with(tracing_subscriber::fmt::layer())
        .init();

    info!("Starting orderbook service...");

    // Initialize Prometheus metrics
    let metrics_port: u16 = std::env::var("METRICS_PORT")
        .unwrap_or_else(|_| "9091".into())
        .parse()
        .unwrap_or(9091);

    PrometheusBuilder::new()
        .with_http_listener(([0, 0, 0, 0], metrics_port))
        .install()?;

    info!(
        "Prometheus metrics available at http://0.0.0.0:{}/metrics",
        metrics_port
    );

    // Configuration from environment
    let nats_url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://localhost:4222".into());
    let redis_url =
        Some(std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://localhost:6379".into()));
    let http_port: u16 = std::env::var("HTTP_PORT")
        .unwrap_or_else(|_| "8080".into())
        .parse()
        .unwrap_or(8080);
    let nats_subject =
        std::env::var("NATS_SUBJECT").unwrap_or_else(|_| "normalized.polymarket.>".into());

    // Create shared store (lock-free)
    let store = OrderbookStore::new();

    // Connect to NATS
    info!("Connecting to NATS at {}...", nats_url);
    let nats_client = NatsClient::connect(&nats_url).await?;
    info!("Connected to NATS");

    // Create shutdown channel
    let (shutdown_tx, shutdown_rx) = mpsc::channel::<()>(1);

    // Check if change publishing is enabled (for gateway integration)
    let publish_changes = std::env::var("PUBLISH_CHANGES")
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false);

    if publish_changes {
        info!("Change publishing ENABLED - will publish to orderbook.changes.>");
    } else {
        info!("Change publishing DISABLED - set PUBLISH_CHANGES=true to enable gateway integration");
    }

    // Create and spawn orderbook service
    let service_config = OrderbookServiceConfig {
        subject: nats_subject.clone(),
        metrics_interval_secs: 5,
        redis_url,
        publish_changes,
    };

    let service = OrderbookService::new(store.clone(), nats_client, service_config, shutdown_rx);

    let service_handle = tokio::spawn(async move {
        if let Err(e) = service.run().await {
            error!("OrderbookService failed: {:?}", e);
        }
    });

    info!(
        "OrderbookService spawned, subscribing to '{}'",
        nats_subject
    );

    // Create HTTP server
    let app_state = AppState { store };
    let router = create_router(app_state);

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", http_port)).await?;
    info!("HTTP API listening on http://0.0.0.0:{}", http_port);
    info!("Available endpoints:");
    info!("  GET /health              - Health check");
    info!("  GET /stats               - Service statistics");
    info!("  GET /orderbooks          - List all markets and assets");
    info!("  GET /orderbook/{{market}} - All orderbooks for a market");
    info!("  GET /orderbook/{{market}}/{{asset}} - Specific orderbook");
    info!("  GET /bbo/{{market}}       - All BBOs for a market");
    info!("  GET /bbo/{{market}}/{{asset}} - Specific BBO");

    // Run HTTP server with graceful shutdown
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal(shutdown_tx))
        .await?;

    // Wait for service to stop
    let _ = service_handle.await;

    info!("Orderbook service stopped");
    Ok(())
}

/// Wait for shutdown signal (Ctrl+C).
async fn shutdown_signal(shutdown_tx: mpsc::Sender<()>) {
    tokio::signal::ctrl_c().await.ok();
    info!("Received shutdown signal");
    let _ = shutdown_tx.send(()).await;
}
