//! Trading strategy crate.
//!
//! Provides:
//! - Strategy trait for common interface
//! - MakerMMStrategy for market making on single outcomes
//! - BundleMakerStrategy for YES/NO arbitrage
//! - RebalancingArbStrategy for price sum arbitrage
//! - Signal processing utilities

pub mod maker_mm;
pub mod bundle_maker;
pub mod auto_hedge;
pub mod unaffected_arb;
pub mod flow;
pub mod signals;
pub mod traits;
pub mod rebalancing_arb;

pub use maker_mm::MakerMMStrategy;
pub use bundle_maker::BundleMakerStrategy;
pub use auto_hedge::AutoHedgeStrategy;
pub use unaffected_arb::UnaffectedArbStrategy;
pub use flow::{FlowSignal, FlowSignalConfig, FlowSnapshot};
pub use signals::{Signal, SignalProcessor};
pub use traits::{Strategy, StrategyAction, StrategyContext, WorkingOrder};
pub use rebalancing_arb::{
    RebalancingArbStrategy,
    RebalancingArbConfig,
    RebalancingArbOpportunity,
    RebalancingArbState,
    ArbType,
    ArbSummary,
    run_arb_scanner,
};
