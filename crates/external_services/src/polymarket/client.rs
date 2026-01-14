//! Polymarket REST API client.

use crate::error::{Error, Result};
use crate::polymarket::types::{Event, EventData, TokenMapping};
use tracing::{debug, info};

/// Base URL for Polymarket Gamma API.
const GAMMA_API_BASE_URL: &str = "https://gamma-api.polymarket.com";

/// Polymarket REST API client.
#[derive(Debug, Clone)]
pub struct PolymarketClient {
    http: reqwest::Client,
    base_url: String,
}

impl Default for PolymarketClient {
    fn default() -> Self {
        Self::new()
    }
}

impl PolymarketClient {
    /// Create a new Polymarket client with default settings.
    pub fn new() -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: GAMMA_API_BASE_URL.to_string(),
        }
    }

    /// Create a new Polymarket client with custom base URL.
    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: base_url.into(),
        }
    }

    /// Fetch event by slug from Polymarket API.
    ///
    /// # Arguments
    /// * `slug` - Event slug (e.g., "highest-temperature-in-seoul-on-january-12")
    ///
    /// # Returns
    /// Event with all markets and their clobTokenIds.
    pub async fn fetch_event_by_slug(&self, slug: &str) -> Result<Event> {
        let url = format!("{}/events?slug={}", self.base_url, slug);
        debug!("Fetching event from: {}", url);

        let response = self.http.get(&url).send().await?;

        if !response.status().is_success() {
            return Err(Error::Api(format!(
                "API returned status {}: {}",
                response.status(),
                response.text().await.unwrap_or_default()
            )));
        }

        let events: Vec<Event> = response.json().await?;

        events.into_iter().next().ok_or_else(|| {
            Error::EventNotFound(format!("No event found with slug: {}", slug))
        })
    }

    /// Fetch event by ID from Polymarket API.
    pub async fn fetch_event_by_id(&self, event_id: &str) -> Result<Event> {
        let url = format!("{}/events/{}", self.base_url, event_id);
        debug!("Fetching event from: {}", url);

        let response = self.http.get(&url).send().await?;

        if !response.status().is_success() {
            return Err(Error::Api(format!(
                "API returned status {}: {}",
                response.status(),
                response.text().await.unwrap_or_default()
            )));
        }

        let event: Event = response.json().await?;
        Ok(event)
    }

    /// Extract token mappings from an event.
    pub fn extract_token_mappings(event: &Event) -> Vec<TokenMapping> {
        super::types::extract_token_mappings(event)
    }

    /// Create EventData from an event (for Redis storage).
    pub fn create_event_data(event: &Event) -> EventData {
        EventData::from_event(event)
    }

    /// Fetch event and return EventData ready for Redis storage.
    pub async fn fetch_event_data(&self, slug: &str) -> Result<EventData> {
        let event = self.fetch_event_by_slug(slug).await?;
        let event_data = EventData::from_event(&event);

        info!(
            "Fetched event '{}' with {} markets, {} tokens",
            slug,
            event_data.market_count,
            event_data.tokens.len()
        );

        Ok(event_data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_creation() {
        let client = PolymarketClient::new();
        assert_eq!(client.base_url, GAMMA_API_BASE_URL);
    }

    #[test]
    fn test_client_with_custom_url() {
        let client = PolymarketClient::with_base_url("https://custom.api.com");
        assert_eq!(client.base_url, "https://custom.api.com");
    }
}
