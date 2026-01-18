//! Polymarket executor implementing the Executor trait.
//!
//! This executor uses the official Polymarket SDK for correct EIP-712 signing.

use async_trait::async_trait;
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::{debug, info, warn};

use crate::error::{Error, Result};
use crate::platforms::polymarket::PolymarketSdkClient;
use crate::traits::Executor;
use crate::types::{OrderRequest, OrderResponse, OrderStatus, OrderType};

/// Polymarket executor using the official SDK.
///
/// This executor uses `polymarket-client-sdk` which handles all EIP-712
/// signing correctly, including automatic neg_risk detection and proper
/// amount normalization.
pub struct PolymarketExecutor {
    /// SDK client for API interaction.
    client: PolymarketSdkClient,
    /// Dry-run mode - print orders instead of submitting.
    dry_run: AtomicBool,
}

impl PolymarketExecutor {
    /// Create from environment variables.
    ///
    /// Required env vars:
    /// - `PRIVATE_KEY` or `POLYMARKET_PRIVATE_KEY` - Ethereum private key
    ///
    /// Optional:
    /// - `POLY_SIGNATURE_TYPE` - "0" (EOA), "1" (POLY_PROXY), "2" (GNOSIS_SAFE)
    /// - `DRY_RUN` - Set to "1" or "true" to enable dry-run mode
    pub async fn from_env() -> Result<Self> {
        info!("Initializing Polymarket executor with SDK client...");

        let client = PolymarketSdkClient::from_env()
            .await
            .map_err(Error::Platform)?;

        info!(
            "SDK client initialized, signer: {}",
            client.signer_address()
        );

        // Check for DRY_RUN env var
        let dry_run = std::env::var("DRY_RUN")
            .map(|v| v == "1" || v.to_lowercase() == "true")
            .unwrap_or(false);

        if dry_run {
            warn!("*** DRY RUN MODE ENABLED - Orders will NOT be submitted to Polymarket ***");
        }

        Ok(Self {
            client,
            dry_run: AtomicBool::new(dry_run),
        })
    }

    /// Alias for `from_env()` - SDK handles authentication automatically.
    pub async fn from_env_with_derivation() -> Result<Self> {
        Self::from_env().await
    }

    /// Enable or disable dry-run mode.
    pub fn set_dry_run(&self, enabled: bool) {
        self.dry_run.store(enabled, Ordering::Relaxed);
    }

    /// Check if dry-run mode is enabled.
    pub fn is_dry_run(&self) -> bool {
        self.dry_run.load(Ordering::Relaxed)
    }
}

#[async_trait]
impl Executor for PolymarketExecutor {
    fn platform_name(&self) -> &'static str {
        "polymarket"
    }

    async fn submit_order(&self, req: OrderRequest) -> Result<OrderResponse> {
        // Check for dry-run mode
        if self.is_dry_run() {
            match req.order_type {
                OrderType::Market => {
                    debug!(
                        "Submitting market {} order: {:?} for {}",
                        req.side, req.amount, req.token_id
                    );
                    info!("=== DRY RUN - Market Order NOT submitted ===");
                    info!("Platform: polymarket");
                    info!("Side: {:?}", req.side);
                    info!("Token ID: {}", req.token_id);
                    info!("Amount: {:?}", req.amount);
                    info!("=============================================");

                    return Ok(OrderResponse {
                        order_id: format!(
                            "dry-run-market-{}",
                            chrono::Utc::now().timestamp_millis()
                        ),
                        platform: "polymarket".to_string(),
                        status: OrderStatus::Pending,
                        filled_size: None,
                        remaining_size: None,
                        routed_price: None,
                    });
                }
                _ => {
                    debug!(
                        "Submitting {} limit order: {:?} {} @ {:?}",
                        req.side, req.size, req.token_id, req.price
                    );
                    info!("=== DRY RUN - Limit Order NOT submitted ===");
                    info!("Platform: polymarket");
                    info!("Side: {:?}", req.side);
                    info!("Token ID: {}", req.token_id);
                    info!("Price: {:?}", req.price);
                    info!("Size: {:?}", req.size);
                    info!("Order Type: {:?}", req.order_type);
                    info!("============================================");

                    return Ok(OrderResponse {
                        order_id: format!("dry-run-{}", chrono::Utc::now().timestamp_millis()),
                        platform: "polymarket".to_string(),
                        status: OrderStatus::Pending,
                        filled_size: None,
                        remaining_size: req.size,
                        routed_price: req.price,
                    });
                }
            }
        }

        // Submit via SDK client (handles both market and limit orders)
        self.client
            .submit_order(&req)
            .await
            .map_err(Error::Platform)
    }

    async fn cancel_order(&self, order_id: &str) -> Result<()> {
        self.client
            .cancel_order(order_id)
            .await
            .map_err(Error::Platform)
    }

    async fn get_open_orders(&self) -> Result<Vec<OrderResponse>> {
        let orders = self
            .client
            .get_open_orders()
            .await
            .map_err(Error::Platform)?;

        Ok(orders.into_iter().map(|o| o.into()).collect())
    }

    async fn get_order_status(&self, order_id: &str) -> Result<OrderStatus> {
        let order = self
            .client
            .get_order(order_id)
            .await
            .map_err(Error::Platform)?;

        Ok(order.status.into())
    }

    async fn health_check(&self) -> Result<bool> {
        self.client.health_check().await.map_err(Error::Platform)
    }
}

impl std::fmt::Debug for PolymarketExecutor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PolymarketExecutor")
            .field("client", &self.client)
            .field("dry_run", &self.is_dry_run())
            .finish()
    }
}
