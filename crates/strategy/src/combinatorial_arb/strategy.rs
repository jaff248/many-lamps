use crate::{MarketSnapshot, MarketTokenKey, Strategy, StrategyAction, StrategyContext};
use mtrader_core::{Side, Size, Tick, SIZE_DECIMALS};
use tracing::{info, warn};

use super::{Dependency, DependencyType, CombinatorialArbConfig};
use super::config::DependencyList;

pub struct CombinatorialArbStrategy {
    config: CombinatorialArbConfig,
    /// Active dependencies indexed for fast lookup
    dependency_map: DependencyList,
    active: bool,
}

impl CombinatorialArbStrategy {
    pub fn new(config: CombinatorialArbConfig) -> Self {
        Self {
            dependency_map: config.dependencies.clone(),
            config,
            active: false,
        }
    }

    pub(crate) fn check_implication_arb(
        &self,
        dep: &Dependency,
        source_price: f64,
        target_price: f64,
    ) -> Option<Vec<StrategyAction>> {
        // Implication: Source IMPLIES Target.
        // Logical constraint: Prob(Source) <= Prob(Target).
        // Arb opportunity: Price(Source) > Price(Target).
        // Action: Sell Source (overpriced), Buy Target (underpriced).

        if source_price > target_price {
            let diff = source_price - target_price;
            let profit_bps = (diff * 10000.0) as u32;

            if profit_bps >= dep.min_profit_bps.max(self.config.min_profit_threshold_bps) {
                info!(
                    "Combinatorial Arb (Implication): {} > {} (diff: {} bps)",
                    dep.source_token_id, dep.target_token_id, profit_bps
                );

                let size_shares = self.size_for_legs(source_price, target_price)?;
                let mut actions = Vec::new();

                // 1. Sell Source (Short) - MarketSell for selling shares
                actions.push(StrategyAction::PlaceOrder {
                    side: Side::Sell,
                    kind: mtrader_execution::OrderKind::MarketSell { size_shares },
                    order_type: mtrader_execution::OrderType::FOK,
                    reason: mtrader_core::OrderReason::Signal,
                });

                // 2. Buy Target (Long) - MarketBuy for buying with USDC
                let usdc_amount = (size_shares as f64 * target_price * SIZE_DECIMALS as f64 / SIZE_DECIMALS as f64) as u64;
                actions.push(StrategyAction::PlaceOrder {
                    side: Side::Buy,
                    kind: mtrader_execution::OrderKind::MarketBuy { usdc_amount },
                    order_type: mtrader_execution::OrderType::FOK,
                    reason: mtrader_core::OrderReason::Signal,
                });

                return Some(actions);
            }
        }
        None
    }

    fn size_for_legs(&self, source_price: f64, target_price: f64) -> Option<Size> {
        let source_size = self.size_from_price(source_price)?;
        let target_size = self.size_from_price(target_price)?;
        let size_shares = source_size.min(target_size);
        if size_shares == 0 {
            None
        } else {
            Some(size_shares)
        }
    }

    fn size_from_price(&self, price: f64) -> Option<Size> {
        if price <= 0.0 {
            return None;
        }

        let size_shares = (self.config.max_leg_size_usdc / price) * SIZE_DECIMALS as f64;
        Some(size_shares as u64)
    }

    fn snapshot_price(snapshot: &MarketSnapshot) -> Option<f64> {
        if let Some(mid_tick) = snapshot.mid_tick {
            return Some(Self::tick_to_price(mid_tick));
        }

        match (snapshot.best_bid, snapshot.best_ask) {
            (Some(bid), Some(ask)) => Some(Self::tick_to_price((bid + ask) / 2)),
            (Some(bid), None) => Some(Self::tick_to_price(bid)),
            (None, Some(ask)) => Some(Self::tick_to_price(ask)),
            (None, None) => None,
        }
    }

    fn tick_to_price(tick: Tick) -> f64 {
        tick as f64 / 10000.0
    }
}

impl Strategy for CombinatorialArbStrategy {
    fn name(&self) -> &str {
        "combinatorial_arb"
    }

    fn on_update(&mut self, ctx: &StrategyContext) -> Vec<StrategyAction> {
        if !self.active {
            return Vec::new();
        }

        let mut actions = Vec::new();

        // Iterate over all dependencies
        for dep in &self.dependency_map {
            let source_key = MarketTokenKey {
                market_id: dep.source_market_id.clone(),
                token_id: dep.source_token_id.clone(),
            };
            let target_key = MarketTokenKey {
                market_id: dep.target_market_id.clone(),
                token_id: dep.target_token_id.clone(),
            };

            let source_snapshot = match ctx.market_snapshots.get(&source_key) {
                Some(snapshot) => snapshot,
                None => {
                    warn!(
                        "Missing source market snapshot for {}/{}",
                        dep.source_market_id, dep.source_token_id
                    );
                    continue;
                }
            };
            let target_snapshot = match ctx.market_snapshots.get(&target_key) {
                Some(snapshot) => snapshot,
                None => {
                    warn!(
                        "Missing target market snapshot for {}/{}",
                        dep.target_market_id, dep.target_token_id
                    );
                    continue;
                }
            };

            let source_price = match Self::snapshot_price(source_snapshot) {
                Some(price) => price,
                None => {
                    warn!(
                        "No valid source price for {}/{}",
                        dep.source_market_id, dep.source_token_id
                    );
                    continue;
                }
            };
            let target_price = match Self::snapshot_price(target_snapshot) {
                Some(price) => price,
                None => {
                    warn!(
                        "No valid target price for {}/{}",
                        dep.target_market_id, dep.target_token_id
                    );
                    continue;
                }
            };

            match dep.relation {
                DependencyType::Implication => {
                    if let Some(arb_actions) =
                        self.check_implication_arb(dep, source_price, target_price)
                    {
                        actions.extend(arb_actions);
                    }
                }
                _ => {} // Implement other types
            }
        }

        actions
    }

    fn on_fill(&mut self, _ctx: &StrategyContext, _side: Side, _price: u16, _size: u64) {
        // Handle fill updates (e.g., update position tracking for multi-leg trade)
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
