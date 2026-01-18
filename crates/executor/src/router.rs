//! Smart order router using NATS-updated local BBO cache.
//!
//! The SmartRouter routes orders to the platform with the best price:
//! - BUY orders go to the platform with the lowest ask
//! - SELL orders go to the platform with the highest bid
//!
//! If a platform is specified in the request, smart routing is bypassed.

use rust_decimal::Decimal;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::info;

use crate::bbo_cache::BboCache;
use crate::error::{Error, Result};
use crate::traits::Executor;
use crate::{OrderRequest, OrderResponse, OrderSide, OrderType};

/// Smart order router using NATS-updated local BBO cache.
///
/// Provides ~0μs lookup latency for routing decisions.
pub struct SmartRouter {
    /// Platform name -> Executor
    executors: HashMap<String, Arc<dyn Executor>>,
    /// Local BBO cache (updated via NATS subscription)
    bbo_cache: Arc<BboCache>,
}

impl SmartRouter {
    /// Create router with NATS-based BBO cache.
    pub fn new(bbo_cache: Arc<BboCache>) -> Self {
        Self {
            executors: HashMap::new(),
            bbo_cache,
        }
    }

    /// Register an executor for a platform.
    pub fn register(&mut self, executor: Arc<dyn Executor>) {
        let name = executor.platform_name().to_string();
        info!("Registering executor for platform: {}", name);
        self.executors.insert(name, executor);
    }

    /// Get executor by platform name.
    pub fn get_executor(&self, platform: &str) -> Option<Arc<dyn Executor>> {
        self.executors.get(platform).cloned()
    }

    /// Get all registered platform names.
    pub fn platforms(&self) -> Vec<&str> {
        self.executors.keys().map(|s| s.as_str()).collect()
    }

    /// Find best platform from local cache.
    ///
    /// - BUY: find lowest ask price
    /// - SELL: find highest bid price
    ///
    /// Only considers platforms that have registered executors.
    fn find_best_platform(
        &self,
        market_id: &str,
        asset_id: &str,
        side: OrderSide,
    ) -> Option<(String, Decimal)> {
        let result = match side {
            OrderSide::Buy => self.bbo_cache.best_ask(market_id, asset_id),
            OrderSide::Sell => self.bbo_cache.best_bid(market_id, asset_id),
        };

        // Only return if we have an executor for this platform
        result.filter(|(platform, _)| self.executors.contains_key(platform))
    }

    /// Submit order with smart routing.
    ///
    /// Handles both limit and market orders. If platform not specified,
    /// routes to platform with best price.
    pub async fn submit_order(&self, mut req: OrderRequest) -> Result<OrderResponse> {
        // If platform specified, route directly (bypass smart routing)
        if let Some(ref platform) = req.platform {
            let executor = self
                .get_executor(platform)
                .ok_or_else(|| Error::UnknownPlatform(platform.clone()))?;
            return executor.submit_order(req).await;
        }

        // Smart routing: find best platform from local cache
        let (best_platform, best_price) = self
            .find_best_platform(&req.market_id, &req.token_id, req.side)
            .ok_or(Error::NoAvailablePlatform)?;

        // Verify we have an executor for this platform
        let executor = self
            .get_executor(&best_platform)
            .ok_or_else(|| Error::UnknownPlatform(best_platform.clone()))?;

        // Log based on order type
        match req.order_type {
            OrderType::Market => {
                info!(
                    "Smart routing market order: {} {:?} -> {} (best {} at {})",
                    req.side,
                    req.amount,
                    best_platform,
                    if req.side == OrderSide::Buy {
                        "ask"
                    } else {
                        "bid"
                    },
                    best_price
                );
            }
            _ => {
                info!(
                    "Smart routing limit order: {} {:?} @ {:?} -> {} (best {} at {})",
                    req.side,
                    req.size,
                    req.price,
                    best_platform,
                    if req.side == OrderSide::Buy {
                        "ask"
                    } else {
                        "bid"
                    },
                    best_price
                );
            }
        }

        req.platform = Some(best_platform);

        let mut response = executor.submit_order(req).await?;
        response.routed_price = Some(best_price);

        Ok(response)
    }

    /// Cancel an order on a specific platform.
    pub async fn cancel_order(&self, platform: &str, order_id: &str) -> Result<()> {
        let executor = self
            .get_executor(platform)
            .ok_or_else(|| Error::UnknownPlatform(platform.to_string()))?;
        executor.cancel_order(order_id).await
    }

    /// Get open orders from a specific platform.
    pub async fn get_open_orders(&self, platform: &str) -> Result<Vec<OrderResponse>> {
        let executor = self
            .get_executor(platform)
            .ok_or_else(|| Error::UnknownPlatform(platform.to_string()))?;
        executor.get_open_orders().await
    }

    /// Health check for all registered platforms.
    pub async fn health_check(&self) -> HashMap<String, bool> {
        let mut results = HashMap::new();
        for (name, executor) in &self.executors {
            let healthy = executor.health_check().await.unwrap_or(false);
            results.insert(name.clone(), healthy);
        }
        results
    }

    /// Get the BBO cache reference.
    pub fn bbo_cache(&self) -> &Arc<BboCache> {
        &self.bbo_cache
    }
}

impl std::fmt::Debug for SmartRouter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SmartRouter")
            .field("platforms", &self.executors.keys().collect::<Vec<_>>())
            .field("cached_markets", &self.bbo_cache.market_count())
            .finish()
    }
}
