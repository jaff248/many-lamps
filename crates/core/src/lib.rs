//! Core types, events, and system health management for Many Lamps trading system.
//!
//! This crate provides:
//! - Type definitions shared across all crates
//! - SystemHealth state machine with SAFE_MODE
//! - Event types for the deterministic core loop
//! - Fee model with fixed-point arithmetic

pub mod clock;
pub mod events;
pub mod fees;
pub mod health;
pub mod types;

pub use clock::*;
pub use events::*;
pub use fees::*;
pub use health::*;
pub use types::*;
