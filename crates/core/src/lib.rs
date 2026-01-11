//! Core types, events, and system health management for Many Lamps trading system.
//!
//! This crate provides:
//! - Type definitions shared across all crates
//! - SystemHealth state machine with SAFE_MODE
//! - Event types for the deterministic core loop
//! - Fee model with fixed-point arithmetic
//! - Normalized data contracts for ML pipelines

pub mod clock;
pub mod data_contracts;
pub mod events;
pub mod fees;
pub mod health;
pub mod health_monitor;
pub mod types;

pub use clock::*;
pub use data_contracts::*;
pub use events::*;
pub use fees::*;
pub use health::*;
pub use health_monitor::*;
pub use types::*;
