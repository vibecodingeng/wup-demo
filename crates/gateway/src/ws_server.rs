//! WebSocket server handler using Axum.

use crate::client::{ClientRegistry, ClientState};
use crate::error::{GatewayError, Result};
use crate::protocol::{ClientMessage, ServerMessage};
use crate::router::ChangeRouter;
use axum::{
    extract::{
        ws::{Message, WebSocket},
        State, WebSocketUpgrade,
    },
    response::IntoResponse,
    routing::get,
    Router,
};
use futures::{SinkExt, StreamExt};
use metrics::{counter, gauge};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::interval;
use tower_http::cors::CorsLayer;
use tracing::{debug, info, warn};

/// Shared application state.
pub struct AppState {
    pub registry: Arc<ClientRegistry>,
    pub router: Arc<ChangeRouter>,
}

/// Create the WebSocket router.
pub fn create_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/ws", get(ws_handler))
        .route("/health", get(health_handler))
        .with_state(state)
        .layer(CorsLayer::permissive())
}

/// Health check handler.
async fn health_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let clients = state.registry.client_count();
    let subscriptions = state.registry.subscription_count();
    format!(
        r#"{{"status":"ok","clients":{},"subscriptions":{}}}"#,
        clients, subscriptions
    )
}

/// WebSocket upgrade handler.
async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_socket(socket, state))
}

/// Handle a WebSocket connection.
async fn handle_socket(socket: WebSocket, state: Arc<AppState>) {
    // Split the socket into sender and receiver
    let (mut ws_tx, mut ws_rx) = socket.split();

    // Create unbounded channel for outgoing messages
    let (tx, mut rx) = mpsc::unbounded_channel::<Message>();

    // Create client state
    let client = Arc::new(ClientState::new(tx));
    let client_id = state.registry.register(client.clone());

    counter!("gateway_connections_total").increment(1);
    gauge!("gateway_active_connections").set(state.registry.client_count() as f64);

    info!("Client {} connected", client_id);

    // Spawn task to forward messages from channel to WebSocket
    let send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if ws_tx.send(msg).await.is_err() {
                break;
            }
        }
    });

    // Ping interval for keepalive
    let mut ping_interval = interval(Duration::from_secs(30));
    ping_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    // Handle incoming messages
    loop {
        tokio::select! {
            biased;

            // Handle incoming WebSocket messages
            msg = ws_rx.next() => {
                match msg {
                    Some(Ok(msg)) => {
                        if let Err(e) = handle_message(&state, &client, msg).await {
                            warn!("Error handling message from {}: {:?}", client_id, e);
                            // Send error to client
                            let _ = client.send(ServerMessage::Error {
                                message: e.to_string(),
                                code: "PROCESSING_ERROR".to_string(),
                            });
                        }
                    }
                    Some(Err(e)) => {
                        warn!("WebSocket error for {}: {:?}", client_id, e);
                        break;
                    }
                    None => {
                        // Connection closed
                        break;
                    }
                }
            }

            // Send ping periodically
            _ = ping_interval.tick() => {
                if client.tx.send(Message::Ping(vec![].into())).is_err() {
                    break;
                }
            }
        }
    }

    // Cleanup
    state.registry.unregister(&client_id);
    send_task.abort();

    counter!("gateway_disconnections_total").increment(1);
    gauge!("gateway_active_connections").set(state.registry.client_count() as f64);

    info!("Client {} disconnected", client_id);
}

/// Handle a single WebSocket message.
async fn handle_message(
    state: &Arc<AppState>,
    client: &Arc<ClientState>,
    msg: Message,
) -> Result<()> {
    match msg {
        Message::Text(text) => {
            let client_msg: ClientMessage = serde_json::from_str(&text)?;
            handle_client_message(state, client, client_msg).await
        }
        Message::Binary(data) => {
            // Try to parse as JSON
            let client_msg: ClientMessage = serde_json::from_slice(&data)?;
            handle_client_message(state, client, client_msg).await
        }
        Message::Ping(data) => {
            client.update_ping();
            client
                .tx
                .send(Message::Pong(data))
                .map_err(|_| GatewayError::ChannelSend)?;
            Ok(())
        }
        Message::Pong(_) => {
            client.update_ping();
            Ok(())
        }
        Message::Close(_) => {
            // Will be handled by the connection loop
            Ok(())
        }
    }
}

/// Handle a parsed client message.
async fn handle_client_message(
    state: &Arc<AppState>,
    client: &Arc<ClientState>,
    msg: ClientMessage,
) -> Result<()> {
    match msg {
        ClientMessage::Subscribe { subjects } => {
            debug!("Client {} subscribing to {:?}", client.id, subjects);

            // Validate subjects
            for subject in &subjects {
                if let Some(error) = crate::subscription::validate_pattern(subject) {
                    return Err(GatewayError::InvalidSubject(error));
                }
            }

            // Add subscriptions
            state.registry.subscribe(&client.id, &subjects)?;

            // Send confirmation
            client.send(ServerMessage::Subscribed {
                subjects: subjects.clone(),
            })?;

            // Fetch and send initial snapshots for each subject
            for subject in &subjects {
                if let Err(e) = state.router.send_snapshot(client, subject).await {
                    warn!(
                        "Failed to send snapshot for {} to {}: {:?}",
                        subject, client.id, e
                    );
                }
            }

            counter!("gateway_subscriptions_total").increment(subjects.len() as u64);
            Ok(())
        }
        ClientMessage::Unsubscribe { subjects } => {
            debug!("Client {} unsubscribing from {:?}", client.id, subjects);

            state.registry.unsubscribe(&client.id, &subjects)?;

            client.send(ServerMessage::Unsubscribed { subjects })?;

            Ok(())
        }
        ClientMessage::Ping => {
            client.update_ping();
            client.send(ServerMessage::Pong)?;
            Ok(())
        }
    }
}
