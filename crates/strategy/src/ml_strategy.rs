//! ML-based trading strategy using T-KAN predictions.
//!
//! This strategy uses ML signals from `mtrader-ml` to make trading decisions.

use crate::traits::{Strategy, StrategyAction, StrategyContext};
use mtrader_core::{OrderReason, Side, Size, Tick};
use mtrader_execution::{OrderKind, OrderType};
use mtrader_ml::{FeatureBuffer, TkanConfig, TkanModel, TkanSignal};
use tracing::info;

/// Configuration for ML strategy
#[derive(Debug, Clone)]
pub struct MlStrategyConfig {
    /// Minimum confidence threshold (0.0 to 1.0)
    pub min_confidence: f64,
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
            min_confidence: 0.6,
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

        Self {
            name: name.to_string(),
            config,
            model,
            feature_buffer,
            last_signal: None,
            market_id: market_id.to_string(),
            is_active: true,
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

    /// Calculate target position based on ML signal
    fn calculate_target_position(&self, signal: &TkanSignal, current_position: i64) -> i64 {
        if !signal.is_actionable(self.config.min_confidence) {
            return current_position;
        }

        // ML signal direction: -1.0 (bearish) to 1.0 (bullish)
        let direction = signal.direction;
        let confidence = signal.confidence;

        // Scale position based on confidence and direction
        // Max position change scaled by confidence
        let max_change = (self.config.min_position_change as f64 * confidence) as i64;
        
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
}
