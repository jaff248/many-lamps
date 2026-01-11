//! MTrader TUI Application - Production Trading Terminal
//!
//! Features: Auto-trading, risk management, strategy configuration, backtesting

use crate::format_backtest_report;
use anyhow::Result;
use chrono::Local;
use crossterm::{
    event::{self, Event, KeyCode},
    terminal::{disable_raw_mode, enable_raw_mode},
};
use many_lamps_core::fees::MarketFeeProfile;
use mtrader_sim::{load_snapshots, run_backtest, BacktestConfig as SimBacktestConfig};
use mtrader_strategy::traits::{Strategy, StrategyAction, StrategyContext};
use mtrader_strategy::bundle_maker::BundleMakerConfig;
use mtrader_strategy::bundle_maker::BundleMakerStrategy;
use mtrader_strategy::maker_mm::MakerMMConfig;
use mtrader_strategy::maker_mm::MakerMMStrategy;
use mtrader_strategy::unaffected_arb::UnaffectedArbConfig;
use mtrader_strategy::unaffected_arb::UnaffectedArbStrategy;
use mtrader_strategy::auto_hedge::{AutoHedgeConfig, AutoHedgeStrategy};
use mtrader_strategy::combinatorial_arb::{CombinatorialArbConfig, CombinatorialArbStrategy};
use mtrader_strategy::ml_strategy::MlStrategy;
use mtrader_strategy::ml_strategy::MlStrategyConfig;
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Widget},
    Terminal,
};
use std::collections::HashMap;
use std::{io::{self, Stdout}, time::{Duration, Instant}};

const VERSION: &str = "2.0.0";

/// Built-in supported markets (12 crypto up/down markets)
pub const DEFAULT_MARKETS: &[(&str, &str, f64, f64)] = &[
    ("btc-updown-15m", "BTC Up/Down 15min", 0.5200, 150000.0),
    ("btc-updown-1h", "BTC Up/Down 1hour", 0.4850, 85000.0),
    ("btc-updown-4h", "BTC Up/Down 4hour", 0.5100, 95000.0),
    ("eth-updown-15m", "ETH Up/Down 15min", 0.5250, 120000.0),
    ("eth-updown-1h", "ETH Up/Down 1hour", 0.4980, 90000.0),
    ("sol-updown-15m", "SOL Up/Down 15min", 0.5120, 95000.0),
    ("doge-updown-5m", "DOGE Up/Down 5min", 0.4950, 320000.0),
    ("avax-updown-15m", "AVAX Up/Down 15min", 0.5050, 45000.0),
    ("matic-updown-15m", "MATIC Up/Down 15min", 0.4980, 38000.0),
    ("link-updown-15m", "LINK Up/Down 15min", 0.5100, 52000.0),
    ("ada-updown-15m", "ADA Up/Down 15min", 0.4920, 28000.0),
    ("dot-updown-15m", "DOT Up/Down 15min", 0.4880, 35000.0),
];

/// All available strategies for selection
pub const AVAILABLE_STRATEGIES: &[&str] = &[
    "maker_mm",
    "bundle_maker",
    "unaffected_arb",
    "rebalancing_arb",
    "auto_hedge",
    "combinatorial_arb",
    "ml",
];

/// Trading mode (paper or live)
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum TradingMode {
    #[default]
    Paper,
    Live,
}

/// Auto-trading state
#[derive(Clone, Debug, PartialEq, Default)]
pub enum AutoTradingState {
    #[default]
    Disabled,
    Running,
    Paused,
    Halted(String),
}

/// Strategy signal for display
#[derive(Clone, Debug)]
pub struct StrategySignal {
    pub timestamp: String,
    pub signal_type: String,
    pub details: String,
    pub side: Option<String>,
    pub size: Option<i64>,
    pub price: Option<f64>,
}

impl Default for StrategySignal {
    fn default() -> Self {
        Self {
            timestamp: String::new(),
            signal_type: "No Signal".to_string(),
            details: "Waiting for market data...".to_string(),
            side: None,
            size: None,
            price: None,
        }
    }
}

/// Menu navigation
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum MenuItem {
    #[default]
    MainMenu,
    PaperTrading,
    Record,
    Backtest,
    Replay,
    MarketSelection,
    MarketBrowser,
    LiveTrading,
    Strategies,
    StrategySettings,
    RiskSettings,
    Help,
    Quit,
}

/// Input mode for text editing
#[derive(Clone, Debug, PartialEq, Default)]
pub enum InputMode {
    #[default]
    Normal,
    Editing(EditField),
    Confirming(ConfirmAction),
}

#[derive(Clone, Debug)]
pub enum EditField {
    MarketId,
    BacktestDir,
    WalletAddress,
    StrategyParam(String),
}

impl PartialEq for EditField {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (EditField::MarketId, EditField::MarketId) => true,
            (EditField::BacktestDir, EditField::BacktestDir) => true,
            (EditField::WalletAddress, EditField::WalletAddress) => true,
            (EditField::StrategyParam(a), EditField::StrategyParam(b)) => a == b,
            _ => false,
        }
    }
}

#[derive(Clone, Debug)]
pub enum ConfirmAction {
    EnableLiveTrading,
    ClearPosition,
    ResetCircuitBreaker,
    DeleteMarket,
}

impl PartialEq for ConfirmAction {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (ConfirmAction::EnableLiveTrading, ConfirmAction::EnableLiveTrading) => true,
            (ConfirmAction::ClearPosition, ConfirmAction::ClearPosition) => true,
            (ConfirmAction::ResetCircuitBreaker, ConfirmAction::ResetCircuitBreaker) => true,
            (ConfirmAction::DeleteMarket, ConfirmAction::DeleteMarket) => true,
            _ => false,
        }
    }
}

/// Market information
#[derive(Clone, Debug)]
pub struct Market {
    pub condition_id: String,
    pub name: String,
    pub price: f64,
    pub volume: f64,
}

impl Default for Market {
    fn default() -> Self {
        Self {
            condition_id: "btc-updown-15m".to_string(),
            name: "BTC Up/Down 15min".to_string(),
            price: 0.5200,
            volume: 150000.0,
        }
    }
}

/// Polymarket API market data
#[derive(Clone, Debug, Default)]
pub struct PolymarketMarket {
    pub condition_id: String,
    pub question: String,
    pub slug: String,
    pub active: bool,
    pub yes_price: Option<f64>,
    pub no_price: Option<f64>,
    pub volume: f64,
    pub liquidity: f64,
}

/// Market selection state for the TUI
#[derive(Clone, Debug, Default)]
pub struct MarketSelection {
    pub markets: Vec<PolymarketMarket>,
    pub filtered_markets: Vec<PolymarketMarket>,
    pub selected_index: usize,
    pub search_query: String,
    pub is_searching: bool,
    pub is_loading: bool,
    pub sort_by: MarketSortBy,
    pub sort_desc: bool,
    pub page: usize,
    pub page_size: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum MarketSortBy {
    #[default]
    Volume,
    Name,
    Price,
    Activity,
}

impl MarketSelection {
    pub fn new() -> Self {
        Self {
            markets: Vec::new(),
            filtered_markets: Vec::new(),
            selected_index: 0,
            search_query: String::new(),
            is_searching: false,
            is_loading: false,
            sort_by: MarketSortBy::Volume,
            sort_desc: true,
            page: 0,
            page_size: 20,
        }
    }

    pub fn filter_and_sort(&mut self) {
        // Filter by search query
        if self.search_query.is_empty() {
            self.filtered_markets = self.markets.clone();
        } else {
            let query = self.search_query.to_lowercase();
            self.filtered_markets = self
                .markets
                .iter()
                .filter(|m| {
                    m.question.to_lowercase().contains(&query)
                        || m.slug.to_lowercase().contains(&query)
                        || m.condition_id.to_lowercase().contains(&query)
                })
                .cloned()
                .collect();
        }

        // Sort markets
        match self.sort_by {
            MarketSortBy::Volume => {
                self.filtered_markets
                    .sort_by(|a, b| b.volume.partial_cmp(&a.volume).unwrap_or(std::cmp::Ordering::Equal));
            }
            MarketSortBy::Name => {
                self.filtered_markets
                    .sort_by(|a, b| a.question.cmp(&b.question));
            }
            MarketSortBy::Price => {
                self.filtered_markets
                    .sort_by(|a, b| {
                        let a_price = a.yes_price.unwrap_or(0.5);
                        let b_price = b.yes_price.unwrap_or(0.5);
                        b_price.partial_cmp(&a_price).unwrap_or(std::cmp::Ordering::Equal)
                    });
            }
            MarketSortBy::Activity => {
                // Sort by volume for activity
                self.filtered_markets
                    .sort_by(|a, b| b.volume.partial_cmp(&a.volume).unwrap_or(std::cmp::Ordering::Equal));
            }
        }

        if self.sort_desc {
            self.filtered_markets.reverse();
        }

        // Reset pagination
        self.page = 0;
        self.selected_index = 0;
    }

    pub fn total_pages(&self) -> usize {
        (self.filtered_markets.len() + self.page_size - 1) / self.page_size
    }

    pub fn current_page_markets(&self) -> &[PolymarketMarket] {
        let start = self.page * self.page_size;
        let end = std::cmp::min(start + self.page_size, self.filtered_markets.len());
        &self.filtered_markets[start..end]
    }

    pub fn can_next_page(&self) -> bool {
        self.page < self.total_pages().saturating_sub(1)
    }

    pub fn can_prev_page(&self) -> bool {
        self.page > 0
    }

    pub fn next_page(&mut self) {
        if self.can_next_page() {
            self.page += 1;
            self.selected_index = 0;
        }
    }

    pub fn prev_page(&mut self) {
        if self.can_prev_page() {
            self.page -= 1;
            self.selected_index = 0;
        }
    }

    pub fn select_next(&mut self) {
        let page_count = self.current_page_markets().len();
        if page_count > 0 {
            self.selected_index = (self.selected_index + 1).min(page_count - 1);
        }
    }

    pub fn select_prev(&mut self) {
        if self.selected_index > 0 {
            self.selected_index -= 1;
        }
    }

    pub fn selected_market(&self) -> Option<&PolymarketMarket> {
        let page_markets = self.current_page_markets();
        page_markets.get(self.selected_index)
    }
}

/// Risk configuration for TUI
#[derive(Clone, Debug)]
pub struct RiskConfig {
    pub max_position: i64,
    pub max_order_size: u64,
    pub max_bet_percentage: f64,
    pub max_portfolio_percentage: f64,
    pub max_drawdown_bps: i64,
    pub max_loss_per_hour: i64,
    pub stop_consecutive_losses: u32,
    pub max_daily_trades: u64,
}

impl Default for RiskConfig {
    fn default() -> Self {
        Self {
            max_position: 200,
            max_order_size: 20,
            max_bet_percentage: 5.0,
            max_portfolio_percentage: 20.0,
            max_drawdown_bps: 500,
            max_loss_per_hour: 100_000_000,
            stop_consecutive_losses: 10,
            max_daily_trades: 0,
        }
    }
}

/// Strategy parameters (configurable)
#[derive(Clone, Debug)]
pub struct StrategyParams {
    pub spread_bps: u16,
    pub order_size: u64,
    pub num_levels: u8,
    pub inventory_skew: bool,
    pub sum_target: f64,
    pub edge_threshold_bps: u16,
    pub dip_threshold: f64,
    pub window_minutes: u64,
}

impl Default for StrategyParams {
    fn default() -> Self {
        Self {
            spread_bps: 50,
            order_size: 10,
            num_levels: 1,
            inventory_skew: true,
            sum_target: 1.0,
            edge_threshold_bps: 20,
            dip_threshold: 0.02,
            window_minutes: 60,
        }
    }
}

/// Application state
impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState")
            .field("current_menu", &self.current_menu)
            .field("selected_index", &self.selected_index)
            .field("input_mode", &self.input_mode)
            .field("text_input", &self.text_input)
            .field("edit_field", &self.edit_field)
            .field("trading_mode", &self.trading_mode)
            .field("auto_trading", &self.auto_trading)
            .field("market", &self.market)
            .field("markets_list", &self.markets_list)
            .field("selected_market", &self.selected_market)
            .field("market_search", &self.market_search)
            .field("strategy_name", &self.strategy_name)
            .field("strategy_params", &self.strategy_params)
            .field("active_strategy", &"<Box<dyn Strategy>>")
            .field("risk_config", &self.risk_config)
            .field("circuit_breaker_tripped", &self.circuit_breaker_tripped)
            .field("circuit_breaker_reason", &self.circuit_breaker_reason)
            .field("consecutive_losses", &self.consecutive_losses)
            .field("position", &self.position)
            .field("realized_pnl", &self.realized_pnl)
            .field("unrealized_pnl", &self.unrealized_pnl)
            .field("total_fees", &self.total_fees)
            .field("trades_count", &self.trades_count)
            .field("daily_trades", &self.daily_trades)
            .field("best_bid", &self.best_bid)
            .field("best_ask", &self.best_ask)
            .field("active_bids", &self.active_bids)
            .field("active_asks", &self.active_asks)
            .field("is_recording", &self.is_recording)
            .field("recording_start", &self.recording_start)
            .field("recording_events", &self.recording_events)
            .field("backtest_dir", &self.backtest_dir)
            .field("backtest_balance", &self.backtest_balance)
            .field("backtest_results", &self.backtest_results)
            .field("replay_files", &self.replay_files)
            .field("selected_replay", &self.selected_replay)
            .field("replay_speed", &self.replay_speed)
            .field("replay_paused", &self.replay_paused)
            .field("replay_progress", &self.replay_progress)
            .field("wallet_address", &self.wallet_address)
            .field("risk_limit", &self.risk_limit)
            .field("status_message", &self.status_message)
            .field("last_action_time", &self.last_action_time)
            .field("activity_log", &self.activity_log)
            .finish()
    }
}

pub struct AppState {
    // Navigation
    pub current_menu: MenuItem,
    pub selected_index: usize,
    pub input_mode: InputMode,
    pub text_input: String,
    pub edit_field: Option<EditField>,

    // Trading mode
    pub trading_mode: TradingMode,
    pub auto_trading: AutoTradingState,

    // Market
    pub market: Market,
    pub markets_list: Vec<Market>,
    pub selected_market: usize,
    pub market_search: String,
    pub market_selection: MarketSelection,

    // Strategy
    pub strategy_name: String,
    pub strategy_params: StrategyParams,
    pub active_strategy: Option<Box<dyn Strategy>>,
    pub strategy_signals: Vec<StrategySignal>,
    pub selected_strategy_index: usize,

    // Risk
    pub risk_config: RiskConfig,
    pub circuit_breaker_tripped: bool,
    pub circuit_breaker_reason: String,
    pub consecutive_losses: u32,

    // Position & PnL
    pub position: i64,
    pub realized_pnl: i64,
    pub unrealized_pnl: i64,
    pub total_fees: i64,
    pub trades_count: u64,
    pub daily_trades: u64,

    // Order book
    pub best_bid: f64,
    pub best_ask: f64,
    pub active_bids: usize,
    pub active_asks: usize,

    // Recording
    pub is_recording: bool,
    pub recording_start: Option<Instant>,
    pub recording_events: u64,

    // Backtest
    pub backtest_dir: String,
    pub backtest_balance: f64,
    pub backtest_results: Option<String>,

    // Replay
    pub replay_files: Vec<String>,
    pub selected_replay: usize,
    pub replay_speed: f64,
    pub replay_paused: bool,
    pub replay_progress: f64,

    // Live trading
    pub wallet_address: String,
    pub risk_limit: f64,

    // UI state
    pub status_message: String,
    pub last_action_time: Instant,
    pub activity_log: Vec<String>,
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

impl AppState {
    pub fn new() -> Self {
        let markets_list: Vec<Market> = DEFAULT_MARKETS
            .iter()
            .map(|(id, name, price, vol)| Market {
                condition_id: id.to_string(),
                name: name.to_string(),
                price: *price,
                volume: *vol,
            })
            .collect();

        Self {
            current_menu: MenuItem::MainMenu,
            selected_index: 0,
            input_mode: InputMode::Normal,
            text_input: String::new(),
            edit_field: None,
            trading_mode: TradingMode::Paper,
            auto_trading: AutoTradingState::Disabled,
            market: markets_list.first().cloned().unwrap_or_default(),
            markets_list,
            selected_market: 0,
            market_search: String::new(),
            market_selection: MarketSelection::new(),
            strategy_name: "maker_mm".to_string(),
            strategy_params: StrategyParams::default(),
            active_strategy: None,
            strategy_signals: Vec::new(),
            selected_strategy_index: 0,
            risk_config: RiskConfig::default(),
            circuit_breaker_tripped: false,
            circuit_breaker_reason: String::new(),
            consecutive_losses: 0,
            position: 0,
            realized_pnl: 0,
            unrealized_pnl: 0,
            total_fees: 0,
            trades_count: 0,
            daily_trades: 0,
            best_bid: 0.5200,
            best_ask: 0.5300,
            active_bids: 0,
            active_asks: 0,
            is_recording: false,
            recording_start: None,
            recording_events: 0,
            backtest_dir: "data/recordings".to_string(),
            backtest_balance: 1000.0,
            backtest_results: None,
            replay_files: vec!["No recordings found".to_string()],
            selected_replay: 0,
            replay_speed: 1.0,
            replay_paused: true,
            replay_progress: 0.0,
            wallet_address: String::new(),
            risk_limit: 10000.0,
            status_message: "Welcome to MTrader v2.0!".to_string(),
            last_action_time: Instant::now(),
            activity_log: Vec::new(),
        }
    }

    pub fn set_status(&mut self, msg: &str) {
        self.status_message = msg.to_string();
        self.last_action_time = Instant::now();
        let ts = Local::now().format("%H:%M:%S").to_string();
        self.activity_log.insert(0, format!("[{}] {}", ts, msg));
        if self.activity_log.len() > 50 {
            self.activity_log.pop();
        }
    }

    /// Create a strategy instance based on strategy name
    pub fn create_strategy(&mut self) -> Result<(), String> {
        let market_id = self.market.condition_id.clone();
        
        self.active_strategy = match self.strategy_name.as_str() {
            "maker_mm" => {
                let config = MakerMMConfig {
                    half_spread_ticks: self.strategy_params.spread_bps / 10,
                    order_size: self.strategy_params.order_size * 1_000_000,
                    max_position: self.risk_config.max_position * 1_000_000,
                    skew_factor: if self.strategy_params.inventory_skew { 0.3 } else { 0.0 },
                    ..Default::default()
                };
                Some(Box::new(MakerMMStrategy::new(
                    format!("maker_mm_{}", market_id),
                    config,
                )) as Box<dyn Strategy>)
            }
            "bundle_maker" => {
                let config = BundleMakerConfig {
                    leg_size: self.strategy_params.order_size * 1_000_000,
                    min_profit_micro_usdc: 100_000,
                    fee_profile: MarketFeeProfile::crypto_15m(),
                    ..Default::default()
                };
                Some(Box::new(BundleMakerStrategy::new(
                    format!("bundle_maker_{}", market_id),
                    config,
                    format!("{}_yes", market_id),
                    format!("{}_no", market_id),
                )) as Box<dyn Strategy>)
            }
            "unaffected_arb" => {
                let config = UnaffectedArbConfig {
                    leg_size: self.strategy_params.order_size * 1_000_000,
                    min_profit_micro_usdc: 100_000,
                    fee_profile: MarketFeeProfile::zero("unaffected"),
                    ..Default::default()
                };
                Some(Box::new(UnaffectedArbStrategy::new(
                    format!("unaffected_arb_{}", market_id),
                    config,
                    format!("{}_yes", market_id),
                    format!("{}_no", market_id),
                )) as Box<dyn Strategy>)
            }
            "rebalancing_arb" => {
                // Note: RebalancingArbStrategy uses async API, not the Strategy trait
                // It's a separate scanning strategy that can be used independently
                self.set_status("Use API scanner for rebalancing arb (not yet integrated with TUI)");
                return Err("RebalancingArb requires async API scanner".to_string());
            }
            "auto_hedge" => {
                let config = AutoHedgeConfig {
                    leg_size: self.strategy_params.order_size * 1_000_000,
                    sum_target: self.strategy_params.sum_target,
                    dip_threshold: self.strategy_params.dip_threshold,
                    window_minutes: self.strategy_params.window_minutes,
                    ..Default::default()
                };
                Some(Box::new(AutoHedgeStrategy::new(
                    format!("auto_hedge_{}", market_id),
                    config,
                    format!("{}_yes", market_id),
                    format!("{}_no", market_id),
                )) as Box<dyn Strategy>)
            }
            "combinatorial_arb" => {
                let config = CombinatorialArbConfig {
                    min_profit_threshold_bps: self.strategy_params.edge_threshold_bps as u32,
                    ..Default::default()
                };
                Some(Box::new(CombinatorialArbStrategy::new(config)) as Box<dyn Strategy>)
            }
            "ml" => {
                let config = MlStrategyConfig {
                    min_position_change: self.strategy_params.order_size as i64,
                    ..Default::default()
                };
                Some(Box::new(MlStrategy::new(
                    &format!("ml_{}", market_id),
                    &market_id,
                    config,
                )) as Box<dyn Strategy>)
            }
            _ => {
                // Default to maker_mm
                let config = MakerMMConfig {
                    half_spread_ticks: self.strategy_params.spread_bps / 10,
                    order_size: self.strategy_params.order_size * 1_000_000,
                    ..Default::default()
                };
                Some(Box::new(MakerMMStrategy::new(
                    format!("maker_mm_{}", market_id),
                    config,
                )) as Box<dyn Strategy>)
            }
        };
        
        self.set_status(&format!("Strategy '{}' initialized", self.strategy_name));
        Ok(())
    }

    /// Switch to next strategy in the list
    pub fn next_strategy(&mut self) {
        let idx = AVAILABLE_STRATEGIES
            .iter()
            .position(|&s| s == self.strategy_name)
            .unwrap_or(0);
        self.strategy_name = AVAILABLE_STRATEGIES[(idx + 1) % AVAILABLE_STRATEGIES.len()].to_string();
        self.set_status(&format!("Strategy: {}", self.strategy_name));
    }

    /// Build StrategyContext from current app state
    pub fn build_strategy_context(&self, now_ns: u64) -> StrategyContext {
        // Convert best_bid/best_ask (prices) to ticks
        let best_bid_tick = (self.best_bid * 10000.0) as i16;
        let best_ask_tick = (self.best_ask * 10000.0) as i16;
        
        // Calculate mid tick and spread
        let (mid_tick, spread_ticks) = if best_ask_tick > best_bid_tick {
            let mid = (best_bid_tick + best_ask_tick) / 2;
            let spread = best_ask_tick - best_bid_tick;
            (Some(mid as _), Some(spread as _))
        } else {
            (None, None)
        };
        
        StrategyContext {
            now_ns,
            asset_id: self.market.condition_id.clone(),
            position: mtrader_risk::Position::new(),
            pnl: mtrader_risk::PnLSnapshot {
                timestamp_ns: now_ns,
                realized_pnl: self.realized_pnl,
                unrealized_pnl: self.unrealized_pnl,
                total_pnl: self.realized_pnl + self.unrealized_pnl,
                total_fees: self.total_fees,
                net_pnl: self.realized_pnl + self.unrealized_pnl - self.total_fees,
                high_water_mark: 0,
                drawdown: 0,
                drawdown_bps: 0,
            },
            best_bid: if self.best_bid > 0.0 { Some(best_bid_tick as _) } else { None },
            best_ask: if self.best_ask > 0.0 { Some(best_ask_tick as _) } else { None },
            best_bid_size: self.active_bids as _,
            best_ask_size: self.active_asks as _,
            mid_tick,
            spread_ticks,
            our_bids: Vec::new(),
            our_asks: Vec::new(),
            market_snapshots: HashMap::new(),
        }
    }

    pub fn log_trade(&mut self, side: &str, size: i64, price: f64) {
        let ts = Local::now().format("%H:%M:%S.%3f").to_string();
        self.activity_log
            .insert(0, format!("[{}] {} {} @ {:.4}", ts, side, size, price));
        self.trades_count += 1;
        self.daily_trades += 1;
        if self.activity_log.len() > 50 {
            self.activity_log.pop();
        }
    }

    pub fn recording_elapsed(&self) -> Option<Duration> {
        self.recording_start
            .map(|s| Instant::now().duration_since(s))
    }

    pub fn spread(&self) -> f64 {
        self.best_ask - self.best_bid
    }

    pub fn drawdown_bps(&self) -> i64 {
        if self.realized_pnl < 0 {
            let balance = self.backtest_balance.max(1.0);
            (-self.realized_pnl as f64 / balance * 10000.0) as i64
        } else {
            0
        }
    }

    pub fn add_custom_market(&mut self, condition_id: &str) {
        let market = Market {
            condition_id: condition_id.to_string(),
            name: format!("Custom: {}", condition_id),
            price: 0.5000,
            volume: 0.0,
        };
        self.markets_list.push(market.clone());
        self.set_status(&format!("Added market: {}", condition_id));
    }

    /// Add a strategy signal to the signal list
    pub fn add_strategy_signal(&mut self, signal: StrategySignal) {
        self.strategy_signals.insert(0, signal);
        if self.strategy_signals.len() > 10 {
            self.strategy_signals.pop();
        }
    }
}

pub struct App {
    state: AppState,
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl App {
    pub fn new() -> Result<Self> {
        enable_raw_mode()?;
        let stdout = io::stdout();
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;
        Ok(Self {
            state: AppState::new(),
            terminal,
        })
    }

    pub fn run(&mut self) -> Result<()> {
        self.terminal.clear()?;
        self.load_replay_files();
        loop {
            self.terminal
                .draw(|f| f.render_widget(&TuiApp { state: &self.state }, f.area()))?;
            if event::poll(Duration::from_millis(50))? {
                if let Event::Key(key) = event::read()? {
                    self.handle_input(key);
                }
            }
            
            // Strategy execution loop - runs when auto-trading is enabled
            self.execute_strategy_cycle();
            
            if self.state.current_menu == MenuItem::Quit {
                break;
            }
        }
        disable_raw_mode()?;
        Ok(())
    }

    /// Execute one cycle of strategy logic
    fn execute_strategy_cycle(&mut self) {
        if self.state.auto_trading != AutoTradingState::Running {
            return;
        }
        
        // Initialize strategy if not already done
        if self.state.active_strategy.is_none() {
            if let Err(e) = self.state.create_strategy() {
                self.state.set_status(&format!("Strategy init failed: {}", e));
                self.state.auto_trading = AutoTradingState::Disabled;
                return;
            }
        }
        
        // Build context first (before any borrow of self.state)
        let now_ns = std::time::UNIX_EPOCH.elapsed().unwrap_or_default().as_nanos() as u64;
        let ctx = self.state.build_strategy_context(now_ns);
        
        // Activate strategy if not already active
        let strategy_name = self.state.strategy_name.clone();
        if let Some(ref mut strategy) = self.state.active_strategy {
            if !strategy.is_active() {
                strategy.activate();
            }
            
            // Get actions from strategy
            let actions = strategy.on_update(&ctx);
            
            // Execute each action
            for action in actions {
                if let Err(e) = self.execute_strategy_action(action, &ctx) {
                    self.state.set_status(&format!("Action error: {}", e));
                }
            }
        }
        // Set status after strategy operations
        if self.state.auto_trading == AutoTradingState::Running {
            self.state.set_status(&format!("Strategy '{}' activated", strategy_name));
        }
    }

    /// Execute a strategy action and update state
    fn execute_strategy_action(&mut self, action: StrategyAction, ctx: &StrategyContext) -> Result<(), String> {
        match action {
            StrategyAction::PlaceOrder { side, kind, reason, .. } => {
                // Create signal for display
                let ts = Local::now().format("%H:%M:%S.%3f").to_string();
                let size = match kind {
                    mtrader_execution::OrderKind::Limit { size_shares, .. } => size_shares as i64,
                    _ => 1_000_000,
                };
                let price = ctx.mid_tick.map(|t| t as f64 / 10000.0);
                
                let signal = StrategySignal {
                    timestamp: ts.clone(),
                    signal_type: format!("{:?}", side),
                    details: format!("{:?} - {:?}", kind, reason),
                    side: Some(format!("{:?}", side)),
                    size: Some(size),
                    price,
                };
                self.state.add_strategy_signal(signal);
                
                // Log the order
                let order_info = format!("ORDER: {:?} {:?} - {:?}", side, kind, reason);
                self.state.activity_log.insert(0, format!("[{}] {}", ts, order_info));
                
                // Simulate fill for paper trading (immediate fill at mid price)
                if let Some(mid_tick) = ctx.mid_tick {
                    // Update position
                    let position_delta = match side {
                        many_lamps_core::Side::Buy => size,
                        many_lamps_core::Side::Sell => -size,
                    };
                    self.state.position += position_delta;
                    
                    // Calculate PnL impact (simplified)
                    let price_f = mid_tick as f64 / 10000.0;
                    let pnl_impact = (position_delta as f64 * price_f * 1_000_000.0) as i64;
                    
                    self.state.set_status(&format!(
                        "Filled: {:?} {} @ {:.4} (Pos: {})",
                        side, size, price_f, self.state.position
                    ));
                    
                    // Simulate fill callback to strategy
                    if let Some(ref mut strategy) = self.state.active_strategy {
                        strategy.on_fill(ctx, side, mid_tick, size as _);
                    }
                    
                    self.state.trades_count += 1;
                    self.state.daily_trades += 1;
                    
                    // Add simulated fee
                    let fee = (size as f64 * price_f * 0.001) as i64; // 10 bps fee estimate
                    self.state.total_fees += fee;
                    
                    // Update realized PnL (simplified)
                    self.state.realized_pnl += pnl_impact - fee;
                }
                Ok(())
            }
            
            StrategyAction::CancelOrder { client_order_id, reason } => {
                let ts = Local::now().format("%H:%M:%S.%3f").to_string();
                let signal = StrategySignal {
                    timestamp: ts.clone(),
                    signal_type: "CANCEL".to_string(),
                    details: format!("{:?} - {}", client_order_id, reason),
                    side: None,
                    size: None,
                    price: None,
                };
                self.state.add_strategy_signal(signal);
                
                self.state.activity_log.insert(0, format!("[{}] CANCEL: {:?} - {}", ts, client_order_id, reason));
                self.state.set_status(&format!("Cancelled order: {:?}", client_order_id));
                Ok(())
            }
            
            StrategyAction::AmendOrder { client_order_id, new_kind, reason } => {
                let ts = Local::now().format("%H:%M:%S.%3f").to_string();
                let signal = StrategySignal {
                    timestamp: ts.clone(),
                    signal_type: "AMEND".to_string(),
                    details: format!("{:?} -> {:?} - {}", client_order_id, new_kind, reason),
                    side: None,
                    size: None,
                    price: None,
                };
                self.state.add_strategy_signal(signal);
                
                self.state.activity_log.insert(0, format!("[{}] AMEND: {:?} -> {:?} - {}", ts, client_order_id, new_kind, reason));
                self.state.set_status(&format!("Amended order: {:?}", client_order_id));
                Ok(())
            }
            
            StrategyAction::NoOp => {
                // No action needed
                Ok(())
            }
        }
    }

    fn load_replay_files(&mut self) {
        let path = &self.state.backtest_dir;
        if let Ok(entries) = std::fs::read_dir(path) {
            self.state.replay_files = entries
                .filter_map(|e| e.ok())
                .filter(|e| {
                    e.path()
                        .extension()
                        .map(|ext| ext == "parquet")
                        .unwrap_or(false)
                })
                .map(|e| e.file_name().to_string_lossy().to_string())
                .collect();
        }
        if self.state.replay_files.is_empty() {
            self.state.replay_files = vec!["No recordings found".to_string()];
        }
    }

    fn handle_input(&mut self, key: event::KeyEvent) {
        match &self.state.input_mode {
            InputMode::Normal => self.handle_normal(key),
            InputMode::Editing(_) => self.handle_edit(key),
            InputMode::Confirming(_) => self.handle_confirm(key),
        }
    }

    fn handle_normal(&mut self, key: event::KeyEvent) {
        match self.state.current_menu {
            MenuItem::MainMenu => self.main_menu(key),
            MenuItem::PaperTrading => self.paper(key),
            MenuItem::Record => self.record(key),
            MenuItem::Backtest => self.backtest(key),
            MenuItem::Replay => self.replay(key),
            MenuItem::MarketSelection => self.market_selection(key),
            MenuItem::MarketBrowser => self.browser(key),
            MenuItem::LiveTrading => self.live(key),
            MenuItem::Strategies => self.strategies(key),
            MenuItem::StrategySettings => self.strategy_settings(key),
            MenuItem::RiskSettings => self.risk_settings(key),
            MenuItem::Help => self.help(key),
            _ => {}
        }
    }

    fn handle_edit(&mut self, key: event::KeyEvent) {
        match key.code {
            KeyCode::Enter => {
                self.apply_edit();
                self.state.input_mode = InputMode::Normal;
                self.state.edit_field = None;
            }
            KeyCode::Esc => {
                self.state.input_mode = InputMode::Normal;
                self.state.text_input.clear();
                self.state.edit_field = None;
                self.state.set_status("Edit cancelled.");
            }
            KeyCode::Backspace => {
                self.state.text_input.pop();
            }
            KeyCode::Char(c) => {
                self.state.text_input.push(c);
            }
            KeyCode::Delete => {
                self.state.text_input.clear();
            }
            _ => {}
        }
    }

    fn apply_edit(&mut self) {
        let input = self.state.text_input.clone();
        if input.is_empty() {
            return;
        }
        match self.state.edit_field.take() {
            Some(EditField::MarketId) => self.state.add_custom_market(&input),
            Some(EditField::BacktestDir) => {
                self.state.backtest_dir = input;
                self.state.set_status("Directory updated.");
                self.load_replay_files();
            }
            Some(EditField::WalletAddress) => {
                self.state.wallet_address = input;
                self.state.set_status("Wallet updated.");
            }
            Some(EditField::StrategyParam(param)) => {
                // Parse and update strategy param
                self.state.set_status(&format!("Updated {}", param));
            }
            None => {}
        }
        self.state.text_input.clear();
    }

    fn handle_confirm(&mut self, key: event::KeyEvent) {
        match key.code {
            KeyCode::Enter | KeyCode::Char('y') => {
                self.state.input_mode = InputMode::Normal;
                self.state.set_status("Confirmed.");
            }
            _ => {
                self.state.input_mode = InputMode::Normal;
                self.state.set_status("Cancelled.");
            }
        }
    }

    fn main_menu(&mut self, key: event::KeyEvent) {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.state.selected_index = self.state.selected_index.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.state.selected_index = self.state.selected_index.saturating_add(1);
                if self.state.selected_index > 10 {
                    self.state.selected_index = 0;
                }
            }
            KeyCode::Enter => {
                self.state.current_menu = match self.state.selected_index {
                    0 => {
                        self.state.set_status("Paper Trading");
                        MenuItem::PaperTrading
                    }
                    1 => {
                        self.state.set_status("Recording");
                        MenuItem::Record
                    }
                    2 => {
                        self.state.set_status("Backtest");
                        MenuItem::Backtest
                    }
                    3 => {
                        self.state.set_status("Replay");
                        MenuItem::Replay
                    }
                    4 => {
                        self.state.set_status("Market Browser");
                        MenuItem::MarketBrowser
                    }
                    5 => {
                        self.state.set_status("Live Trading");
                        MenuItem::LiveTrading
                    }
                    6 => {
                        self.state.set_status("Strategies");
                        MenuItem::Strategies
                    }
                    7 => {
                        self.state.set_status("Strategy Settings");
                        MenuItem::StrategySettings
                    }
                    8 => {
                        self.state.set_status("Risk Settings");
                        MenuItem::RiskSettings
                    }
                    9 => {
                        self.state.set_status("Help");
                        MenuItem::Help
                    }
                    _ => {
                        self.state.set_status("Goodbye!");
                        MenuItem::Quit
                    }
                };
            }
            KeyCode::Esc | KeyCode::Char('q') => self.state.current_menu = MenuItem::Quit,
            _ => {}
        }
    }

    fn paper(&mut self, key: event::KeyEvent) {
        match key.code {
            KeyCode::Char('c') => {
                // Toggle connection - shows persistent state
                self.state.best_bid = self.state.market.price - 0.005;
                self.state.best_ask = self.state.market.price + 0.005;
                self.state.set_status(&format!(
                    "Connected: Bid {:.4} Ask {:.4}",
                    self.state.best_bid, self.state.best_ask
                ));
            }
            KeyCode::Char('r') => {
                self.state.is_recording = !self.state.is_recording;
                if self.state.is_recording {
                    self.state.recording_start = Some(Instant::now());
                    self.state.recording_events = 0;
                    self.state.set_status("Recording started.");
                } else {
                    self.state.set_status("Recording stopped.");
                    self.state.recording_start = None;
                }
            }
            KeyCode::Char('s') => {
                self.state.next_strategy();
            }
            KeyCode::Char('a') => {
                // Auto-trading toggle
                let was_halted = matches!(self.state.auto_trading, AutoTradingState::Halted(_));
                match self.state.auto_trading {
                    AutoTradingState::Disabled => {
                        self.state.auto_trading = AutoTradingState::Running;
                        // Initialize and activate strategy
                        if let Err(e) = self.state.create_strategy() {
                            self.state.set_status(&format!("Strategy init failed: {}", e));
                            self.state.auto_trading = AutoTradingState::Disabled;
                        } else {
                            self.state.set_status("Auto-trading ENABLED. Strategy is now active.");
                        }
                    }
                    AutoTradingState::Running => {
                        self.state.auto_trading = AutoTradingState::Paused;
                        self.state.set_status("Auto-trading paused.");
                        // Deactivate strategy
                        if let Some(ref mut strategy) = self.state.active_strategy {
                            strategy.deactivate();
                        }
                    }
                    AutoTradingState::Paused => {
                        self.state.auto_trading = AutoTradingState::Running;
                        self.state.set_status("Auto-trading resumed.");
                        // Reactivate strategy
                        if let Some(ref mut strategy) = self.state.active_strategy {
                            strategy.activate();
                        }
                    }
                    AutoTradingState::Halted(_) => {
                        self.state.auto_trading = AutoTradingState::Disabled;
                        self.state.circuit_breaker_tripped = false;
                        self.state.active_strategy = None;
                        self.state.set_status("Auto-trading reset. Circuit breaker cleared.");
                    }
                }
                if was_halted {
                    self.state.circuit_breaker_tripped = false;
                }
            }
            KeyCode::Char('e') => {
                self.state.input_mode = InputMode::Editing(EditField::MarketId);
                self.state.text_input = self.state.market.condition_id.clone();
                self.state.set_status("Enter market ID...");
            }
            KeyCode::Char('l') => {
                // Update order book - show persistent state
                self.state.set_status(&format!(
                    "Book: Bid {:.4} Ask {:.4} Spread {:.4}",
                    self.state.best_bid,
                    self.state.best_ask,
                    self.state.spread()
                ));
            }
            KeyCode::Char('p') => {
                // FIXED: Show position and PnL - persistent values
                self.state.set_status(&format!(
                    "Position: {} | Realized PnL: ${:.2} | Unrealized: ${:.2}",
                    self.state.position,
                    self.state.realized_pnl as f64 / 1e6,
                    self.state.unrealized_pnl as f64 / 1e6
                ));
            }
            KeyCode::Char('q') | KeyCode::Esc => {
                self.state.auto_trading = AutoTradingState::Disabled;
                self.state.active_strategy = None;
                self.state.is_recording = false;
                self.state.set_status("Back to menu.");
                self.state.current_menu = MenuItem::MainMenu;
            }
            KeyCode::Char('m') => {
                // Switch to market selection
                self.state.current_menu = MenuItem::MarketSelection;
                self.state.set_status("Select a market...");
            }
            _ => {}
        }
    }

    fn record(&mut self, key: event::KeyEvent) {
        match key.code {
            KeyCode::Enter => {
                self.state.is_recording = !self.state.is_recording;
                if self.state.is_recording {
                    self.state.recording_start = Some(Instant::now());
                    self.state.recording_events = 0;
                    self.state.set_status("Recording started.");
                } else {
                    self.state.set_status("Recording saved.");
                    self.state.recording_start = None;
                }
            }
            KeyCode::Char('e') => {
                self.state.input_mode = InputMode::Editing(EditField::MarketId);
                self.state.text_input = self.state.market.condition_id.clone();
            }
            KeyCode::Char('a') => {
                self.state.input_mode = InputMode::Editing(EditField::MarketId);
                self.state.text_input = String::new();
                self.state.set_status("Enter condition_id to add market...");
            }
            KeyCode::Char('q') | KeyCode::Esc => self.state.current_menu = MenuItem::MainMenu,
            _ => {}
        }
    }

    fn backtest(&mut self, key: event::KeyEvent) {
        match key.code {
            KeyCode::Enter => {
                let path = std::path::Path::new(&self.state.backtest_dir);
                match load_snapshots(path) {
                    Ok(snapshots) if !snapshots.is_empty() => {
                        let config = SimBacktestConfig {
                            starting_balance_micro: (self.state.backtest_balance * 1_000_000.0)
                                as i64,
                            leg_size: self.state.strategy_params.order_size,
                            sum_target: self.state.strategy_params.sum_target,
                            dip_threshold: self.state.strategy_params.dip_threshold,
                            window_minutes: self.state.strategy_params.window_minutes,
                            dip_window_ms: 3_000,
                            fee_rate_bps: 50,
                            spread_bps: self.state.strategy_params.spread_bps as f64,
                            leg2_timeout_seconds: 60,
                        };
                        let report = run_backtest(&snapshots, &config);
                        let end_balance = report.ending_balance_micro as f64 / 1_000_000.0;
                        let roi = report.roi_pct;
                        self.state.backtest_results = Some(format_backtest_report(
                            self.state.backtest_balance,
                            end_balance,
                            roi,
                            report.cycles,
                            report.leg1_triggers,
                            report.leg2_triggers,
                            report.stop_losses,
                            report.round_losses,
                        ));
                        self.state
                            .set_status(&format!("Backtest complete. ROI: {:.2}%", roi));
                    }
                    Ok(_) => {
                        self.state.backtest_results =
                            Some("No snapshots found in directory.".to_string());
                        self.state
                            .set_status("Backtest failed: no snapshots found.");
                    }
                    Err(err) => {
                        self.state.backtest_results = Some(format!("Backtest error: {err}"));
                        self.state.set_status("Backtest failed. Check logs.");
                    }
                }
            }
            }
            KeyCode::Char('2') => {
                self.state.replay_speed = 2.0;
                self.state.set_status("Speed: 2x");
            }
            KeyCode::Char('5') => {
                self.state.replay_speed = 5.0;
                self.state.set_status("Speed: 5x");
            }
            KeyCode::Char('q') | KeyCode::Esc => self.state.current_menu = MenuItem::MainMenu,
            _ => {}
        }
    }

    fn browser(&mut self, key: event::KeyEvent) {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') if self.state.selected_market > 0 => {
                self.state.selected_market -= 1;
            }
            KeyCode::Down | KeyCode::Char('j')
                if self.state.selected_market < self.state.markets_list.len().saturating_sub(1) =>
            {
                self.state.selected_market += 1;
            }
            KeyCode::Enter => {
                if !self.state.markets_list.is_empty() {
                    self.state.market = self.state.markets_list[self.state.selected_market].clone();
                    self.state
                        .set_status(&format!("Selected: {}", self.state.market.name));
                }
            }
            KeyCode::Char('t') => {
                // Trade this market - go to paper trading
                if !self.state.markets_list.is_empty() {
                    self.state.market = self.state.markets_list[self.state.selected_market].clone();
                    self.state
                        .set_status(&format!("Trading: {}", self.state.market.name));
                    self.state.current_menu = MenuItem::PaperTrading;
                }
            }
            KeyCode::Char('q') | KeyCode::Esc => self.state.current_menu = MenuItem::MainMenu,
            _ => {}
        }
    }

    fn market_selection(&mut self, key: event::KeyEvent) {
        let ms = &mut self.state.market_selection;
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                ms.select_prev();
            }
            KeyCode::Down | KeyCode::Char('j') => {
                ms.select_next();
            }
            KeyCode::PageUp => {
                ms.prev_page();
            }
            KeyCode::PageDown => {
                ms.next_page();
            }
            KeyCode::Enter => {
                if let Some(market) = ms.selected_market() {
                    // Convert PolymarketMarket to Market and set as current
                    let price = market.yes_price.unwrap_or(0.5);
                    self.state.market = Market {
                        condition_id: market.condition_id.clone(),
                        name: market.question.clone(),
                        price,
                        volume: market.volume,
                    };
                    self.state.set_status(&format!("Selected: {}", market.question));
                    self.state.current_menu = MenuItem::PaperTrading;
                }
            }
            KeyCode::Char('/') => {
                ms.is_searching = true;
                self.state.set_status("Search markets...");
            }
            KeyCode::Char('r') => {
                // Refresh markets - in real implementation would call API
                self.state.set_status("Refreshing markets...");
                // Would fetch from Polymarket API here
            }
            KeyCode::Char('v') => {
                ms.sort_by = MarketSortBy::Volume;
                ms.sort_desc = true;
                ms.filter_and_sort();
            }
            KeyCode::Char('n') => {
                ms.sort_by = MarketSortBy::Name;
                ms.filter_and_sort();
            }
            KeyCode::Char('p') => {
                ms.sort_by = MarketSortBy::Price;
                ms.filter_and_sort();
            }
            KeyCode::Esc | KeyCode::Char('q') => {
                ms.is_searching = false;
                ms.search_query.clear();
                ms.filter_and_sort();
                self.state.current_menu = MenuItem::MainMenu;
            }
            _ => {}
        }
    }

    fn live(&mut self, key: event::KeyEvent) {
        match key.code {
            KeyCode::Enter => {
                self.state.input_mode = InputMode::Confirming(ConfirmAction::EnableLiveTrading);
                self.state
                    .set_status("Press Enter to confirm live trading...");
            }
            KeyCode::Char('e') => {
                self.state.input_mode = InputMode::Editing(EditField::WalletAddress);
                self.state.text_input = self.state.wallet_address.clone();
                self.state.set_status("Enter wallet address...");
            }
            KeyCode::Char('q') | KeyCode::Esc => self.state.current_menu = MenuItem::MainMenu,
            _ => {}
        }
    }

    fn strategies(&mut self, key: event::KeyEvent) {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') if self.state.selected_strategy_index > 0 => {
                self.state.selected_strategy_index -= 1;
            }
            KeyCode::Down | KeyCode::Char('j')
                if self.state.selected_strategy_index < AVAILABLE_STRATEGIES.len() - 1 =>
            {
                self.state.selected_strategy_index += 1;
            }
            KeyCode::Enter => {
                self.state.strategy_name =
                    AVAILABLE_STRATEGIES[self.state.selected_strategy_index].to_string();
                self.state.set_status(&format!(
                    "Selected strategy: {}",
                    self.state.strategy_name
                ));
            }
            KeyCode::Char('s') | KeyCode::Right => {
                self.state.selected_strategy_index = (self.state.selected_strategy_index + 1)
                    .min(AVAILABLE_STRATEGIES.len() - 1);
            }
            KeyCode::Char('w') | KeyCode::Left => {
                self.state.selected_strategy_index =
                    self.state.selected_strategy_index.saturating_sub(1);
            }
            KeyCode::Char('q') | KeyCode::Esc => self.state.current_menu = MenuItem::MainMenu,
            _ => {}
        }
    }

    fn strategy_settings(&mut self, key: event::KeyEvent) {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.state.selected_index = self.state.selected_index.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.state.selected_index = self.state.selected_index.saturating_add(1);
                if self.state.selected_index > 7 {
                    self.state.selected_index = 0;
                }
            }
            KeyCode::Left | KeyCode::Char('-') => {
                self.adjust_strategy_param(-1);
            }
            KeyCode::Right | KeyCode::Char('+') => {
                self.adjust_strategy_param(1);
            }
            KeyCode::Char('s') => {
                let strats = [
                    "maker_mm",
                    "bundle_maker",
                    "unaffected_arb",
                    "rebalancing_arb",
                    "ml",
                ];
                let idx = strats
                    .iter()
                    .position(|&s| s == self.state.strategy_name)
                    .unwrap_or(0);
                self.state.strategy_name = strats[(idx + 1) % strats.len()].to_string();
                self.state
                    .set_status(&format!("Strategy: {}", self.state.strategy_name));
            }
            KeyCode::Char('q') | KeyCode::Esc => self.state.current_menu = MenuItem::MainMenu,
            _ => {}
        }
    }

    fn adjust_strategy_param(&mut self, delta: i32) {
        match self.state.selected_index {
            0 => {
                let new_val =
                    (self.state.strategy_params.spread_bps as i32 + delta * 5).clamp(10, 200);
                self.state.strategy_params.spread_bps = new_val as u16;
            }
            1 => {
                let new_val =
                    (self.state.strategy_params.order_size as i64 + delta as i64).clamp(1, 100);
                self.state.strategy_params.order_size = new_val as u64;
            }
            2 => {
                let new_val = (self.state.strategy_params.num_levels as i32 + delta).clamp(1, 5);
                self.state.strategy_params.num_levels = new_val as u8;
            }
            3 => {
                if delta != 0 {
                    self.state.strategy_params.inventory_skew =
                        !self.state.strategy_params.inventory_skew;
                }
            }
            4 => {
                let new_val =
                    (self.state.strategy_params.sum_target + delta as f64 * 0.01).clamp(0.50, 1.50);
                self.state.strategy_params.sum_target = new_val;
            }
            5 => {
                let new_val = (self.state.strategy_params.edge_threshold_bps as i32 + delta * 5)
                    .clamp(5, 200);
                self.state.strategy_params.edge_threshold_bps = new_val as u16;
            }
            6 => {
                let new_val = (self.state.strategy_params.dip_threshold + delta as f64 * 0.005)
                    .clamp(0.0, 0.2);
                self.state.strategy_params.dip_threshold = new_val;
            }
            7 => {
                let new_val = (self.state.strategy_params.window_minutes as i64 + delta as i64 * 5)
                    .clamp(5, 240);
                self.state.strategy_params.window_minutes = new_val as u64;
            }
            _ => {}
        }
        self.state.set_status("Parameter updated.");
    }

    fn risk_settings(&mut self, key: event::KeyEvent) {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.state.selected_index = self.state.selected_index.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.state.selected_index = self.state.selected_index.saturating_add(1);
                if self.state.selected_index > 7 {
                    self.state.selected_index = 0;
                }
            }
            KeyCode::Left | KeyCode::Char('-') => {
                self.adjust_risk_param(-1);
            }
            KeyCode::Right | KeyCode::Char('+') => {
                self.adjust_risk_param(1);
            }
            KeyCode::Char('r') => {
                // Reset to defaults
                self.state.risk_config = RiskConfig::default();
                self.state.set_status("Risk settings reset to defaults.");
            }
            KeyCode::Char('q') | KeyCode::Esc => self.state.current_menu = MenuItem::MainMenu,
            _ => {}
        }
    }

    fn adjust_risk_param(&mut self, delta: i32) {
        match self.state.selected_index {
            0 => {
                let new_val = (self.state.risk_config.max_position as i64 + delta as i64 * 10)
                    .clamp(10, 1000);
                self.state.risk_config.max_position = new_val;
            }
            1 => {
                let new_val =
                    (self.state.risk_config.max_order_size as i64 + delta as i64).clamp(1, 100);
                self.state.risk_config.max_order_size = new_val as u64;
            }
            2 => {
                let new_val = (self.state.risk_config.max_bet_percentage + delta as f64 * 0.5)
                    .clamp(0.5, 10.0);
                self.state.risk_config.max_bet_percentage = new_val;
            }
            3 => {
                let new_val = (self.state.risk_config.max_portfolio_percentage
                    + delta as f64 * 1.0)
                    .clamp(5.0, 50.0);
                self.state.risk_config.max_portfolio_percentage = new_val;
            }
            4 => {
                let new_val = (self.state.risk_config.max_drawdown_bps as i64 + delta as i64 * 50)
                    .clamp(100, 2000);
                self.state.risk_config.max_drawdown_bps = new_val;
            }
            5 => {
                let new_val = (self.state.risk_config.max_loss_per_hour as i64
                    + delta as i64 * 10_000_000)
                    .clamp(10_000_000, 1_000_000_000);
                self.state.risk_config.max_loss_per_hour = new_val;
            }
            6 => {
                let new_val =
                    (self.state.risk_config.stop_consecutive_losses as i32 + delta).clamp(3, 20);
                self.state.risk_config.stop_consecutive_losses = new_val as u32;
            }
            7 => {
                let new_val = (self.state.risk_config.max_daily_trades as i64 + delta as i64 * 10)
                    .clamp(0, 1000);
                self.state.risk_config.max_daily_trades = new_val as u64;
            }
            _ => {}
        }
        self.state.set_status("Risk parameter updated.");
    }

    fn help(&mut self, key: event::KeyEvent) {
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc | KeyCode::Enter => {
                self.state.current_menu = MenuItem::MainMenu;
                self.state.set_status("Back to menu.");
            }
            KeyCode::Up | KeyCode::Char('k') => {}
            KeyCode::Down | KeyCode::Char('j') => {}
            _ => {}
        }
    }
}

/// TUI App Widget
pub struct TuiApp<'a> {
    state: &'a AppState,
}

impl<'a> TuiApp<'a> {
    pub fn new(state: &'a AppState) -> Self {
        Self { state }
    }
}

impl<'a> Widget for &TuiApp<'a> {
    fn render(self, area: Rect, buf: &mut ratatui::buffer::Buffer) {
        match self.state.current_menu {
            MenuItem::MainMenu => render_main_menu(self.state, area, buf),
            MenuItem::PaperTrading => render_paper_trading(self.state, area, buf),
            MenuItem::Record => render_record(self.state, area, buf),
            MenuItem::Backtest => render_backtest(self.state, area, buf),
            MenuItem::Replay => render_replay(self.state, area, buf),
            MenuItem::MarketSelection => render_market_selection(self.state, area, buf),
            MenuItem::MarketBrowser => render_market_browser(self.state, area, buf),
            MenuItem::LiveTrading => render_live_trading(self.state, area, buf),
            MenuItem::Strategies => render_strategies(self.state, area, buf),
            MenuItem::StrategySettings => render_strategy_settings(self.state, area, buf),
            MenuItem::RiskSettings => render_risk_settings(self.state, area, buf),
            MenuItem::Help => render_help(self.state, area, buf),
            MenuItem::Quit => render_main_menu(self.state, area, buf),
        }
    }
}

fn render_main_menu(state: &AppState, area: Rect, buf: &mut ratatui::buffer::Buffer) {
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(10),
        Constraint::Length(3),
    ])
    .split(area);

    let title = Paragraph::new(format!(" MTrader v{} ", VERSION))
        .style(Style::default().bg(Color::Blue).fg(Color::White))
        .alignment(Alignment::Center);
    title.render(chunks[0], buf);

    let items = [
        "Paper Trading",
        "Recording",
        "Backtest",
        "Replay",
        "Market Browser",
        "Live Trading",
        "Strategies",
        "Strategy Settings",
        "Risk Settings",
        "Help",
    ];
    let mut lines = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let prefix = if i == state.selected_index {
            "▶ "
        } else {
            "  "
        };
        let style = if i == state.selected_index {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        lines.push(ListItem::new(Line::from(vec![
            Span::raw(prefix.to_string()),
            Span::styled(item.to_string(), style),
        ])));
    }
    let menu = List::new(lines).block(Block::default().title(" Main Menu ").borders(Borders::ALL));
    menu.render(chunks[1], buf);

    let footer = Paragraph::new(" [↑/↓] Navigate  [Enter] Select  [Esc] Quit ")
        .style(Style::default().bg(Color::DarkGray).fg(Color::White))
        .alignment(Alignment::Center);
    footer.render(chunks[2], buf);
}

fn render_paper_trading(state: &AppState, area: Rect, buf: &mut ratatui::buffer::Buffer) {
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(10),
        Constraint::Length(6),
    ])
    .split(area);

    // Header with auto-trading status
    let auto_status = match &state.auto_trading {
        AutoTradingState::Disabled => "DISABLED".to_string(),
        AutoTradingState::Running => "🤖 RUNNING".to_string(),
        AutoTradingState::Paused => "⏸ PAUSED".to_string(),
        AutoTradingState::Halted(ref r) => format!("HALTED: {}", r),
    };
    let header = format!(
        " Paper Trading | {} | {} | [q] Menu ",
        state.market.name, auto_status
    );
    Paragraph::new(header)
        .style(Style::default().bg(Color::Blue).fg(Color::White))
        .alignment(Alignment::Center)
        .render(chunks[0], buf);

    // Three column layout for signals
    let columns = Layout::horizontal([
        Constraint::Percentage(30),
        Constraint::Percentage(35),
        Constraint::Percentage(35),
    ])
    .split(chunks[1]);

    // Strategy Signals panel
    let signals_content = if state.strategy_signals.is_empty() {
        "Strategy Signals\n\nNo signals yet.\n\nPress [a] to start\nauto-trading.".to_string()
    } else {
        let mut content = String::from("Strategy Signals\n\n");
        for signal in state.strategy_signals.iter().take(5) {
            let side_mark = signal.side.as_ref()
                .map(|s| if s.contains("Buy") { "🟢" } else if s.contains("Sell") { "🔴" } else { "⚪" })
                .unwrap_or("⚪");
            content.push_str(&format!(
                "{} [{}] {}\n  {}\n\n",
                side_mark,
                signal.timestamp,
                signal.signal_type,
                signal.details
            ));
        }
        content
    };
    Paragraph::new(signals_content)
        .block(Block::default().title(" Signals ").borders(Borders::ALL))
        .render(columns[0], buf);

    // Market data panel
    let market_content = format!(
        "Order Book\n\nBid: {:.4}\nAsk: {:.4}\nSpread: {:.4}\n\nStrategy: {}\nSpread: {}bps\nSize: {}",
        state.best_bid, state.best_ask, state.spread(), state.strategy_name, state.strategy_params.spread_bps, state.strategy_params.order_size
    );
    Paragraph::new(market_content)
        .block(Block::default().title(" Market ").borders(Borders::ALL))
        .render(columns[1], buf);

    // Account panel
    let drawdown = state.drawdown_bps();
    let _drawdown_color = if drawdown > state.risk_config.max_drawdown_bps as i64 {
        Color::Red
    } else {
        Color::Yellow
    };
    let account_content = format!(
        "Position & PnL\n\nPosition: {}\nRealized PnL: ${:.2}\nUnrealized: ${:.2}\nFees: ${:.2}\nTrades: {}\nDrawdown: {}bps ({:.2}%)\n\n[c] Connect [a] Auto\n[l] Book [p] Position\n[s] Strategy [m] Market\n[r] Record",
        state.position,
        state.realized_pnl as f64 / 1e6,
        state.unrealized_pnl as f64 / 1e6,
        state.total_fees as f64 / 1e6,
        state.trades_count,
        drawdown,
        drawdown as f64 / 100.0
    );
    Paragraph::new(account_content)
        .block(Block::default().title(" Account ").borders(Borders::ALL))
        .render(columns[2], buf);

    // Activity log
    let log: Vec<ListItem> = state
        .activity_log
        .iter()
        .take(5)
        .map(|a| ListItem::new(a.clone()))
        .collect();
    List::new(log)
        .block(Block::default().title(" Activity ").borders(Borders::ALL))
        .render(chunks[2], buf);
}

fn render_record(state: &AppState, area: Rect, buf: &mut ratatui::buffer::Buffer) {
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(8),
        Constraint::Length(3),
    ])
    .split(area);

    let rec = if state.is_recording {
        " 🔴 RECORDING"
    } else {
        ""
    };
    Paragraph::new(format!(" Recording{} ", rec))
        .style(Style::default().bg(Color::DarkGray).fg(Color::White))
        .alignment(Alignment::Center)
        .render(chunks[0], buf);

    let elapsed = state
        .recording_elapsed()
        .map(|d| format!("{:.1}s", d.as_secs_f64()))
        .unwrap_or_default();
    let content = if state.is_recording {
        format!(
            "Market: {}\n\nEvents: {}\nDuration: {}\n\n[Enter] Stop\n[a] Add Market\n[q] Back",
            state.market.condition_id, state.recording_events, elapsed
        )
    } else {
        format!(
            "Market: {}\n\n[Enter] Start Recording\n[a] Add Market\n[q] Back",
            state.market.condition_id
        )
    };
    Paragraph::new(content)
        .block(Block::default().title(" Recording ").borders(Borders::ALL))
        .alignment(Alignment::Center)
        .render(chunks[1], buf);
    Paragraph::new(" [Enter] Start/Stop  [a] Add Market  [q] Back ")
        .style(Style::default().bg(Color::DarkGray))
        .render(chunks[2], buf);
}

fn render_backtest(state: &AppState, area: Rect, buf: &mut ratatui::buffer::Buffer) {
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(8),
        Constraint::Length(3),
    ])
    .split(area);

    Paragraph::new(" Backtest ")
        .style(Style::default().bg(Color::DarkGray).fg(Color::White))
        .alignment(Alignment::Center)
        .render(chunks[0], buf);

    let content = match &state.backtest_results {
        Some(results) => results.clone(),
        None => format!(
            "Strategy: {}\nSpread: {}bps | Size: {}\n\nDirectory: {}\nStarting Balance: ${:.0}\n\n[Enter] Run Backtest\n[e] Edit Directory\n[s] Change Balance\n[q] Back",
            state.strategy_name,
            state.strategy_params.spread_bps,
            state.strategy_params.order_size,
            state.backtest_dir,
            state.backtest_balance
        ),
    };
    Paragraph::new(content)
        .block(Block::default().title(" Results ").borders(Borders::ALL))
        .render(chunks[1], buf);
    Paragraph::new(" [Enter] Run  [e] Directory  [s] Balance  [q] Back ")
        .style(Style::default().bg(Color::DarkGray))
        .render(chunks[2], buf);
}

fn render_replay(state: &AppState, area: Rect, buf: &mut ratatui::buffer::Buffer) {
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(8),
        Constraint::Length(3),
    ])
    .split(area);

    let status = if state.replay_paused {
        "⏸ PAUSED"
    } else {
        "▶ PLAYING"
    };
    Paragraph::new(format!(" Replay | {} | {}x ", status, state.replay_speed))
        .style(Style::default().bg(Color::DarkGray).fg(Color::White))
        .alignment(Alignment::Center)
        .render(chunks[0], buf);

    let items: Vec<ListItem> = state
        .replay_files
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let marker = if i == state.selected_replay {
                "▶"
            } else {
                " "
            };
            ListItem::new(format!("{} {}", marker, f))
        })
        .collect();
    List::new(items)
        .block(Block::default().title(" Recordings ").borders(Borders::ALL))
        .render(chunks[1], buf);
    Paragraph::new(" [↑/↓] Select  [Enter] Play/Pause  [1/2/5] Speed  [q] Back ")
        .style(Style::default().bg(Color::DarkGray))
        .render(chunks[2], buf);
}

fn render_market_selection(state: &AppState, area: Rect, buf: &mut ratatui::buffer::Buffer) {
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(1),
        Constraint::Min(10),
        Constraint::Length(1),
        Constraint::Length(3),
    ])
    .split(area);

    let ms = &state.market_selection;
    let loading = if ms.is_loading {
        " [Loading...]"
    } else {
        ""
    };
    Paragraph::new(format!(" Market Selection{} ", loading))
        .style(Style::default().bg(Color::DarkGray).fg(Color::White))
        .alignment(Alignment::Center)
        .render(chunks[0], buf);

    // Search bar
    let search_prefix = if ms.is_searching { "> " } else { "  " };
    let search_text = if ms.search_query.is_empty() {
        format!("{}[type to search...]", search_prefix)
    } else {
        format!("{}{}", search_prefix, ms.search_query)
    };
    Paragraph::new(search_text)
        .style(if ms.is_searching {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::DarkGray)
        })
        .render(chunks[1], buf);

    // Market list
    let page_markets = ms.current_page_markets();
    let items: Vec<ListItem> = page_markets
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let marker = if i == ms.selected_index { "▶" } else { " " };
            let price_str = match (m.yes_price, m.no_price) {
                (Some(yes), Some(no)) => format!("Yes: {:.2}% | No: {:.2}%", yes * 100.0, no * 100.0),
                (Some(yes), None) => format!("Yes: {:.2}%", yes * 100.0),
                _ => "N/A".to_string(),
            };
            let active = if m.active { "✓" } else { "" };
            ListItem::new(format!(
                "{} {} | {} | Vol: {:.0} | {}",
                marker,
                active,
                m.question.chars().take(40).collect::<String>(),
                m.volume,
                price_str
            ))
        })
        .collect();
    List::new(items)
        .block(Block::default().title(" Markets ").borders(Borders::ALL))
        .render(chunks[2], buf);

    // Pagination info
    let total_pages = ms.total_pages();
    let page_info = format!("Page {}/{}", ms.page + 1, total_pages.max(1));
    Paragraph::new(page_info)
        .style(Style::default().fg(Color::DarkGray))
        .alignment(Alignment::Center)
        .render(chunks[3], buf);

    // Footer with shortcuts
    let sort_by = match ms.sort_by {
        MarketSortBy::Volume => "Volume",
        MarketSortBy::Name => "Name",
        MarketSortBy::Price => "Price",
        MarketSortBy::Activity => "Activity",
    };
    let footer = format!(
        " [↑/↓] Select  [Enter] Trade  [/] Search  [r] Refresh  [v/n/p] Sort: {}  [PgUp/Dn] Page  [q] Back ",
        sort_by
    );
    Paragraph::new(footer)
        .style(Style::default().bg(Color::DarkGray))
        .render(chunks[4], buf);
}

fn render_market_browser(state: &AppState, area: Rect, buf: &mut ratatui::buffer::Buffer) {
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(10),
        Constraint::Length(3),
    ])
    .split(area);

    Paragraph::new(" Market Browser ")
        .style(Style::default().bg(Color::DarkGray).fg(Color::White))
        .alignment(Alignment::Center)
        .render(chunks[0], buf);

    let items: Vec<ListItem> = state
        .markets_list
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let marker = if i == state.selected_market {
                "▶"
            } else {
                " "
            };
            ListItem::new(format!(
                "{} {} | {:.2}% | Vol: {:.0}",
                marker,
                m.name,
                m.price * 100.0,
                m.volume
            ))
        })
        .collect();
    List::new(items)
        .block(Block::default().title(" Markets ").borders(Borders::ALL))
        .render(chunks[1], buf);
    Paragraph::new(" [↑/↓] Select  [Enter] View  [t] Trade  [q] Back ")
        .style(Style::default().bg(Color::DarkGray))
        .render(chunks[2], buf);
}

fn render_live_trading(state: &AppState, area: Rect, buf: &mut ratatui::buffer::Buffer) {
    let chunks = Layout::vertical([
        Constraint::Length(5),
        Constraint::Min(8),
        Constraint::Length(3),
    ])
    .split(area);

    Paragraph::new(" ⚠️ REAL MONEY TRADING ⚠️ ")
        .style(Style::default().bg(Color::Red).fg(Color::White))
        .alignment(Alignment::Center)
        .render(chunks[0], buf);

    let wallet_preview = state.wallet_address.chars().take(8).collect::<String>();
    let content = format!(
        "Wallet: {}...\nRisk Limit: ${:.0}\nStrategy: {}\n\nRisk Settings:\n  Max Position: {}\n  Max Drawdown: {}bps\n  Stop Losses: {}\n\n[Enter] Enable Live Trading\n[e] Set Wallet Address\n[q] Back",
        wallet_preview,
        state.risk_limit,
        state.strategy_name,
        state.risk_config.max_position,
        state.risk_config.max_drawdown_bps,
        state.risk_config.stop_consecutive_losses
    );
    Paragraph::new(content)
        .block(
            Block::default()
                .title(" Live Trading ")
                .borders(Borders::ALL),
        )
        .render(chunks[1], buf);
    Paragraph::new(" [Enter] Enable  [e] Wallet  [q] Back ")
        .style(Style::default().bg(Color::DarkGray))
        .render(chunks[2], buf);
}

fn render_strategy_settings(state: &AppState, area: Rect, buf: &mut ratatui::buffer::Buffer) {
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(10),
        Constraint::Length(3),
    ])
    .split(area);

    Paragraph::new(format!(" Strategy Settings | {} ", state.strategy_name))
        .style(Style::default().bg(Color::DarkGray).fg(Color::White))
        .alignment(Alignment::Center)
        .render(chunks[0], buf);

    let items = [
        format!("Spread (bps): {}", state.strategy_params.spread_bps),
        format!("Order Size: {}", state.strategy_params.order_size),
        format!("Levels: {}", state.strategy_params.num_levels),
        format!(
            "Inventory Skew: {}",
            if state.strategy_params.inventory_skew {
                "ON"
            } else {
                "OFF"
            }
        ),
        format!("Sum Target: {}", state.strategy_params.sum_target),
        format!(
            "Edge Threshold: {}bps",
            state.strategy_params.edge_threshold_bps
        ),
        format!("Dip Threshold: {}", state.strategy_params.dip_threshold),
        format!("Window (min): {}", state.strategy_params.window_minutes),
    ];
    let mut lines = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let marker = if i == state.selected_index {
            "▶"
        } else {
            " "
        };
        let style = if i == state.selected_index {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        lines.push(ListItem::new(Line::from(vec![
            Span::raw(marker),
            Span::styled(format!(" {}", item), style),
        ])));
    }
    List::new(lines)
        .block(Block::default().title(" Parameters ").borders(Borders::ALL))
        .render(chunks[1], buf);
    Paragraph::new(" [↑/↓] Select  [←/→] Adjust  [s] Switch Strategy  [q] Back ")
        .style(Style::default().bg(Color::DarkGray))
        .render(chunks[2], buf);
}

fn render_risk_settings(state: &AppState, area: Rect, buf: &mut ratatui::buffer::Buffer) {
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(10),
        Constraint::Length(3),
    ])
    .split(area);

    Paragraph::new(" Risk Settings ")
        .style(Style::default().bg(Color::DarkGray).fg(Color::White))
        .alignment(Alignment::Center)
        .render(chunks[0], buf);

    let daily_limit = if state.risk_config.max_daily_trades == 0 {
        "Unlimited".to_string()
    } else {
        state.risk_config.max_daily_trades.to_string()
    };
    let items = [
        format!("Max Position: {}", state.risk_config.max_position),
        format!("Max Order Size: {}", state.risk_config.max_order_size),
        format!("Max Bet %: {:.1}", state.risk_config.max_bet_percentage),
        format!(
            "Max Portfolio %: {:.1}",
            state.risk_config.max_portfolio_percentage
        ),
        format!(
            "Max Drawdown: {}bps ({:.1}%)",
            state.risk_config.max_drawdown_bps,
            state.risk_config.max_drawdown_bps as f64 / 100.0
        ),
        format!(
            "Max Loss/Hour: ${:.0}",
            state.risk_config.max_loss_per_hour as f64 / 1e6
        ),
        format!(
            "Stop After Losses: {}",
            state.risk_config.stop_consecutive_losses
        ),
        format!("Daily Trade Limit: {}", daily_limit),
    ];
    let mut lines = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let marker = if i == state.selected_index {
            "▶"
        } else {
            " "
        };
        let style = if i == state.selected_index {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        lines.push(ListItem::new(Line::from(vec![
            Span::raw(marker),
            Span::styled(format!(" {}", item), style),
        ])));
    }
    List::new(lines)
        .block(Block::default().title(" Limits ").borders(Borders::ALL))
        .render(chunks[1], buf);
    Paragraph::new(" [↑/↓] Select  [←/→] Adjust  [r] Reset  [q] Back ")
        .style(Style::default().bg(Color::DarkGray))
        .render(chunks[2], buf);
}

fn render_strategies(state: &AppState, area: Rect, buf: &mut ratatui::buffer::Buffer) {
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(10),
        Constraint::Length(3),
    ])
    .split(area);

    let strategy_descriptions: HashMap<&str, &str> = HashMap::from([
        ("maker_mm", "Market making with spread capture"),
        ("bundle_maker", "Bundle arbitrage for correlated markets"),
        ("unaffected_arb", "Arbitrage on unaffected assets"),
        ("rebalancing_arb", "NO/YES price rebalancing arbitrage"),
        ("auto_hedge", "Automatic hedging for positions"),
        ("combinatorial_arb", "Multi-market combinatorial arbitrage"),
        ("ml", "Machine learning based trading"),
    ]);

    Paragraph::new(" Select Strategy ")
        .style(Style::default().bg(Color::DarkGray).fg(Color::White))
        .alignment(Alignment::Center)
        .render(chunks[0], buf);

    let mut lines = Vec::new();
    for (i, &strategy_id) in AVAILABLE_STRATEGIES.iter().enumerate() {
        let marker = if i == state.selected_strategy_index {
            "▶"
        } else {
            " "
        };
        let style = if i == state.selected_strategy_index {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        let desc = strategy_descriptions.get(strategy_id).unwrap_or(&"");
        let active = if strategy_id == state.strategy_name { " [ACTIVE]" } else { "" };
        lines.push(ListItem::new(Line::from(vec![
            Span::raw(marker),
            Span::styled(format!(" {} {}{}", strategy_id, desc, active), style),
        ])));
    }
    List::new(lines)
        .block(Block::default().title(" Available Strategies ").borders(Borders::ALL))
        .render(chunks[1], buf);

    let current_desc = strategy_descriptions
        .get(state.strategy_name.as_str())
        .unwrap_or(&"");
    let footer = format!(
        " [↑/↓/j/k] Select  [Enter] Confirm  [q] Back  | Selected: {}",
        state.strategy_name
    );
    Paragraph::new(footer)
        .style(Style::default().bg(Color::DarkGray))
        .render(chunks[2], buf);
}

fn render_help(_state: &AppState, area: Rect, buf: &mut ratatui::buffer::Buffer) {
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(10),
        Constraint::Length(3),
    ])
    .split(area);

    Paragraph::new(" Keyboard Shortcuts ")
        .style(Style::default().bg(Color::DarkGray).fg(Color::White))
        .alignment(Alignment::Center)
        .render(chunks[0], buf);

    let help = "GENERAL:\n  ↑/↓ or j/k  Navigate menus\n  Enter        Select / Confirm\n  Esc or q    Back / Cancel\n\nPAPER TRADING:\n  c            Connect to market\n  a            Toggle auto-trading\n  r            Toggle recording\n  s            Switch strategy\n  l            Update order book\n  p            Show position/PnL\n  e            Edit market ID\n\nSTRATEGY SETTINGS:\n  ↑/↓          Select parameter\n  ←/→         Adjust value\n  s            Switch strategy\n\nRISK SETTINGS:\n  ↑/↓          Select limit\n  ←/→         Adjust value\n  r            Reset to defaults\n\nRECORDING:\n  Enter        Start/Stop\n  a            Add custom market\n\nREPLAY:\n  1/2/5        Speed controls\n  Enter        Play/Pause\n\nLIVE TRADING:\n  e            Set wallet address\n  Enter        Enable live mode";
    Paragraph::new(help)
        .block(Block::default().title(" Help ").borders(Borders::ALL))
        .render(chunks[1], buf);
    Paragraph::new(" [q] Back ")
        .style(Style::default().bg(Color::DarkGray))
        .render(chunks[2], buf);
}

pub fn run_tui() -> Result<()> {
    let mut app = App::new()?;
    app.run()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_drawdown_bps_no_loss() {
        let mut state = AppState::new();
        state.realized_pnl = 100_000_000; // +$100
        assert_eq!(state.drawdown_bps(), 0);
    }

    #[test]
    fn test_spread_calculation() {
        let mut state = AppState::new();
        state.best_bid = 0.5200;
        state.best_ask = 0.5300;
        // Use approximate comparison for floating point
        assert!((state.spread() - 0.01).abs() < 1e-10);
    }

    #[test]
    fn test_activity_log_max_50() {
        let mut state = AppState::new();
        for i in 0..60 {
            state.set_status(&format!("Action {}", i));
        }
        assert_eq!(state.activity_log.len(), 50);
        // Most recent should be the last added (59)
        assert!(state.activity_log[0].contains("Action 59"));
    }

    #[test]
    fn test_set_status() {
        let mut state = AppState::new();
        state.set_status("Test message");
        assert_eq!(state.status_message, "Test message");
        assert_eq!(state.activity_log.len(), 1);
    }

    #[test]
    fn test_auto_trading_state_variants() {
        let disabled = AutoTradingState::Disabled;
        let running = AutoTradingState::Running;
        let paused = AutoTradingState::Paused;
        let halted = AutoTradingState::Halted("Max drawdown".to_string());

        assert_ne!(disabled, running);
        assert_ne!(running, paused);
        assert_ne!(paused, halted);
    }

    #[test]
    fn test_edit_field_equality() {
        assert_eq!(EditField::MarketId, EditField::MarketId);
        assert_eq!(EditField::BacktestDir, EditField::BacktestDir);
        assert_eq!(EditField::WalletAddress, EditField::WalletAddress);
        assert_eq!(
            EditField::StrategyParam("spread".to_string()),
            EditField::StrategyParam("spread".to_string())
        );
        assert_ne!(EditField::MarketId, EditField::BacktestDir);
    }

    #[test]
    fn test_confirm_action_equality() {
        assert_eq!(
            ConfirmAction::EnableLiveTrading,
            ConfirmAction::EnableLiveTrading
        );
        assert_ne!(
            ConfirmAction::EnableLiveTrading,
            ConfirmAction::ClearPosition
        );
    }

    #[test]
    fn test_market_default() {
        let market = Market::default();
        assert_eq!(market.condition_id, "btc-updown-15m");
        assert_eq!(market.name, "BTC Up/Down 15min");
        assert_eq!(market.price, 0.5200);
    }

    #[test]
    fn test_risk_config_default() {
        let config = RiskConfig::default();
        assert_eq!(config.max_position, 200);
        assert_eq!(config.max_order_size, 20);
        assert_eq!(config.max_bet_percentage, 5.0);
        assert_eq!(config.max_portfolio_percentage, 20.0);
        assert_eq!(config.max_drawdown_bps, 500);
        assert_eq!(config.stop_consecutive_losses, 10);
    }

    #[test]
    fn test_strategy_params_default() {
        let params = StrategyParams::default();
        assert_eq!(params.spread_bps, 50);
        assert_eq!(params.order_size, 10);
        assert_eq!(params.inventory_skew, true);
    }

    #[test]
    fn test_default_markets_count() {
        assert_eq!(DEFAULT_MARKETS.len(), 12);
    }

    #[test]
    fn test_trading_mode_variants() {
        assert_eq!(TradingMode::Paper, TradingMode::Paper);
        assert_eq!(TradingMode::Live, TradingMode::Live);
        assert_ne!(TradingMode::Paper, TradingMode::Live);
    }

    #[test]
    fn test_menu_item_variants() {
        assert_eq!(MenuItem::MainMenu, MenuItem::MainMenu);
        assert_eq!(MenuItem::PaperTrading, MenuItem::PaperTrading);
        assert_ne!(MenuItem::MainMenu, MenuItem::PaperTrading);
    }
}
