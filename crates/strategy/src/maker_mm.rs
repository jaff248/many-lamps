//! Market making strategy for single outcomes.
//!
//! Places quotes around fair value with inventory management.

use crate::flow::FlowSignal;
use crate::signals::Signal;
use crate::traits::{Strategy, StrategyAction, StrategyContext};
use mtrader_core::{OrderReason, Side, Size, Tick};
use mtrader_execution::OrderType;
use serde::{Deserialize, Serialize};

/// Configuration for maker MM strategy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MakerMMConfig {
    /// Target half-spread in ticks
    pub half_spread_ticks: u16,
    /// Order size in centishares
    pub order_size: Size,
    /// Maximum position (absolute, centishares)
    pub max_position: i64,
    /// Position skew factor (0.0 to 1.0)
    /// Higher = more aggressive skew away from position
    pub skew_factor: f64,
    /// Minimum edge required to quote (ticks)
    pub min_edge_ticks: u16,
    /// Re-quote threshold (ticks away from target)
    pub requote_threshold_ticks: u16,
    /// Whether to quote both sides
    pub quote_both_sides: bool,
    /// Alpha-flow skew scale (ticks per unit of alpha flow)
    pub flow_alpha_skew_scale: f64,
    /// Impact-flow spread scale (ticks per unit of impact flow)
    pub flow_impact_spread_scale: f64,
    /// Impact-flow size scale (fractional reduction per unit impact flow)
    pub flow_impact_size_scale: f64,
    /// Maximum additional spread from impact flow (ticks)
    pub flow_max_spread_ticks: u16,
    /// Maximum total skew from flow signals (ticks)
    pub flow_max_skew_ticks: i16,
    /// Minimum flow strength to apply adjustments
    pub flow_min_strength: f64,
}

impl Default for MakerMMConfig {
    fn default() -> Self {
        Self {
            half_spread_ticks: 1,
            order_size: 1_000_000, // $100 at mid price
            max_position: 5_000_000, // $500 max position
            skew_factor: 0.3,
            min_edge_ticks: 1,
            requote_threshold_ticks: 1,
            quote_both_sides: true,
            flow_alpha_skew_scale: 2.0,
            flow_impact_spread_scale: 2.0,
            flow_impact_size_scale: 0.5,
            flow_max_spread_ticks: 4,
            flow_max_skew_ticks: 4,
            flow_min_strength: 0.2,
        }
    }
}

/// Market making strategy.
pub struct MakerMMStrategy {
    name: String,
    config: MakerMMConfig,
    active: bool,
    /// Current signal (if any)
    signal: Option<Signal>,
    /// Last computed fair value
    fair_value_tick: Option<Tick>,
    /// Flow-derived alpha/impact signal
    flow_signal: Option<FlowSignal>,
}

impl MakerMMStrategy {
    pub fn new(name: String, config: MakerMMConfig) -> Self {
        Self {
            name,
            config,
            active: false,
            signal: None,
            fair_value_tick: None,
            flow_signal: None,
        }
    }

    /// Update the signal used for fair value adjustment.
    pub fn set_signal(&mut self, signal: Signal) {
        self.signal = Some(signal);
    }

    /// Update flow-derived signal for alpha/impact adjustments.
    pub fn set_flow_signal(&mut self, signal: FlowSignal) {
        self.flow_signal = Some(signal);
    }

    /// Calculate fair value from mid price and signal.
    fn calculate_fair_value(&self, mid_tick: Tick) -> Tick {
        let signal_adjustment = match &self.signal {
            Some(s) if s.is_strong() => {
                // Adjust fair value in direction of signal
                (s.value * 2.0).round() as i16
            }
            _ => 0,
        };

        let adjusted = mid_tick as i16 + signal_adjustment;
        adjusted.max(1).min(99) as Tick
    }

    /// Calculate position-based skew.
    fn calculate_skew(&self, position: i64, flow_skew: i16) -> i16 {
        // Skew away from position to reduce inventory
        let position_ratio = position as f64 / self.config.max_position as f64;
        let skew_ticks = (position_ratio * self.config.skew_factor * 5.0).round() as i16;
        let combined = skew_ticks.clamp(-5, 5) + flow_skew;
        combined.clamp(-self.config.flow_max_skew_ticks, self.config.flow_max_skew_ticks)
    }

    /// Calculate target bid tick.
    fn target_bid(&self, fair_value: Tick, skew: i16, half_spread_ticks: u16) -> Tick {
        let target = fair_value as i16 - half_spread_ticks as i16 - skew;
        target.max(1) as Tick
    }

    /// Calculate target ask tick.
    fn target_ask(&self, fair_value: Tick, skew: i16, half_spread_ticks: u16) -> Tick {
        let target = fair_value as i16 + half_spread_ticks as i16 - skew;
        target.min(99).max(1) as Tick
    }

    /// Check if we should quote this side given position.
    fn should_quote_side(&self, side: Side, position: i64) -> bool {
        if !self.config.quote_both_sides {
            // Only quote the side that reduces position
            return match side {
                Side::Buy => position <= 0,
                Side::Sell => position >= 0,
            };
        }

        // Check position limits
        match side {
            Side::Buy => position < self.config.max_position,
            Side::Sell => position > -self.config.max_position,
        }
    }

    /// Determine order size based on position.
    fn calculate_order_size(&self, side: Side, position: i64, impact_factor: f64) -> Size {
        let base_size = self.config.order_size;
        let adjusted_size = (base_size as f64 * impact_factor.clamp(0.0, 1.0)).round() as Size;

        // Reduce size as we approach position limit
        let headroom = match side {
            Side::Buy => (self.config.max_position - position).max(0) as u64,
            Side::Sell => (self.config.max_position + position).max(0) as u64,
        };

        adjusted_size.min(headroom)
    }

    fn flow_skew_ticks(&self) -> i16 {
        let Some(flow) = self.flow_signal else {
            return 0;
        };
        if flow.alpha_strength < self.config.flow_min_strength {
            return 0;
        }
        let scaled = -flow.alpha_flow * flow.alpha_strength * self.config.flow_alpha_skew_scale;
        scaled
            .round()
            .clamp(-(self.config.flow_max_skew_ticks as f64), self.config.flow_max_skew_ticks as f64)
            as i16
    }

    fn flow_spread_adjustment(&self) -> u16 {
        let Some(flow) = self.flow_signal else {
            return 0;
        };
        if flow.impact_strength < self.config.flow_min_strength {
            return 0;
        }
        let impact = flow.impact_flow.abs() * flow.impact_strength * self.config.flow_impact_spread_scale;
        let ticks = impact
            .round()
            .clamp(0.0, self.config.flow_max_spread_ticks as f64) as u16;
        ticks
    }

    fn flow_size_factor(&self) -> f64 {
        let Some(flow) = self.flow_signal else {
            return 1.0;
        };
        if flow.impact_strength < self.config.flow_min_strength {
            return 1.0;
        }
        let reduction = (flow.impact_flow.abs() * flow.impact_strength * self.config.flow_impact_size_scale)
            .clamp(0.0, 1.0);
        1.0 - reduction
    }
}

impl Strategy for MakerMMStrategy {
    fn name(&self) -> &str {
        &self.name
    }

    fn on_update(&mut self, ctx: &StrategyContext) -> Vec<StrategyAction> {
        if !self.active {
            return vec![];
        }

        let mut actions = Vec::new();

        // Need a two-sided market
        let mid_tick = match ctx.mid_tick {
            Some(m) => m,
            None => return vec![],
        };

        // Calculate fair value and skew
        let fair_value = self.calculate_fair_value(mid_tick);
        self.fair_value_tick = Some(fair_value);

        let flow_skew = self.flow_skew_ticks();
        let skew = self.calculate_skew(ctx.position.net_size, flow_skew);
        let half_spread = self.config.half_spread_ticks + self.flow_spread_adjustment();
        let target_bid = self.target_bid(fair_value, skew, half_spread);
        let target_ask = self.target_ask(fair_value, skew, half_spread);

        // Check if we need to re-quote bids
        if self.should_quote_side(Side::Buy, ctx.position.net_size) {
            let need_new_bid = ctx.our_bids.is_empty()
                || ctx.our_bids.iter().all(|&t| {
                    (t as i16 - target_bid as i16).unsigned_abs() > self.config.requote_threshold_ticks
                });

            if need_new_bid {
                // Cancel existing bids if they're too far
                for &tick in &ctx.our_bids {
                    if (tick as i16 - target_bid as i16).unsigned_abs()
                        > self.config.requote_threshold_ticks
                    {
                        actions.push(StrategyAction::CancelOrder {
                            order_id: format!("bid-{}", tick), // Simplified - real impl needs order tracking
                            reason: "Re-quoting bid".to_string(),
                        });
                    }
                }

                // Check edge
                if let Some(best_ask) = ctx.best_ask {
                    let edge = best_ask as i16 - target_bid as i16;
                    if edge >= self.config.min_edge_ticks as i16 {
                        let size =
                            self.calculate_order_size(Side::Buy, ctx.position.net_size, self.flow_size_factor());
                        if size > 0 {
                            actions.push(StrategyAction::PlaceOrder {
                                side: Side::Buy,
                                tick: target_bid,
                                size,
                                order_type: OrderType::Limit,
                                reason: OrderReason::MakerQuote,
                            });
                        }
                    }
                }
            }
        }

        // Check if we need to re-quote asks
        if self.should_quote_side(Side::Sell, ctx.position.net_size) {
            let need_new_ask = ctx.our_asks.is_empty()
                || ctx.our_asks.iter().all(|&t| {
                    (t as i16 - target_ask as i16).unsigned_abs() > self.config.requote_threshold_ticks
                });

            if need_new_ask {
                // Cancel existing asks if they're too far
                for &tick in &ctx.our_asks {
                    if (tick as i16 - target_ask as i16).unsigned_abs()
                        > self.config.requote_threshold_ticks
                    {
                        actions.push(StrategyAction::CancelOrder {
                            order_id: format!("ask-{}", tick),
                            reason: "Re-quoting ask".to_string(),
                        });
                    }
                }

                // Check edge
                if let Some(best_bid) = ctx.best_bid {
                    let edge = target_ask as i16 - best_bid as i16;
                    if edge >= self.config.min_edge_ticks as i16 {
                        let size =
                            self.calculate_order_size(Side::Sell, ctx.position.net_size, self.flow_size_factor());
                        if size > 0 {
                            actions.push(StrategyAction::PlaceOrder {
                                side: Side::Sell,
                                tick: target_ask,
                                size,
                                order_type: OrderType::Limit,
                                reason: OrderReason::MakerQuote,
                            });
                        }
                    }
                }
            }
        }

        actions
    }

    fn on_fill(&mut self, _ctx: &StrategyContext, _side: Side, _tick: Tick, _size: Size) {
        // Could adjust parameters based on fill patterns
    }

    fn on_halt(&mut self) {
        self.active = false;
    }

    fn on_resume(&mut self) {
        // Don't auto-resume - require explicit activation
    }

    fn is_active(&self) -> bool {
        self.active
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
    use mtrader_risk::{PnLSnapshot, Position};

    fn make_context(mid: Tick, position: i64) -> StrategyContext {
        StrategyContext {
            now_ns: 1000,
            asset_id: "test".to_string(),
            position: Position {
                net_size: position,
                ..Default::default()
            },
            pnl: PnLSnapshot {
                timestamp_ns: 1000,
                realized_pnl: 0,
                unrealized_pnl: 0,
                total_pnl: 0,
                total_fees: 0,
                net_pnl: 0,
                high_water_mark: 0,
                drawdown: 0,
                drawdown_bps: 0,
            },
            best_bid: Some(mid - 1),
            best_ask: Some(mid + 1),
            best_bid_size: 100_000,
            best_ask_size: 100_000,
            mid_tick: Some(mid),
            spread_ticks: Some(2),
            our_bids: vec![],
            our_asks: vec![],
        }
    }

    #[test]
    fn test_inactive_strategy() {
        let mut strategy = MakerMMStrategy::new("test".to_string(), MakerMMConfig::default());
        let ctx = make_context(50, 0);

        let actions = strategy.on_update(&ctx);
        assert!(actions.is_empty());
    }

    #[test]
    fn test_quotes_both_sides() {
        let mut strategy = MakerMMStrategy::new("test".to_string(), MakerMMConfig::default());
        strategy.activate();

        let ctx = make_context(50, 0);
        let actions = strategy.on_update(&ctx);

        // Should have at least bid and ask orders
        let buys = actions.iter().filter(|a| matches!(a, StrategyAction::PlaceOrder { side: Side::Buy, .. })).count();
        let sells = actions.iter().filter(|a| matches!(a, StrategyAction::PlaceOrder { side: Side::Sell, .. })).count();

        assert!(buys > 0);
        assert!(sells > 0);
    }

    #[test]
    fn test_skew_with_position() {
        let strategy = MakerMMStrategy::new("test".to_string(), MakerMMConfig {
            max_position: 1_000_000,
            skew_factor: 0.5,
            ..Default::default()
        });

        // Long position should skew quotes lower (to sell more)
        let skew_long = strategy.calculate_skew(500_000, 0);
        assert!(skew_long > 0); // Positive skew moves quotes down

        // Short position should skew quotes higher (to buy more)
        let skew_short = strategy.calculate_skew(-500_000, 0);
        assert!(skew_short < 0); // Negative skew moves quotes up
    }

    #[test]
    fn test_flow_skew_biases_quotes() {
        let mut strategy = MakerMMStrategy::new("test".to_string(), MakerMMConfig {
            flow_alpha_skew_scale: 4.0,
            flow_max_skew_ticks: 4,
            ..Default::default()
        });
        strategy.activate();
        strategy.set_flow_signal(FlowSignal {
            alpha_flow: 0.5,
            impact_flow: 0.0,
            alpha_strength: 1.0,
            impact_strength: 0.0,
            timestamp_ns: 1000,
        });

        let ctx = make_context(50, 0);
        let actions = strategy.on_update(&ctx);
        let best_bid = actions.iter().filter_map(|a| match a {
            StrategyAction::PlaceOrder { side: Side::Buy, tick, .. } => Some(*tick),
            _ => None,
        }).max();

        assert!(best_bid.unwrap_or(0) > 49);
    }

    #[test]
    fn test_flow_impact_reduces_size() {
        let mut strategy = MakerMMStrategy::new("test".to_string(), MakerMMConfig {
            order_size: 1_000_000,
            flow_impact_size_scale: 1.0,
            ..Default::default()
        });
        strategy.activate();
        strategy.set_flow_signal(FlowSignal {
            alpha_flow: 0.0,
            impact_flow: 0.8,
            alpha_strength: 0.0,
            impact_strength: 1.0,
            timestamp_ns: 1000,
        });

        let ctx = make_context(50, 0);
        let actions = strategy.on_update(&ctx);
        let order_sizes: Vec<Size> = actions.iter().filter_map(|a| match a {
            StrategyAction::PlaceOrder { size, .. } => Some(*size),
            _ => None,
        }).collect();

        assert!(order_sizes.iter().all(|&size| size < 1_000_000));
    }
}
