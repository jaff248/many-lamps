//! TUI Command - Launch the full-screen interactive interface

use anyhow::Result;
use mtrader_dashboard::App;

/// Launch the interactive TUI application with optional pre-selected market
pub async fn run(market_id: Option<String>) -> Result<()> {
    let mut app = App::new()?;

    // If a market ID is provided, try to set it as the current market
    if let Some(id) = market_id {
        app.set_market_by_id(&id);
    }

    app.run()?;
    Ok(())
}
