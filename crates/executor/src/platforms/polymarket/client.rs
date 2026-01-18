//! Polymarket SDK client wrapper.
//!
//! This module wraps the official `polymarket-client-sdk` crate to provide
//! a simpler interface that matches our existing abstractions.

use polymarket_client_sdk::auth::state::Authenticated;
use polymarket_client_sdk::auth::{LocalSigner, Normal, Signer};
use polymarket_client_sdk::clob::types::request::OrdersRequest;
use polymarket_client_sdk::clob::types::{
    Amount, OrderStatusType, OrderType as SdkOrderType, Side, SignatureType,
};
use polymarket_client_sdk::clob::{Client, Config};
use std::sync::Arc;
use tracing::{debug, info};

use crate::platforms::error::{Error, Result};
use crate::platforms::polymarket::types::OpenOrder;
use crate::types::{OrderRequest, OrderResponse, OrderSide, OrderStatus, OrderType};

impl From<OrderStatusType> for OrderStatus {
    fn from(status: OrderStatusType) -> Self {
        match status {
            OrderStatusType::Live => OrderStatus::Open,
            OrderStatusType::Matched => OrderStatus::Matched,
            OrderStatusType::Canceled => OrderStatus::Cancelled,
            OrderStatusType::Delayed => OrderStatus::Pending,
            OrderStatusType::Unknown => OrderStatus::Unknown,
            _ => OrderStatus::Unknown, // Handle any future variants
        }
    }
}

// Type alias for secp256k1 curve used by Ethereum
type K256Signer = LocalSigner<k256::ecdsa::SigningKey>;

/// Polymarket SDK client wrapper.
///
/// Uses the official `polymarket-client-sdk` for correct EIP-712 signing
/// and API interaction.
pub struct PolymarketSdkClient {
    /// The authenticated SDK client.
    client: Client<Authenticated<Normal>>,
    /// The local signer for order signing.
    signer: Arc<K256Signer>,
}

impl PolymarketSdkClient {
    /// Create a new SDK client from environment variables.
    ///
    /// Required environment variables:
    /// - `PRIVATE_KEY` or `POLYMARKET_PRIVATE_KEY` - Ethereum private key (hex, with or without 0x prefix)
    ///
    /// Optional:
    /// - `POLY_SIGNATURE_TYPE` - "0" (EOA), "1" (PROXY), "2" (GNOSIS_SAFE)
    pub async fn from_env() -> Result<Self> {
        // Try PRIVATE_KEY first, fall back to POLYMARKET_PRIVATE_KEY
        let private_key = std::env::var("PRIVATE_KEY")
            .or_else(|_| std::env::var("POLYMARKET_PRIVATE_KEY"))
            .map_err(|_| Error::MissingEnv("PRIVATE_KEY or POLYMARKET_PRIVATE_KEY"))?;

        // Strip 0x prefix if present
        let key_str = private_key.strip_prefix("0x").unwrap_or(&private_key);

        // Parse the private key and create signer with Polygon chain ID
        let signer: K256Signer = key_str
            .parse()
            .map_err(|e| Error::InvalidPrivateKey(format!("{:?}", e)))?;

        // Set chain ID for Polygon (137)
        let signer = signer.with_chain_id(Some(137));

        Self::with_signer(signer).await
    }

    /// Create a new SDK client with an explicit signer.
    pub async fn with_signer(signer: K256Signer) -> Result<Self> {
        let signer = Arc::new(signer);

        // Determine signature type from environment
        let sig_type = std::env::var("POLY_SIGNATURE_TYPE").unwrap_or_else(|_| "0".to_string());

        // Get host from environment or use default
        let host = std::env::var("POLYMARKET_CLOB_HOST")
            .unwrap_or_else(|_| "https://clob.polymarket.com".to_string());

        // Use server time if configured (adds latency but more accurate)
        let use_server_time = std::env::var("POLYMARKET_USE_SERVER_TIME")
            .map(|v| v == "1" || v.to_lowercase() == "true")
            .unwrap_or(false);

        info!("Initializing Polymarket SDK client...");
        info!("Host: {}", host);
        info!("Signer address: {:?}", signer.address());
        info!("Signature type: {}", sig_type);
        info!("Use server time: {}", use_server_time);

        // Build configuration using the Config builder API
        let config = Config::builder().use_server_time(use_server_time).build();

        // Create the client with explicit host and config
        let client = Client::new(&host, config)
            .map_err(|e| Error::Signing(format!("Failed to create client: {}", e)))?;

        // Build authentication based on signature type
        let auth_builder = client.authentication_builder(&*signer);

        let client = match sig_type.as_str() {
            "1" => {
                info!("Using PROXY signature type");
                auth_builder
                    .signature_type(SignatureType::Proxy)
                    .authenticate()
                    .await
                    .map_err(|e| Error::Signing(e.to_string()))?
            }
            "2" => {
                info!("Using GNOSIS_SAFE signature type");
                auth_builder
                    .signature_type(SignatureType::GnosisSafe)
                    .authenticate()
                    .await
                    .map_err(|e| Error::Signing(e.to_string()))?
            }
            _ => {
                info!("Using EOA signature type");
                auth_builder
                    .signature_type(SignatureType::Eoa)
                    .authenticate()
                    .await
                    .map_err(|e| Error::Signing(e.to_string()))?
            }
        };

        info!("Polymarket SDK client authenticated successfully");

        Ok(Self { client, signer })
    }

    /// Submit an order (limit or market).
    ///
    /// - **Market orders** (`order_type: Market`): Requires `amount`.
    ///   - BUY: Amount is in USDC
    ///   - SELL: Amount is in shares (Polymarket API requirement)
    ///
    /// - **Limit orders** (`order_type: GTC/GTD/FOK`): Requires `price` and `size`.
    pub async fn submit_order(&self, req: &OrderRequest) -> Result<OrderResponse> {
        let side = match req.side {
            OrderSide::Buy => Side::Buy,
            OrderSide::Sell => Side::Sell,
        };

        match req.order_type {
            OrderType::Market => {
                // Market order - requires amount
                let amount_value = req
                    .amount
                    .ok_or_else(|| Error::InvalidOrder("Market orders require 'amount'".into()))?;

                debug!(
                    "SDK: Submitting market {} order: {} {} for {}",
                    req.side,
                    amount_value,
                    if req.side == OrderSide::Buy {
                        "USDC"
                    } else {
                        "shares"
                    },
                    req.token_id
                );

                // Key fix: BUY orders use USDC, SELL orders use shares
                let amount = match req.side {
                    OrderSide::Buy => Amount::usdc(amount_value),
                    OrderSide::Sell => Amount::shares(amount_value),
                }
                .map_err(|e| Error::InvalidOrder(e.to_string()))?;

                let order = self
                    .client
                    .market_order()
                    .token_id(&req.token_id)
                    .amount(amount)
                    .side(side)
                    .order_type(SdkOrderType::FOK)
                    .build()
                    .await
                    .map_err(|e| Error::InvalidOrder(e.to_string()))?;

                debug!("SDK: Market order built, signing...");

                let signed = self
                    .client
                    .sign(&*self.signer, order)
                    .await
                    .map_err(|e| Error::Signing(e.to_string()))?;

                debug!("SDK: Market order signed, submitting...");

                let response =
                    self.client
                        .post_order(signed)
                        .await
                        .map_err(|e| Error::ApiError {
                            status: 0,
                            message: e.to_string(),
                        })?;

                info!(
                    "SDK: Market order submitted successfully, id={}",
                    response.order_id
                );

                Ok(OrderResponse {
                    order_id: response.order_id.to_string(),
                    platform: "polymarket".to_string(),
                    status: response.status.into(),
                    filled_size: None,
                    remaining_size: None,
                    routed_price: None,
                })
            }
            order_type => {
                // Limit order - requires price and size
                let price = req
                    .price
                    .ok_or_else(|| Error::InvalidOrder("Limit orders require 'price'".into()))?;
                let size = req
                    .size
                    .ok_or_else(|| Error::InvalidOrder("Limit orders require 'size'".into()))?;

                debug!(
                    "SDK: Submitting {} limit order: {} {} @ {}",
                    req.side, size, req.token_id, price
                );

                let sdk_order_type = match order_type {
                    OrderType::GTC => SdkOrderType::GTC,
                    OrderType::GTD => SdkOrderType::GTD,
                    OrderType::FOK => SdkOrderType::FOK,
                    OrderType::Market => unreachable!(), // Already handled above
                };

                let order = self
                    .client
                    .limit_order()
                    .token_id(&req.token_id)
                    .size(size)
                    .price(price)
                    .side(side)
                    .order_type(sdk_order_type)
                    .build()
                    .await
                    .map_err(|e| Error::InvalidOrder(e.to_string()))?;

                debug!("SDK: Limit order built, signing...");

                let signed = self
                    .client
                    .sign(&*self.signer, order)
                    .await
                    .map_err(|e| Error::Signing(e.to_string()))?;

                debug!("SDK: Limit order signed, submitting...");

                let response =
                    self.client
                        .post_order(signed)
                        .await
                        .map_err(|e| Error::ApiError {
                            status: 0,
                            message: e.to_string(),
                        })?;

                info!(
                    "SDK: Limit order submitted successfully, id={}",
                    response.order_id
                );

                Ok(OrderResponse {
                    order_id: response.order_id.to_string(),
                    platform: "polymarket".to_string(),
                    status: response.status.into(),
                    filled_size: None,
                    remaining_size: Some(size),
                    routed_price: Some(price),
                })
            }
        }
    }

    /// Cancel an order by ID.
    pub async fn cancel_order(&self, order_id: &str) -> Result<()> {
        debug!("SDK: Cancelling order {}", order_id);

        self.client
            .cancel_order(order_id)
            .await
            .map_err(|e| Error::ApiError {
                status: 0,
                message: e.to_string(),
            })?;

        info!("SDK: Order {} cancelled", order_id);
        Ok(())
    }

    /// Cancel all open orders.
    pub async fn cancel_all_orders(&self) -> Result<()> {
        debug!("SDK: Cancelling all orders");

        self.client
            .cancel_all_orders()
            .await
            .map_err(|e| Error::ApiError {
                status: 0,
                message: e.to_string(),
            })?;

        info!("SDK: All orders cancelled");
        Ok(())
    }

    /// Get open orders.
    pub async fn get_open_orders(&self) -> Result<Vec<OpenOrder>> {
        debug!("SDK: Fetching open orders");

        // Create request for all orders
        let request = OrdersRequest::default();

        let page = self
            .client
            .orders(&request, None)
            .await
            .map_err(|e| Error::ApiError {
                status: 0,
                message: e.to_string(),
            })?;

        Ok(page
            .data
            .into_iter()
            .map(|o| OpenOrder {
                order_id: o.id.to_string(),
                order_hash: String::new(),
                condition_id: o.market.to_string(),
                token_id: o.asset_id.to_string(),
                side: format!("{:?}", o.side),
                price: o.price.to_string(),
                original_size: o.original_size.to_string(),
                size_remaining: o.size_matched.to_string(),
                order_type: String::new(),
                status: o.status,
                created_at: String::new(),
            })
            .collect())
    }

    /// Get a specific order by ID.
    pub async fn get_order(&self, order_id: &str) -> Result<OpenOrder> {
        debug!("SDK: Fetching order {}", order_id);

        let order = self
            .client
            .order(order_id)
            .await
            .map_err(|e| Error::ApiError {
                status: 0,
                message: e.to_string(),
            })?;

        Ok(OpenOrder {
            order_id: order.id.to_string(),
            order_hash: String::new(),
            condition_id: order.market.to_string(),
            token_id: order.asset_id.to_string(),
            side: format!("{:?}", order.side),
            price: order.price.to_string(),
            original_size: order.original_size.to_string(),
            size_remaining: order.size_matched.to_string(),
            order_type: String::new(),
            status: order.status,
            created_at: String::new(),
        })
    }

    /// Perform a health check.
    pub async fn health_check(&self) -> Result<bool> {
        debug!("SDK: Health check");

        self.client.ok().await.map_err(|e| Error::ApiError {
            status: 0,
            message: e.to_string(),
        })?;

        Ok(true)
    }

    /// Get the signer's address.
    pub fn signer_address(&self) -> String {
        format!("{:?}", self.signer.address())
    }
}

impl std::fmt::Debug for PolymarketSdkClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PolymarketSdkClient")
            .field("signer_address", &self.signer_address())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_order_side_conversion() {
        // Just verify the mapping compiles correctly
        let _buy: Side = match OrderSide::Buy {
            OrderSide::Buy => Side::Buy,
            OrderSide::Sell => Side::Sell,
        };
        let _sell: Side = match OrderSide::Sell {
            OrderSide::Buy => Side::Buy,
            OrderSide::Sell => Side::Sell,
        };
    }
}
