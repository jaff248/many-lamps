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

pub mod config;
pub mod dependency;
pub mod price;
pub mod strategy;
pub mod tests;

// Re-exports for backward compatibility
pub use config::CombinatorialArbConfig;
pub use dependency::{Dependency, DependencyType};
pub use strategy::CombinatorialArbStrategy;
