//! Trading strategy crate.
//!
//! Provides:
//! - Strategy trait for common interface
//! - MakerMMStrategy for market making on single outcomes
//! - BundleMakerStrategy for YES/NO arbitrage
//! - RebalancingArbStrategy for price sum arbitrage
//! - MLStrategy for machine learning predictions
//! - Signal processing utilities

pub mod auto_hedge;
pub mod bundle_maker;
pub mod combinatorial_arb;
pub mod ensemble;
pub mod flow;
pub mod maker_mm;
pub mod ml_strategy;
pub mod rebalancing_arb;
pub mod regime_detector;
pub mod signals;
pub mod traits;
pub mod unaffected_arb;

pub use auto_hedge::AutoHedgeStrategy;
pub use bundle_maker::BundleMakerStrategy;
pub use combinatorial_arb::{
    CombinatorialArbConfig, CombinatorialArbStrategy, Dependency, DependencyType,
};
pub use ensemble::{EnsembleConfig, EnsembleStrategyPerformance, StrategyEnsemble};
pub use flow::{FlowSignal, FlowSignalConfig, FlowSnapshot};
pub use maker_mm::MakerMMStrategy;
pub use ml_strategy::{MlStrategy, MlStrategyConfig, StrategyPerformance, StrategyStats, TradeOutcome};
pub use rebalancing_arb::{
    run_arb_scanner, ArbSummary, ArbType, RebalancingArbConfig, RebalancingArbOpportunity,
    RebalancingArbState, RebalancingArbStrategy,
};
pub use signals::{Signal, SignalProcessor};
pub use traits::{
    MarketSnapshot, MarketTokenKey, Strategy, StrategyAction, StrategyContext, WorkingOrder,
};
pub use unaffected_arb::UnaffectedArbStrategy;
pub use regime_detector::{
    MarketRegime, RegimeConditionalSelector, RegimeDetector, RegimeDetectorConfig,
};
