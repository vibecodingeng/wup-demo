//! Event service library.
//!
//! Fetches event data from external exchanges and caches in Redis.

pub mod api;
pub mod error;

pub use api::{create_router, AppState};
pub use error::{Error, Result};
// Re-export SharedRedisClient from external_services for convenience
pub use external_services::SharedRedisClient;
