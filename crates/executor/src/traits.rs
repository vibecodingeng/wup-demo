//! Executor trait for platform-specific order execution.
//!
//! Each platform (Polymarket, Kalshi, etc.) implements this trait to provide
//! a unified interface for order execution.

use crate::{OrderRequest, OrderResponse, OrderStatus};
use async_trait::async_trait;

use crate::error::Result;

/// Trait for platform-specific order execution.
///
/// Each platform (Polymarket, Kalshi) implements this trait to provide
/// order submission, cancellation, and status queries.
///
/// # Example
///
/// ```ignore
/// #[async_trait]
/// impl Executor for PolymarketExecutor {
///     fn platform_name(&self) -> &'static str {
///         "polymarket"
///     }
///
///     async fn submit_order(&self, req: OrderRequest) -> Result<OrderResponse> {
///         // Implementation
///     }
///     // ...
/// }
/// ```
#[async_trait]
pub trait Executor: Send + Sync {
    /// Get the platform name (e.g., "polymarket", "kalshi").
    fn platform_name(&self) -> &'static str;

    /// Submit an order (limit or market) to the platform.
    ///
    /// # Order Types
    ///
    /// - **Market orders** (`order_type: Market`): Immediate execution at best price.
    ///   Requires `amount`. BUY amounts are in USDC, SELL amounts are in shares.
    ///
    /// - **Limit orders** (`order_type: GTC/GTD/FOK`): Requires `price` and `size`.
    ///
    /// # Returns
    ///
    /// Order response with order ID and status.
    async fn submit_order(&self, req: OrderRequest) -> Result<OrderResponse>;

    /// Cancel an open order.
    ///
    /// # Arguments
    ///
    /// * `order_id` - Platform-specific order ID
    async fn cancel_order(&self, order_id: &str) -> Result<()>;

    /// Get all open orders for this platform.
    async fn get_open_orders(&self) -> Result<Vec<OrderResponse>>;

    /// Get the status of a specific order.
    ///
    /// # Arguments
    ///
    /// * `order_id` - Platform-specific order ID
    async fn get_order_status(&self, order_id: &str) -> Result<OrderStatus>;

    /// Check if the platform is healthy/connected.
    async fn health_check(&self) -> Result<bool>;
}
