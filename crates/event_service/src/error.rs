//! Error types for the event service.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("Redis error: {0}")]
    Redis(#[from] redis::RedisError),

    #[error("External service error: {0}")]
    ExternalService(#[from] external_services::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Event not found: {0}")]
    NotFound(String),

    #[error("Internal error: {0}")]
    Internal(String),
}

pub type Result<T> = std::result::Result<T, Error>;
