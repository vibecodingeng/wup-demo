//! NATS client implementation with JetStream support.

use anyhow::Result;
use async_nats::jetstream::{self, stream::Stream};
use async_nats::{Client, Subscriber};
use std::time::Duration;
use tracing::info;

/// Default retention period for streams (5 minutes).
pub const DEFAULT_RETENTION_SECS: u64 = 300;

/// Default max messages per stream.
pub const DEFAULT_MAX_MESSAGES: i64 = 1_000_000;

/// Default max bytes per stream (1GB).
pub const DEFAULT_MAX_BYTES: i64 = 1_073_741_824;

/// Configuration for creating a stream.
#[derive(Debug, Clone)]
pub struct StreamConfig {
    /// Stream name.
    pub name: String,
    /// Subject patterns to capture.
    pub subjects: Vec<String>,
    /// Retention period in seconds.
    pub max_age_secs: u64,
    /// Maximum number of messages.
    pub max_messages: i64,
    /// Maximum bytes.
    pub max_bytes: i64,
}

impl StreamConfig {
    /// Create a new stream config for a service.
    ///
    /// # Arguments
    /// * `service` - Service name (e.g., "polymarket")
    /// * `stream_type` - Type of stream (e.g., "market", "normalized")
    ///
    /// Creates stream named `{SERVICE}_{TYPE}` with subject `{type}.{service}.>`
    pub fn for_service(service: &str, stream_type: &str) -> Self {
        let name = format!("{}_{}", service.to_uppercase(), stream_type.to_uppercase());
        let subject = format!("{}.{}.>", stream_type, service);

        Self {
            name,
            subjects: vec![subject],
            max_age_secs: DEFAULT_RETENTION_SECS,
            max_messages: DEFAULT_MAX_MESSAGES,
            max_bytes: DEFAULT_MAX_BYTES,
        }
    }

    /// Set retention period in seconds.
    pub fn with_retention(mut self, secs: u64) -> Self {
        self.max_age_secs = secs;
        self
    }

    /// Add additional subject patterns.
    pub fn with_subjects(mut self, subjects: Vec<String>) -> Self {
        self.subjects = subjects;
        self
    }
}

/// Wrapper around the NATS client with JetStream context.
#[derive(Clone)]
pub struct NatsClient {
    client: Client,
    jetstream: jetstream::Context,
}

impl NatsClient {
    /// Connect to a NATS server and create a JetStream context.
    pub async fn connect(url: &str) -> Result<Self> {
        info!("Connecting to NATS at {}", url);
        let client = async_nats::connect(url).await?;
        let jetstream = jetstream::new(client.clone());

        Ok(Self { client, jetstream })
    }

    /// Subscribe to a subject pattern using NATS Core (low-latency push).
    /// Messages are delivered immediately as they arrive - no polling.
    pub async fn subscribe(&self, subject: &str) -> Result<Subscriber> {
        info!("Subscribing to subject pattern: {}", subject);
        let subscriber = self.client.subscribe(subject.to_string()).await?;
        Ok(subscriber)
    }

    /// Create or get a stream with the given configuration.
    pub async fn ensure_stream_with_config(&self, config: &StreamConfig) -> Result<Stream> {
        info!(
            "Ensuring stream '{}' exists (subjects: {:?}, retention: {}s)",
            config.name, config.subjects, config.max_age_secs
        );

        let stream = self
            .jetstream
            .get_or_create_stream(jetstream::stream::Config {
                name: config.name.clone(),
                subjects: config.subjects.clone(),
                retention: jetstream::stream::RetentionPolicy::Limits,
                max_messages: config.max_messages,
                max_bytes: config.max_bytes,
                max_age: Duration::from_secs(config.max_age_secs),
                storage: jetstream::stream::StorageType::File,
                ..Default::default()
            })
            .await?;

        info!("Stream '{}' ready", config.name);
        Ok(stream)
    }

    /// Convenience method to create a market data stream for a service.
    /// Stream name: `{SERVICE}_MARKET`, subjects: `market.{service}.>`
    pub async fn ensure_market_stream(&self, service: &str) -> Result<Stream> {
        let config = StreamConfig::for_service(service, "market");
        self.ensure_stream_with_config(&config).await
    }

    /// Convenience method to create a normalized data stream for a service.
    /// Stream name: `{SERVICE}_NORMALIZED`, subjects: `normalized.{service}.>`
    pub async fn ensure_normalized_stream(&self, service: &str) -> Result<Stream> {
        let config = StreamConfig::for_service(service, "normalized");
        self.ensure_stream_with_config(&config).await
    }

    /// Publish a message to JetStream (with acknowledgment).
    pub async fn publish(&self, subject: impl Into<String>, payload: bytes::Bytes) -> Result<()> {
        self.jetstream
            .publish(subject.into(), payload)
            .await?
            .await?;
        Ok(())
    }

    /// Publish a message using NATS Core (fire-and-forget, lowest latency).
    pub async fn publish_fast(&self, subject: &str, payload: bytes::Bytes) -> Result<()> {
        self.client.publish(subject.to_string(), payload).await?;
        Ok(())
    }

    /// Get the underlying JetStream context.
    pub fn context(&self) -> &jetstream::Context {
        &self.jetstream
    }
}
