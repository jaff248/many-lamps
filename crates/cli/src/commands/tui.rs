//! TUI Command - Launch the full-screen interactive interface

use anyhow::Result;
use mtrader_dashboard::run_tui;

/// Launch the interactive TUI application
pub async fn run() -> Result<()> {
    run_tui()?;
    Ok(())
}
