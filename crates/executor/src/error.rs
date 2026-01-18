//! Error types for the executor service.

use thiserror::Error;

/// Result type alias for executor operations.
pub type Result<T> = std::result::Result<T, Error>;

/// Executor errors.
#[derive(Debug, Error)]
pub enum Error {
    /// Missing environment variable.
    #[error("Missing environment variable: {0}")]
    MissingEnv(&'static str),

    /// Unknown platform.
    #[error("Unknown platform: {0}")]
    UnknownPlatform(String),

    /// No available platform for routing.
    #[error("No available platform for order routing")]
    NoAvailablePlatform,

    /// Platform error.
    #[error("Platform error: {0}")]
    Platform(#[from] crate::platforms::error::Error),

    /// JSON error.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// NATS error.
    #[error("NATS error: {0}")]
    Nats(String),

    /// Decimal parse error.
    #[error("Decimal parse error: {0}")]
    DecimalParse(#[from] rust_decimal::Error),

    /// Order submission failed.
    #[error("Order submission failed: {0}")]
    OrderFailed(String),
}
