//! Single-Condition Market Rebalancing Arbitrage Strategy
//!
//! Based on arXiv:2508.03474 research findings:
//! - When YES + NO prices ≠ $1, arbitrage opportunity exists
//! - Long: sum < $1 → buy both, profit = 1 - sum
//! - Short: sum > $1 → sell both, profit = sum - 1
//!
//! Key implementation details:
//! - Monitor price sum via API scanning
//! - Trigger when sum deviates > 2% from $1
//! - Non-atomic execution (partial fill risk)
//! - Dry-run mode for paper trading

use std::error::Error;
use chrono::{DateTime, Utc};
use mtrader_core::Size;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn, error};

// Re-export from research crate for convenience
pub use mtrader_research::Market;
pub use mtrader_research::MarketToken;
pub use mtrader_research::fetch_active_markets;

/// Arbitrage opportunity detected
#[derive(Debug, Clone)]
pub struct RebalancingArbOpportunity {
    pub market_slug: String,
    pub condition_id: String,
    pub yes_price: f64,
    pub no_price: f64,
    pub price_sum: f64,
    pub deviation_from_one: f64,  // |sum - 1|
    pub arb_type: ArbType,
    pub profit_per_dollar: f64,
    pub estimated_profit_usd: f64,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ArbType {
    Long,   // YES + NO < $1, buy both
    Short,  // YES + NO > $1, sell both
}

/// Configuration for rebalancing arbitrage
#[derive(Debug, Clone)]
pub struct RebalancingArbConfig {
    /// Minimum deviation from $1 to trigger (e.g., 0.02 = 2%)
    pub trigger_threshold: f64,
    /// Minimum profit per dollar to act on
    pub min_profit_per_dollar: f64,
    /// Maximum position size in USD
    pub max_position_usd: f64,
    /// Slippage tolerance (%)
    pub slippage_tolerance: f64,
    /// Whether to actually execute (paper vs live)
    pub dry_run: bool,
}

impl Default for RebalancingArbConfig {
    fn default() -> Self {
        Self {
            trigger_threshold: 0.02,      // 2% deviation
            min_profit_per_dollar: 0.01,  // 1% minimum profit
            max_position_usd: 100.0,      // Conservative sizing
            slippage_tolerance: 0.005,    // 0.5% slippage
            dry_run: true,                // Paper trading by default
        }
    }
}

/// State for rebalancing arbitrage
#[derive(Debug, Clone)]
pub struct RebalancingArbState {
    pub config: RebalancingArbConfig,
    pub opportunities: Vec<RebalancingArbOpportunity>,
    pub executed_trades: Vec<ExecutedArbTrade>,
    pub total_profit_usd: f64,
    pub total_long_profit_usd: f64,
    pub total_short_profit_usd: f64,
    pub last_check: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct ExecutedArbTrade {
    pub opportunity: RebalancingArbOpportunity,
    pub yes_filled: Size,
    pub no_filled: Size,
    pub yes_avg_price: f64,
    pub no_avg_price: f64,
    pub realized_profit: f64,
    pub executed_at: DateTime<Utc>,
}

impl Default for RebalancingArbState {
    fn default() -> Self {
        Self {
            config: RebalancingArbConfig::default(),
            opportunities: Vec::new(),
            executed_trades: Vec::new(),
            total_profit_usd: 0.0,
            total_long_profit_usd: 0.0,
            total_short_profit_usd: 0.0,
            last_check: Utc::now(),
        }
    }
}

/// Market rebalancing arbitrage strategy (API-based, no gateway dependency)
pub struct RebalancingArbStrategy {
    state: Arc<RwLock<RebalancingArbState>>,
}

impl RebalancingArbStrategy {
    /// Create new strategy instance
    pub fn new(config: Option<RebalancingArbConfig>) -> Self {
        let state = Arc::new(RwLock::new(RebalancingArbState {
            config: config.unwrap_or_default(),
            ..Default::default()
        }));

        Self { state }
    }

    /// Check for arbitrage opportunities in a market
    pub async fn check_market_arb(&self, market: &Market) -> Option<RebalancingArbOpportunity> {
        if market.tokens.len() < 2 {
            return None;
        }

        let yes_price = market.tokens.iter()
            .find(|t| t.outcome.to_lowercase() == "yes")
            .map(|t| t.price)?;

        let no_price = market.tokens.iter()
            .find(|t| t.outcome.to_lowercase() == "no")
            .map(|t| t.price)?;

        let price_sum = yes_price + no_price;
        let deviation = (price_sum - 1.0).abs();

        let config = {
            let state = self.state.read().await;
            state.config.clone()
        };

        // Check if deviation exceeds threshold
        if deviation < config.trigger_threshold {
            return None;
        }

        let arb_type = if price_sum < 1.0 {
            ArbType::Long
        } else {
            ArbType::Short
        };

        let profit_per_dollar = deviation;
        let estimated_profit = deviation * config.max_position_usd;

        // Check minimum profit threshold
        if profit_per_dollar < config.min_profit_per_dollar {
            return None;
        }

        Some(RebalancingArbOpportunity {
            market_slug: market.market_slug.clone(),
            condition_id: market.condition_id.clone(),
            yes_price,
            no_price,
            price_sum,
            deviation_from_one: deviation,
            arb_type,
            profit_per_dollar,
            estimated_profit_usd: estimated_profit,
            timestamp: Utc::now(),
        })
    }

    /// Execute arbitrage trade (or simulate in dry-run mode)
    pub async fn execute_arb(
        &self,
        opportunity: &RebalancingArbOpportunity,
    ) -> Result<ExecutedArbTrade, Box<dyn Error>> {
        let config = {
            let state = self.state.read().await;
            state.config.clone()
        };

        if config.dry_run {
            // Simulate execution
            info!("[DRY RUN] Rebalancing arbitrage opportunity detected:");
            info!("  Market: {}", opportunity.market_slug);
            info!("  YES price: {:.4}, NO price: {:.4}", opportunity.yes_price, opportunity.no_price);
            info!("  Sum: {:.4}, Deviation: {:.2}%", opportunity.price_sum, opportunity.deviation_from_one * 100.0);
            info!("  Type: {:?}", opportunity.arb_type);
            info!("  Estimated profit: ${:.2}", opportunity.estimated_profit_usd);

            let yes_size = (config.max_position_usd * opportunity.yes_price * 1_000_000.0) as u64;
            let no_size = (config.max_position_usd * opportunity.no_price * 1_000_000.0) as u64;

            let simulated_trade = ExecutedArbTrade {
                opportunity: opportunity.clone(),
                yes_filled: yes_size,
                no_filled: no_size,
                yes_avg_price: opportunity.yes_price,
                no_avg_price: opportunity.no_price,
                realized_profit: opportunity.estimated_profit_usd,
                executed_at: Utc::now(),
            };

            // Update state
            let mut state = self.state.write().await;
            state.opportunities.push(opportunity.clone());
            state.executed_trades.push(simulated_trade.clone());
            state.total_profit_usd += simulated_trade.realized_profit;

            match opportunity.arb_type {
                ArbType::Long => state.total_long_profit_usd += simulated_trade.realized_profit,
                ArbType::Short => state.total_short_profit_usd += simulated_trade.realized_profit,
            }

            Ok(simulated_trade)
        } else {
            // Real execution - place orders
            Err("Real execution not yet implemented".into())
        }
    }

    /// Scan all active markets for arbitrage opportunities
    pub async fn scan_for_opportunities(&self) -> Vec<RebalancingArbOpportunity> {
        let mut opportunities = Vec::new();

        // Fetch active markets
        match fetch_active_markets().await {
            Ok(markets) => {
                for market in markets {
                    if let Some(arb) = self.check_market_arb(&market).await {
                        opportunities.push(arb);
                    }
                }
            }
            Err(e) => {
                error!("Failed to fetch active markets: {}", e);
            }
        }

        // Update last check time
        let mut state = self.state.write().await;
        state.last_check = Utc::now();

        opportunities
    }

    /// Get current strategy state
    pub async fn get_state(&self) -> RebalancingArbState {
        self.state.read().await.clone()
    }

    /// Get summary statistics
    pub async fn get_summary(&self) -> ArbSummary {
        let state = self.state.read().await.clone();

        ArbSummary {
            total_opportunities: state.opportunities.len(),
            total_executed: state.executed_trades.len(),
            total_profit_usd: state.total_profit_usd,
            long_profit_usd: state.total_long_profit_usd,
            short_profit_usd: state.total_short_profit_usd,
            avg_profit_per_trade: if !state.executed_trades.is_empty() {
                state.total_profit_usd / state.executed_trades.len() as f64
            } else {
                0.0
            },
            last_check: state.last_check,
        }
    }
}

/// Summary statistics for rebalancing arbitrage
#[derive(Debug, Clone)]
pub struct ArbSummary {
    pub total_opportunities: usize,
    pub total_executed: usize,
    pub total_profit_usd: f64,
    pub long_profit_usd: f64,
    pub short_profit_usd: f64,
    pub avg_profit_per_trade: f64,
    pub last_check: DateTime<Utc>,
}

impl std::fmt::Display for ArbSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "=== Rebalancing Arbitrage Summary ===\n")?;
        write!(f, "Total opportunities detected: {}\n", self.total_opportunities)?;
        write!(f, "Total executed trades: {}\n", self.total_executed)?;
        write!(f, "Total profit: ${:.2}\n", self.total_profit_usd)?;
        write!(f, "  - Long arbitrage: ${:.2}\n", self.long_profit_usd)?;
        write!(f, "  - Short arbitrage: ${:.2}\n", self.short_profit_usd)?;
        write!(f, "Avg profit per trade: ${:.2}\n", self.avg_profit_per_trade)?;
        write!(f, "Last check: {}\n", self.last_check)?;
        Ok(())
    }
}

/// Run arbitrage scanner loop
pub async fn run_arb_scanner(
    strategy: &RebalancingArbStrategy,
    interval_secs: u64,
) {
    let interval = tokio::time::Duration::from_secs(interval_secs);

    loop {
        let opportunities = strategy.scan_for_opportunities().await;

        if !opportunities.is_empty() {
            info!("Found {} arbitrage opportunities", opportunities.len());

            for opp in opportunities {
                match strategy.execute_arb(&opp).await {
                    Ok(trade) => {
                        info!(
                            "Executed {} arbitrage: ${:.2} profit",
                            format!("{:?}", trade.opportunity.arb_type),
                            trade.realized_profit
                        );
                    }
                    Err(e) => {
                        warn!("Failed to execute arbitrage: {}", e);
                    }
                }
            }
        }

        tokio::time::sleep(interval).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_market(yes_price: f64, no_price: f64) -> Market {
        Market {
            condition_id: "test".to_string(),
            question_id: "test".to_string(),
            question: "Test Market".to_string(),
            market_slug: "test-market".to_string(),
            active: true,
            closed: false,
            tokens: vec![
                MarketToken { token_id: "1".to_string(), outcome: "Yes".to_string(), price: yes_price, winner: false },
                MarketToken { token_id: "2".to_string(), outcome: "No".to_string(), price: no_price, winner: false },
            ],
            minimum_order_size: "15".to_string(),
            minimum_tick_size: "0.01".to_string(),
            fee_rate_bps: Some("0".to_string()),
        }
    }

    #[test]
    fn test_long_arb_detection() {
        // YES=0.55, NO=0.40, sum=0.95 < 1.0 → Long arbitrage
        let market = create_test_market(0.55, 0.40);
        let strategy = RebalancingArbStrategy::new(None);
        let opp = tokio::runtime::Runtime::new().unwrap().block_on(strategy.check_market_arb(&market));

        assert!(opp.is_some());
        let opp = opp.unwrap();
        assert_eq!(opp.arb_type, ArbType::Long);
        assert!((opp.price_sum - 0.95).abs() < 0.001);
        assert!((opp.deviation_from_one - 0.05).abs() < 0.001);
    }

    #[test]
    fn test_short_arb_detection() {
        // YES=0.60, NO=0.45, sum=1.05 > 1.0 → Short arbitrage
        let market = create_test_market(0.60, 0.45);
        let strategy = RebalancingArbStrategy::new(None);
        let opp = tokio::runtime::Runtime::new().unwrap().block_on(strategy.check_market_arb(&market));

        assert!(opp.is_some());
        let opp = opp.unwrap();
        assert_eq!(opp.arb_type, ArbType::Short);
        assert!((opp.price_sum - 1.05).abs() < 0.001);
    }

    #[test]
    fn test_no_arb_when_sum_equals_one() {
        // YES=0.50, NO=0.50, sum=1.0 → No arbitrage
        let market = create_test_market(0.50, 0.50);
        let strategy = RebalancingArbStrategy::new(None);
        let opp = tokio::runtime::Runtime::new().unwrap().block_on(strategy.check_market_arb(&market));

        assert!(opp.is_none());
    }

    #[test]
    fn test_no_arb_below_threshold() {
        // YES=0.51, NO=0.48, sum=0.99 < 1.0 but deviation = 1% < 2% threshold
        let market = create_test_market(0.51, 0.48);
        let strategy = RebalancingArbStrategy::new(None);
        let opp = tokio::runtime::Runtime::new().unwrap().block_on(strategy.check_market_arb(&market));

        assert!(opp.is_none());
    }
}
