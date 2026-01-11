use super::dependency::Dependency;

/// List of dependencies type alias
pub type DependencyList = Vec<Dependency>;

/// Configuration for the combinatorial arbitrage strategy
#[derive(Debug, Clone)]
pub struct CombinatorialArbConfig {
    /// List of known dependencies to monitor
    pub dependencies: DependencyList,
    /// Maximum position size (in USDC) per trade leg
    pub max_leg_size_usdc: f64,
    /// Minimum profit threshold in basis points to trigger execution
    pub min_profit_threshold_bps: u32,
}

impl Default for CombinatorialArbConfig {
    fn default() -> Self {
        Self {
            dependencies: vec![],
            max_leg_size_usdc: 100.0,
            min_profit_threshold_bps: 50, // 0.5%
        }
    }
}
