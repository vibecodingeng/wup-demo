//! WebSocket handler trait for platform adapters.

use crate::error::Result;
use crate::messages::ControlCommand;
use async_trait::async_trait;

/// Trait that platform adapters implement to handle WebSocket events.
/// The WsManager calls these methods when events occur.
#[async_trait]
pub trait WsHandler: Send + Sync + 'static {
    /// Returns the WebSocket URL to connect to.
    fn url(&self) -> &str;

    /// Returns the message to send immediately after connection (e.g., subscription payload).
    /// Return None if no initial message is needed.
    fn on_connect_message(&self) -> Option<String>;

    /// Called when a text message is received from the WebSocket.
    /// The handler should process and publish the message (e.g., to NATS).
    async fn on_message(&self, msg: &str) -> Result<()>;

    /// Called when a binary message is received from the WebSocket.
    /// Default implementation ignores binary messages.
    async fn on_binary_message(&self, _data: &[u8]) -> Result<()> {
        Ok(())
    }

    /// Called when the connection is lost (before reconnect attempt).
    async fn on_disconnect(&self) {}

    /// Called when a reconnection is successful.
    async fn on_reconnect(&self) {}

    /// Handle a control command (subscribe/unsubscribe).
    /// Returns the message to send to the WebSocket to update subscriptions.
    async fn handle_command(&self, cmd: ControlCommand) -> Option<String>;

    /// Get the current list of subscribed asset IDs.
    fn subscribed_ids(&self) -> Vec<String>;

    /// Get the current subscription count.
    fn subscription_count(&self) -> usize {
        self.subscribed_ids().len()
    }
}
