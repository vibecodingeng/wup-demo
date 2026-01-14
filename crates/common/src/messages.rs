//! Control messages for dynamic subscription management.

/// Commands that can be sent to a WsManager to update subscriptions at runtime.
#[derive(Debug, Clone)]
pub enum ControlCommand {
    /// Subscribe to additional asset IDs
    Subscribe(Vec<String>),
    /// Unsubscribe from asset IDs
    Unsubscribe(Vec<String>),
    /// Graceful shutdown
    Shutdown,
}
