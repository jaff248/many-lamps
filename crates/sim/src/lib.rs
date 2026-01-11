//! Paper trading simulator.
//!
//! Provides:
//! - Print-driven fill simulation (fills when price touches our level)
//! - Deterministic replay from recorded events
//! - Simulated order latency
//! - Fee calculation matching live environment

pub mod event_driven_backtest;
pub mod fill_sim;
pub mod paper_book;
pub mod realistic_fills;
pub mod recorded_backtest;
pub mod replay;
pub mod walk_forward;

pub use event_driven_backtest::{
    BacktestMetrics, BacktestResults, EventDrivenBacktest, EventDrivenBacktestConfig,
    SimulationEvent,
};
pub use fill_sim::{FillSimConfig, FillSimulator, SimulatedFill};
pub use paper_book::{PaperBook, PaperOrder, PerformanceReport, TradeRecord};
pub use realistic_fills::{
    RealisticFill, RealisticFillConfig, RealisticFillSimulator, QueuePosition,
    QueuePositionModel, FillCheckResult,
};
pub use recorded_backtest::{
    load_snapshots, run_backtest, BacktestConfig, BacktestReport, RecordedSnapshot,
};
pub use replay::ReplayEngine;
pub use walk_forward::{
    AggregatedMetrics, BootstrapResults, MonteCarloResult, OptimizationMetric,
    OverfitAnalysis, RobustnessAssessment, WalkForwardConfig, WalkForwardOptimizer,
    WalkForwardResults, WindowSplit,
};
