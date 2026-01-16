//! Client state and registry management.
//!
//! Uses lock-free DashMap for high-throughput concurrent access.
//!
//! OPTIMIZATION: Uses separate indexes for exact subjects and wildcard patterns
//! to achieve O(1) lookup for exact matches + O(W) for wildcards (W << total patterns).

use crate::error::{GatewayError, Result};
use crate::protocol::ServerMessage;
use axum::extract::ws::Message;
use chrono::Utc;
use dashmap::{DashMap, DashSet};
use std::collections::HashSet;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::sync::RwLock;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};
use uuid::Uuid;

/// Check if a subscription pattern contains wildcards.
#[inline]
fn is_wildcard_pattern(pattern: &str) -> bool {
    pattern.contains('*') || pattern.contains('>')
}

/// Unique client identifier.
pub type ClientId = Uuid;

/// Default buffer size for client message channels.
/// Sized for ~1 second of high-frequency updates at 1000 msgs/sec.
pub const CLIENT_CHANNEL_BUFFER_SIZE: usize = 1000;

/// State for a single connected client.
pub struct ClientState {
    /// Unique client identifier.
    pub id: ClientId,
    /// Channel to send messages to the client's WebSocket.
    /// OPTIMIZATION: Bounded channel to prevent OOM with slow clients.
    pub tx: mpsc::Sender<Message>,
    /// Current subscriptions (subject patterns).
    pub subscriptions: DashSet<String>,
    /// Timestamp when client connected.
    pub connected_at: i64,
    /// Timestamp of last ping received.
    pub last_ping: AtomicI64,
}

impl ClientState {
    /// Create a new client state with bounded channel.
    pub fn new(tx: mpsc::Sender<Message>) -> Self {
        let now = Utc::now().timestamp_millis();
        Self {
            id: Uuid::new_v4(),
            tx,
            subscriptions: DashSet::new(),
            connected_at: now,
            last_ping: AtomicI64::new(now),
        }
    }

    /// Send a message to this client.
    /// Uses try_send for non-blocking behavior - drops message if buffer full.
    pub fn send(&self, msg: ServerMessage) -> Result<()> {
        let json = serde_json::to_string(&msg)?;
        self.tx
            .try_send(Message::Text(json.into()))
            .map_err(|_| GatewayError::ChannelSend)
    }

    /// Try to send a raw message to this client.
    /// Returns true if sent, false if buffer full (slow client).
    pub fn try_send_raw(&self, msg: Message) -> bool {
        self.tx.try_send(msg).is_ok()
    }

    /// Update the last ping timestamp.
    pub fn update_ping(&self) {
        self.last_ping
            .store(Utc::now().timestamp_millis(), Ordering::Relaxed);
    }

    /// Get the last ping timestamp.
    pub fn last_ping_time(&self) -> i64 {
        self.last_ping.load(Ordering::Relaxed)
    }

    /// Add subscriptions to this client.
    pub fn add_subscriptions(&self, subjects: &[String]) {
        for subject in subjects {
            self.subscriptions.insert(subject.clone());
        }
    }

    /// Remove subscriptions from this client.
    pub fn remove_subscriptions(&self, subjects: &[String]) {
        for subject in subjects {
            self.subscriptions.remove(subject);
        }
    }

    /// Check if this client is subscribed to a subject.
    pub fn is_subscribed(&self, subject: &str) -> bool {
        self.subscriptions.contains(subject)
    }

    /// Get all current subscriptions.
    pub fn get_subscriptions(&self) -> Vec<String> {
        self.subscriptions.iter().map(|s| s.clone()).collect()
    }
}

/// Lock-free registry of connected clients.
///
/// Maintains:
/// - Client ID → Client State mapping
/// - Exact subject → Client IDs for O(1) lookups
/// - Wildcard patterns → Client IDs for O(W) scanning (W = wildcard count)
pub struct ClientRegistry {
    /// Client ID → Client State.
    clients: DashMap<ClientId, Arc<ClientState>>,
    /// All subscription patterns (for backward compat and unsubscribe).
    subscriptions: DashMap<String, DashSet<ClientId>>,
    /// OPTIMIZATION: Exact subject → Client IDs (no wildcards).
    /// O(1) lookup for exact matches.
    exact_subscriptions: DashMap<String, DashSet<ClientId>>,
    /// OPTIMIZATION: Wildcard patterns only.
    /// Stored separately to avoid scanning exact patterns.
    /// Uses RwLock<Vec> since wildcard patterns are rare and change infrequently.
    wildcard_patterns: RwLock<Vec<(String, DashSet<ClientId>)>>,
}

impl ClientRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            clients: DashMap::new(),
            subscriptions: DashMap::new(),
            exact_subscriptions: DashMap::new(),
            wildcard_patterns: RwLock::new(Vec::new()),
        }
    }

    /// Register a new client.
    pub fn register(&self, client: Arc<ClientState>) -> ClientId {
        let id = client.id;
        self.clients.insert(id, client);
        info!("Client {} registered", id);
        id
    }

    /// Unregister a client and clean up subscriptions.
    pub fn unregister(&self, client_id: &ClientId) {
        if let Some((_, client)) = self.clients.remove(client_id) {
            // Remove from all subscription indexes
            for subject in client.subscriptions.iter() {
                let subject_str = &*subject;

                // Remove from main index
                if let Some(client_set) = self.subscriptions.get(subject_str) {
                    client_set.remove(client_id);
                }

                // Remove from exact or wildcard index
                if is_wildcard_pattern(subject_str) {
                    // Remove from wildcard patterns
                    if let Ok(patterns) = self.wildcard_patterns.read() {
                        for (_, clients) in patterns.iter() {
                            clients.remove(client_id);
                        }
                    }
                } else {
                    // Remove from exact subscriptions
                    if let Some(client_set) = self.exact_subscriptions.get(subject_str) {
                        client_set.remove(client_id);
                    }
                }
            }
            info!("Client {} unregistered", client_id);
        }
    }

    /// Get a client by ID.
    pub fn get(&self, client_id: &ClientId) -> Option<Arc<ClientState>> {
        self.clients.get(client_id).map(|r| r.clone())
    }

    /// Add subscriptions for a client.
    pub fn subscribe(&self, client_id: &ClientId, subjects: &[String]) -> Result<()> {
        let client = self
            .clients
            .get(client_id)
            .ok_or_else(|| GatewayError::ClientNotFound(client_id.to_string()))?;

        for subject in subjects {
            // Add to client's subscriptions
            client.subscriptions.insert(subject.clone());

            // Add to main reverse index
            self.subscriptions
                .entry(subject.clone())
                .or_default()
                .insert(*client_id);

            // OPTIMIZATION: Add to appropriate index based on pattern type
            if is_wildcard_pattern(subject) {
                // Add to wildcard patterns
                let mut patterns = self.wildcard_patterns.write().unwrap();
                if let Some((_, clients)) = patterns.iter().find(|(p, _)| p == subject) {
                    clients.insert(*client_id);
                } else {
                    let clients = DashSet::new();
                    clients.insert(*client_id);
                    patterns.push((subject.clone(), clients));
                }
            } else {
                // Add to exact subscriptions (O(1) lookup)
                self.exact_subscriptions
                    .entry(subject.clone())
                    .or_default()
                    .insert(*client_id);
            }
        }

        debug!(
            "Client {} subscribed to {} subjects",
            client_id,
            subjects.len()
        );
        Ok(())
    }

    /// Remove subscriptions for a client.
    pub fn unsubscribe(&self, client_id: &ClientId, subjects: &[String]) -> Result<()> {
        let client = self
            .clients
            .get(client_id)
            .ok_or_else(|| GatewayError::ClientNotFound(client_id.to_string()))?;

        for subject in subjects {
            // Remove from client's subscriptions
            client.subscriptions.remove(subject);

            // Remove from main reverse index
            if let Some(client_set) = self.subscriptions.get(subject) {
                client_set.remove(client_id);
            }

            // OPTIMIZATION: Remove from appropriate index
            if is_wildcard_pattern(subject) {
                // Remove from wildcard patterns
                if let Ok(patterns) = self.wildcard_patterns.read() {
                    if let Some((_, clients)) = patterns.iter().find(|(p, _)| p == subject) {
                        clients.remove(client_id);
                    }
                }
            } else {
                // Remove from exact subscriptions
                if let Some(client_set) = self.exact_subscriptions.get(subject) {
                    client_set.remove(client_id);
                }
            }
        }

        debug!(
            "Client {} unsubscribed from {} subjects",
            client_id,
            subjects.len()
        );
        Ok(())
    }

    /// Get all clients subscribed to a specific subject (exact match).
    pub fn get_subscribers_exact(&self, subject: &str) -> Vec<Arc<ClientState>> {
        if let Some(client_ids) = self.subscriptions.get(subject) {
            client_ids
                .iter()
                .filter_map(|id| self.clients.get(&*id).map(|c| c.clone()))
                .collect()
        } else {
            Vec::new()
        }
    }

    /// Get all clients subscribed to subjects matching a pattern.
    /// This is used for routing NATS messages to clients with wildcard subscriptions.
    ///
    /// OPTIMIZATION: Uses O(1) exact lookup + O(W) wildcard scan where W = wildcard patterns.
    /// Previously was O(P) where P = all patterns.
    pub fn get_matching_subscribers(&self, subject: &str) -> Vec<Arc<ClientState>> {
        let mut matched_clients = HashSet::new();

        // PHASE 1: O(1) exact match lookup
        // Most subscriptions are exact, so this handles the common case fast
        if let Some(client_ids) = self.exact_subscriptions.get(subject) {
            for client_id in client_ids.iter() {
                matched_clients.insert(*client_id);
            }
        }

        // PHASE 2: O(W) wildcard pattern scan (W << total patterns)
        // Only scan patterns that contain wildcards
        let subject_parts: Vec<&str> = subject.split('.').collect();
        if let Ok(patterns) = self.wildcard_patterns.read() {
            for (pattern, client_ids) in patterns.iter() {
                if crate::subscription::matches_subject(pattern, &subject_parts) {
                    for client_id in client_ids.iter() {
                        matched_clients.insert(*client_id);
                    }
                }
            }
        }

        // Collect client states
        matched_clients
            .into_iter()
            .filter_map(|id| self.clients.get(&id).map(|c| c.clone()))
            .collect()
    }

    /// Get the total number of connected clients.
    pub fn client_count(&self) -> usize {
        self.clients.len()
    }

    /// Get the total number of active subscriptions.
    pub fn subscription_count(&self) -> usize {
        self.subscriptions.len()
    }

    /// Broadcast a message to all clients matching a subject.
    pub fn broadcast(&self, subject: &str, msg: &ServerMessage) {
        let clients = self.get_matching_subscribers(subject);
        if clients.is_empty() {
            return;
        }

        // Pre-serialize the message once
        let json = match serde_json::to_string(msg) {
            Ok(j) => j,
            Err(e) => {
                warn!("Failed to serialize broadcast message: {}", e);
                return;
            }
        };

        for client in clients {
            if let Err(e) = client.tx.try_send(Message::Text(json.clone().into())) {
                debug!("Failed to send to client {}: {}", client.id, e);
            }
        }
    }

    /// Remove stale clients that haven't pinged in a while.
    pub fn cleanup_stale_clients(&self, max_idle_ms: i64) {
        let now = Utc::now().timestamp_millis();
        let mut stale_ids = Vec::new();

        for entry in self.clients.iter() {
            let client = entry.value();
            if now - client.last_ping_time() > max_idle_ms {
                stale_ids.push(*entry.key());
            }
        }

        for id in stale_ids {
            warn!("Removing stale client {}", id);
            self.unregister(&id);
        }
    }
}

impl Default for ClientRegistry {
    fn default() -> Self {
        Self::new()
    }
}
