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
use mtrader_core::{Side, Size};
use mtrader_execution::{OrderKind, OrderType};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use rust_decimal_macros::dec;
use tracing::info;

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
                    reason: format!("CombArb: Sell Overpriced Source {}", dep.source_token_id),
                });

                // 2. Buy Target (Long)
                actions.push(StrategyAction::PlaceOrder {
                    side: Side::Buy,
                    kind: OrderKind::Market { size_shares },
                    order_type: OrderType::GoodForDay,
                    reason: format!("CombArb: Buy Underpriced Target {}", dep.target_token_id),
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
            return Vec::new();
        }

        let mut actions = Vec::new();

        // Iterate over all dependencies
        for dep in &self.dependency_map {
            // For now, we assume the context contains data for the current market being processed.
            // In a real multi-market strategy, `ctx` needs to provide access to ALL markets.
            // This is a limitation of the current `StrategyContext` which is per-market.
            // TODO: Refactor StrategyContext to support multi-market view.
            
            // Placeholder logic assuming ctx has prices we need (which it currently doesn't fully support)
            let source_price = dec!(0.60); // Mock
            let target_price = dec!(0.55); // Mock

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

#[cfg(test)]
mod tests {
    use super::*;
    use mtrader_book::ArrayBook;
    use mtrader_risk::{PnLSnapshot, Position};

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
}
