//! Trading strategy crate.
//!
//! Provides:
//! - Strategy trait for common interface
//! - MakerMMStrategy for market making on single outcomes
//! - BundleMakerStrategy for YES/NO arbitrage
//! - Signal processing utilities

pub mod maker_mm;
pub mod bundle_maker;
pub mod signals;
pub mod traits;

pub use maker_mm::MakerMMStrategy;
pub use bundle_maker::BundleMakerStrategy;
pub use signals::{Signal, SignalProcessor};
pub use traits::{Strategy, StrategyAction, StrategyContext, WorkingOrder};
