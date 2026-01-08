//! Auto-hedge strategy for 15-minute UP/DOWN markets.
//!
//! Watches the early part of each round for sharp dumps, buys the dumped side,
//! then waits to hedge by buying the opposite side when the sum is below a
//! target threshold.

use crate::traits::{Strategy, StrategyAction, StrategyContext};
use mtrader_core::{OrderReason, Side, Size, Tick};
use mtrader_execution::{OrderKind, OrderType};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};

const TICK_SCALE: f64 = 10000.0;
const NS_PER_MS: u64 = 1_000_000;
const NS_PER_SEC: u64 = 1_000_000_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoHedgeConfig {
    /// Size per leg in micro-shares.
    pub leg_size: Size,
    /// Sum target (price_up + price_down <= sum_target).
    pub sum_target: f64,
    /// Percent drop required (0.15 = 15%).
    pub dip_threshold: f64,
    /// Sliding window for dip detection (ms).
    pub dip_window_ms: u64,
    /// Minutes from round start to allow leg 1.
    pub window_minutes: u64,
    /// Timeout to force exit if leg 2 not reached.
    pub leg2_timeout_seconds: u64,
}

impl Default for AutoHedgeConfig {
    fn default() -> Self {
        Self {
            leg_size: 10_000_000, // 10 shares
            sum_target: 0.95,
            dip_threshold: 0.15,
            dip_window_ms: 3_000,
            window_minutes: 2,
            leg2_timeout_seconds: 100,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HedgeSide {
    Up,
    Down,
}

impl HedgeSide {
    fn opposite(self) -> Self {
        match self {
            HedgeSide::Up => HedgeSide::Down,
            HedgeSide::Down => HedgeSide::Up,
        }
    }
}

#[derive(Debug, Clone)]
struct Leg1State {
    side: HedgeSide,
    entry_tick: Tick,
    entry_time_ns: u64,
}

/// Auto-hedge strategy state machine.
pub struct AutoHedgeStrategy {
    name: String,
    config: AutoHedgeConfig,
    active: bool,
    up_asset_id: String,
    down_asset_id: String,
    round_start_ns: Option<u64>,
    leg1: Option<Leg1State>,
    best_bid: HashMap<String, Tick>,
    best_ask: HashMap<String, Tick>,
    price_history: HashMap<String, VecDeque<(u64, Tick)>>,
}

impl AutoHedgeStrategy {
    pub fn new(
        name: String,
        config: AutoHedgeConfig,
        up_asset_id: String,
        down_asset_id: String,
    ) -> Self {
        Self {
            name,
            config,
            active: false,
            up_asset_id,
            down_asset_id,
            round_start_ns: None,
            leg1: None,
            best_bid: HashMap::new(),
            best_ask: HashMap::new(),
            price_history: HashMap::new(),
        }
    }

    fn asset_side(&self, asset_id: &str) -> Option<HedgeSide> {
        if asset_id == self.up_asset_id {
            Some(HedgeSide::Up)
        } else if asset_id == self.down_asset_id {
            Some(HedgeSide::Down)
        } else {
            None
        }
    }

    fn within_leg1_window(&self, now_ns: u64) -> bool {
        let Some(round_start) = self.round_start_ns else {
            return true;
        };
        let window_ns = self.config.window_minutes * 60 * NS_PER_SEC;
        now_ns.saturating_sub(round_start) <= window_ns
    }

    fn update_price_history(&mut self, asset_id: &str, now_ns: u64, ask: Tick) {
        let history = self.price_history.entry(asset_id.to_string()).or_default();
        history.push_back((now_ns, ask));
        let window_ns = self.config.dip_window_ms * NS_PER_MS;
        while let Some((ts, _)) = history.front() {
            if now_ns.saturating_sub(*ts) > window_ns {
                history.pop_front();
            } else {
                break;
            }
        }
    }

    fn dip_triggered(&self, asset_id: &str, current_tick: Tick) -> bool {
        let Some(history) = self.price_history.get(asset_id) else {
            return false;
        };
        let max_tick = history.iter().map(|(_, tick)| *tick).max().unwrap_or(current_tick);
        if max_tick == 0 {
            return false;
        }
        let drop = (max_tick as f64 - current_tick as f64) / max_tick as f64;
        drop >= self.config.dip_threshold
    }

    fn sum_target_tick(&self) -> Tick {
        (self.config.sum_target * TICK_SCALE).round() as Tick
    }

    fn leg2_timeout_ns(&self) -> u64 {
        self.config.leg2_timeout_seconds * NS_PER_SEC
    }

    fn leg1_expired(&self, now_ns: u64) -> bool {
        self.leg1
            .as_ref()
            .map(|leg1| now_ns.saturating_sub(leg1.entry_time_ns) >= self.leg2_timeout_ns())
            .unwrap_or(false)
    }

    fn best_ask_for(&self, side: HedgeSide) -> Option<Tick> {
        let asset_id = match side {
            HedgeSide::Up => &self.up_asset_id,
            HedgeSide::Down => &self.down_asset_id,
        };
        self.best_ask.get(asset_id).copied()
    }

    fn best_bid_for(&self, side: HedgeSide) -> Option<Tick> {
        let asset_id = match side {
            HedgeSide::Up => &self.up_asset_id,
            HedgeSide::Down => &self.down_asset_id,
        };
        self.best_bid.get(asset_id).copied()
    }

    fn place_buy(&self, price_tick: Tick) -> StrategyAction {
        StrategyAction::PlaceOrder {
            side: Side::Buy,
            kind: OrderKind::Limit {
                price_tick,
                size_shares: self.config.leg_size,
            },
            order_type: OrderType::Limit,
            reason: OrderReason::Signal,
        }
    }

    fn place_sell(&self, price_tick: Tick) -> StrategyAction {
        StrategyAction::PlaceOrder {
            side: Side::Sell,
            kind: OrderKind::Limit {
                price_tick,
                size_shares: self.config.leg_size,
            },
            order_type: OrderType::Limit,
            reason: OrderReason::RiskKill,
        }
    }

    fn maybe_trigger_leg1(&mut self, asset_id: &str, now_ns: u64) -> Option<StrategyAction> {
        if !self.within_leg1_window(now_ns) || self.leg1.is_some() {
            return None;
        }

        let side = self.asset_side(asset_id)?;
        let ask = self.best_ask.get(asset_id).copied()?;
        if !self.dip_triggered(asset_id, ask) {
            return None;
        }

        self.leg1 = Some(Leg1State {
            side,
            entry_tick: ask,
            entry_time_ns: now_ns,
        });

        Some(self.place_buy(ask))
    }

    fn maybe_trigger_leg2(&mut self) -> Option<StrategyAction> {
        let leg1 = self.leg1.as_ref()?;
        let opposite = leg1.side.opposite();
        let opp_ask = self.best_ask_for(opposite)?;
        let sum_target_tick = self.sum_target_tick();
        if leg1.entry_tick as u32 + opp_ask as u32 <= sum_target_tick as u32 {
            let action = self.place_buy(opp_ask);
            self.leg1 = None;
            return Some(action);
        }
        None
    }

    fn maybe_stop_loss(&mut self, now_ns: u64) -> Option<StrategyAction> {
        if !self.leg1_expired(now_ns) {
            return None;
        }

        let leg1 = self.leg1.take()?;
        let best_bid = self.best_bid_for(leg1.side)?;
        Some(self.place_sell(best_bid))
    }
}

impl Strategy for AutoHedgeStrategy {
    fn name(&self) -> &str {
        &self.name
    }

    fn on_update(&mut self, ctx: &StrategyContext) -> Vec<StrategyAction> {
        if !self.active {
            return vec![];
        }

        let now_ns = ctx.now_ns;
        self.round_start_ns.get_or_insert(now_ns);

        if let Some(best_bid) = ctx.best_bid {
            self.best_bid.insert(ctx.asset_id.clone(), best_bid);
        }
        if let Some(best_ask) = ctx.best_ask {
            self.best_ask.insert(ctx.asset_id.clone(), best_ask);
            self.update_price_history(&ctx.asset_id, now_ns, best_ask);
        }

        let mut actions = Vec::new();

        if let Some(action) = self.maybe_stop_loss(now_ns) {
            actions.push(action);
            return actions;
        }

        if let Some(action) = self.maybe_trigger_leg2() {
            actions.push(action);
            return actions;
        }

        if let Some(action) = self.maybe_trigger_leg1(&ctx.asset_id, now_ns) {
            actions.push(action);
        }

        actions
    }

    fn on_fill(&mut self, _ctx: &StrategyContext, _side: Side, _tick: Tick, _size: Size) {}

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

    fn context(asset_id: &str, now_ns: u64, best_ask: Tick) -> StrategyContext {
        StrategyContext {
            now_ns,
            asset_id: asset_id.to_string(),
            position: Default::default(),
            pnl: PnLSnapshot {
                timestamp_ns: now_ns,
                realized_pnl: 0,
                unrealized_pnl: 0,
                total_pnl: 0,
                total_fees: 0,
                net_pnl: 0,
                high_water_mark: 0,
                drawdown: 0,
                drawdown_bps: 0,
            },
            best_bid: Some(best_ask.saturating_sub(10)),
            best_ask: Some(best_ask),
            best_bid_size: 0,
            best_ask_size: 0,
            mid_tick: None,
            spread_ticks: None,
            our_bids: vec![],
            our_asks: vec![],
        }
    }

    #[test]
    fn triggers_leg1_on_dip() {
        let mut strategy = AutoHedgeStrategy::new(
            "auto".to_string(),
            AutoHedgeConfig {
                dip_threshold: 0.1,
                ..Default::default()
            },
            "up".to_string(),
            "down".to_string(),
        );
        strategy.activate();

        let mut actions = strategy.on_update(&context("up", 0, 6000));
        actions.extend(strategy.on_update(&context("up", 1_000_000, 5000)));

        assert!(actions.iter().any(|action| matches!(action, StrategyAction::PlaceOrder { side: Side::Buy, .. })));
    }

    #[test]
    fn triggers_leg2_on_sum_target() {
        let mut strategy = AutoHedgeStrategy::new(
            "auto".to_string(),
            AutoHedgeConfig {
                sum_target: 0.95,
                dip_threshold: 0.1,
                ..Default::default()
            },
            "up".to_string(),
            "down".to_string(),
        );
        strategy.activate();

        strategy.on_update(&context("up", 0, 6000));
        strategy.on_update(&context("up", 1_000_000, 5000));
        let actions = strategy.on_update(&context("down", 2_000_000, 4500));

        assert!(actions.iter().any(|action| matches!(action, StrategyAction::PlaceOrder { side: Side::Buy, .. })));
    }
}
