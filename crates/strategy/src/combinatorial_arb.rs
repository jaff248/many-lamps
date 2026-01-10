//! Combinatorial Arbitrage Strategy
//!
//! Based on "Unravelling the Probabilistic Forest: Arbitrage in Prediction Markets" (arXiv:2508.03474).
//! This strategy identifies arbitrage opportunities between semantically dependent markets.
//!
//! # Concepts
//! - **Market Rebalancing (Intra-market)**: Standard arb where sum of outcome prices != $1.
//! - **Combinatorial Arbitrage (Inter-market)**: Arb between two markets that share dependent conditions.
//!   - Example: Market A ("Team A wins") vs Market B ("Team A wins by > 2 points").
//!   - Logical implication: "Team A wins by > 2" IMPLIES "Team A wins".
//!   - Arbitrage condition: Price("Wins by > 2") > Price("Wins").
//!   - Execution: Sell "Wins by > 2" (Short) + Buy "Wins" (Long).
//!
//! # Implementation
//! This module provides the `CombinatorialArbStrategy` which:
//! 1. Maintains a graph of market dependencies (currently manual/config-based, future LLM-based).
//! 2. Monitors prices of dependent condition pairs.
//! 3. Executes spread trades when logical invariants are violated.

use crate::{Strategy, StrategyAction, StrategyContext};
use mtrader_core::{OrderReason, Side, Size, Tick};
use mtrader_execution::{OrderKind, OrderType};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use rust_decimal_macros::dec;
use std::collections::HashMap;
use tracing::{info, warn};

/// Types of dependency between two outcomes
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DependencyType {
    /// A implies B (if A happens, B MUST happen).
    /// Arb: Price(A) cannot be > Price(B).
    /// If Price(A) > Price(B), buy B, sell A.
    Implication,

    /// A and B are mutually exclusive (cannot both happen).
    /// Arb: Price(A) + Price(B) cannot be > 1.
    /// If Price(A) + Price(B) > 1, sell A, sell B.
    MutuallyExclusive,

    /// A and B are identical (should have same price).
    /// Arb: Price(A) != Price(B).
    /// If Price(A) < Price(B), buy A, sell B.
    Identical,
}

/// A directed edge in the dependency graph
#[derive(Debug, Clone)]
pub struct Dependency {
    pub source_market_id: String,
    pub source_token_id: String,
    pub target_market_id: String,
    pub target_token_id: String,
    pub relation: DependencyType,
    pub min_profit_bps: u32,
}

#[derive(Debug, Clone)]
pub struct CombinatorialArbConfig {
    /// List of known dependencies to monitor
    pub dependencies: Vec<Dependency>,
    /// Maximum position size (in USDC) per trade leg
    pub max_leg_size_usdc: Decimal,
    /// Minimum profit threshold in basis points to trigger execution
    pub min_profit_threshold_bps: u32,
}

impl Default for CombinatorialArbConfig {
    fn default() -> Self {
        Self {
            dependencies: vec![],
            max_leg_size_usdc: dec!(100.0),
            min_profit_threshold_bps: 50, // 0.5%
        }
    }
}

pub struct CombinatorialArbStrategy {
    config: CombinatorialArbConfig,
    /// Active dependencies indexed for fast lookup
    dependency_map: Vec<Dependency>,
    /// Optional market/token price snapshot for multi-market evaluation.
    price_snapshot: HashMap<(String, String), Decimal>,
    active: bool,
}

impl CombinatorialArbStrategy {
    pub fn new(config: CombinatorialArbConfig) -> Self {
        Self {
            dependency_map: config.dependencies.clone(),
            config,
            price_snapshot: HashMap::new(),
            active: false,
        }
    }

    pub fn set_price_snapshot(&mut self, snapshot: HashMap<(String, String), Decimal>) {
        self.price_snapshot = snapshot;
    }

    fn snapshot_price(&self, market_id: &str, token_id: &str) -> Option<Decimal> {
        self.price_snapshot
            .get(&(market_id.to_string(), token_id.to_string()))
            .copied()
    }

    fn check_implication_arb(
        &self,
        dep: &Dependency,
        source_price: Decimal,
        target_price: Decimal,
    ) -> Option<Vec<StrategyAction>> {
        // Implication: Source IMPLIES Target.
        // Logical constraint: Prob(Source) <= Prob(Target).
        // Arb opportunity: Price(Source) > Price(Target).
        // Action: Sell Source (overpriced), Buy Target (underpriced).

        if source_price > target_price {
            let diff = source_price - target_price;
            let profit_bps = (diff * dec!(10000)).to_u32().unwrap_or(0);

            if profit_bps >= dep.min_profit_bps.max(self.config.min_profit_threshold_bps) {
                info!(
                    "Combinatorial Arb (Implication): {} > {} (diff: {} bps)",
                    dep.source_token_id, dep.target_token_id, profit_bps
                );

                let mut actions = Vec::new();
                let size_shares = 10; // Placeholder: Calculate based on max_leg_size_usdc

                // 1. Sell Source (Short)
                actions.push(StrategyAction::PlaceOrder {
                    side: Side::Sell,
                    kind: OrderKind::Market { size_shares },
                    order_type: OrderType::GoodForDay,
                    reason: OrderReason::Signal,
                });

                // 2. Buy Target (Long)
                actions.push(StrategyAction::PlaceOrder {
                    side: Side::Buy,
                    kind: OrderKind::Market { size_shares },
                    order_type: OrderType::GoodForDay,
                    reason: OrderReason::Signal,
                });

                return Some(actions);
            }
        }
        None
    }
}

impl Strategy for CombinatorialArbStrategy {
    fn name(&self) -> &str {
        "combinatorial_arb"
    }

    fn on_update(&mut self, _ctx: &StrategyContext) -> Vec<StrategyAction> {
        if !self.active {
            return vec![];
        }

        let mut actions = Vec::new();

        // Iterate over all dependencies
        for dep in &self.dependency_map {
            // For now, we assume the context contains data for the current market being processed.
            // In a real multi-market strategy, `ctx` needs to provide access to ALL markets.
            // This is a limitation of the current `StrategyContext` which is per-market.
            // TODO: Refactor StrategyContext to support multi-market view.

            let has_snapshot = !self.price_snapshot.is_empty();
            let source_price = match self.snapshot_price(&dep.source_market_id, &dep.source_token_id)
            {
                Some(price) => price,
                None if has_snapshot => {
                    warn!(
                        "Missing source price for market {} token {} in snapshot",
                        dep.source_market_id, dep.source_token_id
                    );
                    continue;
                }
                None => dec!(0.60), // Mock fallback
            };
            let target_price = match self.snapshot_price(&dep.target_market_id, &dep.target_token_id)
            {
                Some(price) => price,
                None if has_snapshot => {
                    warn!(
                        "Missing target price for market {} token {} in snapshot",
                        dep.target_market_id, dep.target_token_id
                    );
                    continue;
                }
                None => dec!(0.55), // Mock fallback
            };

            match dep.relation {
                DependencyType::Implication => {
                    if let Some(arb_actions) = self.check_implication_arb(dep, source_price, target_price) {
                        actions.extend(arb_actions);
                    }
                }
                _ => {} // Implement other types
            }
        }

        actions
    }

    fn on_fill(&mut self, _ctx: &StrategyContext, _side: Side, _tick: Tick, _size: Size) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use mtrader_risk::{PnLSnapshot, Position};

    fn mock_context() -> StrategyContext {
        StrategyContext {
            now_ns: 0,
            asset_id: "asset".to_string(),
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
            best_bid: None,
            best_ask: None,
            best_bid_size: 0,
            best_ask_size: 0,
            mid_tick: None,
            spread_ticks: None,
            our_bids: vec![],
            our_asks: vec![],
        }
    }

    #[test]
    fn test_implication_arb_detection() {
        let dep = Dependency {
            source_market_id: "m1".to_string(),
            source_token_id: "t1".to_string(),
            target_market_id: "m2".to_string(),
            target_token_id: "t2".to_string(),
            relation: DependencyType::Implication,
            min_profit_bps: 10,
        };

        let config = CombinatorialArbConfig {
            dependencies: vec![dep.clone()],
            ..Default::default()
        };

        let strategy = CombinatorialArbStrategy::new(config);

        // Case 1: No Arb (Source < Target)
        let actions = strategy.check_implication_arb(&dep, dec!(0.40), dec!(0.50));
        assert!(actions.is_none());

        // Case 2: Arb Exists (Source > Target by 5%)
        let actions = strategy.check_implication_arb(&dep, dec!(0.55), dec!(0.50));
        assert!(actions.is_some());
        let actions = actions.unwrap();
        assert_eq!(actions.len(), 2); // Sell Source, Buy Target
    }

    #[test]
    fn test_on_update_with_snapshot_implication_actions() {
        let deps = vec![
            Dependency {
                source_market_id: "m1".to_string(),
                source_token_id: "t1".to_string(),
                target_market_id: "m2".to_string(),
                target_token_id: "t2".to_string(),
                relation: DependencyType::Implication,
                min_profit_bps: 10,
            },
            Dependency {
                source_market_id: "m3".to_string(),
                source_token_id: "t3".to_string(),
                target_market_id: "m4".to_string(),
                target_token_id: "t4".to_string(),
                relation: DependencyType::Implication,
                min_profit_bps: 10,
            },
        ];

        let config = CombinatorialArbConfig {
            dependencies: deps.clone(),
            ..Default::default()
        };
        let mut strategy = CombinatorialArbStrategy::new(config);
        strategy.activate();

        let mut snapshot = HashMap::new();
        snapshot.insert(("m1".to_string(), "t1".to_string()), dec!(0.70));
        snapshot.insert(("m2".to_string(), "t2".to_string()), dec!(0.60));
        snapshot.insert(("m3".to_string(), "t3".to_string()), dec!(0.65));
        snapshot.insert(("m4".to_string(), "t4".to_string()), dec!(0.55));
        strategy.set_price_snapshot(snapshot);

        let actions = strategy.on_update(&mock_context());
        assert_eq!(actions.len(), 4);

        let mut sides = actions.iter().filter_map(|action| {
            if let StrategyAction::PlaceOrder { side, .. } = action {
                Some(*side)
            } else {
                None
            }
        });

        assert_eq!(sides.next(), Some(Side::Sell));
        assert_eq!(sides.next(), Some(Side::Buy));
        assert_eq!(sides.next(), Some(Side::Sell));
        assert_eq!(sides.next(), Some(Side::Buy));
    }

    #[test]
    fn test_on_update_with_snapshot_respects_implication() {
        let deps = vec![Dependency {
            source_market_id: "m1".to_string(),
            source_token_id: "t1".to_string(),
            target_market_id: "m2".to_string(),
            target_token_id: "t2".to_string(),
            relation: DependencyType::Implication,
            min_profit_bps: 10,
        }];

        let config = CombinatorialArbConfig {
            dependencies: deps,
            ..Default::default()
        };
        let mut strategy = CombinatorialArbStrategy::new(config);
        strategy.activate();

        let mut snapshot = HashMap::new();
        snapshot.insert(("m1".to_string(), "t1".to_string()), dec!(0.45));
        snapshot.insert(("m2".to_string(), "t2".to_string()), dec!(0.55));
        strategy.set_price_snapshot(snapshot);

        let actions = strategy.on_update(&mock_context());
        assert!(actions.is_empty());
    }
}
