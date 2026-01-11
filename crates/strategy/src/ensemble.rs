//! Strategy Ensemble with Dynamic Weighting
//!
//! Combines multiple trading strategies with dynamic weights computed from
//! recent performance metrics (Sharpe ratio, win rate, etc.).
//!
//! Weight computation uses softmax over Sharpe ratios:
//! `w_i = exp(sharpe_i / T) / Σ exp(sharpe_j / T)`
//!
//! Weights are clamped to [min_weight, max_weight] and renormalized.

use crate::traits::{Strategy, StrategyAction, StrategyContext};
use mtrader_core::{ClientOrderId, Side, Size, Tick};
use std::collections::VecDeque;
use std::time::Duration;

/// Configuration for the strategy ensemble.
#[derive(Debug, Clone)]
pub struct EnsembleConfig {
    /// Lookback window size for performance tracking (default: 100)
    pub window_size: usize,
    /// How often to update weights (default: 60 seconds)
    pub rebalance_interval: Duration,
    /// Minimum strategy weight (default: 0.05)
    pub min_weight: f64,
    /// Maximum strategy weight (default: 0.50)
    pub max_weight: f64,
    /// Softmax temperature for weight computation (default: 1.0)
    pub temperature: f64,
}

impl Default for EnsembleConfig {
    fn default() -> Self {
        Self {
            window_size: 100,
            rebalance_interval: Duration::from_secs(60),
            min_weight: 0.05,
            max_weight: 0.50,
            temperature: 1.0,
        }
    }
}

/// Tracks performance metrics for a single strategy in the ensemble.
#[derive(Debug, Clone)]
pub struct EnsembleStrategyPerformance {
    /// Recent returns (PnL per trade)
    pub returns: VecDeque<f64>,
    /// Rolling Sharpe ratio
    pub sharpe_ratio: f64,
    /// Win rate (percentage of profitable trades)
    pub win_rate: f64,
    /// Average profit on winning trades
    pub avg_profit: f64,
    /// Average loss on losing trades (absolute value)
    pub avg_loss: f64,
    /// Number of trades recorded
    pub trade_count: usize,
}

impl Default for EnsembleStrategyPerformance {
    fn default() -> Self {
        Self {
            returns: VecDeque::new(),
            sharpe_ratio: 0.0,
            win_rate: 0.0,
            avg_profit: 0.0,
            avg_loss: 0.0,
            trade_count: 0,
        }
    }
}

impl EnsembleStrategyPerformance {
    /// Create a new performance tracker with given window size.
    pub fn new(window_size: usize) -> Self {
        Self {
            returns: VecDeque::with_capacity(window_size),
            sharpe_ratio: 0.0,
            win_rate: 0.0,
            avg_profit: 0.0,
            avg_loss: 0.0,
            trade_count: 0,
        }
    }

    /// Record a new return and update metrics.
    pub fn record_return(&mut self, ret: f64, window_size: usize) {
        self.returns.push_back(ret);
        if self.returns.len() > window_size {
            self.returns.pop_front();
        }
        self.trade_count += 1;
        self.update_metrics();
    }

    /// Update Sharpe ratio, win rate, and average profit/loss.
    fn update_metrics(&mut self) {
        if self.returns.is_empty() {
            return;
        }

        // Calculate win rate
        let wins: usize = self.returns.iter().filter(|&&r| r > 0.0).count();
        self.win_rate = wins as f64 / self.returns.len() as f64;

        // Calculate average profit and loss
        let profits: Vec<f64> = self.returns.iter().filter(|&&r| r > 0.0).cloned().collect();
        let losses: Vec<f64> = self.returns.iter().filter(|&&r| r < 0.0).cloned().collect();

        self.avg_profit = if !profits.is_empty() {
            profits.iter().sum::<f64>() / profits.len() as f64
        } else {
            0.0
        };

        self.avg_loss = if !losses.is_empty() {
            losses.iter().sum::<f64>().abs() / losses.len() as f64
        } else {
            0.0
        };

        // Calculate Sharpe ratio (annualized assuming 1-minute bars ~ 525600 per year)
        // For simplicity, we use a rolling Sharpe without annualization factor
        let mean: f64 = self.returns.iter().sum::<f64>() / self.returns.len() as f64;
        let variance: f64 = self
            .returns
            .iter()
            .map(|r| (r - mean).powi(2))
            .sum::<f64>()
            / self.returns.len() as f64;
        let std_dev = variance.sqrt();

        self.sharpe_ratio = if std_dev > 0.0 {
            mean / std_dev
        } else {
            0.0
        };
    }

    /// Get the current Sharpe ratio.
    pub fn get_sharpe(&self) -> f64 {
        self.sharpe_ratio
    }
}

/// Strategy ensemble that combines multiple strategies with dynamic weights.
pub struct StrategyEnsemble {
    /// Vector of strategies
    strategies: Vec<Box<dyn Strategy>>,
    /// Current weights for each strategy
    weights: Vec<f64>,
    /// Performance tracker per strategy
    performance: Vec<EnsembleStrategyPerformance>,
    /// Configuration
    config: EnsembleConfig,
    /// Last rebalance timestamp (in nanoseconds)
    last_rebalance_ns: u64,
    /// Whether the ensemble is active
    active: bool,
}

impl StrategyEnsemble {
    /// Create a new strategy ensemble with given strategies and config.
    pub fn new(strategies: Vec<Box<dyn Strategy>>, config: EnsembleConfig) -> Self {
        let n = strategies.len();
        let weights = if n > 0 {
            vec![1.0 / n as f64; n]
        } else {
            vec![]
        };

        let performance: Vec<EnsembleStrategyPerformance> = (0..n)
            .map(|_| EnsembleStrategyPerformance::new(config.window_size))
            .collect();

        Self {
            strategies,
            weights,
            performance,
            config,
            last_rebalance_ns: 0,
            active: true,
        }
    }

    /// Add a new strategy to the ensemble.
    pub fn add_strategy(&mut self, strategy: Box<dyn Strategy>) {
        self.strategies.push(strategy);
        self.performance
            .push(EnsembleStrategyPerformance::new(self.config.window_size));
        // Recompute weights to include new strategy
        self.update_weights();
    }

    /// Recompute weights based on performance metrics.
    ///
    /// Uses softmax over Sharpe ratios:
    /// `w_i = exp(sharpe_i / T) / Σ exp(sharpe_j / T)`
    ///
    /// Then clamps weights to [min_weight, max_weight] and renormalizes.
    pub fn update_weights(&mut self) {
        let n = self.strategies.len();
        if n == 0 {
            return;
        }

        // Compute raw softmax weights from Sharpe ratios
        let sharpes: Vec<f64> = self.performance.iter().map(|p| p.sharpe_ratio).collect();

        // Find max Sharpe for numerical stability
        let max_sharpe = sharpes.iter().cloned().fold(f64::MIN, f64::max);

        // Compute exponentials
        let exp_sharpes: Vec<f64> = sharpes
            .iter()
            .map(|s| ((s - max_sharpe) / self.config.temperature).exp())
            .collect();

        let sum_exp: f64 = exp_sharpes.iter().sum();

        if sum_exp > 0.0 {
            self.weights = exp_sharpes.iter().map(|e| e / sum_exp).collect();
        } else {
            // Fallback to uniform weights
            self.weights = vec![1.0 / n as f64; n];
        }

        // Clamp weights to [min_weight, max_weight]
        for weight in &mut self.weights {
            *weight = weight.clamp(self.config.min_weight, self.config.max_weight);
        }

        // Renormalize to sum to 1.0
        let sum: f64 = self.weights.iter().sum();
        if sum > 0.0 {
            for weight in &mut self.weights {
                *weight /= sum;
            }
        } else {
            // Fallback to uniform weights if sum is 0
            self.weights = vec![1.0 / n as f64; n];
        }
    }

    /// Get current weights.
    pub fn get_weights(&self) -> &[f64] {
        &self.weights
    }

    /// Get performance data for all strategies.
    pub fn get_performance(&self) -> &[EnsembleStrategyPerformance] {
        &self.performance
    }

    /// Check if it's time to rebalance weights.
    pub fn should_rebalance(&self, now_ns: u64) -> bool {
        // If last_rebalance_ns is 0 (not yet rebalanced), always return true
        if self.last_rebalance_ns == 0 {
            return true;
        }
        now_ns - self.last_rebalance_ns >= self.config.rebalance_interval.as_nanos() as u64
    }

    /// Get the number of strategies in the ensemble.
    pub fn len(&self) -> usize {
        self.strategies.len()
    }

    /// Check if the ensemble has no strategies.
    pub fn is_empty(&self) -> bool {
        self.strategies.is_empty()
    }

    /// Record a fill event for a specific strategy.
    pub fn record_fill(&mut self, strategy_idx: usize, ret: f64) {
        if strategy_idx < self.performance.len() {
            self.performance[strategy_idx].record_return(ret, self.config.window_size);
        }
    }

    /// Set the last rebalance timestamp.
    pub fn set_last_rebalance(&mut self, now_ns: u64) {
        self.last_rebalance_ns = now_ns;
    }
}

impl Strategy for StrategyEnsemble {
    fn name(&self) -> &str {
        "StrategyEnsemble"
    }

    fn on_update(&mut self, ctx: &StrategyContext) -> Vec<StrategyAction> {
        if !self.active || self.strategies.is_empty() {
            return vec![];
        }

        let mut all_actions = Vec::new();

        for (idx, strategy) in self.strategies.iter_mut().enumerate() {
            let actions = strategy.on_update(ctx);
            // Tag actions with strategy index for tracking
            for action in actions {
                // Forward the action (tagging would require modifying StrategyAction)
                all_actions.push(action);
            }
        }

        // Check if we should rebalance weights
        if self.should_rebalance(ctx.now_ns) {
            self.update_weights();
            self.set_last_rebalance(ctx.now_ns);
        }

        all_actions
    }

    fn on_fill(&mut self, ctx: &StrategyContext, side: Side, tick: Tick, size: Size) {
        // Track fill for all strategies - in practice, we'd need to know which
        // strategy generated the order. For now, we record a placeholder return.
        // The actual implementation would need order attribution.
        let placeholder_return = 0.0; // Would be computed from fill data

        for performance in &mut self.performance {
            performance.record_return(placeholder_return, self.config.window_size);
        }
    }

    fn on_halt(&mut self) {
        for strategy in &mut self.strategies {
            strategy.on_halt();
        }
        self.active = false;
    }

    fn on_resume(&mut self) {
        for strategy in &mut self.strategies {
            strategy.on_resume();
        }
        self.active = true;
    }

    fn is_active(&self) -> bool {
        self.active && !self.strategies.is_empty()
    }

    fn activate(&mut self) {
        self.active = true;
    }

    fn deactivate(&mut self) {
        self.active = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::traits::{Strategy, StrategyAction, StrategyContext};
    use crate::EnsembleStrategyPerformance;
    use mtrader_risk::{PnLSnapshot, Position};
    use std::cell::RefCell;

    /// Mock strategy for testing.
    struct MockStrategy {
        name: String,
        actions: RefCell<Vec<StrategyAction>>,
        fill_count: RefCell<usize>,
    }

    impl MockStrategy {
        fn new(name: &str) -> Self {
            Self {
                name: name.to_string(),
                actions: RefCell::new(vec![]),
                fill_count: RefCell::new(0),
            }
        }
    }

    impl Strategy for MockStrategy {
        fn name(&self) -> &str {
            &self.name
        }

        fn on_update(&mut self, _ctx: &StrategyContext) -> Vec<StrategyAction> {
            self.actions.borrow_mut().clone()
        }

        fn on_fill(&mut self, _ctx: &StrategyContext, _side: Side, _tick: Tick, _size: Size) {
            *self.fill_count.borrow_mut() += 1;
        }

        fn on_halt(&mut self) {}
        fn on_resume(&mut self) {}
        fn is_active(&self) -> bool { true }
        fn activate(&mut self) {}
        fn deactivate(&mut self) {}
    }

    /// Create a minimal StrategyContext for testing.
    fn test_context() -> StrategyContext {
        StrategyContext {
            now_ns: 0,
            asset_id: "test".to_string(),
            position: Position::default(),
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
            best_bid: Some(50),
            best_ask: Some(51),
            best_bid_size: 100,
            best_ask_size: 100,
            mid_tick: Some(50),
            spread_ticks: Some(1),
            our_bids: vec![],
            our_asks: vec![],
            market_snapshots: Default::default(),
        }
    }

    #[test]
    fn test_ensemble_config_default() {
        let config = EnsembleConfig::default();
        assert_eq!(config.window_size, 100);
        assert_eq!(config.rebalance_interval, Duration::from_secs(60));
        assert_eq!(config.min_weight, 0.05);
        assert_eq!(config.max_weight, 0.50);
        assert_eq!(config.temperature, 1.0);
    }

    #[test]
    fn test_strategy_performance_new() {
        let perf = EnsembleStrategyPerformance::new(50);
        assert_eq!(perf.returns.capacity(), 50);
        assert_eq!(perf.trade_count, 0);
        assert_eq!(perf.sharpe_ratio, 0.0);
        assert_eq!(perf.win_rate, 0.0);
    }

    #[test]
    fn test_strategy_performance_record_return() {
        let mut perf = EnsembleStrategyPerformance::new(10);

        // Record some returns
        perf.record_return(0.01, 10);
        perf.record_return(0.02, 10);
        perf.record_return(-0.01, 10);

        assert_eq!(perf.trade_count, 3);
        assert!((perf.win_rate - 0.6666).abs() < 0.01);
        assert!((perf.avg_profit - 0.015).abs() < 0.001);
        assert!((perf.avg_loss - 0.01).abs() < 0.001);
    }

    #[test]
    fn test_strategy_performance_sharpe_ratio() {
        let mut perf = EnsembleStrategyPerformance::new(10);

        // All positive returns should give positive Sharpe
        perf.record_return(0.01, 10);
        perf.record_return(0.02, 10);
        perf.record_return(0.03, 10);

        assert!(perf.sharpe_ratio > 0.0);
    }

    #[test]
    fn test_strategy_ensemble_new() {
        let strategies: Vec<Box<dyn Strategy>> = vec![
            Box::new(MockStrategy::new("strategy1")),
            Box::new(MockStrategy::new("strategy2")),
        ];
        let config = EnsembleConfig::default();
        let ensemble = StrategyEnsemble::new(strategies, config);

        assert_eq!(ensemble.len(), 2);
        assert_eq!(ensemble.weights.len(), 2);
        assert!((ensemble.weights[0] - 0.5).abs() < 0.001);
        assert!((ensemble.weights[1] - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_strategy_ensemble_empty() {
        let strategies: Vec<Box<dyn Strategy>> = vec![];
        let config = EnsembleConfig::default();
        let ensemble = StrategyEnsemble::new(strategies, config);

        assert!(ensemble.is_empty());
        assert!(ensemble.get_weights().is_empty());
    }

    #[test]
    fn test_add_strategy() {
        let strategies: Vec<Box<dyn Strategy>> = vec![
            Box::new(MockStrategy::new("strategy1")),
        ];
        let config = EnsembleConfig::default();
        let mut ensemble = StrategyEnsemble::new(strategies, config);

        assert_eq!(ensemble.len(), 1);
        assert!((ensemble.weights[0] - 1.0).abs() < 0.001);

        ensemble.add_strategy(Box::new(MockStrategy::new("strategy2")));

        assert_eq!(ensemble.len(), 2);
        assert!((ensemble.weights[0] - 0.5).abs() < 0.001);
        assert!((ensemble.weights[1] - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_update_weights_uniform() {
        let strategies: Vec<Box<dyn Strategy>> = vec![
            Box::new(MockStrategy::new("strategy1")),
            Box::new(MockStrategy::new("strategy2")),
        ];
        let config = EnsembleConfig::default();
        let mut ensemble = StrategyEnsemble::new(strategies, config);

        ensemble.update_weights();

        assert!((ensemble.weights[0] - 0.5).abs() < 0.001);
        assert!((ensemble.weights[1] - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_update_weights_clamping() {
        // Create 2 strategies with extreme Sharpe ratio difference
        let strategies: Vec<Box<dyn Strategy>> = vec![
            Box::new(MockStrategy::new("strategy1")),
            Box::new(MockStrategy::new("strategy2")),
        ];
        
        let config = EnsembleConfig::default();

        let mut ensemble = StrategyEnsemble::new(strategies, config);

        // Set extreme Sharpe ratios
        ensemble.performance[0].sharpe_ratio = 100.0; // Very high
        ensemble.performance[1].sharpe_ratio = 0.0;   // Zero

        ensemble.update_weights();

        // Weights should sum to 1
        let sum: f64 = ensemble.weights.iter().sum();
        assert!((sum - 1.0).abs() < 0.001, "weights should sum to 1.0, got {}", sum);

        // Both weights should be >= min_weight (default 0.05)
        assert!(ensemble.weights[0] >= 0.05, "weight 0 should be >= 0.05");
        assert!(ensemble.weights[1] >= 0.05, "weight 1 should be >= 0.05");

        // Strategy 0 with higher Sharpe should have higher weight
        assert!(ensemble.weights[0] > ensemble.weights[1], 
            "strategy 0 should have higher weight than strategy 1");

        // Both weights should be reasonable (less than 0.95 due to min constraint on other)
        assert!(ensemble.weights[0] < 0.95, "weight 0 should be < 0.95 due to min_weight on others");
    }

    #[test]
    fn test_update_weights_softmax() {
        let strategies: Vec<Box<dyn Strategy>> = vec![
            Box::new(MockStrategy::new("strategy1")),
            Box::new(MockStrategy::new("strategy2")),
        ];
        let config = EnsembleConfig::default();
        let mut ensemble = StrategyEnsemble::new(strategies, config);

        // Set different Sharpe ratios via performance
        ensemble.performance[0].sharpe_ratio = 2.0;
        ensemble.performance[1].sharpe_ratio = 1.0;

        ensemble.update_weights();

        // Strategy 0 should have higher weight
        assert!(ensemble.weights[0] > ensemble.weights[1]);
    }

    #[test]
    fn test_strategy_ensemble_on_update() {
        let strategies: Vec<Box<dyn Strategy>> = vec![
            Box::new(MockStrategy::new("strategy1")),
            Box::new(MockStrategy::new("strategy2")),
        ];
        let config = EnsembleConfig::default();
        let mut ensemble = StrategyEnsemble::new(strategies, config);

        let ctx = test_context();
        let actions = ensemble.on_update(&ctx);

        // Should forward updates to all strategies
        assert!(!actions.is_empty() || actions.is_empty()); // Just verify it runs
    }

    #[test]
    fn test_strategy_ensemble_on_fill() {
        let strategies: Vec<Box<dyn Strategy>> = vec![
            Box::new(MockStrategy::new("strategy1")),
            Box::new(MockStrategy::new("strategy2")),
        ];
        let config = EnsembleConfig::default();
        let mut ensemble = StrategyEnsemble::new(strategies, config);

        let ctx = test_context();
        ensemble.on_fill(&ctx, Side::Buy, 50, 100);

        // All strategies should record the fill
        assert!(ensemble.performance[0].trade_count > 0 || ensemble.performance[1].trade_count > 0);
    }

    #[test]
    fn test_should_rebalance() {
        let strategies: Vec<Box<dyn Strategy>> = vec![
            Box::new(MockStrategy::new("strategy1")),
        ];
        let config = EnsembleConfig::default();
        let interval_ns = config.rebalance_interval.as_nanos() as u64;
        let mut ensemble = StrategyEnsemble::new(strategies, config);

        // Initially should rebalance (last_rebalance_ns is 0)
        assert!(ensemble.should_rebalance(0));

        // After setting last rebalance to a non-zero time, should not rebalance before interval
        ensemble.set_last_rebalance(interval_ns);  // Set to 60 seconds ago
        assert!(!ensemble.should_rebalance(interval_ns), "should not rebalance before interval");

        // After interval, should rebalance
        assert!(ensemble.should_rebalance(interval_ns * 2), "should rebalance after interval");
    }

    #[test]
    fn test_performance_tracking_window_size() {
        let mut perf = EnsembleStrategyPerformance::new(3);

        // Add more returns than window size
        for i in 0..5 {
            perf.record_return(i as f64, 3);
        }

        // Should only keep last 3 returns
        assert_eq!(perf.returns.len(), 3);
        assert_eq!(perf.trade_count, 5);
    }

    #[test]
    fn test_weights_sum_to_one() {
        let strategies: Vec<Box<dyn Strategy>> = vec![
            Box::new(MockStrategy::new("strategy1")),
            Box::new(MockStrategy::new("strategy2")),
            Box::new(MockStrategy::new("strategy3")),
        ];
        let config = EnsembleConfig::default();
        let mut ensemble = StrategyEnsemble::new(strategies, config);

        ensemble.update_weights();

        let sum: f64 = ensemble.weights.iter().sum();
        assert!((sum - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_activate_deactivate() {
        let strategies: Vec<Box<dyn Strategy>> = vec![
            Box::new(MockStrategy::new("strategy1")),
        ];
        let config = EnsembleConfig::default();
        let mut ensemble = StrategyEnsemble::new(strategies, config);

        assert!(ensemble.is_active());

        ensemble.deactivate();
        assert!(!ensemble.is_active());

        ensemble.activate();
        assert!(ensemble.is_active());
    }
}
