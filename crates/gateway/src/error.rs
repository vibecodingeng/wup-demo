//! Gateway error types.

use thiserror::Error;

/// Gateway error type.
#[derive(Debug, Error)]
pub enum GatewayError {
    /// NATS connection error.
    #[error("NATS error: {0}")]
    Nats(#[from] async_nats::Error),

    /// NATS subscription error.
    #[error("NATS subscription error: {0}")]
    NatsSubscribe(#[from] async_nats::SubscribeError),

    /// Anyhow error (for compatibility with nats_client).
    #[error("Error: {0}")]
    Anyhow(#[from] anyhow::Error),

    /// JSON serialization/deserialization error.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// HTTP client error.
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    /// WebSocket error.
    #[error("WebSocket error: {0}")]
    WebSocket(String),

    /// Client not found.
    #[error("Client not found: {0}")]
    ClientNotFound(String),

    /// Invalid subscription subject.
    #[error("Invalid subscription subject: {0}")]
    InvalidSubject(String),

    /// Channel send error.
    #[error("Channel send error")]
    ChannelSend,

    /// Internal error.
    #[error("Internal error: {0}")]
    Internal(String),
}

impl From<tokio::sync::mpsc::error::SendError<axum::extract::ws::Message>> for GatewayError {
    fn from(_: tokio::sync::mpsc::error::SendError<axum::extract::ws::Message>) -> Self {
        GatewayError::ChannelSend
    }
}

/// Result type for gateway operations.
pub type Result<T> = std::result::Result<T, GatewayError>;
