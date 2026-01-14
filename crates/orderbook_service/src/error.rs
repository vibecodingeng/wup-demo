//! Error types for the orderbook service.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("NATS error: {0}")]
    Nats(#[from] async_nats::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("UTF-8 error: {0}")]
    Utf8(#[from] std::str::Utf8Error),

    #[error("Orderbook not found: {0}")]
    NotFound(String),

    #[error("Internal error: {0}")]
    Internal(String),
}

pub type Result<T> = std::result::Result<T, Error>;
