//! TUI Screens - Additional screen components for the MTrader TUI

use serde::{Deserialize, Serialize};

/// Backtest configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BacktestConfig {
    pub input_dir: String,
    pub shares_per_leg: u64,
    pub sum_target: f64,
    pub dip_threshold: f64,
    pub window_minutes: u64,
    pub dip_window_ms: u64,
    pub fee_rate_bps: u16,
    pub spread_bps: f64,
    pub leg2_timeout_seconds: u64,
    pub starting_balance: f64,
    pub output_report: Option<String>,
}

/// Market information
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MarketInfo {
    pub condition_id: String,
    pub question: String,
    pub market_slug: String,
    pub active: bool,
    pub yes_price: Option<f64>,
    pub no_price: Option<f64>,
    pub volume: Option<f64>,
    pub liquidity: Option<f64>,
}

/// Sample markets for the browser
pub fn get_sample_markets() -> Vec<MarketInfo> {
    vec![
        MarketInfo {
            condition_id: "btc-updown-15m-1767933000".to_string(),
            question: "Will BTC be up or down in 15 minutes?".to_string(),
            market_slug: "btc-updown-15m-1767933000".to_string(),
            active: true,
            yes_price: Some(0.55),
            no_price: Some(0.45),
            volume: Some(50000.0),
            liquidity: Some(10000.0),
        },
        MarketInfo {
            condition_id: "btc-updown-15m-1767994200".to_string(),
            question: "Will BTC be up or down in 15 minutes?".to_string(),
            market_slug: "btc-updown-15m-1767994200".to_string(),
            active: true,
            yes_price: Some(0.52),
            no_price: Some(0.48),
            volume: Some(35000.0),
            liquidity: Some(8000.0),
        },
        MarketInfo {
            condition_id: "eth-updown-15m-1767933000".to_string(),
            question: "Will ETH be up or down in 15 minutes?".to_string(),
            market_slug: "eth-updown-15m-1767933000".to_string(),
            active: true,
            yes_price: Some(0.58),
            no_price: Some(0.42),
            volume: Some(25000.0),
            liquidity: Some(5000.0),
        },
    ]
}

/// Get available strategies
pub fn get_available_strategies() -> Vec<StrategyInfo> {
    vec![
        StrategyInfo {
            id: "maker_mm".to_string(),
            name: "Maker MM".to_string(),
            description: "Market making strategy that earns spread".to_string(),
        },
        StrategyInfo {
            id: "bundle_maker".to_string(),
            name: "Bundle Maker".to_string(),
            description: "Bundle arbitrage for correlated markets".to_string(),
        },
        StrategyInfo {
            id: "unaffected_arb".to_string(),
            name: "Unaffected Arb".to_string(),
            description: "Arbitrage on unaffected assets".to_string(),
        },
        StrategyInfo {
            id: "rebalancing_arb".to_string(),
            name: "Rebalancing Arb".to_string(),
            description: "NO/YES price rebalancing arbitrage".to_string(),
        },
    ]
}

/// Strategy information
#[derive(Debug, Clone)]
pub struct StrategyInfo {
    pub id: String,
    pub name: String,
    pub description: String,
}
