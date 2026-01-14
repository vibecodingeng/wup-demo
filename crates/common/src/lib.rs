//! Common types, traits, and utilities for the betting aggregator.

pub mod error;
pub mod messages;
pub mod ws_handler;
pub mod ws_manager;

pub use error::Error;
pub use messages::ControlCommand;
pub use ws_handler::WsHandler;
pub use ws_manager::{WsManager, WsManagerConfig};
