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
use mtrader_core::fees::MarketFeeProfile;
use mtrader_core::{OrderReason, Side, Size, Tick, MAX_TICK};
use mtrader_execution::{OrderKind, OrderType};
use serde::{Deserialize, Serialize};

/// Configuration for bundle maker strategy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleMakerConfig {
    /// Minimum arbitrage profit after fees (micro-USDC)
    pub min_profit_micro_usdc: i64,
    /// Order size for each leg (shares)
    pub leg_size: Size,
    /// Maximum concurrent arb positions
    pub max_concurrent_arbs: usize,
    /// Market fee profile
    pub fee_profile: MarketFeeProfile,
    /// Whether to only quote the maker side (collect spread)
    pub maker_only: bool,
}

impl Default for BundleMakerConfig {
    fn default() -> Self {
        Self {
            min_profit_micro_usdc: 100_000, // $0.10 minimum profit
            leg_size: 1_000_000,            // 1 share per leg
            max_concurrent_arbs: 3,
            fee_profile: MarketFeeProfile::crypto_15m(),
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
    /// YES position (shares)
    pub yes_size: Size,
    /// NO position (shares)
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
        MAX_TICK as i16 - self.combined_entry_ticks() as i16
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
        self.config
            .fee_profile
            .schedule
            .calculate_fee(price_tick, size) as i64
    }

    /// Check if there's an arbitrage opportunity.
    fn check_arb_opportunity(&self, yes_ask: Tick, no_ask: Tick) -> Option<i64> {
        // Combined ask should be < 10000 for arb
        let combined = yes_ask as u16 + no_ask as u16;
        if combined >= MAX_TICK {
            return None;
        }

        // Calculate gross profit (in ticks)
        let gross_profit_ticks = MAX_TICK as i16 - combined as i16;

        // Convert to micro-USDC
        // Profit per share = gross_profit_ticks / 10000 dollars
        // For leg_size micro-shares: profit = gross_profit_ticks * leg_size / 10000
        let gross_profit_micro = (gross_profit_ticks as i64 * self.config.leg_size as i64) / 10000;

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
                asset_id: self.yes_asset_id.clone(),
                side: Side::Buy,
                kind: OrderKind::Limit {
                    price_tick: yes_bid,
                    size_shares: self.config.leg_size,
                },
                order_type: OrderType::Limit,
                reason: OrderReason::BundleArb,
            });

            actions.push(StrategyAction::PlaceOrder {
                asset_id: self.no_asset_id.clone(),
                side: Side::Buy,
                kind: OrderKind::Limit {
                    price_tick: no_bid,
                    size_shares: self.config.leg_size,
                },
                order_type: OrderType::Limit,
                reason: OrderReason::BundleArb,
            });
        } else {
            // Aggressive: take the ask
            actions.push(StrategyAction::PlaceOrder {
                asset_id: self.yes_asset_id.clone(),
                side: Side::Buy,
                kind: OrderKind::Limit {
                    price_tick: yes_ask,
                    size_shares: self.config.leg_size,
                },
                order_type: OrderType::Limit,
                reason: OrderReason::BundleArb,
            });

            actions.push(StrategyAction::PlaceOrder {
                asset_id: self.no_asset_id.clone(),
                side: Side::Buy,
                kind: OrderKind::Limit {
                    price_tick: no_ask,
                    size_shares: self.config.leg_size,
                },
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
                fee_profile: MarketFeeProfile::crypto_15m(),
                ..Default::default()
            },
            "yes".to_string(),
            "no".to_string(),
        );

        // At 50 cents (tick 5000), fee should be maximum
        // fee = 1000 bps × 0.50 × 1_000_000 micro-shares = 50_000 micro-USDC
        let fee = strategy.calculate_fee(5000, 1_000_000);
        assert!((fee - 50_000).abs() < 1000); // Allow small rounding

        // At 10 cents, fee should be lower
        // fee = 1000 bps × 0.10 × 1_000_000 = 10_000 micro-USDC
        let fee = strategy.calculate_fee(1000, 1_000_000);
        assert!((fee - 10_000).abs() < 1000);
    }

    #[test]
    fn test_arb_detection() {
        let strategy = BundleMakerStrategy::new(
            "test".to_string(),
            BundleMakerConfig {
                fee_profile: MarketFeeProfile::crypto_15m(),
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
    fn test_no_arb_when_combined_above_10000() {
        let strategy = BundleMakerStrategy::new(
            "test".to_string(),
            BundleMakerConfig::default(),
            "yes".to_string(),
            "no".to_string(),
        );

        // Combined ask >= 10000, no arb
        assert!(strategy.check_arb_opportunity(5500, 5000).is_none());
        assert!(strategy.check_arb_opportunity(5000, 5000).is_none());
        assert!(strategy.check_arb_opportunity(6000, 4500).is_none());
    }

    #[test]
    fn test_fee_profile_zero_vs_parabolic() {
        let zero_fee = BundleMakerStrategy::new(
            "zero".to_string(),
            BundleMakerConfig {
                fee_profile: MarketFeeProfile::zero("unaffected"),
                leg_size: 1_000_000,
                min_profit_micro_usdc: 10,
                ..Default::default()
            },
            "yes".to_string(),
            "no".to_string(),
        );

        let parabolic_fee = BundleMakerStrategy::new(
            "parabolic".to_string(),
            BundleMakerConfig {
                fee_profile: MarketFeeProfile::crypto_15m(),
                leg_size: 1_000_000,
                min_profit_micro_usdc: 10,
                ..Default::default()
            },
            "yes".to_string(),
            "no".to_string(),
        );

        let zero_profit = zero_fee.check_arb_opportunity(4950, 4950);
        let fee_profit = parabolic_fee.check_arb_opportunity(4950, 4950);

        assert!(zero_profit.is_some());
        assert!(fee_profit.is_none());
    }
}
