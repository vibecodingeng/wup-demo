//! Redis client for event data storage.

use crate::error::Result;
use external_services::aggregate::AggregateMetadata;
use external_services::polymarket::{EventData, TokenMapping};
use redis::AsyncCommands;
use tracing::{debug, info};

/// Redis key prefix for events: event:{platform}:{slug}
const EVENT_KEY_PREFIX: &str = "event:";

/// Redis key prefix for aggregates: aggregate:{aggregate_id}
const AGGREGATE_KEY_PREFIX: &str = "aggregate:";

/// Redis client wrapper for event operations.
#[derive(Clone)]
pub struct RedisClient {
    client: redis::Client,
}

impl RedisClient {
    /// Create a new Redis client.
    pub fn new(redis_url: &str) -> Result<Self> {
        let client = redis::Client::open(redis_url)?;
        Ok(Self { client })
    }

    /// Get an async connection.
    async fn get_connection(&self) -> Result<redis::aio::MultiplexedConnection> {
        let conn = self.client.get_multiplexed_async_connection().await?;
        Ok(conn)
    }

    /// Store event data in Redis with platform prefix.
    /// Key format: event:{platform}:{slug}
    pub async fn store_event(&self, platform: &str, event_slug: &str, data: &EventData) -> Result<()> {
        let mut conn = self.get_connection().await?;
        let key = format!("{}{}:{}", EVENT_KEY_PREFIX, platform, event_slug);
        let json = serde_json::to_string(data)?;

        conn.set::<_, _, ()>(&key, &json).await?;
        info!(
            "Stored event '{}:{}' in Redis with {} tokens",
            platform, event_slug, data.tokens.len()
        );

        Ok(())
    }

    /// Get event data from Redis with platform prefix.
    /// Key format: event:{platform}:{slug}
    pub async fn get_event(&self, platform: &str, event_slug: &str) -> Result<Option<EventData>> {
        let mut conn = self.get_connection().await?;
        let key = format!("{}{}:{}", EVENT_KEY_PREFIX, platform, event_slug);

        let json: Option<String> = conn.get(&key).await?;

        match json {
            Some(j) => {
                let data: EventData = serde_json::from_str(&j)?;
                debug!("Retrieved event '{}:{}' from Redis", platform, event_slug);
                Ok(Some(data))
            }
            None => Ok(None),
        }
    }

    /// Get token mappings for an event.
    pub async fn get_token_mappings(&self, platform: &str, event_slug: &str) -> Result<Vec<TokenMapping>> {
        let event = self.get_event(platform, event_slug).await?;
        Ok(event.map(|e| e.tokens).unwrap_or_default())
    }

    /// Delete event data from Redis.
    pub async fn delete_event(&self, platform: &str, event_slug: &str) -> Result<()> {
        let mut conn = self.get_connection().await?;
        let key = format!("{}{}:{}", EVENT_KEY_PREFIX, platform, event_slug);

        conn.del::<_, ()>(&key).await?;
        info!("Deleted event '{}:{}' from Redis", platform, event_slug);

        Ok(())
    }

    /// Check if event exists in Redis.
    pub async fn event_exists(&self, platform: &str, event_slug: &str) -> Result<bool> {
        let mut conn = self.get_connection().await?;
        let key = format!("{}{}:{}", EVENT_KEY_PREFIX, platform, event_slug);

        let exists: bool = conn.exists(&key).await?;
        Ok(exists)
    }

    /// List all stored events (returns platform:slug pairs).
    pub async fn list_events(&self) -> Result<Vec<String>> {
        let mut conn = self.get_connection().await?;
        let pattern = format!("{}*", EVENT_KEY_PREFIX);

        let keys: Vec<String> = conn.keys(&pattern).await?;
        let slugs: Vec<String> = keys
            .into_iter()
            .map(|k| k.strip_prefix(EVENT_KEY_PREFIX).unwrap_or(&k).to_string())
            .collect();

        Ok(slugs)
    }

    // =========================================================================
    // Aggregate Mapping Methods
    // =========================================================================

    /// Set mapping from platform event slug to aggregate ID.
    /// Key format: {platform}:event:{slug} -> aggregate_id
    pub async fn set_event_mapping(
        &self,
        platform: &str,
        slug: &str,
        aggregate_id: &str,
    ) -> Result<()> {
        let mut conn = self.get_connection().await?;
        let key = format!("{}:event:{}", platform, slug);

        conn.set::<_, _, ()>(&key, aggregate_id).await?;
        info!(
            "Mapped {}:{} -> aggregate {}",
            platform, slug, aggregate_id
        );

        Ok(())
    }

    /// Get aggregate ID for a platform's event slug.
    /// Key format: {platform}:event:{slug}
    pub async fn get_aggregate_id(&self, platform: &str, slug: &str) -> Result<Option<String>> {
        let mut conn = self.get_connection().await?;
        let key = format!("{}:event:{}", platform, slug);

        let id: Option<String> = conn.get(&key).await?;
        Ok(id)
    }

    /// Delete mapping from platform event slug to aggregate ID.
    pub async fn delete_event_mapping(&self, platform: &str, slug: &str) -> Result<()> {
        let mut conn = self.get_connection().await?;
        let key = format!("{}:event:{}", platform, slug);

        conn.del::<_, ()>(&key).await?;
        info!("Deleted mapping {}:{}", platform, slug);

        Ok(())
    }

    /// Store aggregate metadata.
    /// Key format: aggregate:{aggregate_id}
    pub async fn store_aggregate(&self, metadata: &AggregateMetadata) -> Result<()> {
        let mut conn = self.get_connection().await?;
        let key = format!("{}{}", AGGREGATE_KEY_PREFIX, metadata.aggregate_id);
        let json = serde_json::to_string(metadata)?;

        conn.set::<_, _, ()>(&key, &json).await?;
        info!("Stored aggregate '{}'", metadata.aggregate_id);

        Ok(())
    }

    /// Get aggregate metadata.
    pub async fn get_aggregate(&self, aggregate_id: &str) -> Result<Option<AggregateMetadata>> {
        let mut conn = self.get_connection().await?;
        let key = format!("{}{}", AGGREGATE_KEY_PREFIX, aggregate_id);

        let json: Option<String> = conn.get(&key).await?;

        match json {
            Some(j) => {
                let data: AggregateMetadata = serde_json::from_str(&j)?;
                debug!("Retrieved aggregate '{}' from Redis", aggregate_id);
                Ok(Some(data))
            }
            None => Ok(None),
        }
    }

    /// Delete aggregate metadata.
    pub async fn delete_aggregate(&self, aggregate_id: &str) -> Result<()> {
        let mut conn = self.get_connection().await?;
        let key = format!("{}{}", AGGREGATE_KEY_PREFIX, aggregate_id);

        conn.del::<_, ()>(&key).await?;
        info!("Deleted aggregate '{}'", aggregate_id);

        Ok(())
    }

    /// List all stored aggregate IDs.
    pub async fn list_aggregates(&self) -> Result<Vec<String>> {
        let mut conn = self.get_connection().await?;
        let pattern = format!("{}*", AGGREGATE_KEY_PREFIX);

        let keys: Vec<String> = conn.keys(&pattern).await?;
        let ids: Vec<String> = keys
            .into_iter()
            .map(|k| k.strip_prefix(AGGREGATE_KEY_PREFIX).unwrap_or(&k).to_string())
            .collect();

        Ok(ids)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_prefix() {
        assert_eq!(EVENT_KEY_PREFIX, "event:");
    }
}
