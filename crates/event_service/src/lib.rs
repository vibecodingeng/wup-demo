//! Event service library.
//!
//! Fetches event data from external exchanges and caches in Redis.

pub mod api;
pub mod error;
pub mod redis_client;

pub use api::{create_router, AppState};
pub use error::{Error, Result};
pub use redis_client::RedisClient;
