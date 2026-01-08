//! Paper trading simulator.
//!
//! Provides:
//! - Print-driven fill simulation (fills when price touches our level)
//! - Deterministic replay from recorded events
//! - Simulated order latency
//! - Fee calculation matching live environment

pub mod fill_sim;
pub mod paper_book;
pub mod replay;
pub mod recorded_backtest;

pub use fill_sim::{FillSimulator, FillSimConfig, SimulatedFill};
pub use paper_book::PaperBook;
pub use replay::ReplayEngine;
pub use recorded_backtest::{BacktestConfig, BacktestReport, RecordedSnapshot, load_snapshots, run_backtest};
