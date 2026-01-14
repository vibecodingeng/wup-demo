//! Aggregate mapping types and helpers for multi-platform orderbook aggregation.
//!
//! Different platforms may have different slugs for the same event.
//! This module provides types and Redis helpers to map platform-specific slugs
//! to a unified aggregate ID.
//!
//! # Redis Key Structure
//!
//! - `{platform}:event:{slug}` -> aggregate_id (mapping)
//! - `aggregate:{agg_id}` -> AggregateMetadata (unified event info)
//! - `event:{platform}:{slug}` -> EventData (full platform-specific event data)
//!
//! # Example
//!
//! ```ignore
//! // Map polymarket and kalshi slugs to same aggregate
//! set_event_mapping(&mut conn, "polymarket", "us-election-2024", "AGG123").await?;
//! set_event_mapping(&mut conn, "kalshi", "2024-presidential", "AGG123").await?;
//!
//! // Later, resolve aggregate ID from platform slug
//! let agg_id = get_aggregate_id(&mut conn, "polymarket", "us-election-2024").await?;
//! // agg_id == Some("AGG123")
//! ```

use redis::AsyncCommands;
use serde::{Deserialize, Serialize};

/// Redis key prefix for aggregate metadata.
pub const AGGREGATE_KEY_PREFIX: &str = "aggregate:";

/// Set mapping from platform event slug to aggregate ID.
///
/// Redis key format: `{platform}:event:{slug}` -> `aggregate_id`
pub async fn set_event_mapping(
    conn: &mut redis::aio::MultiplexedConnection,
    platform: &str,
    slug: &str,
    aggregate_id: &str,
) -> redis::RedisResult<()> {
    let key = format!("{}:event:{}", platform, slug);
    conn.set(&key, aggregate_id).await
}

/// Get aggregate ID for a platform's event slug.
///
/// Returns `None` if no mapping exists.
pub async fn get_aggregate_id(
    conn: &mut redis::aio::MultiplexedConnection,
    platform: &str,
    slug: &str,
) -> redis::RedisResult<Option<String>> {
    let key = format!("{}:event:{}", platform, slug);
    conn.get(&key).await
}

/// Delete mapping from platform event slug to aggregate ID.
pub async fn delete_event_mapping(
    conn: &mut redis::aio::MultiplexedConnection,
    platform: &str,
    slug: &str,
) -> redis::RedisResult<()> {
    let key = format!("{}:event:{}", platform, slug);
    conn.del(&key).await
}

/// Store aggregate metadata in Redis.
pub async fn set_aggregate_metadata(
    conn: &mut redis::aio::MultiplexedConnection,
    metadata: &AggregateMetadata,
) -> redis::RedisResult<()> {
    let key = format!("{}{}", AGGREGATE_KEY_PREFIX, metadata.aggregate_id);
    let json = serde_json::to_string(metadata).map_err(|e| {
        redis::RedisError::from((
            redis::ErrorKind::TypeError,
            "Failed to serialize AggregateMetadata",
            e.to_string(),
        ))
    })?;
    conn.set(&key, json).await
}

/// Get aggregate metadata from Redis.
pub async fn get_aggregate_metadata(
    conn: &mut redis::aio::MultiplexedConnection,
    aggregate_id: &str,
) -> redis::RedisResult<Option<AggregateMetadata>> {
    let key = format!("{}{}", AGGREGATE_KEY_PREFIX, aggregate_id);
    let json: Option<String> = conn.get(&key).await?;
    match json {
        Some(j) => {
            let metadata: AggregateMetadata = serde_json::from_str(&j).map_err(|e| {
                redis::RedisError::from((
                    redis::ErrorKind::TypeError,
                    "Failed to deserialize AggregateMetadata",
                    e.to_string(),
                ))
            })?;
            Ok(Some(metadata))
        }
        None => Ok(None),
    }
}

/// Delete aggregate metadata from Redis.
pub async fn delete_aggregate_metadata(
    conn: &mut redis::aio::MultiplexedConnection,
    aggregate_id: &str,
) -> redis::RedisResult<()> {
    let key = format!("{}{}", AGGREGATE_KEY_PREFIX, aggregate_id);
    conn.del(&key).await
}

/// Unified aggregate metadata for cross-platform event aggregation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregateMetadata {
    /// Unique aggregate identifier (e.g., "AGG123").
    pub aggregate_id: String,
    /// Human-readable name for the aggregate.
    pub name: String,
    /// Optional description.
    pub description: Option<String>,
    /// List of platforms that have mapped to this aggregate.
    pub platforms: Vec<String>,
    /// Timestamp when aggregate was created (milliseconds).
    pub created_at: i64,
}

impl AggregateMetadata {
    /// Create a new aggregate metadata.
    pub fn new(aggregate_id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            aggregate_id: aggregate_id.into(),
            name: name.into(),
            description: None,
            platforms: Vec::new(),
            created_at: chrono::Utc::now().timestamp_millis(),
        }
    }

    /// Add a platform to the aggregate.
    pub fn add_platform(&mut self, platform: impl Into<String>) {
        let p = platform.into();
        if !self.platforms.contains(&p) {
            self.platforms.push(p);
        }
    }
}

/// Request to create a new aggregate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateAggregateRequest {
    pub aggregate_id: String,
    pub name: String,
    pub description: Option<String>,
}

/// Request to map a platform slug to an aggregate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MapPlatformRequest {
    pub platform: String,
    pub slug: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_aggregate_metadata_new() {
        let meta = AggregateMetadata::new("AGG123", "US Election 2024");
        assert_eq!(meta.aggregate_id, "AGG123");
        assert_eq!(meta.name, "US Election 2024");
        assert!(meta.platforms.is_empty());
    }

    #[test]
    fn test_aggregate_metadata_add_platform() {
        let mut meta = AggregateMetadata::new("AGG123", "US Election 2024");
        meta.add_platform("polymarket");
        meta.add_platform("kalshi");
        meta.add_platform("polymarket"); // Duplicate should not be added
        assert_eq!(meta.platforms, vec!["polymarket", "kalshi"]);
    }
}
