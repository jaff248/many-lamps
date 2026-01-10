//! Arbitrage strategy for unaffected (fee-free) markets.
//!
//! Uses a fee profile to determine eligibility and calculate edge.

use crate::traits::{Strategy, StrategyAction, StrategyContext};
use mtrader_core::fees::{FeeScheduleKind, MarketFeeProfile};
use mtrader_core::{OrderReason, Side, Size, Tick, MAX_TICK};
use mtrader_execution::{OrderKind, OrderType};
use serde::{Deserialize, Serialize};

/// Configuration for unaffected market arbitrage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnaffectedArbConfig {
    /// Minimum arbitrage profit after fees (micro-USDC).
    pub min_profit_micro_usdc: i64,
    /// Order size for each leg (shares).
    pub leg_size: Size,
    /// Maximum concurrent arb positions.
    pub max_concurrent_arbs: usize,
    /// Fee profile for this market.
    pub fee_profile: MarketFeeProfile,
    /// Eligible fee schedules for this strategy.
    pub eligible_fee_schedules: Vec<FeeScheduleKind>,
    /// Whether to only quote the maker side (collect spread).
    pub maker_only: bool,
}

impl Default for UnaffectedArbConfig {
    fn default() -> Self {
        Self {
            min_profit_micro_usdc: 100_000, // $0.10 minimum profit
            leg_size: 1_000_000,            // 1 share per leg
            max_concurrent_arbs: 3,
            fee_profile: MarketFeeProfile::zero("unaffected"),
            eligible_fee_schedules: vec![FeeScheduleKind::Zero],
            maker_only: true,
        }
    }
}

/// State of an arb position.
#[derive(Debug, Clone)]
pub struct ArbPosition {
    pub yes_asset_id: String,
    pub no_asset_id: String,
    pub yes_size: Size,
    pub no_size: Size,
    pub yes_entry_tick: Tick,
    pub no_entry_tick: Tick,
    pub entered_at_ns: u64,
}

/// Unaffected-market arbitrage strategy.
pub struct UnaffectedArbStrategy {
    name: String,
    config: UnaffectedArbConfig,
    active: bool,
    positions: Vec<ArbPosition>,
    yes_asset_id: String,
    no_asset_id: String,
}

impl UnaffectedArbStrategy {
    pub fn new(
        name: String,
        config: UnaffectedArbConfig,
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

    fn is_eligible(&self) -> bool {
        self.config
            .eligible_fee_schedules
            .contains(&self.config.fee_profile.schedule.kind())
    }

    fn calculate_fee(&self, price_tick: Tick, size: Size) -> i64 {
        self.config
            .fee_profile
            .schedule
            .calculate_fee(price_tick, size) as i64
    }

    fn check_arb_opportunity(&self, yes_ask: Tick, no_ask: Tick) -> Option<i64> {
        let combined = yes_ask as u16 + no_ask as u16;
        if combined >= MAX_TICK {
            return None;
        }

        let gross_profit_ticks = MAX_TICK as i16 - combined as i16;
        let gross_profit_micro = (gross_profit_ticks as i64 * self.config.leg_size as i64) / 10000;

        let yes_fee = self.calculate_fee(yes_ask, self.config.leg_size);
        let no_fee = self.calculate_fee(no_ask, self.config.leg_size);
        let net_profit = gross_profit_micro - (yes_fee + no_fee);

        if net_profit >= self.config.min_profit_micro_usdc {
            Some(net_profit)
        } else {
            None
        }
    }

    fn enter_arb(&mut self, yes_ask: Tick, no_ask: Tick, now_ns: u64) -> Vec<StrategyAction> {
        let mut actions = Vec::new();

        if self.config.maker_only {
            let yes_bid = yes_ask.saturating_sub(1).max(1);
            let no_bid = no_ask.saturating_sub(1).max(1);

            actions.push(StrategyAction::PlaceOrder {
                side: Side::Buy,
                kind: OrderKind::Limit {
                    price_tick: yes_bid,
                    size_shares: self.config.leg_size,
                },
                order_type: OrderType::Limit,
                reason: OrderReason::BundleArb,
            });

            actions.push(StrategyAction::PlaceOrder {
                side: Side::Buy,
                kind: OrderKind::Limit {
                    price_tick: no_bid,
                    size_shares: self.config.leg_size,
                },
                order_type: OrderType::Limit,
                reason: OrderReason::BundleArb,
            });
        } else {
            actions.push(StrategyAction::PlaceOrder {
                side: Side::Buy,
                kind: OrderKind::Limit {
                    price_tick: yes_ask,
                    size_shares: self.config.leg_size,
                },
                order_type: OrderType::Limit,
                reason: OrderReason::BundleArb,
            });

            actions.push(StrategyAction::PlaceOrder {
                side: Side::Buy,
                kind: OrderKind::Limit {
                    price_tick: no_ask,
                    size_shares: self.config.leg_size,
                },
                order_type: OrderType::Limit,
                reason: OrderReason::BundleArb,
            });
        }

        self.positions.push(ArbPosition {
            yes_asset_id: self.yes_asset_id.clone(),
            no_asset_id: self.no_asset_id.clone(),
            yes_size: 0,
            no_size: 0,
            yes_entry_tick: yes_ask,
            no_entry_tick: no_ask,
            entered_at_ns: now_ns,
        });

        actions
    }
}

impl Strategy for UnaffectedArbStrategy {
    fn name(&self) -> &str {
        &self.name
    }

    fn on_update(&mut self, _ctx: &StrategyContext) -> Vec<StrategyAction> {
        if !self.active || !self.is_eligible() {
            return vec![];
        }

        if self.positions.len() >= self.config.max_concurrent_arbs {
            return vec![];
        }

        vec![]
    }

    fn on_fill(&mut self, ctx: &StrategyContext, side: Side, tick: Tick, size: Size) {
        if side != Side::Buy {
            return;
        }

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

    fn on_resume(&mut self) {}

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
    use mtrader_risk::PnLSnapshot;

    #[test]
    fn test_eligibility_filter() {
        let mut strategy = UnaffectedArbStrategy::new(
            "test".to_string(),
            UnaffectedArbConfig {
                fee_profile: MarketFeeProfile::crypto_15m(),
                eligible_fee_schedules: vec![FeeScheduleKind::Zero],
                ..Default::default()
            },
            "yes".to_string(),
            "no".to_string(),
        );

        strategy.activate();
        let pnl = PnLSnapshot {
            timestamp_ns: 0,
            realized_pnl: 0,
            unrealized_pnl: 0,
            total_pnl: 0,
            total_fees: 0,
            net_pnl: 0,
            high_water_mark: 0,
            drawdown: 0,
            drawdown_bps: 0,
        };
        let ctx = StrategyContext {
            now_ns: 0,
            asset_id: "yes".to_string(),
            position: Default::default(),
            pnl,
            best_bid: None,
            best_ask: None,
            best_bid_size: 0,
            best_ask_size: 0,
            mid_tick: None,
            spread_ticks: None,
            our_bids: vec![],
            our_asks: vec![],
        };

        assert!(strategy.on_update(&ctx).is_empty());
    }

    #[test]
    fn test_zero_fee_opportunity() {
        let strategy = UnaffectedArbStrategy::new(
            "test".to_string(),
            UnaffectedArbConfig {
                fee_profile: MarketFeeProfile::zero("unaffected"),
                eligible_fee_schedules: vec![FeeScheduleKind::Zero],
                leg_size: 1_000_000,
                min_profit_micro_usdc: 1,
                ..Default::default()
            },
            "yes".to_string(),
            "no".to_string(),
        );

        let profit = strategy.check_arb_opportunity(4000, 5000);
        assert!(profit.is_some());
    }
}
