//! Gateway for Polymarket CLOB WebSocket and REST API.
//!
//! This crate provides:
//! - WebSocket client for market data (book, price_change, trade events)
//! - REST client for snapshots and order management
//! - Message parsing with raw frame recording
//! - Connection management with reconnection logic
//! - Polymarket gateway with EIP-712 authentication and rate limiting

pub mod error;
pub mod messages;
pub mod parser;
pub mod polymarket;
pub mod rest_client;
pub mod ws_client;

pub use error::GatewayError;
pub use messages::*;
pub use parser::*;
pub use polymarket::*;
pub use rest_client::*;
pub use ws_client::*;
