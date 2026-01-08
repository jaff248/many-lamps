//! Backtesting command for recorded auto-hedge snapshots.

use crate::config::Config;
use anyhow::Result;
use mtrader_sim::{BacktestConfig, load_snapshots, run_backtest};
use std::path::Path;
use tracing::info;

pub fn run_auto_backtest(
    _config: &Config,
    input: &str,
    shares: u64,
    sum_target: f64,
    dip_threshold: f64,
    window_minutes: u64,
    dip_window_ms: u64,
    fee_rate_bps: u16,
    spread_bps: f64,
    leg2_timeout_seconds: u64,
    starting_balance_usdc: f64,
    report_path: Option<&str>,
) -> Result<()> {
    let input_path = Path::new(input);
    let snapshots = load_snapshots(input_path)?;
    info!(count = snapshots.len(), "Loaded recorded snapshots");

    let config = BacktestConfig {
        starting_balance_micro: (starting_balance_usdc * 1_000_000.0).round() as i64,
        leg_size: shares * 1_000_000,
        sum_target,
        dip_threshold,
        dip_window_ms,
        window_minutes,
        leg2_timeout_seconds,
        fee_rate_bps,
        spread_bps,
    };

    let report = run_backtest(&snapshots, &config);
    let json = serde_json::to_string_pretty(&report)?;

    if let Some(path) = report_path {
        std::fs::write(path, &json)?;
        info!(path = path, "Wrote backtest report");
    } else {
        println!("{json}");
    }

    Ok(())
}
