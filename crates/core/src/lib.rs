//! Core types, events, and system health management for Many Lamps trading system.
//!
//! This crate provides:
//! - Type definitions shared across all crates
//! - SystemHealth state machine with SAFE_MODE
//! - Event types for the deterministic core loop
//! - Fee model with fixed-point arithmetic

pub mod types;
pub mod events;
pub mod health;
pub mod fees;
pub mod clock;

pub use types::*;
pub use events::*;
pub use health::*;
pub use fees::*;
pub use clock::*;
