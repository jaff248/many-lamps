//! TUI Command - Launch the full-screen interactive interface

use anyhow::Result;
use mtrader_dashboard::{run_tui, AppState, MenuItem, Market, PolymarketMarket};

/// Launch the interactive TUI application with optional pre-selected market
pub async fn run(market_id: Option<String>) -> Result<()> {
    let mut app = mtrader_dashboard::App::new()?;
    
    // If a market ID is provided, try to set it as the current market
    if let Some(id) = market_id {
        // Try to find the market in the default list first
        let found = app.state.markets_list.iter().find(|m| m.condition_id == id);
        if let Some(market) = found {
            app.state.market = market.clone();
        } else {
            // Create a custom market entry
            app.state.market = Market {
                condition_id: id.clone(),
                name: format!("Custom: {}", id),
                price: 0.5000,
                volume: 0.0,
            };
        }
        app.state.set_status(&format!("Market set to: {}", id));
    }
    
    app.run()?;
    Ok(())
}
