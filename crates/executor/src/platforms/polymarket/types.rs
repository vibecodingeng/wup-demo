//! Polymarket-specific types for CLOB API.

use polymarket_client_sdk::clob::types::OrderStatusType;
use serde::{Deserialize, Serialize};

/// Tick size for Polymarket markets.
///
/// Different markets have different tick sizes which affect rounding precision:
/// - `Tenth` (0.1): Price 1 decimal, Size 2 decimals, Amount 3 decimals
/// - `Hundredth` (0.01): Price 2 decimals, Size 2 decimals, Amount 4 decimals
/// - `Thousandth` (0.001): Price 3 decimals, Size 2 decimals, Amount 5 decimals (most common)
/// - `TenThousandth` (0.0001): Price 4 decimals, Size 2 decimals, Amount 6 decimals
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TickSize {
    /// 0.1 tick size
    Tenth,
    /// 0.01 tick size
    Hundredth,
    /// 0.001 tick size (most common)
    #[default]
    Thousandth,
    /// 0.0001 tick size
    TenThousandth,
}

impl TickSize {
    /// Get the rounding configuration for this tick size.
    pub fn round_config(&self) -> RoundConfig {
        match self {
            TickSize::Tenth => RoundConfig {
                price: 1,
                size: 2,
                amount: 3,
            },
            TickSize::Hundredth => RoundConfig {
                price: 2,
                size: 2,
                amount: 4,
            },
            TickSize::Thousandth => RoundConfig {
                price: 3,
                size: 2,
                amount: 5,
            },
            TickSize::TenThousandth => RoundConfig {
                price: 4,
                size: 2,
                amount: 6,
            },
        }
    }

    /// Parse tick size from string representation.
    ///
    /// # Examples
    ///
    /// ```
    /// use platforms::polymarket::TickSize;
    ///
    /// assert_eq!(TickSize::parse("0.001"), Some(TickSize::Thousandth));
    /// assert_eq!(TickSize::parse("invalid"), None);
    /// ```
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "0.1" => Some(TickSize::Tenth),
            "0.01" => Some(TickSize::Hundredth),
            "0.001" => Some(TickSize::Thousandth),
            "0.0001" => Some(TickSize::TenThousandth),
            _ => None,
        }
    }

    /// Convert to string representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            TickSize::Tenth => "0.1",
            TickSize::Hundredth => "0.01",
            TickSize::Thousandth => "0.001",
            TickSize::TenThousandth => "0.0001",
        }
    }
}

/// Rounding configuration for order amounts based on tick size.
#[derive(Debug, Clone, Copy)]
pub struct RoundConfig {
    /// Number of decimal places for price rounding.
    pub price: u32,
    /// Number of decimal places for size rounding (always 2).
    pub size: u32,
    /// Maximum decimal places for maker/taker amount.
    pub amount: u32,
}

/// Polymarket order side (numeric representation).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolymarketSide {
    /// Buy side (0)
    Buy = 0,
    /// Sell side (1)
    Sell = 1,
}

impl From<crate::types::OrderSide> for PolymarketSide {
    fn from(side: crate::types::OrderSide) -> Self {
        match side {
            crate::types::OrderSide::Buy => PolymarketSide::Buy,
            crate::types::OrderSide::Sell => PolymarketSide::Sell,
        }
    }
}

/// Polymarket signed order structure for submission.
#[derive(Debug, Clone, Serialize)]
pub struct SignedOrder {
    /// Random salt for uniqueness.
    pub salt: String,
    /// Maker address (funder).
    pub maker: String,
    /// Signer address (can be same as maker).
    pub signer: String,
    /// Taker address (0x0 for open orders).
    pub taker: String,
    /// Token ID (CLOB token ID).
    #[serde(rename = "tokenId")]
    pub token_id: String,
    /// Maker amount (what maker provides).
    #[serde(rename = "makerAmount")]
    pub maker_amount: String,
    /// Taker amount (what maker receives).
    #[serde(rename = "takerAmount")]
    pub taker_amount: String,
    /// Expiration timestamp.
    pub expiration: String,
    /// Nonce for replay protection.
    pub nonce: String,
    /// Fee rate in basis points.
    #[serde(rename = "feeRateBps")]
    pub fee_rate_bps: String,
    /// Order side (0 = BUY, 1 = SELL).
    pub side: String,
    /// Signature type (0 = EOA, 1 = POLY_PROXY, 2 = POLY_GNOSIS_SAFE).
    #[serde(rename = "signatureType")]
    pub signature_type: u8,
    /// EIP-712 signature.
    pub signature: String,
}

/// Order submission request body.
#[derive(Debug, Clone, Serialize)]
pub struct OrderSubmitRequest {
    /// Signed order data.
    pub order: SignedOrder,
    /// Owner (API key).
    pub owner: String,
    /// Order type (GTC, GTD, FOK).
    #[serde(rename = "orderType")]
    pub order_type: String,
    /// Post-only flag (optional, prevents immediate matching).
    #[serde(rename = "postOnly", skip_serializing_if = "Option::is_none")]
    pub post_only: Option<bool>,
}

/// Order submission response.
#[derive(Debug, Clone, Deserialize)]
pub struct OrderSubmitResponse {
    /// Order ID.
    #[serde(rename = "orderID")]
    pub order_id: String,
    /// Whether the order was successfully placed.
    pub success: bool,
    /// Error message if any.
    #[serde(default)]
    pub error_msg: Option<String>,
    /// Transaction hashes if immediately filled.
    #[serde(default, rename = "transactionsHashes")]
    pub transaction_hashes: Option<Vec<String>>,
}

/// Open order from the API.
#[derive(Debug, Clone, Deserialize)]
pub struct OpenOrder {
    /// Order ID.
    #[serde(rename = "orderID")]
    pub order_id: String,
    /// Order hash.
    #[serde(rename = "orderHash")]
    pub order_hash: String,
    /// Market condition ID.
    #[serde(rename = "conditionID")]
    pub condition_id: String,
    /// Token ID.
    #[serde(rename = "tokenID")]
    pub token_id: String,
    /// Order side.
    pub side: String,
    /// Order price.
    pub price: String,
    /// Original size.
    #[serde(rename = "originalSize")]
    pub original_size: String,
    /// Size remaining.
    #[serde(rename = "sizeRemaining")]
    pub size_remaining: String,
    /// Order type.
    #[serde(rename = "type")]
    pub order_type: String,
    /// Order status.
    pub status: OrderStatusType,
    /// Created timestamp.
    #[serde(rename = "createdAt")]
    pub created_at: String,
}

/// API credentials response (from L1 auth derivation).
#[derive(Debug, Clone, Deserialize)]
pub struct ApiCredentials {
    /// API key.
    #[serde(rename = "apiKey")]
    pub api_key: String,
    /// Secret (base64 encoded).
    pub secret: String,
    /// Passphrase.
    pub passphrase: String,
}

/// Market information response.
#[derive(Debug, Clone, Deserialize)]
pub struct MarketInfo {
    /// Condition ID.
    #[serde(rename = "condition_id")]
    pub condition_id: String,
    /// Question.
    pub question: String,
    /// Token IDs.
    pub tokens: Vec<TokenInfo>,
    /// Whether this market uses neg_risk.
    #[serde(default)]
    pub neg_risk: bool,
}

/// Token information.
#[derive(Debug, Clone, Deserialize)]
pub struct TokenInfo {
    /// Token ID.
    #[serde(rename = "token_id")]
    pub token_id: String,
    /// Outcome name.
    pub outcome: String,
}

/// Cancel orders request.
#[derive(Debug, Clone, Serialize)]
pub struct CancelOrdersRequest {
    /// Order IDs to cancel.
    #[serde(rename = "orderIDs")]
    pub order_ids: Vec<String>,
}

/// Cancel orders response.
#[derive(Debug, Clone, Deserialize)]
pub struct CancelOrdersResponse {
    /// Cancelled order IDs.
    #[serde(default)]
    pub cancelled: Vec<String>,
    /// Failed cancellations.
    #[serde(default)]
    pub failed: Vec<String>,
}

/// Convert an OpenOrder to the common OrderResponse type.
impl From<OpenOrder> for crate::OrderResponse {
    fn from(order: OpenOrder) -> Self {
        use rust_decimal::Decimal;
        use std::str::FromStr;

        let remaining = Decimal::from_str(&order.size_remaining).ok();
        let original = Decimal::from_str(&order.original_size).ok();
        let filled = match (original, remaining) {
            (Some(o), Some(r)) => Some(o - r),
            _ => None,
        };

        Self {
            order_id: order.order_id,
            platform: "polymarket".to_string(),
            status: order.status.into(),
            filled_size: filled,
            remaining_size: remaining,
            routed_price: None,
        }
    }
}
