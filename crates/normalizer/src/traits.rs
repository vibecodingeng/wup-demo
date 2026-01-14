//! Core traits for exchange adapters (plugin interface).
//!
//! To add a new exchange, implement the `ExchangeAdapter` trait.
//!
//! # Example
//!
//! ```ignore
//! pub struct KalshiAdapter;
//!
//! impl ExchangeAdapter for KalshiAdapter {
//!     const NAME: &'static str = "kalshi";
//!     const FILTER_SUBJECT: &'static str = "market.kalshi.>";
//!
//!     fn parse_and_transform(&self, payload: &str) -> Result<Vec<NormalizedOrderbook>> {
//!         // Parse Kalshi messages and transform to normalized format
//!     }
//! }
//! ```

use crate::schema::NormalizedOrderbook;
use anyhow::Result;

/// Configuration for an exchange adapter.
#[derive(Debug, Clone)]
pub struct AdapterConfig {
    /// Source NATS stream name (for JetStream persistence).
    pub source_stream: String,
    /// Subject filter pattern to subscribe to (e.g., "market.polymarket.>").
    pub filter_subject: String,
    /// Destination stream name (for JetStream persistence).
    pub dest_stream: String,
    /// Output subject prefix (e.g., "normalized.polymarket").
    pub output_subject_prefix: String,
}

/// Core trait for exchange adapters.
///
/// Implement this trait to add support for a new exchange.
/// The normalizer service is generic over this trait.
pub trait ExchangeAdapter: Send + Sync + 'static {
    /// Exchange name (e.g., "polymarket", "kalshi").
    const NAME: &'static str;

    /// Default NATS subject filter for this exchange.
    const FILTER_SUBJECT: &'static str;

    /// Create default adapter configuration.
    fn default_config() -> AdapterConfig {
        AdapterConfig {
            source_stream: "MARKET_DATA".to_string(),
            filter_subject: Self::FILTER_SUBJECT.to_string(),
            dest_stream: "NORMALIZED_DATA".to_string(),
            output_subject_prefix: format!("normalized.{}", Self::NAME),
        }
    }

    /// Parse raw message and transform to normalized orderbook format.
    ///
    /// Returns a vector because one raw message may produce multiple
    /// normalized messages (e.g., price_change with multiple assets).
    ///
    /// Returns empty vector for messages that should be skipped.
    fn parse_and_transform(&self, payload: &str) -> Result<Vec<NormalizedOrderbook>>;

    /// Build the output subject for a normalized message.
    ///
    /// Default implementation uses: `{prefix}.{truncated_clob_token_id}`
    fn build_output_subject(&self, config: &AdapterConfig, msg: &NormalizedOrderbook) -> String {
        let short_id = if msg.clob_token_id.len() > 16 {
            &msg.clob_token_id[..16]
        } else {
            &msg.clob_token_id
        };
        format!("{}.{}", config.output_subject_prefix, short_id)
    }

    /// Get metrics labels for this adapter.
    fn metrics_labels(&self) -> Vec<(&'static str, &'static str)> {
        vec![("exchange", Self::NAME)]
    }
}

/// Trait for exchanges that support orderbook data.
pub trait OrderbookExchange: ExchangeAdapter {
    /// Whether this exchange provides full snapshots.
    fn supports_snapshots(&self) -> bool {
        true
    }

    /// Whether this exchange provides delta updates.
    fn supports_deltas(&self) -> bool {
        true
    }
}

/// Trait for exchanges that support sportsbook data.
pub trait SportsbookExchange: ExchangeAdapter {
    /// Whether this exchange provides live betting.
    fn supports_live_betting(&self) -> bool {
        false
    }
}
