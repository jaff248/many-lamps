use crate::{MarketTokenKey, Strategy, StrategyAction, StrategyContext};
use mtrader_core::{OrderReason, Side, Size, SIZE_DECIMALS};
use mtrader_execution::{OrderKind, OrderType};
use tracing::{info, warn};

use super::config::DependencyList;
use super::price;
use super::{CombinatorialArbConfig, Dependency, DependencyType};

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

        if source_price <= target_price {
            return None;
        }

        let diff = source_price - target_price;
        let profit_bps = (diff * 10000.0) as u32;

        if !self.meets_profit_threshold(dep, profit_bps) {
            return None;
        }

        info!(
            "Combinatorial Arb (Implication): {} > {} (diff: {} bps)",
            dep.source_token_id, dep.target_token_id, profit_bps
        );

        let size_shares = self.size_for_legs(source_price, target_price)?;
        let buy_usdc = self.usdc_for_size(target_price, size_shares)?;

        Some(vec![
            StrategyAction::PlaceOrder {
                side: Side::Sell,
                kind: OrderKind::MarketSell { size_shares },
                order_type: OrderType::FOK,
                reason: OrderReason::Signal,
            },
            StrategyAction::PlaceOrder {
                side: Side::Buy,
                kind: OrderKind::MarketBuy {
                    usdc_amount: buy_usdc,
                },
                order_type: OrderType::FOK,
                reason: OrderReason::Signal,
            },
        ])
    }

    pub(crate) fn check_mutually_exclusive_arb(
        &self,
        dep: &Dependency,
        source_price: f64,
        target_price: f64,
    ) -> Option<Vec<StrategyAction>> {
        // Mutually exclusive: Price(Source) + Price(Target) <= 1.
        let sum = source_price + target_price;
        if sum <= 1.0 {
            return None;
        }

        let profit_bps = ((sum - 1.0) * 10000.0) as u32;
        if !self.meets_profit_threshold(dep, profit_bps) {
            return None;
        }

        info!(
            "Combinatorial Arb (Mutually Exclusive): {} + {} = {:.4} ({} bps)",
            dep.source_token_id, dep.target_token_id, sum, profit_bps
        );

        let size_shares = self.size_for_legs(source_price, target_price)?;

        Some(vec![
            StrategyAction::PlaceOrder {
                side: Side::Sell,
                kind: OrderKind::MarketSell { size_shares },
                order_type: OrderType::FOK,
                reason: OrderReason::Signal,
            },
            StrategyAction::PlaceOrder {
                side: Side::Sell,
                kind: OrderKind::MarketSell { size_shares },
                order_type: OrderType::FOK,
                reason: OrderReason::Signal,
            },
        ])
    }

    pub(crate) fn check_identical_arb(
        &self,
        dep: &Dependency,
        source_price: f64,
        target_price: f64,
    ) -> Option<Vec<StrategyAction>> {
        let diff = (source_price - target_price).abs();
        if diff <= 0.0 {
            return None;
        }

        let profit_bps = (diff * 10000.0) as u32;
        if !self.meets_profit_threshold(dep, profit_bps) {
            return None;
        }

        let (sell_price, buy_price, sell_label, buy_label, sell_token, buy_token) =
            if source_price > target_price {
                (
                    source_price,
                    target_price,
                    "source",
                    "target",
                    &dep.source_token_id,
                    &dep.target_token_id,
                )
            } else {
                (
                    target_price,
                    source_price,
                    "target",
                    "source",
                    &dep.target_token_id,
                    &dep.source_token_id,
                )
            };

        info!(
            "Combinatorial Arb (Identical): sell {} {} / buy {} {} ({} bps)",
            sell_label, sell_token, buy_label, buy_token, profit_bps
        );

        let size_shares = self.size_for_legs(sell_price, buy_price)?;
        let buy_usdc = self.usdc_for_size(buy_price, size_shares)?;

        Some(vec![
            StrategyAction::PlaceOrder {
                side: Side::Sell,
                kind: OrderKind::MarketSell { size_shares },
                order_type: OrderType::FOK,
                reason: OrderReason::Signal,
            },
            StrategyAction::PlaceOrder {
                side: Side::Buy,
                kind: OrderKind::MarketBuy {
                    usdc_amount: buy_usdc,
                },
                order_type: OrderType::FOK,
                reason: OrderReason::Signal,
            },
        ])
    }

    fn meets_profit_threshold(&self, dep: &Dependency, profit_bps: u32) -> bool {
        profit_bps >= dep.min_profit_bps.max(self.config.min_profit_threshold_bps)
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

    fn usdc_for_size(&self, price: f64, size_shares: Size) -> Option<u64> {
        if price <= 0.0 || size_shares == 0 {
            return None;
        }
        let shares = size_shares as f64 / SIZE_DECIMALS as f64;
        let usdc = (shares * price * SIZE_DECIMALS as f64) as u64;
        if usdc == 0 {
            None
        } else {
            Some(usdc)
        }
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

            let source_price = match price::snapshot_price(source_snapshot) {
                Some(price) => price,
                None => {
                    warn!(
                        "No valid source price for {}/{}",
                        dep.source_market_id, dep.source_token_id
                    );
                    continue;
                }
            };
            let target_price = match price::snapshot_price(target_snapshot) {
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
                DependencyType::MutuallyExclusive => {
                    if let Some(arb_actions) =
                        self.check_mutually_exclusive_arb(dep, source_price, target_price)
                    {
                        actions.extend(arb_actions);
                    }
                }
                DependencyType::Identical => {
                    if let Some(arb_actions) =
                        self.check_identical_arb(dep, source_price, target_price)
                    {
                        actions.extend(arb_actions);
                    }
                }
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
