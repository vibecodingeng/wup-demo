//! Executor service entry point.
//!
//! This service provides smart order routing to the best platform
//! based on local BBO cache updated via NATS subscription.

use anyhow::Result;
use executor::{api, bbo_cache::BboCache, platforms::PolymarketExecutor, router::SmartRouter};
use std::sync::Arc;
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    // Load environment variables
    dotenvy::dotenv().ok();

    // Initialize tracing
    tracing_subscriber::fmt::init();

    // Startup banner
    info!("=========================================");
    info!("       EXECUTOR SERVICE STARTING        ");
    info!("=========================================");

    // Load configuration
    let nats_url =
        std::env::var("NATS_URL").unwrap_or_else(|_| "nats://localhost:4222".to_string());
    let port: u16 = std::env::var("EXECUTOR_PORT")
        .unwrap_or_else(|_| "8083".to_string())
        .parse()?;

    info!("Configuration:");
    info!("  NATS_URL: {}", nats_url);
    info!("  EXECUTOR_PORT: {}", port);

    // Connect to NATS for BBO cache subscription
    info!("Connecting to NATS...");
    let nats_client = nats_client::NatsClient::connect(&nats_url).await?;
    info!("Connected to NATS at {}", nats_url);

    // Create BBO cache and start NATS subscription
    info!("Starting BBO cache subscription...");
    let bbo_cache = Arc::new(BboCache::new());
    bbo_cache.clone().start_subscription(nats_client).await?;
    info!("BBO cache subscription started (orderbook.changes.>)");

    // Create smart router with BBO cache
    let mut router = SmartRouter::new(bbo_cache);

    // Register Polymarket executor (uses L2 credentials from env, or derives them)
    info!("Initializing Polymarket executor...");
    let polymarket = Arc::new(PolymarketExecutor::from_env_with_derivation().await?);
    router.register(polymarket);
    info!("Registered Polymarket executor");

    // TODO: Register Kalshi executor when implemented
    // let kalshi = Arc::new(KalshiExecutor::from_env()?);
    // router.register(kalshi);

    let state = Arc::new(api::AppState { router });

    // Create HTTP router
    let app = api::create_router(state);

    // Start HTTP server
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await?;

    info!("=========================================");
    info!("  Executor service ready on port {}   ", port);
    info!("=========================================");
    info!("Endpoints:");
    info!("  POST /order              - Smart routed order");
    info!("  POST /order/:platform    - Direct platform order");
    info!("  DELETE /order/:p/:id     - Cancel order");
    info!("  GET /orders/:platform    - List open orders");
    info!("  GET /health              - Health check");
    info!("=========================================");

    axum::serve(listener, app).await?;

    Ok(())
}
