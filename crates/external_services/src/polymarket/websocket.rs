//! Polymarket WebSocket utilities.

use serde::Serialize;

/// Polymarket CLOB WebSocket URL.
pub const WS_URL: &str = "wss://ws-subscriptions-clob.polymarket.com/ws/market";

/// Maximum subscriptions per WebSocket connection.
pub const MAX_SUBSCRIPTIONS: usize = 50;

/// Subscription message structure.
#[derive(Debug, Serialize)]
struct SubscriptionMessage {
    #[serde(rename = "type")]
    msg_type: String,
    assets_ids: Vec<String>,
}

/// Build a subscription message for Polymarket WebSocket.
///
/// # Arguments
/// * `token_ids` - Asset IDs to subscribe to
///
/// # Returns
/// JSON string for WebSocket subscription.
pub fn build_subscription_message(token_ids: &[String]) -> String {
    let msg = SubscriptionMessage {
        msg_type: "market".to_string(),
        assets_ids: token_ids.to_vec(),
    };
    serde_json::to_string(&msg).unwrap()
}

/// Unsubscription message structure.
#[derive(Debug, Serialize)]
struct UnsubscriptionMessage {
    #[serde(rename = "type")]
    msg_type: String,
    assets_ids: Vec<String>,
}

/// Build an unsubscription message for Polymarket WebSocket.
///
/// # Arguments
/// * `token_ids` - Asset IDs to unsubscribe from
///
/// # Returns
/// JSON string for WebSocket unsubscription.
pub fn build_unsubscription_message(token_ids: &[String]) -> String {
    let msg = UnsubscriptionMessage {
        msg_type: "market".to_string(),
        assets_ids: token_ids.to_vec(),
    };
    // Polymarket uses same format for unsub, but with different handling
    serde_json::to_string(&msg).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_subscription_message() {
        let tokens = vec!["token1".to_string(), "token2".to_string()];
        let msg = build_subscription_message(&tokens);

        let parsed: serde_json::Value = serde_json::from_str(&msg).unwrap();
        assert_eq!(parsed["type"], "market");
        assert_eq!(parsed["assets_ids"][0], "token1");
        assert_eq!(parsed["assets_ids"][1], "token2");
    }

    #[test]
    fn test_constants() {
        assert_eq!(WS_URL, "wss://ws-subscriptions-clob.polymarket.com/ws/market");
        assert_eq!(MAX_SUBSCRIPTIONS, 50);
    }
}
