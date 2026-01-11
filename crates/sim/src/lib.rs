//! Paper trading simulator.
//!
//! Provides:
//! - Print-driven fill simulation (fills when price touches our level)
//! - Deterministic replay from recorded events
//! - Simulated order latency
//! - Fee calculation matching live environment

pub mod fill_sim;
pub mod paper_book;
pub mod recorded_backtest;
pub mod replay;

pub use fill_sim::{FillSimConfig, FillSimulator, SimulatedFill};
pub use paper_book::{PaperBook, PaperOrder, PerformanceReport, TradeRecord};
pub use recorded_backtest::{
    load_snapshots, run_backtest, BacktestConfig, BacktestReport, RecordedSnapshot,
};
pub use replay::ReplayEngine;
