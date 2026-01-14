//! Generic normalizer service that works with any exchange adapter.
//! Uses NATS Core subscription for lowest latency message delivery.

use crate::schema::NormalizedOrderbook;
use crate::traits::{AdapterConfig, ExchangeAdapter};
use anyhow::Result;
use futures::StreamExt;
use metrics::counter;
use nats_client::NatsClient;
use std::marker::PhantomData;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

/// Generic normalizer service.
///
/// The service is parameterized by an `ExchangeAdapter` which defines
/// how to parse and transform messages for a specific exchange.
///
/// Uses NATS Core subscription for real-time, low-latency message delivery.
pub struct NormalizerService<A: ExchangeAdapter> {
    adapter: A,
    nats_client: Arc<NatsClient>,
    config: AdapterConfig,
    shutdown_rx: mpsc::Receiver<()>,
    _marker: PhantomData<A>,
}

impl<A: ExchangeAdapter> NormalizerService<A> {
    /// Create a new normalizer service with the given adapter.
    pub fn new(
        adapter: A,
        nats_client: NatsClient,
        config: AdapterConfig,
        shutdown_rx: mpsc::Receiver<()>,
    ) -> Self {
        Self {
            adapter,
            nats_client: Arc::new(nats_client),
            config,
            shutdown_rx,
            _marker: PhantomData,
        }
    }

    /// Create a new normalizer service with default configuration.
    pub fn with_defaults(adapter: A, nats_client: NatsClient, shutdown_rx: mpsc::Receiver<()>) -> Self {
        let config = A::default_config();
        Self::new(adapter, nats_client, config, shutdown_rx)
    }

    /// Run the normalizer service.
    pub async fn run(mut self) -> Result<()> {
        info!(
            "Starting {} normalizer, subscribing to {}",
            A::NAME,
            self.config.filter_subject
        );

        // Ensure destination stream exists for persistence
        self.nats_client
            .ensure_normalized_stream(&self.config.dest_stream)
            .await?;

        // Subscribe using NATS Core for low-latency push delivery
        let mut subscriber = self
            .nats_client
            .subscribe(&self.config.filter_subject)
            .await?;

        info!("{} normalizer running (low-latency mode)", A::NAME);

        loop {
            tokio::select! {
                biased;  // Prioritize shutdown signal

                _ = self.shutdown_rx.recv() => {
                    info!("{} normalizer received shutdown signal", A::NAME);
                    break;
                }

                msg = subscriber.next() => {
                    match msg {
                        Some(nats_msg) => {
                            let payload = std::str::from_utf8(&nats_msg.payload)
                                .unwrap_or("");

                            counter!(
                                "normalizer_messages_received_total",
                                "exchange" => A::NAME
                            ).increment(1);

                            if let Err(e) = self.process_message(payload).await {
                                error!("[{}] Failed to process message: {:?}", A::NAME, e);
                                counter!(
                                    "normalizer_errors_total",
                                    "exchange" => A::NAME,
                                    "error_type" => "processing"
                                ).increment(1);
                            }
                        }
                        None => {
                            warn!("[{}] Subscription ended unexpectedly", A::NAME);
                            break;
                        }
                    }
                }
            }
        }

        info!("{} normalizer service stopped", A::NAME);
        Ok(())
    }

    /// Process a single raw message.
    async fn process_message(&self, payload: &str) -> Result<()> {
        let normalized_messages = match self.adapter.parse_and_transform(payload) {
            Ok(msgs) => msgs,
            Err(e) => {
                debug!("[{}] Failed to parse message: {:?}", A::NAME, e);
                counter!(
                    "normalizer_parse_errors_total",
                    "exchange" => A::NAME
                )
                .increment(1);
                return Ok(());
            }
        };

        for normalized in normalized_messages {
            self.publish_normalized(&normalized).await?;
        }

        Ok(())
    }

    /// Publish a normalized message to NATS.
    async fn publish_normalized(&self, msg: &NormalizedOrderbook) -> Result<()> {
        let subject = self.adapter.build_output_subject(&self.config, msg);
        let payload = serde_json::to_vec(msg)?;

        // Use fast publish for lowest latency
        self.nats_client
            .publish_fast(&subject, bytes::Bytes::from(payload))
            .await?;

        let message_type = match msg.message_type {
            crate::schema::OrderbookMessageType::Snapshot => "snapshot",
            crate::schema::OrderbookMessageType::Delta => "delta",
        };

        counter!(
            "normalizer_messages_published_total",
            "exchange" => A::NAME,
            "message_type" => message_type
        )
        .increment(1);

        debug!("[{}] Published to {}", A::NAME, subject);

        Ok(())
    }
}

/// Builder for creating normalizer services with custom configuration.
pub struct NormalizerServiceBuilder<A: ExchangeAdapter> {
    adapter: A,
    config: AdapterConfig,
}

impl<A: ExchangeAdapter> NormalizerServiceBuilder<A> {
    /// Create a new builder with the given adapter.
    pub fn new(adapter: A) -> Self {
        let config = A::default_config();
        Self { adapter, config }
    }

    /// Set the source stream name.
    pub fn source_stream(mut self, stream: impl Into<String>) -> Self {
        self.config.source_stream = stream.into();
        self
    }

    /// Set the filter subject for subscription.
    pub fn filter_subject(mut self, subject: impl Into<String>) -> Self {
        self.config.filter_subject = subject.into();
        self
    }

    /// Set the destination stream name.
    pub fn dest_stream(mut self, stream: impl Into<String>) -> Self {
        self.config.dest_stream = stream.into();
        self
    }

    /// Set the output subject prefix.
    pub fn output_subject_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.config.output_subject_prefix = prefix.into();
        self
    }

    /// Build the normalizer service.
    pub fn build(
        self,
        nats_client: NatsClient,
        shutdown_rx: mpsc::Receiver<()>,
    ) -> NormalizerService<A> {
        NormalizerService::new(self.adapter, nats_client, self.config, shutdown_rx)
    }
}
