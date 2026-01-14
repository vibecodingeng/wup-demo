//! Gateway service entry point.
//!
//! WebSocket gateway for real-time orderbook streaming to clients.

use anyhow::Result;
use gateway::{create_router, AppState, ChangeRouter, ClientRegistry, RouterConfig};
use metrics_exporter_prometheus::PrometheusBuilder;
use nats_client::NatsClient;
use std::env;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::signal;
use tokio::sync::mpsc;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    // Load environment variables from .env file
    dotenvy::dotenv().ok();

    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    info!("Starting Gateway service");

    // Read configuration from environment
    let nats_url = env::var("NATS_URL").unwrap_or_else(|_| "nats://localhost:4222".to_string());
    let http_port: u16 = env::var("HTTP_PORT")
        .unwrap_or_else(|_| "8082".to_string())
        .parse()
        .expect("HTTP_PORT must be a number");
    let metrics_port: u16 = env::var("METRICS_PORT")
        .unwrap_or_else(|_| "9093".to_string())
        .parse()
        .expect("METRICS_PORT must be a number");
    let orderbook_service_url =
        env::var("ORDERBOOK_SERVICE_URL").unwrap_or_else(|_| "http://localhost:8080".to_string());
    let nats_subject =
        env::var("NATS_SUBJECT").unwrap_or_else(|_| "orderbook.changes.>".to_string());

    info!("Configuration:");
    info!("  NATS_URL: {}", nats_url);
    info!("  HTTP_PORT: {}", http_port);
    info!("  METRICS_PORT: {}", metrics_port);
    info!("  ORDERBOOK_SERVICE_URL: {}", orderbook_service_url);
    info!("  NATS_SUBJECT: {}", nats_subject);

    // Start Prometheus metrics server
    PrometheusBuilder::new()
        .with_http_listener(([0, 0, 0, 0], metrics_port))
        .install()
        .expect("Failed to start Prometheus exporter");
    info!("Prometheus metrics server started on port {}", metrics_port);

    // Connect to NATS
    info!("Connecting to NATS at {}", nats_url);
    let nats_client = NatsClient::connect(&nats_url).await?;
    let nats_client = Arc::new(nats_client);
    info!("Connected to NATS");

    // Create client registry
    let registry = Arc::new(ClientRegistry::new());

    // Create router config
    let router_config = RouterConfig {
        nats_subject,
        orderbook_service_url,
    };

    // Create change router
    let router = Arc::new(ChangeRouter::new(
        registry.clone(),
        nats_client.clone(),
        router_config,
    ));

    // Create shutdown channel for router
    let (router_shutdown_tx, router_shutdown_rx) = mpsc::channel(1);

    // Spawn router task
    let router_clone = router.clone();
    let router_handle = tokio::spawn(async move {
        if let Err(e) = router_clone.run(router_shutdown_rx).await {
            error!("Router error: {:?}", e);
        }
    });

    // Create application state
    let state = Arc::new(AppState { registry, router });

    // Create HTTP router
    let app = create_router(state);

    // Start HTTP server
    let addr = SocketAddr::from(([0, 0, 0, 0], http_port));
    let listener = TcpListener::bind(addr).await?;
    info!("Gateway listening on {}", addr);

    // Run server with graceful shutdown
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    // Shutdown router
    info!("Shutting down router...");
    let _ = router_shutdown_tx.send(()).await;
    let _ = router_handle.await;

    info!("Gateway stopped");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("Failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => info!("Received Ctrl+C"),
        _ = terminate => info!("Received terminate signal"),
    }
}
