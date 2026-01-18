//! Error types for the platforms crate.

use thiserror::Error;

/// Result type alias for platform operations.
pub type Result<T> = std::result::Result<T, Error>;

/// Platform errors.
#[derive(Debug, Error)]
pub enum Error {
    /// Missing environment variable.
    #[error("Missing environment variable: {0}")]
    MissingEnv(&'static str),

    /// Invalid configuration.
    #[error("Invalid configuration: {0}")]
    InvalidConfig(String),

    /// HTTP request failed.
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    /// JSON serialization/deserialization error.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// Invalid private key.
    #[error("Invalid private key: {0}")]
    InvalidPrivateKey(String),

    /// Invalid address.
    #[error("Invalid address: {0}")]
    InvalidAddress(String),

    /// Signing error.
    #[error("Signing error: {0}")]
    Signing(String),

    /// No credentials configured.
    #[error("No API credentials configured. Call set_credentials() or derive_credentials() first.")]
    NoCredentials,

    /// API error response.
    #[error("API error: {status} - {message}")]
    ApiError { status: u16, message: String },

    /// Order rejected.
    #[error("Order rejected: {0}")]
    OrderRejected(String),

    /// Invalid order parameters.
    #[error("Invalid order parameters: {0}")]
    InvalidOrder(String),

    /// Hex decoding error.
    #[error("Hex decode error: {0}")]
    HexDecode(#[from] hex::FromHexError),

    /// Base64 decoding error.
    #[error("Base64 decode error: {0}")]
    Base64Decode(#[from] base64::DecodeError),
}
