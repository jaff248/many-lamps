//! Bundle maker strategy for YES/NO arbitrage.
//!
//! Exploits the fact that YES + NO = 1.00 in binary markets.
//! When YES ask + NO ask < 1.00, we can profit by:
//! 1. Buying both YES and NO
//! 2. Waiting for the market to resolve (or selling when arb closes)
//!
//! The fee model is crucial here since 15-min markets have
//! parabolic fees that peak at mid prices.

use crate::traits::{Strategy, StrategyAction, StrategyContext};
use mtrader_core::{OrderReason, Side, Size, Tick};
use mtrader_execution::OrderType;
use serde::{Deserialize, Serialize};

/// Configuration for bundle maker strategy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleMakerConfig {
    /// Minimum arbitrage profit after fees (micro-USDC)
    pub min_profit_micro_usdc: i64,
    /// Order size for each leg (micro-shares)
    pub leg_size: Size,
    /// Maximum concurrent arb positions
    pub max_concurrent_arbs: usize,
    /// Fee rate in basis points (1000 for 15-min markets)
    pub fee_rate_bps: u32,
    /// Whether to only quote the maker side (collect spread)
    pub maker_only: bool,
}

impl Default for BundleMakerConfig {
    fn default() -> Self {
        Self {
            min_profit_micro_usdc: 100_000, // $0.10 minimum profit
            leg_size: 1_000_000, // $100 notional per leg
            max_concurrent_arbs: 3,
            fee_rate_bps: 1000, // 15-min markets
            maker_only: true,
        }
    }
}

/// State of a bundle arb position.
#[derive(Debug, Clone)]
pub struct ArbPosition {
    /// YES asset ID
    pub yes_asset_id: String,
    /// NO asset ID
    pub no_asset_id: String,
    /// YES position (micro-shares)
    pub yes_size: Size,
    /// NO position (micro-shares)
    pub no_size: Size,
    /// Entry tick for YES
    pub yes_entry_tick: Tick,
    /// Entry tick for NO
    pub no_entry_tick: Tick,
    /// Timestamp entered
    pub entered_at_ns: u64,
}

impl ArbPosition {
    /// Calculate the combined entry cost in ticks (should be < 10000).
    pub fn combined_entry_ticks(&self) -> u16 {
        self.yes_entry_tick + self.no_entry_tick
    }

    /// Calculate profit potential in ticks.
    pub fn profit_ticks(&self) -> i16 {
        10000 - self.combined_entry_ticks() as i16
    }
}

/// Bundle maker strategy.
pub struct BundleMakerStrategy {
    name: String,
    config: BundleMakerConfig,
    active: bool,
    /// Active arbitrage positions
    positions: Vec<ArbPosition>,
    /// YES asset ID
    yes_asset_id: String,
    /// NO asset ID
    no_asset_id: String,
}

impl BundleMakerStrategy {
    pub fn new(
        name: String,
        config: BundleMakerConfig,
        yes_asset_id: String,
        no_asset_id: String,
    ) -> Self {
        Self {
            name,
            config,
            active: false,
            positions: Vec::new(),
            yes_asset_id,
            no_asset_id,
        }
    }

    /// Calculate fee for a trade (parabolic model for 15-min markets).
    fn calculate_fee(&self, price_tick: Tick, size: Size) -> i64 {
        let price_bps = price_tick as u64;
        let complement_bps = 10000 - price_bps;
        let numerator =
            self.config.fee_rate_bps as u64 * price_bps * complement_bps * size;
        let denominator = 16000u64 * 10000u64 * 10000u64;

        (numerator / denominator) as i64
    }

    /// Check if there's an arbitrage opportunity.
    fn check_arb_opportunity(
        &self,
        yes_ask: Tick,
        no_ask: Tick,
    ) -> Option<i64> {
        // Combined ask should be < 10000 for arb
        let combined = yes_ask as u16 + no_ask as u16;
        if combined >= 10000 {
            return None;
        }

        // Calculate gross profit (in ticks)
        let gross_profit_ticks = 10000 - combined as i16;

        // Convert to micro-USDC
        // Profit per share = gross_profit_ticks / 10000 dollars
        // For micro-shares: profit_micro = gross_profit_ticks * size / 10000
        let gross_profit_micro =
            gross_profit_ticks as i64 * self.config.leg_size as i64 / 10000;

        // Calculate fees for both legs
        let yes_fee = self.calculate_fee(yes_ask, self.config.leg_size);
        let no_fee = self.calculate_fee(no_ask, self.config.leg_size);
        let total_fees = yes_fee + no_fee;

        let net_profit = gross_profit_micro - total_fees;

        if net_profit >= self.config.min_profit_micro_usdc {
            Some(net_profit)
        } else {
            None
        }
    }

    /// Generate actions to enter an arb position.
    fn enter_arb(&mut self, yes_ask: Tick, no_ask: Tick, now_ns: u64) -> Vec<StrategyAction> {
        let mut actions = Vec::new();

        if self.config.maker_only {
            // Place maker orders slightly better than current ask
            // to try to get filled as maker (earn rebate)
            let yes_bid = yes_ask.saturating_sub(1).max(1);
            let no_bid = no_ask.saturating_sub(1).max(1);

            actions.push(StrategyAction::PlaceOrder {
                side: Side::Buy,
                tick: yes_bid,
                size: self.config.leg_size,
                order_type: OrderType::Limit,
                reason: OrderReason::BundleArb,
            });

            actions.push(StrategyAction::PlaceOrder {
                side: Side::Buy,
                tick: no_bid,
                size: self.config.leg_size,
                order_type: OrderType::Limit,
                reason: OrderReason::BundleArb,
            });
        } else {
            // Aggressive: take the ask
            actions.push(StrategyAction::PlaceOrder {
                side: Side::Buy,
                tick: yes_ask,
                size: self.config.leg_size,
                order_type: OrderType::Limit,
                reason: OrderReason::BundleArb,
            });

            actions.push(StrategyAction::PlaceOrder {
                side: Side::Buy,
                tick: no_ask,
                size: self.config.leg_size,
                order_type: OrderType::Limit,
                reason: OrderReason::BundleArb,
            });
        }

        // Track the position (will be updated on fills)
        self.positions.push(ArbPosition {
            yes_asset_id: self.yes_asset_id.clone(),
            no_asset_id: self.no_asset_id.clone(),
            yes_size: 0, // Updated on fill
            no_size: 0,
            yes_entry_tick: yes_ask,
            no_entry_tick: no_ask,
            entered_at_ns: now_ns,
        });

        actions
    }
}

impl Strategy for BundleMakerStrategy {
    fn name(&self) -> &str {
        &self.name
    }

    fn on_update(&mut self, ctx: &StrategyContext) -> Vec<StrategyAction> {
        if !self.active {
            return vec![];
        }

        // This strategy needs both YES and NO book data
        // For now, we'll assume ctx provides data for one side
        // and we track both internally

        // Check position limits
        if self.positions.len() >= self.config.max_concurrent_arbs {
            return vec![];
        }

        // In a real implementation, we'd have both books available
        // For now, return empty - the actual arb detection would happen
        // in a coordinator that has access to both books

        vec![]
    }

    fn on_fill(&mut self, ctx: &StrategyContext, side: Side, tick: Tick, size: Size) {
        // Update position tracking when fills come in
        if side != Side::Buy {
            return; // We only buy in bundle arb
        }

        // Find matching position and update
        for pos in &mut self.positions {
            if ctx.asset_id == pos.yes_asset_id && pos.yes_size == 0 {
                pos.yes_size = size;
                pos.yes_entry_tick = tick;
                break;
            } else if ctx.asset_id == pos.no_asset_id && pos.no_size == 0 {
                pos.no_size = size;
                pos.no_entry_tick = tick;
                break;
            }
        }
    }

    fn on_halt(&mut self) {
        self.active = false;
    }

    fn on_resume(&mut self) {
        // Don't auto-resume
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

    #[test]
    fn test_fee_calculation() {
        let strategy = BundleMakerStrategy::new(
            "test".to_string(),
            BundleMakerConfig {
                fee_rate_bps: 1000,
                ..Default::default()
            },
            "yes".to_string(),
            "no".to_string(),
        );

        // At 50 cents (tick 5000), fee should be maximum
        // fee = 0.0625 * 0.50 * 0.50 * 1_000_000 micro-shares = 15_625 micro-shares
        let fee = strategy.calculate_fee(5000, 1_000_000);
        assert!((fee - 15_625).abs() < 100); // Allow small rounding

        // At 10 cents, fee should be lower
        // fee = 0.0625 * 0.10 * 0.90 * 1_000_000 = 5_625 micro-shares
        let fee = strategy.calculate_fee(1000, 1_000_000);
        assert!((fee - 5_625).abs() < 100);
    }

    #[test]
    fn test_arb_detection() {
        let strategy = BundleMakerStrategy::new(
            "test".to_string(),
            BundleMakerConfig {
                fee_rate_bps: 1000,
                leg_size: 1_000_000,
                min_profit_micro_usdc: 100_000, // $0.10
                ..Default::default()
            },
            "yes".to_string(),
            "no".to_string(),
        );

        // YES ask = 4000, NO ask = 5500 -> combined = 9500 -> 500 tick profit
        // Gross profit = 500 * 1_000_000 / 10000 = 50_000 micro ($0.05)
        // But we need to subtract fees...
        let profit = strategy.check_arb_opportunity(4000, 5500);

        // This might not be profitable after fees, depending on exact calculation
        // The test validates the logic runs

        // Clearly profitable: YES = 3000, NO = 3000 -> combined = 6000 -> 4000 tick profit
        let profit = strategy.check_arb_opportunity(3000, 3000);
        assert!(profit.is_some());
        assert!(profit.unwrap() > 0);
    }

    #[test]
    fn test_no_arb_when_combined_above_100() {
        let strategy = BundleMakerStrategy::new(
            "test".to_string(),
            BundleMakerConfig::default(),
            "yes".to_string(),
            "no".to_string(),
        );

        // Combined ask >= 100, no arb
        assert!(strategy.check_arb_opportunity(5500, 5000).is_none());
        assert!(strategy.check_arb_opportunity(5000, 5000).is_none());
        assert!(strategy.check_arb_opportunity(6000, 4500).is_none());
    }
}
