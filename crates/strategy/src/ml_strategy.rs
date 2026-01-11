//! ML-based trading strategy using T-KAN predictions.
//!
//! This strategy uses ML signals from `mtrader-ml` to make trading decisions.

use crate::traits::{Strategy, StrategyAction, StrategyContext};
use mtrader_core::{OrderReason, Side, Size, Tick};
use mtrader_execution::{OrderKind, OrderType};
use mtrader_ml::{FeatureBuffer, TkanConfig, TkanModel, TkanSignal};
use std::collections::VecDeque;
use tracing::{info, warn};

/// Outcome of a completed trade
#[derive(Debug, Clone, PartialEq)]
pub enum TradeOutcome {
    /// Trade was profitable
    Win {
        pnl: f64,
        entry_tick: Tick,
        exit_tick: Tick,
    },
    /// Trade resulted in loss
    Loss {
        pnl: f64,
        entry_tick: Tick,
        exit_tick: Tick,
    },
    /// Trade was flat (no profit/no loss)
    BreakEven {
        entry_tick: Tick,
        exit_tick: Tick,
    },
}

/// Performance statistics for the strategy
#[derive(Debug, Clone, Default)]
pub struct StrategyStats {
    /// Total number of trades
    pub total_trades: u64,
    /// Number of winning trades
    pub winning_trades: u64,
    /// Number of losing trades
    pub losing_trades: u64,
    /// Total PnL
    pub total_pnl: f64,
    /// Average confidence across all trades
    pub avg_confidence: f64,
    /// Win rate (0.0 to 1.0)
    pub win_rate: f64,
    /// Sharpe ratio (if calculable)
    pub sharpe_ratio: Option<f64>,
    /// Maximum drawdown (in ticks)
    pub max_drawdown: f64,
    /// Number of trades skipped due to low confidence
    pub skipped_trades: u64,
}

/// Performance tracking for ML strategy
#[derive(Debug, Clone)]
pub struct StrategyPerformance {
    /// Trade history with outcomes
    trades: Vec<TradeRecord>,
    /// Running PnL values for Sharpe calculation
    pnl_history: Vec<f64>,
    /// Total PnL
    total_pnl: f64,
    /// High water mark for drawdown
    high_water_mark: f64,
    /// Maximum drawdown observed
    max_drawdown: f64,
    /// Sum of confidence scores
    confidence_sum: f64,
    /// Number of trades
    trade_count: u64,
    /// Number of wins
    win_count: u64,
    /// Number of skips due to low confidence
    skipped_count: u64,
}

/// Record of a single trade with ML signal
#[derive(Debug, Clone)]
struct TradeRecord {
    /// Entry signal confidence
    confidence: f64,
    /// Entry signal direction
    direction: f64,
    /// Entry timestamp
    entry_time_ns: u64,
    /// Entry tick
    entry_tick: Tick,
    /// Exit timestamp
    exit_time_ns: u64,
    /// Exit tick
    exit_tick: Tick,
    /// Trade outcome
    outcome: TradeOutcome,
}

impl StrategyPerformance {
    /// Create new empty performance tracker
    pub fn new() -> Self {
        Self {
            trades: Vec::new(),
            pnl_history: Vec::new(),
            total_pnl: 0.0,
            high_water_mark: 0.0,
            max_drawdown: 0.0,
            confidence_sum: 0.0,
            trade_count: 0,
            win_count: 0,
            skipped_count: 0,
        }
    }

    /// Record a skipped trade due to low confidence
    pub fn record_skip(&mut self) {
        self.skipped_count += 1;
    }

    /// Record a completed trade with its outcome
    pub fn record_trade(&mut self, signal: &TkanSignal, entry_tick: Tick, exit_tick: Tick, pnl: f64, exit_time_ns: u64) {
        let outcome = if pnl > 0.0 {
            TradeOutcome::Win {
                pnl,
                entry_tick,
                exit_tick,
            }
        } else if pnl < 0.0 {
            TradeOutcome::Loss {
                pnl,
                entry_tick,
                exit_tick,
            }
        } else {
            TradeOutcome::BreakEven {
                entry_tick,
                exit_tick,
            }
        };

        let record = TradeRecord {
            confidence: signal.confidence,
            direction: signal.direction,
            entry_time_ns: signal.timestamp_ns,
            entry_tick,
            exit_time_ns,
            exit_tick,
            outcome: outcome.clone(),
        };

        self.trades.push(record);
        self.pnl_history.push(pnl);
        self.total_pnl += pnl;
        self.confidence_sum += signal.confidence;
        self.trade_count += 1;

        if pnl > 0.0 {
            self.win_count += 1;
        }

        // Update high water mark and drawdown
        if self.total_pnl > self.high_water_mark {
            self.high_water_mark = self.total_pnl;
        }

        let drawdown = self.high_water_mark - self.total_pnl;
        if drawdown > self.max_drawdown {
            self.max_drawdown = drawdown;
        }
    }

    /// Get performance statistics
    pub fn get_stats(&self) -> StrategyStats {
        let avg_confidence = if self.trade_count > 0 {
            self.confidence_sum / self.trade_count as f64
        } else {
            0.0
        };

        let win_rate = if self.trade_count > 0 {
            self.win_count as f64 / self.trade_count as f64
        } else {
            0.0
        };

        // Calculate Sharpe ratio if enough trades
        let sharpe_ratio = if self.pnl_history.len() >= 10 {
            Self::calculate_sharpe(&self.pnl_history)
        } else {
            None
        };

        StrategyStats {
            total_trades: self.trade_count,
            winning_trades: self.win_count,
            losing_trades: self.trade_count - self.win_count,
            total_pnl: self.total_pnl,
            avg_confidence,
            win_rate,
            sharpe_ratio,
            max_drawdown: self.max_drawdown,
            skipped_trades: self.skipped_count,
        }
    }

    /// Check if we should trade based on historical performance
    pub fn should_trade(&self, _confidence: f64) -> bool {
        // Basic check: always allow if confidence meets threshold
        // Could be extended with more sophisticated logic
        true
    }

    /// Calculate Sharpe ratio from PnL history
    fn calculate_sharpe(pnl_history: &[f64]) -> Option<f64> {
        if pnl_history.len() < 2 {
            return None;
        }

        let mean: f64 = pnl_history.iter().sum::<f64>() / pnl_history.len() as f64;
        let variance: f64 = pnl_history
            .iter()
            .map(|pnl| (pnl - mean).powi(2))
            .sum::<f64>() / pnl_history.len() as f64;

        let std_dev = variance.sqrt();
        if std_dev == 0.0 {
            return Some(0.0);
        }

        // Annualized Sharpe (assuming 1-hour bars = 8760 periods/year)
        let sharpe = (mean / std_dev) * (8760.0f64).sqrt();
        Some(sharpe)
    }
}

impl Default for StrategyPerformance {
    fn default() -> Self {
        Self::new()
    }
}

/// Configuration for ML strategy
#[derive(Debug, Clone)]
pub struct MlStrategyConfig {
    /// Minimum confidence threshold (0.0 to 1.0), default 0.65
    pub min_confidence: f64,
    /// Enable confidence-based position sizing, default true
    pub confidence_scaling: bool,
    /// Enable performance tracking, default true
    pub enable_performance_tracking: bool,
    /// Minimum position change to trigger trade (in shares)
    pub min_position_change: i64,
    /// Whether to use ML predictions for both sides (bid/ask)
    pub use_both_sides: bool,
    /// ML model configuration
    pub ml_config: TkanConfig,
}

impl Default for MlStrategyConfig {
    fn default() -> Self {
        Self {
            min_confidence: 0.65,
            confidence_scaling: true,
            enable_performance_tracking: true,
            min_position_change: 10,
            use_both_sides: true,
            ml_config: TkanConfig::default(),
        }
    }
}

/// ML-based trading strategy
pub struct MlStrategy {
    name: String,
    config: MlStrategyConfig,
    model: Option<TkanModel>,
    feature_buffer: FeatureBuffer,
    last_signal: Option<TkanSignal>,
    market_id: String,
    is_active: bool,
    performance: Option<StrategyPerformance>,
    pending_trades: VecDeque<PendingTrade>,
}

/// Record of a pending trade (filled but not yet closed)
#[derive(Debug, Clone)]
struct PendingTrade {
    signal: TkanSignal,
    entry_tick: Tick,
    entry_time_ns: u64,
}

impl MlStrategy {
    /// Create a new ML strategy
    pub fn new(
        name: &str,
        market_id: &str,
        config: MlStrategyConfig,
    ) -> Self {
        let feature_buffer = FeatureBuffer::new(config.ml_config.window_size);
        
        // Try to load ML model, fall back to dummy if not available
        let model = match TkanModel::load_json(&config.ml_config.model_path, &config.ml_config) {
            Ok(model) => {
                info!("Loaded ML model from {}", config.ml_config.model_path);
                Some(model)
            }
            Err(err) => {
                info!("Could not load ML model from {}: {}. Using dummy model.", 
                      config.ml_config.model_path, err);
                None
            }
        };

        let performance = if config.enable_performance_tracking {
            Some(StrategyPerformance::new())
        } else {
            None
        };

        Self {
            name: name.to_string(),
            config,
            model,
            feature_buffer,
            last_signal: None,
            market_id: market_id.to_string(),
            is_active: true,
            performance,
            pending_trades: VecDeque::new(),
        }
    }

    /// Create ML strategy with default configuration
    pub fn new_default(name: &str, market_id: &str) -> Self {
        Self::new(name, market_id, MlStrategyConfig::default())
    }

    /// Update feature buffer with latest market data
    fn update_features(&mut self, ctx: &StrategyContext) {
        if let (Some(bid), Some(ask)) = (ctx.best_bid, ctx.best_ask) {
            let price_tick = bid; // Use bid price for features
            let spread_ticks = ask - bid;
            
            self.feature_buffer.push(price_tick, spread_ticks as u16);
        }
    }

    /// Calculate position size scaling based on confidence
    fn confidence_multiplier(&self, confidence: f64) -> f64 {
        if !self.config.confidence_scaling {
            return 1.0;
        }

        // Scale position based on confidence level:
        // 0.65-0.75: 50% of max position
        // 0.75-0.85: 75% of max position
        // 0.85-1.0: 100% of max position
        if confidence < 0.65 {
            0.0
        } else if confidence < 0.75 {
            0.5
        } else if confidence < 0.85 {
            0.75
        } else {
            1.0
        }
    }

    /// Calculate target position based on ML signal
    fn calculate_target_position(&mut self, signal: &TkanSignal, current_position: i64) -> i64 {
        let confidence = signal.confidence;

        // Check if confidence meets minimum threshold
        if confidence < self.config.min_confidence {
            // Record skip if performance tracking is enabled and signal was directional
            if signal.direction.abs() > 0.1 {
                if let Some(ref mut perf) = self.performance {
                    perf.record_skip();
                }
                warn!(
                    "ML Strategy {}: Skipping trade due to low confidence {:.2} (threshold: {:.2})",
                    self.name, confidence, self.config.min_confidence
                );
            }
            return current_position;
        }

        // ML signal direction: -1.0 (bearish) to 1.0 (bullish)
        let direction = signal.direction;

        // Get confidence-based multiplier
        let multiplier = self.confidence_multiplier(confidence);
        
        // Max position change scaled by confidence
        let max_change = (self.config.min_position_change as f64 * multiplier) as i64;
        
        if direction > 0.1 {
            // Bullish signal: increase position (buy)
            current_position + max_change
        } else if direction < -0.1 {
            // Bearish signal: decrease position (sell)
            current_position - max_change
        } else {
            // Neutral signal: maintain position
            current_position
        }
    }
}

impl Strategy for MlStrategy {
    fn name(&self) -> &str {
        &self.name
    }

    fn on_update(&mut self, ctx: &StrategyContext) -> Vec<StrategyAction> {
        if !self.is_active || ctx.asset_id != self.market_id {
            return Vec::new();
        }

        // Skip if no market data
        let Some(best_bid) = ctx.best_bid else { return Vec::new() };
        let Some(best_ask) = ctx.best_ask else { return Vec::new() };

        // Update features with latest market data
        self.update_features(ctx);

        // Get features if buffer is full enough
        let features = match self.feature_buffer.to_feature_vector(ctx.now_ns) {
            Some(features) if self.feature_buffer.len() >= self.config.ml_config.window_size => features,
            _ => return Vec::new(),
        };

        // Get ML prediction
        let signal = if let Some(ref model) = self.model {
            model.predict(&features)
        } else {
            TkanSignal::neutral(ctx.now_ns)
        };

        self.last_signal = Some(signal.clone());

        let target_position = self.calculate_target_position(&signal, ctx.position.net_size);
        let current_position = ctx.position.net_size;
        let position_delta = target_position - current_position;

        if position_delta.abs() < self.config.min_position_change {
            return Vec::new();
        }

        let mut actions = Vec::new();

        // Cancel existing orders if position delta is significant
        if position_delta.abs() > self.config.min_position_change {
            for order in &ctx.our_bids {
                actions.push(StrategyAction::CancelOrder {
                    client_order_id: order.client_order_id.clone(),
                    reason: "ML signal override".to_string(),
                });
            }
            for order in &ctx.our_asks {
                actions.push(StrategyAction::CancelOrder {
                    client_order_id: order.client_order_id.clone(),
                    reason: "ML signal override".to_string(),
                });
            }
        }

        // Place new orders based on ML signal
        if position_delta > 0 {
            // Need to buy - use market sell type for buying at bid
            let buy_quantity = position_delta.unsigned_abs();
            actions.push(StrategyAction::PlaceOrder {
                asset_id: ctx.asset_id.clone(),
                side: Side::Buy,
                kind: OrderKind::Limit {
                    price_tick: best_bid,
                    size_shares: buy_quantity as u64,
                },
                order_type: OrderType::Limit,
                reason: OrderReason::Signal,
            });
        } else if position_delta < 0 {
            // Need to sell - use market buy type for selling at ask
            let sell_quantity = position_delta.abs() as u64;
            actions.push(StrategyAction::PlaceOrder {
                asset_id: ctx.asset_id.clone(),
                side: Side::Sell,
                kind: OrderKind::Limit {
                    price_tick: best_ask,
                    size_shares: sell_quantity,
                },
                order_type: OrderType::Limit,
                reason: OrderReason::Signal,
            });
        }

        actions
    }

    fn on_fill(&mut self, ctx: &StrategyContext, side: Side, tick: Tick, size: Size) {
        // Update feature buffer with fill price
        let spread = match (ctx.best_bid, ctx.best_ask) {
            (Some(bid), Some(ask)) => (ask - bid) as u16,
            _ => 10, // Default spread if no market data
        };
        self.feature_buffer.push(tick, spread);
        
        info!(
            "ML Strategy {} fill: {:?} {} @ {}",
            self.name, side, size, tick
        );

        // Record pending trade for outcome tracking
        if let Some(ref signal) = self.last_signal {
            self.pending_trades.push_back(PendingTrade {
                signal: signal.clone(),
                entry_tick: tick,
                entry_time_ns: ctx.now_ns,
            });
        }
    }

    fn on_halt(&mut self) {
        self.is_active = false;
        info!("ML Strategy {} halted", self.name);
    }

    fn on_resume(&mut self) {
        self.is_active = true;
        info!("ML Strategy {} resumed", self.name);
    }

    fn is_active(&self) -> bool {
        self.is_active
    }

    fn activate(&mut self) {
        self.is_active = true;
    }

    fn deactivate(&mut self) {
        self.is_active = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mtrader_risk::{Position, PnLSnapshot};
    use std::collections::HashMap;

    fn create_strategy_context() -> StrategyContext {
        StrategyContext {
            now_ns: 1_000_000_000,
            asset_id: "btc-updown-15m".to_string(),
            position: Position::new(),
            pnl: PnLSnapshot {
                timestamp_ns: 0,
                realized_pnl: 0,
                unrealized_pnl: 0,
                total_pnl: 0,
                total_fees: 0,
                net_pnl: 0,
                high_water_mark: 0,
                drawdown: 0,
                drawdown_bps: 0,
            },
            best_bid: Some(5000),
            best_ask: Some(5010),
            best_bid_size: 100,
            best_ask_size: 100,
            mid_tick: Some(5005),
            spread_ticks: Some(10),
            our_bids: Vec::new(),
            our_asks: Vec::new(),
            market_snapshots: HashMap::new(),
        }
    }

    #[test]
    fn test_ml_strategy_creation() {
        let config = MlStrategyConfig::default();
        let strategy = MlStrategy::new("test_ml", "btc-updown-15m", config);
        
        assert_eq!(strategy.name(), "test_ml");
        assert!(strategy.is_active());
    }

    #[test]
    fn test_ml_strategy_without_ml_model() {
        // Test that strategy works even without ML model file
        let mut config = MlStrategyConfig::default();
        config.ml_config.model_path = "non_existent_model.json".to_string();
        
        let mut strategy = MlStrategy::new("test_no_model", "btc-updown-15m", config);
        
        // Should create dummy model internally
        let ctx = create_strategy_context();
        let actions = strategy.on_update(&ctx);
        
        // Without model, returns neutral signal and no actions
        assert!(actions.is_empty());
    }

    #[test]
    fn test_feature_buffer_updates() {
        let config = MlStrategyConfig::default();
        let mut strategy = MlStrategy::new("test_features", "btc-updown-15m", config);
        
        let ctx = create_strategy_context();
        strategy.on_update(&ctx);
        
        // Should have updated feature buffer
        assert_eq!(strategy.feature_buffer.len(), 1);
    }

    #[test]
    fn test_strategy_halt_resume() {
        let config = MlStrategyConfig::default();
        let mut strategy = MlStrategy::new("test_halt", "btc-updown-15m", config);
        
        assert!(strategy.is_active());
        strategy.on_halt();
        assert!(!strategy.is_active());
        strategy.on_resume();
        assert!(strategy.is_active());
    }

    #[test]
    fn test_confidence_multiplier() {
        let config = MlStrategyConfig::default();
        let strategy = MlStrategy::new_default("test", "market");
        
        // Low confidence (< 0.65)
        assert_eq!(strategy.confidence_multiplier(0.5), 0.0);
        
        // 0.65-0.75: 50%
        assert_eq!(strategy.confidence_multiplier(0.65), 0.5);
        assert_eq!(strategy.confidence_multiplier(0.70), 0.5);
        assert_eq!(strategy.confidence_multiplier(0.74), 0.5);
        
        // 0.75-0.85: 75%
        assert_eq!(strategy.confidence_multiplier(0.75), 0.75);
        assert_eq!(strategy.confidence_multiplier(0.80), 0.75);
        assert_eq!(strategy.confidence_multiplier(0.84), 0.75);
        
        // 0.85-1.0: 100%
        assert_eq!(strategy.confidence_multiplier(0.85), 1.0);
        assert_eq!(strategy.confidence_multiplier(0.90), 1.0);
        assert_eq!(strategy.confidence_multiplier(1.0), 1.0);
    }

    #[test]
    fn test_confidence_multiplier_disabled() {
        let mut config = MlStrategyConfig::default();
        config.confidence_scaling = false;
        let strategy = MlStrategy::new("test", "market", config);
        
        // Should always return 1.0 when disabled
        assert_eq!(strategy.confidence_multiplier(0.5), 1.0);
        assert_eq!(strategy.confidence_multiplier(0.7), 1.0);
        assert_eq!(strategy.confidence_multiplier(0.9), 1.0);
    }

    #[test]
    fn test_strategy_performance_tracking() {
        let mut perf = StrategyPerformance::new();
        
        // Record some trades
        let signal1 = TkanSignal {
            direction: 0.8,
            confidence: 0.75,
            timestamp_ns: 1000,
        };
        perf.record_trade(&signal1, 5000, 5050, 50.0, 2000);
        
        let signal2 = TkanSignal {
            direction: -0.7,
            confidence: 0.85,
            timestamp_ns: 3000,
        };
        perf.record_trade(&signal2, 5050, 5020, -30.0, 4000);
        
        let stats = perf.get_stats();
        assert_eq!(stats.total_trades, 2);
        assert_eq!(stats.winning_trades, 1);
        assert_eq!(stats.losing_trades, 1);
        assert!((stats.total_pnl - 20.0).abs() < 0.01);
        assert!((stats.avg_confidence - 0.8).abs() < 0.01);
        assert!((stats.win_rate - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_strategy_performance_skip_tracking() {
        let mut perf = StrategyPerformance::new();
        
        perf.record_skip();
        perf.record_skip();
        
        let stats = perf.get_stats();
        assert_eq!(stats.skipped_trades, 2);
    }

    #[test]
    fn test_sharpe_ratio_calculation() {
        let mut perf = StrategyPerformance::new();
        
        // Record enough trades for Sharpe calculation
        for i in 0..15 {
            let signal = TkanSignal {
                direction: 0.5,
                confidence: 0.8,
                timestamp_ns: i as u64 * 1000,
            };
            // Alternating wins and losses
            let pnl = if i % 2 == 0 { 10.0 } else { -5.0 };
            perf.record_trade(&signal, 5000, 5010, pnl, (i + 1) as u64 * 1000);
        }
        
        let stats = perf.get_stats();
        assert!(stats.sharpe_ratio.is_some());
        // Sharpe should be calculable with 15 trades
    }

    #[test]
    fn test_trade_outcome_types() {
        let win = TradeOutcome::Win {
            pnl: 100.0,
            entry_tick: 5000,
            exit_tick: 5100,
        };
        
        let loss = TradeOutcome::Loss {
            pnl: -50.0,
            entry_tick: 5000,
            exit_tick: 4950,
        };
        
        let breakeven = TradeOutcome::BreakEven {
            entry_tick: 5000,
            exit_tick: 5000,
        };
        
        match win {
            TradeOutcome::Win { pnl, .. } => assert_eq!(pnl, 100.0),
            _ => panic!("Expected Win"),
        }
        
        match loss {
            TradeOutcome::Loss { pnl, .. } => assert_eq!(pnl, -50.0),
            _ => panic!("Expected Loss"),
        }
        
        match breakeven {
            TradeOutcome::BreakEven { .. } => {},
            _ => panic!("Expected BreakEven"),
        }
    }
}
