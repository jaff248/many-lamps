//! Fee model with fixed-point arithmetic for Polymarket 15-minute markets.
//!
//! Polymarket fees follow a parabolic curve peaking at price=0.50:
//! fee = coefficient × price × (1-price) × size
//!
//! Fee table points (per 100 shares, fee_rate_bps=1000):
//! - price=0.10: ~0.5625 (tokens)
//! - price=0.25: ~1.1719
//! - price=0.50: ~1.5625 (peak)
//! - price=0.75: ~1.1719
//! - price=0.90: ~0.5625
//!
//! The coefficient is derived from fee_rate_bps:
//! coefficient = fee_rate_bps / 16000 (i.e., 1000 bps = 0.0625)

use crate::types::{Size, Tick, Side, SIZE_DECIMALS};

/// Fixed-point scale for fee calculations (10^8 for precision)
const FEE_SCALE: u128 = 100_000_000;

/// Fee model for Polymarket markets
#[derive(Debug, Clone)]
pub struct FeeModel {
    /// Fee rate in basis points (e.g., 1000 for 15-min markets)
    fee_rate_bps: u16,
}

impl FeeModel {
    pub fn new(fee_rate_bps: u16) -> Self {
        Self { fee_rate_bps }
    }

    /// Create fee model for 15-minute BTC markets (1000 bps)
    pub fn btc_15min() -> Self {
        Self::new(1000)
    }

    /// Create fee model for fee-free markets
    pub fn no_fee() -> Self {
        Self::new(0)
    }

    /// Calculate fee in micro-shares (same denomination as Size)
    ///
    /// Formula: fee = (fee_rate_bps / 16000) × price × (1 - price) × size
    ///
    /// Using fixed-point to avoid floating-point in hot path:
    /// fee = (fee_rate_bps × tick × (10000 - tick) × size) / (16000 × 10000 × 10000)
    pub fn calculate_fee(&self, price_tick: Tick, size: Size) -> Size {
        if self.fee_rate_bps == 0 || size == 0 {
            return 0;
        }

        // price as fraction of 10000 (tick)
        // (1 - price) as (10000 - tick)
        let tick = price_tick as u128;
        let complement = (10000 - price_tick) as u128;
        let rate = self.fee_rate_bps as u128;
        let sz = size as u128;

        // fee = rate × tick × complement × size / (16000 × 10000^2)
        // To avoid overflow: compute in steps
        // Maximum values: rate=10000, tick=10000, complement=10000, size=10^18
        // rate × tick × complement = 10^12 (fits in u128)
        // × size = 10^30 (fits in u128)
        // / 1.6e12 = 6.25e17 (fits in u64)

        let numerator = rate * tick * complement * sz;
        let denominator = 16000u128 * 10000u128 * 10000u128;

        (numerator / denominator) as Size
    }

    /// Calculate fee as floating point (for display/logging only)
    pub fn calculate_fee_f64(&self, price: f64, size: f64) -> f64 {
        let rate = self.fee_rate_bps as f64 / 16000.0;
        rate * price * (1.0 - price) * size
    }

    /// Calculate effective fee rate as percentage of trade value
    /// For BUY: fee is in tokens, value = price × size
    /// For SELL: fee is in USDC, proceeds = price × size
    pub fn effective_rate_bps(&self, price_tick: Tick) -> u16 {
        if price_tick == 0 || price_tick == 10000 {
            return 0;
        }

        // effective_rate = fee / value = rate × (1 - price)
        // For BUY at price p: pay p USDC per share, fee is rate × p × (1-p) tokens
        //   value of fee in USDC = rate × p × (1-p) tokens × (1-p) USDC/token = rate × p × (1-p)²
        //   effective rate = rate × (1-p)² / p... actually this gets complex
        //
        // Simpler: effective_rate_bps ≈ fee_rate_bps × (1 - price)
        // This is approximate but useful for quick estimates
        let complement = 10000 - price_tick;
        ((self.fee_rate_bps as u32 * complement as u32) / 10000) as u16
    }

    /// Calculate expected value for bundle arb (buying both YES and NO)
    /// Returns (raw_edge_bps, total_fee, net_edge) all in micro-units
    pub fn bundle_arb_ev(
        &self,
        ask_yes_tick: Tick,
        ask_no_tick: Tick,
        size: Size,
    ) -> BundleArbEv {
        // Raw edge: 1 - ask_yes - ask_no (in ticks, then convert)
        let sum_asks = ask_yes_tick as i32 + ask_no_tick as i32;
        let raw_edge_ticks = 10000i32 - sum_asks;
        
        // Convert to bps (ticks are already in 10000 scale, so same as bps)
        let raw_edge_bps = raw_edge_ticks as i16;

        // Calculate fees for both legs
        let fee_yes = self.calculate_fee(ask_yes_tick, size);
        let fee_no = self.calculate_fee(ask_no_tick, size);
        let total_fees = fee_yes + fee_no;

        // Raw edge in micro-units: (raw_edge_ticks / 10000) × size
        let raw_edge_micro = if raw_edge_ticks >= 0 {
            ((raw_edge_ticks as u64) * size) / 10000
        } else {
            0 // Negative edge means no profit possible
        };

        // Net edge after fees
        let net_edge_micro = raw_edge_micro.saturating_sub(total_fees);

        // Net edge in bps
        let net_edge_bps = if size > 0 {
            ((net_edge_micro as i64 * 10000) / size as i64) as i16
        } else {
            0
        };

        BundleArbEv {
            raw_edge_bps,
            fee_yes,
            fee_no,
            total_fees,
            raw_edge_micro,
            net_edge_micro,
            net_edge_bps,
        }
    }
}

/// Bundle arb expected value calculation result
#[derive(Debug, Clone, Copy)]
pub struct BundleArbEv {
    /// Raw edge in basis points (before fees)
    pub raw_edge_bps: i16,
    /// Fee for buying YES (in micro-units)
    pub fee_yes: Size,
    /// Fee for buying NO (in micro-units)
    pub fee_no: Size,
    /// Total fees (in micro-units)
    pub total_fees: Size,
    /// Raw edge (in micro-units)
    pub raw_edge_micro: Size,
    /// Net edge after fees (in micro-units)
    pub net_edge_micro: Size,
    /// Net edge in basis points
    pub net_edge_bps: i16,
}

impl BundleArbEv {
    /// Check if arb is profitable after fees
    pub fn is_profitable(&self) -> bool {
        self.net_edge_micro > 0
    }

    /// Minimum required edge in bps to be profitable
    pub fn min_edge_for_profit(&self, size: Size) -> i16 {
        if size == 0 {
            return 0;
        }
        ((self.total_fees as i64 * 10000) / size as i64) as i16
    }
}

/// Rebate ledger for tracking maker rebates
/// Rebates are paid daily, not per-trade
#[derive(Debug, Clone, Default)]
pub struct RebateLedger {
    /// Executed maker volume by (market_id, token_id, strategy_id)
    maker_volume: std::collections::HashMap<(String, String, String), Size>,
    /// Daily payout records
    payouts: Vec<RebatePayout>,
}

#[derive(Debug, Clone)]
pub struct RebatePayout {
    pub date: String,
    pub amount_micro: Size,
    pub attributed_to: std::collections::HashMap<String, Size>,
}

impl RebateLedger {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a maker fill
    pub fn record_maker_fill(&mut self, market_id: &str, token_id: &str, strategy_id: &str, size: Size) {
        let key = (market_id.to_string(), token_id.to_string(), strategy_id.to_string());
        *self.maker_volume.entry(key).or_default() += size;
    }

    /// Get total maker volume for a strategy
    pub fn strategy_volume(&self, strategy_id: &str) -> Size {
        self.maker_volume
            .iter()
            .filter(|((_, _, s), _)| s == strategy_id)
            .map(|(_, v)| *v)
            .sum()
    }

    /// Record daily payout and attribute proportionally
    pub fn record_payout(&mut self, date: &str, amount_micro: Size) {
        let total_volume: Size = self.maker_volume.values().sum();
        if total_volume == 0 {
            return;
        }

        let mut attributed = std::collections::HashMap::new();
        for ((_, _, strategy), vol) in &self.maker_volume {
            let share = ((*vol as u128) * (amount_micro as u128) / (total_volume as u128)) as Size;
            *attributed.entry(strategy.clone()).or_default() += share;
        }

        self.payouts.push(RebatePayout {
            date: date.to_string(),
            amount_micro,
            attributed_to: attributed,
        });

        // Reset volume for next period
        self.maker_volume.clear();
    }

    /// Get total rebates attributed to a strategy
    pub fn strategy_rebates(&self, strategy_id: &str) -> Size {
        self.payouts
            .iter()
            .flat_map(|p| p.attributed_to.get(strategy_id))
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fee_at_50_percent() {
        let model = FeeModel::btc_15min();
        // At price 0.50, fee should be 0.0625 × 0.50 × 0.50 × size = 0.015625 × size
        // For 100 shares (100_000_000 micro): fee = 1_562_500 micro = 1.5625 shares
        let fee = model.calculate_fee(5000, 100_000_000);
        assert_eq!(fee, 1_562_500);
    }

    #[test]
    fn test_fee_at_10_percent() {
        let model = FeeModel::btc_15min();
        // At price 0.10, fee = 0.0625 × 0.10 × 0.90 × size = 0.005625 × size
        // For 100 shares: fee = 562_500 micro = 0.5625 shares
        let fee = model.calculate_fee(1000, 100_000_000);
        assert_eq!(fee, 562_500);
    }

    #[test]
    fn test_fee_at_90_percent() {
        let model = FeeModel::btc_15min();
        // At price 0.90, fee = 0.0625 × 0.90 × 0.10 × size = 0.005625 × size
        // Symmetric with 10%
        let fee = model.calculate_fee(9000, 100_000_000);
        assert_eq!(fee, 562_500);
    }

    #[test]
    fn test_fee_at_25_percent() {
        let model = FeeModel::btc_15min();
        // At price 0.25, fee = 0.0625 × 0.25 × 0.75 × size = 0.01171875 × size
        // For 100 shares: fee = 1_171_875 micro = 1.171875 shares
        let fee = model.calculate_fee(2500, 100_000_000);
        assert_eq!(fee, 1_171_875);
    }

    #[test]
    fn test_fee_symmetry() {
        let model = FeeModel::btc_15min();
        let size = 100_000_000u64;
        
        // Fee should be symmetric around 0.50
        assert_eq!(model.calculate_fee(1000, size), model.calculate_fee(9000, size));
        assert_eq!(model.calculate_fee(2000, size), model.calculate_fee(8000, size));
        assert_eq!(model.calculate_fee(3000, size), model.calculate_fee(7000, size));
        assert_eq!(model.calculate_fee(4000, size), model.calculate_fee(6000, size));
    }

    #[test]
    fn test_fee_zero_at_extremes() {
        let model = FeeModel::btc_15min();
        // Fee should be 0 at price 0 and price 1
        assert_eq!(model.calculate_fee(0, 100_000_000), 0);
        assert_eq!(model.calculate_fee(10000, 100_000_000), 0);
    }

    #[test]
    fn test_no_fee_model() {
        let model = FeeModel::no_fee();
        assert_eq!(model.calculate_fee(5000, 100_000_000), 0);
    }

    #[test]
    fn test_bundle_arb_ev_profitable() {
        let model = FeeModel::btc_15min();
        // ask_yes = 0.45, ask_no = 0.45 → sum = 0.90 → raw edge = 0.10 (1000 bps)
        let ev = model.bundle_arb_ev(4500, 4500, 100_000_000);
        
        assert_eq!(ev.raw_edge_bps, 1000);
        assert!(ev.is_profitable());
        assert!(ev.net_edge_bps > 0);
    }

    #[test]
    fn test_bundle_arb_ev_marginal() {
        let model = FeeModel::btc_15min();
        // ask_yes = 0.49, ask_no = 0.49 → sum = 0.98 → raw edge = 0.02 (200 bps)
        let ev = model.bundle_arb_ev(4900, 4900, 100_000_000);
        
        assert_eq!(ev.raw_edge_bps, 200);
        // Fees at 0.49: 0.0625 × 0.49 × 0.51 × size = 0.01561875 × size per leg
        // Total fees ≈ 0.03125 × size = 3_125_000 micro
        // Raw edge = 0.02 × size = 2_000_000 micro
        // This should NOT be profitable
        assert!(!ev.is_profitable());
    }

    #[test]
    fn test_bundle_arb_ev_not_profitable() {
        let model = FeeModel::btc_15min();
        // ask_yes = 0.50, ask_no = 0.50 → sum = 1.00 → raw edge = 0
        let ev = model.bundle_arb_ev(5000, 5000, 100_000_000);
        
        assert_eq!(ev.raw_edge_bps, 0);
        assert!(!ev.is_profitable());
    }

    #[test]
    fn test_bundle_arb_ev_negative_edge() {
        let model = FeeModel::btc_15min();
        // ask_yes = 0.55, ask_no = 0.50 → sum = 1.05 → raw edge = -0.05 (-500 bps)
        let ev = model.bundle_arb_ev(5500, 5000, 100_000_000);
        
        assert_eq!(ev.raw_edge_bps, -500);
        assert!(!ev.is_profitable());
    }

    #[test]
    fn test_rebate_ledger() {
        let mut ledger = RebateLedger::new();
        
        ledger.record_maker_fill("market1", "token1", "strategy_a", 100_000_000);
        ledger.record_maker_fill("market1", "token1", "strategy_b", 200_000_000);
        
        assert_eq!(ledger.strategy_volume("strategy_a"), 100_000_000);
        assert_eq!(ledger.strategy_volume("strategy_b"), 200_000_000);
        
        // Record payout of 3_000_000 micro
        ledger.record_payout("2026-01-06", 3_000_000);
        
        // strategy_a gets 1/3, strategy_b gets 2/3
        assert_eq!(ledger.strategy_rebates("strategy_a"), 1_000_000);
        assert_eq!(ledger.strategy_rebates("strategy_b"), 2_000_000);
    }

    // Test against documented fee table points
    #[test]
    fn test_fee_table_verification() {
        let model = FeeModel::btc_15min();
        let size = 100_000_000u64; // 100 shares
        
        // These are the expected values based on the parabolic formula
        // fee = 0.0625 × price × (1-price) × 100
        struct TestPoint {
            price_tick: Tick,
            expected_fee_shares: f64,
            tolerance: f64,
        }

        let test_points = [
            TestPoint { price_tick: 1000, expected_fee_shares: 0.5625, tolerance: 0.01 },
            TestPoint { price_tick: 2500, expected_fee_shares: 1.1719, tolerance: 0.01 },
            TestPoint { price_tick: 5000, expected_fee_shares: 1.5625, tolerance: 0.01 },
            TestPoint { price_tick: 7500, expected_fee_shares: 1.1719, tolerance: 0.01 },
            TestPoint { price_tick: 9000, expected_fee_shares: 0.5625, tolerance: 0.01 },
        ];

        for tp in test_points {
            let fee = model.calculate_fee(tp.price_tick, size);
            let fee_shares = fee as f64 / 1_000_000.0;
            let diff = (fee_shares - tp.expected_fee_shares).abs();
            assert!(
                diff < tp.tolerance,
                "Fee at tick {} expected {:.3} shares, got {:.3} (diff: {:.4})",
                tp.price_tick,
                tp.expected_fee_shares,
                fee_shares,
                diff
            );
        }
    }
}
