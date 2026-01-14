//! WebSocket connection manager with ping/pong, reconnection, and dynamic subscription support.

use crate::error::{Error, Result};
use crate::messages::ControlCommand;
use crate::ws_handler::WsHandler;
use futures::{SinkExt, StreamExt};
use metrics::{counter, gauge};
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::interval;
use tokio_tungstenite::{
    client_async_tls_with_config,
    tungstenite::protocol::{frame::coding::CloseCode, CloseFrame, Message},
    Connector,
};
use tracing::{debug, error, info, warn};
use url::Url;

/// Configuration for the WebSocket manager.
#[derive(Debug, Clone)]
pub struct WsManagerConfig {
    /// Interval between ping frames.
    pub ping_interval: Duration,
    /// Timeout waiting for pong response.
    pub pong_timeout: Duration,
    /// Initial delay before reconnection attempt.
    pub reconnect_delay: Duration,
    /// Maximum reconnection delay (for exponential backoff).
    pub max_reconnect_delay: Duration,
    /// Label for metrics (e.g., "polymarket").
    pub platform_label: String,
}

impl Default for WsManagerConfig {
    fn default() -> Self {
        Self {
            ping_interval: Duration::from_secs(30),
            pong_timeout: Duration::from_secs(10),
            reconnect_delay: Duration::from_secs(1),
            max_reconnect_delay: Duration::from_secs(30),
            platform_label: "unknown".to_string(),
        }
    }
}

/// WebSocket connection manager.
/// Handles connection lifecycle, ping/pong, reconnection, and dynamic subscription updates.
pub struct WsManager<H: WsHandler> {
    handler: Arc<H>,
    config: WsManagerConfig,
    command_rx: mpsc::Receiver<ControlCommand>,
    worker_id: String,
}

impl<H: WsHandler> WsManager<H> {
    /// Create a new WebSocket manager.
    pub fn new(
        handler: H,
        config: WsManagerConfig,
        command_rx: mpsc::Receiver<ControlCommand>,
        worker_id: String,
    ) -> Self {
        Self {
            handler: Arc::new(handler),
            config,
            command_rx,
            worker_id,
        }
    }

    /// Run the WebSocket manager. This will reconnect on disconnection until shutdown.
    pub async fn run(mut self) -> Result<()> {
        let mut reconnect_delay = self.config.reconnect_delay;
        let mut shutdown = false;

        while !shutdown {
            match self.connect_and_run_loop(&mut shutdown).await {
                Ok(()) => {
                    info!("[{}] WebSocket closed gracefully", self.worker_id);
                    break;
                }
                Err(e) => {
                    counter!("aggregator_errors_total", "platform" => self.config.platform_label.clone(), "error_type" => "disconnect").increment(1);
                    warn!(
                        "[{}] WebSocket disconnected: {:?}, reconnecting in {:?}",
                        self.worker_id, e, reconnect_delay
                    );
                    self.handler.on_disconnect().await;

                    tokio::time::sleep(reconnect_delay).await;

                    // Exponential backoff
                    reconnect_delay = (reconnect_delay * 2).min(self.config.max_reconnect_delay);
                }
            }
        }

        gauge!("aggregator_active_connections", "platform" => self.config.platform_label.clone())
            .decrement(1.0);
        Ok(())
    }

    async fn connect_and_run_loop(&mut self, shutdown: &mut bool) -> Result<()> {
        let url_str = self.handler.url();
        info!("[{}] Connecting to WebSocket: {}", self.worker_id, url_str);

        debug!("[{}] Initiating WebSocket handshake...", self.worker_id);

        // Parse URL to get host and port
        let url = Url::parse(url_str)?;
        let host = url
            .host_str()
            .ok_or_else(|| Error::Generic("No host in URL".to_string()))?;
        let port = url.port().unwrap_or(443);
        let addr_str = format!("{}:{}", host, port);

        // Resolve DNS and prefer IPv4 to avoid IPv6 timeout issues
        let addrs: Vec<SocketAddr> = addr_str
            .to_socket_addrs()
            .map_err(|e| Error::Generic(format!("DNS resolution failed: {}", e)))?
            .collect();

        // Try IPv4 addresses first, then IPv6
        let mut sorted_addrs: Vec<SocketAddr> =
            addrs.iter().filter(|a| a.is_ipv4()).copied().collect();
        sorted_addrs.extend(addrs.iter().filter(|a| a.is_ipv6()).copied());

        debug!(
            "[{}] Resolved addresses (IPv4 first): {:?}",
            self.worker_id, sorted_addrs
        );

        // Connect to the first available address
        let mut tcp_stream = None;
        for addr in &sorted_addrs {
            debug!("[{}] Trying to connect to {}", self.worker_id, addr);
            match tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(addr)).await {
                Ok(Ok(stream)) => {
                    debug!("[{}] TCP connected to {}", self.worker_id, addr);
                    tcp_stream = Some(stream);
                    break;
                }
                Ok(Err(e)) => {
                    debug!("[{}] TCP connect to {} failed: {}", self.worker_id, addr, e);
                }
                Err(_) => {
                    debug!("[{}] TCP connect to {} timed out", self.worker_id, addr);
                }
            }
        }

        let tcp_stream = tcp_stream
            .ok_or_else(|| Error::Generic("All connection attempts failed".to_string()))?;

        // Perform WebSocket handshake over the TCP connection with TLS
        let mut root_store = rustls::RootCertStore::empty();
        let certs = rustls_native_certs::load_native_certs();
        for cert in certs.certs {
            let _ = root_store.add(cert);
        }

        let connector = Connector::Rustls(Arc::new(
            rustls::ClientConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .map_err(|e| Error::Generic(format!("TLS config error: {}", e)))?
            .with_root_certificates(root_store)
            .with_no_client_auth(),
        ));

        let (ws_stream, response) =
            client_async_tls_with_config(url_str, tcp_stream, None, Some(connector)).await?;

        debug!(
            "[{}] WebSocket handshake complete, status: {:?}",
            self.worker_id,
            response.status()
        );
        let (mut write, mut read) = ws_stream.split();

        gauge!("aggregator_active_connections", "platform" => self.config.platform_label.clone())
            .increment(1.0);
        info!("[{}] WebSocket connected", self.worker_id);

        // Send initial subscription message
        if let Some(init_msg) = self.handler.on_connect_message() {
            debug!("[{}] Sending subscription: {}", self.worker_id, init_msg);
            write.send(Message::Text(init_msg)).await?;
        }

        self.handler.on_reconnect().await;

        let mut ping_interval = interval(self.config.ping_interval);
        ping_interval.reset(); // Don't fire immediately

        loop {
            tokio::select! {
                // Handle incoming WebSocket messages
                msg = read.next() => {
                    match msg {
                        Some(Ok(Message::Text(text))) => {
                            counter!("aggregator_messages_received_total", "platform" => self.config.platform_label.clone()).increment(1);
                            if let Err(e) = self.handler.on_message(&text).await {
                                error!("[{}] Error handling message: {:?}", self.worker_id, e);
                                counter!("aggregator_errors_total", "platform" => self.config.platform_label.clone(), "error_type" => "handler").increment(1);
                            }
                        }
                        Some(Ok(Message::Binary(data))) => {
                            if let Err(e) = self.handler.on_binary_message(&data).await {
                                error!("[{}] Error handling binary message: {:?}", self.worker_id, e);
                            }
                        }
                        Some(Ok(Message::Ping(data))) => {
                            debug!("[{}] Received ping, sending pong", self.worker_id);
                            write.send(Message::Pong(data)).await?;
                        }
                        Some(Ok(Message::Pong(_))) => {
                            debug!("[{}] Received pong", self.worker_id);
                        }
                        Some(Ok(Message::Close(frame))) => {
                            info!("[{}] Received close frame: {:?}", self.worker_id, frame);
                            return Err(Error::ConnectionClosed);
                        }
                        Some(Ok(Message::Frame(_))) => {
                            // Raw frame, ignore
                        }
                        Some(Err(e)) => {
                            error!("[{}] WebSocket error: {:?}", self.worker_id, e);
                            return Err(Error::WebSocket(e));
                        }
                        None => {
                            info!("[{}] WebSocket stream ended", self.worker_id);
                            return Err(Error::ConnectionClosed);
                        }
                    }
                }

                // Handle control commands
                cmd = self.command_rx.recv() => {
                    match cmd {
                        Some(ControlCommand::Shutdown) => {
                            info!("[{}] Received shutdown command", self.worker_id);
                            *shutdown = true;
                            let close_frame = CloseFrame {
                                code: CloseCode::Normal,
                                reason: "Shutdown".into(),
                            };
                            let _ = write.send(Message::Close(Some(close_frame))).await;
                            return Ok(());
                        }
                        Some(cmd) => {
                            if let Some(msg) = self.handler.handle_command(cmd).await {
                                debug!("[{}] Sending subscription update: {}", self.worker_id, msg);
                                write.send(Message::Text(msg)).await?;
                            }
                        }
                        None => {
                            // Command channel closed, treat as shutdown
                            info!("[{}] Command channel closed", self.worker_id);
                            *shutdown = true;
                            return Ok(());
                        }
                    }
                }

                // Send periodic pings
                _ = ping_interval.tick() => {
                    debug!("[{}] Sending ping", self.worker_id);
                    write.send(Message::Ping(vec![])).await?;
                }
            }
        }
    }
}
