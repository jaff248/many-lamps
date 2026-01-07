//! Gateway for Polymarket CLOB WebSocket and REST API.
//!
//! This crate provides:
//! - WebSocket client for market data (book, price_change, trade events)
//! - REST client for snapshots and order management
//! - Message parsing with raw frame recording
//! - Connection management with reconnection logic

pub mod messages;
pub mod parser;
pub mod ws_client;
pub mod rest_client;
pub mod error;

pub use messages::*;
pub use parser::*;
pub use ws_client::*;
pub use rest_client::*;
pub use error::GatewayError;
