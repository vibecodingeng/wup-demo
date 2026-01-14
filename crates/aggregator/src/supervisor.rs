//! Connection Supervisor - manages a pool of WebSocket workers.

use anyhow::Result;
use common::{ControlCommand, WsManager, WsManagerConfig};
use external_services::polymarket::TokenMapping;
use metrics::gauge;
use nats_client::NatsClient;
use polymarket::PolymarketHandler;
use std::collections::HashMap;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{error, info, warn};

/// Represents a running worker with its control channel and metadata.
struct WorkerHandle {
    /// Channel to send commands to the worker.
    command_tx: mpsc::Sender<ControlCommand>,
    /// Join handle for the worker task.
    join_handle: JoinHandle<()>,
    /// Current subscriptions in this worker (token IDs).
    subscribed_ids: Vec<String>,
}

/// Supervisor manages multiple WebSocket workers.
/// It distributes subscriptions across workers respecting limits.
pub struct Supervisor {
    /// NATS client shared across all workers.
    nats_client: NatsClient,
    /// Maximum subscriptions per worker.
    max_subs_per_worker: usize,
    /// Active workers indexed by worker ID.
    workers: HashMap<String, WorkerHandle>,
    /// Counter for generating unique worker IDs.
    worker_counter: u64,
    /// Token mappings for subject routing.
    token_mappings: HashMap<String, TokenMapping>,
}

impl Supervisor {
    /// Create a new Supervisor.
    pub fn new(nats_client: NatsClient, max_subs_per_worker: usize) -> Self {
        Self {
            nats_client,
            max_subs_per_worker,
            workers: HashMap::new(),
            worker_counter: 0,
            token_mappings: HashMap::new(),
        }
    }

    /// Subscribe with token mappings (new API).
    /// The supervisor will distribute them across existing workers or spawn new ones.
    pub async fn subscribe_with_mappings(&mut self, mappings: Vec<TokenMapping>) -> Result<()> {
        // Store mappings and filter out already subscribed ones
        let mut new_mappings: Vec<TokenMapping> = Vec::new();

        for mapping in mappings {
            if !self.is_subscribed(&mapping.clob_token_id) {
                self.token_mappings
                    .insert(mapping.clob_token_id.clone(), mapping.clone());
                new_mappings.push(mapping);
            }
        }

        if new_mappings.is_empty() {
            info!("All requested tokens are already subscribed");
            return Ok(());
        }

        info!("Subscribing to {} new tokens", new_mappings.len());

        // First, try to fill existing workers
        for (worker_id, handle) in self.workers.iter_mut() {
            if new_mappings.is_empty() {
                break;
            }

            let available = self
                .max_subs_per_worker
                .saturating_sub(handle.subscribed_ids.len());
            if available > 0 {
                let to_add: Vec<TokenMapping> =
                    new_mappings.drain(..available.min(new_mappings.len())).collect();

                let ids: Vec<String> = to_add.iter().map(|m| m.clob_token_id.clone()).collect();

                if handle
                    .command_tx
                    .send(ControlCommand::Subscribe(ids.clone()))
                    .await
                    .is_ok()
                {
                    handle.subscribed_ids.extend(ids);
                    info!("Added subscriptions to existing worker {}", worker_id);
                } else {
                    // Worker is dead, put mappings back
                    new_mappings.extend(to_add);
                    warn!("Worker {} command channel closed", worker_id);
                }
            }
        }

        // Spawn new workers for remaining mappings
        while !new_mappings.is_empty() {
            let chunk: Vec<TokenMapping> = new_mappings
                .drain(..self.max_subs_per_worker.min(new_mappings.len()))
                .collect();

            self.spawn_worker_with_mappings(chunk).await?;
        }

        self.update_metrics();
        Ok(())
    }

    /// Subscribe to new asset IDs (legacy API).
    /// The supervisor will distribute them across existing workers or spawn new ones.
    pub async fn subscribe(&mut self, ids: Vec<String>) -> Result<()> {
        let mut remaining: Vec<String> = ids
            .into_iter()
            .filter(|id| !self.is_subscribed(id))
            .collect();

        if remaining.is_empty() {
            info!("All requested IDs are already subscribed");
            return Ok(());
        }

        info!("Subscribing to {} new assets", remaining.len());

        // First, try to fill existing workers
        for (worker_id, handle) in self.workers.iter_mut() {
            if remaining.is_empty() {
                break;
            }

            let available = self
                .max_subs_per_worker
                .saturating_sub(handle.subscribed_ids.len());
            if available > 0 {
                let to_add: Vec<String> =
                    remaining.drain(..available.min(remaining.len())).collect();

                if handle
                    .command_tx
                    .send(ControlCommand::Subscribe(to_add.clone()))
                    .await
                    .is_ok()
                {
                    handle.subscribed_ids.extend(to_add);
                    info!("Added subscriptions to existing worker {}", worker_id);
                } else {
                    // Worker is dead, put IDs back
                    remaining.extend(to_add);
                    warn!("Worker {} command channel closed", worker_id);
                }
            }
        }

        // Spawn new workers for remaining IDs
        while !remaining.is_empty() {
            let chunk: Vec<String> = remaining
                .drain(..self.max_subs_per_worker.min(remaining.len()))
                .collect();

            self.spawn_worker(chunk).await?;
        }

        self.update_metrics();
        Ok(())
    }

    /// Unsubscribe from asset IDs.
    pub async fn unsubscribe(&mut self, ids: Vec<String>) -> Result<()> {
        info!("Unsubscribing from {} assets", ids.len());

        for (worker_id, handle) in self.workers.iter_mut() {
            let to_remove: Vec<String> = ids
                .iter()
                .filter(|id| handle.subscribed_ids.contains(id))
                .cloned()
                .collect();

            if !to_remove.is_empty() {
                if handle
                    .command_tx
                    .send(ControlCommand::Unsubscribe(to_remove.clone()))
                    .await
                    .is_ok()
                {
                    handle.subscribed_ids.retain(|id| !to_remove.contains(id));
                    // Also remove from token mappings
                    for id in &to_remove {
                        self.token_mappings.remove(id);
                    }
                    info!("Removed subscriptions from worker {}", worker_id);
                }
            }
        }

        self.update_metrics();
        Ok(())
    }

    /// Check if an ID is already subscribed.
    fn is_subscribed(&self, id: &str) -> bool {
        self.workers
            .values()
            .any(|h| h.subscribed_ids.contains(&id.to_string()))
    }

    /// Spawn a new worker with token mappings.
    async fn spawn_worker_with_mappings(&mut self, mappings: Vec<TokenMapping>) -> Result<()> {
        self.worker_counter += 1;
        let worker_id = format!("poly-worker-{}", self.worker_counter);

        let ids: Vec<String> = mappings.iter().map(|m| m.clob_token_id.clone()).collect();

        info!(
            "Spawning worker {} with {} subscriptions (with mappings)",
            worker_id,
            ids.len()
        );

        let (command_tx, command_rx) = mpsc::channel::<ControlCommand>(32);

        let handler = PolymarketHandler::new_with_mappings(
            mappings,
            self.nats_client.clone(),
            worker_id.clone(),
        );

        let config = WsManagerConfig {
            platform_label: "polymarket".to_string(),
            ..Default::default()
        };

        let manager = WsManager::new(handler, config, command_rx, worker_id.clone());

        let join_handle = tokio::spawn(async move {
            if let Err(e) = manager.run().await {
                error!("Worker failed: {:?}", e);
            }
        });

        self.workers.insert(
            worker_id,
            WorkerHandle {
                command_tx,
                join_handle,
                subscribed_ids: ids,
            },
        );

        self.update_metrics();
        Ok(())
    }

    /// Spawn a new worker with initial subscriptions (legacy).
    async fn spawn_worker(&mut self, initial_ids: Vec<String>) -> Result<()> {
        self.worker_counter += 1;
        let worker_id = format!("poly-worker-{}", self.worker_counter);

        info!(
            "Spawning worker {} with {} subscriptions",
            worker_id,
            initial_ids.len()
        );

        let (command_tx, command_rx) = mpsc::channel::<ControlCommand>(32);

        let handler = PolymarketHandler::new(
            initial_ids.clone(),
            self.nats_client.clone(),
            worker_id.clone(),
        );

        let config = WsManagerConfig {
            platform_label: "polymarket".to_string(),
            ..Default::default()
        };

        let manager = WsManager::new(handler, config, command_rx, worker_id.clone());

        let join_handle = tokio::spawn(async move {
            if let Err(e) = manager.run().await {
                error!("Worker failed: {:?}", e);
            }
        });

        self.workers.insert(
            worker_id,
            WorkerHandle {
                command_tx,
                join_handle,
                subscribed_ids: initial_ids,
            },
        );

        self.update_metrics();
        Ok(())
    }

    /// Run the supervisor. This monitors workers and handles failures.
    pub async fn run(&mut self) -> Result<()> {
        info!("Supervisor running with {} workers", self.workers.len());

        // For now, just wait for all workers to complete
        // In a production system, you'd add:
        // - Health monitoring loop
        // - Worker failure detection and recovery
        // - Runtime subscription management via API

        // Wait for Ctrl+C or termination signal
        tokio::signal::ctrl_c().await.ok();

        info!("Received shutdown signal, stopping workers...");

        // Send shutdown command to all workers
        for (worker_id, handle) in self.workers.iter() {
            info!("Sending shutdown to {}", worker_id);
            let _ = handle.command_tx.send(ControlCommand::Shutdown).await;
        }

        // Wait for all workers to complete
        let mut handles: Vec<JoinHandle<()>> =
            self.workers.drain().map(|(_, h)| h.join_handle).collect();

        for handle in handles.drain(..) {
            let _ = handle.await;
        }

        info!("Supervisor shutting down");
        Ok(())
    }

    /// Update Prometheus metrics.
    fn update_metrics(&self) {
        let total_subs: usize = self.workers.values().map(|h| h.subscribed_ids.len()).sum();
        gauge!("aggregator_active_subscriptions", "platform" => "polymarket")
            .set(total_subs as f64);
        gauge!("aggregator_worker_count", "platform" => "polymarket")
            .set(self.workers.len() as f64);
    }

    /// Get the total number of active subscriptions.
    pub fn total_subscriptions(&self) -> usize {
        self.workers.values().map(|h| h.subscribed_ids.len()).sum()
    }

    /// Get the number of active workers.
    pub fn worker_count(&self) -> usize {
        self.workers.len()
    }
}
