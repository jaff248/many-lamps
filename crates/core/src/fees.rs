//! Fee model with fixed-point arithmetic for Polymarket markets.
//!
//! Polymarket fees are based on min(price, 1-price) rather than a parabolic curve.
//! The exchange fee (USDC-equivalent) is:
//! fee = fee_rate_bps × min(price, 1-price) × size
//!       ---------------------------------------
//!                 10_000 × 10_000
//!
//! Fee table points (per 100 shares, fee_rate_bps=1000):
//! - price=0.10: 1.00 USDC
//! - price=0.25: 2.50
//! - price=0.50: 5.00 (peak)
//! - price=0.75: 2.50
//! - price=0.90: 1.00

use crate::types::{Size, Tick};
use serde::{Deserialize, Serialize};

/// Fee model for Polymarket markets
#[derive(Debug, Clone)]
pub struct FeeModel {
    /// Fee rate in basis points (e.g., 1000 for 15-min markets)
    fee_rate_bps: u16,
}

/// Fee schedule for a market.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FeeSchedule {
    /// Parabolic fee curve with specified rate (bps).
    Parabolic { fee_rate_bps: u16 },
    /// Fee-free trading.
    Zero,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FeeScheduleKind {
    Parabolic,
    Zero,
}

impl FeeSchedule {
    pub fn fee_rate_bps(&self) -> u16 {
        match self {
            FeeSchedule::Parabolic { fee_rate_bps } => *fee_rate_bps,
            FeeSchedule::Zero => 0,
        }
    }

    pub fn kind(&self) -> FeeScheduleKind {
        match self {
            FeeSchedule::Parabolic { .. } => FeeScheduleKind::Parabolic,
            FeeSchedule::Zero => FeeScheduleKind::Zero,
        }
    }

    pub fn calculate_fee(&self, price_tick: Tick, size: Size) -> Size {
        match self {
            FeeSchedule::Parabolic { fee_rate_bps } => {
                FeeModel::new(*fee_rate_bps).calculate_fee(price_tick, size)
            }
            FeeSchedule::Zero => 0,
        }
    }
}

/// Fee profile classification for a market.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarketFeeProfile {
    pub label: String,
    pub schedule: FeeSchedule,
}

impl MarketFeeProfile {
    pub fn crypto_15m() -> Self {
        Self {
            label: "crypto_15m".to_string(),
            schedule: FeeSchedule::Parabolic { fee_rate_bps: 1000 },
        }
    }

    pub fn zero(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            schedule: FeeSchedule::Zero,
        }
    }
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

    /// Calculate fee in micro-USDC equivalent.
    ///
    /// Formula: fee = fee_rate_bps × min(price, 1-price) × size / (10_000 × 10_000)
    pub fn calculate_fee(&self, price_tick: Tick, size: Size) -> Size {
        if self.fee_rate_bps == 0 || size == 0 {
            return 0;
        }
        if price_tick == 0 || price_tick == 10000 {
            return 0;
        }

        let min_tick = price_tick.min(10000 - price_tick) as u128;
        let rate = self.fee_rate_bps as u128;
        let sz = size as u128;

        let numerator = rate * min_tick * sz;
        let denominator = 10_000u128 * 10_000u128;

        (numerator / denominator) as Size
    }

    /// Calculate fee as floating point (for display/logging only)
    pub fn calculate_fee_f64(&self, price: f64, size: f64) -> f64 {
        let rate = self.fee_rate_bps as f64 / 10000.0;
        rate * price.min(1.0 - price) * size
    }

    /// Calculate effective fee rate as percentage of trade value
    /// For BUY: fee is in tokens, value = price × size
    /// For SELL: fee is in USDC, proceeds = price × size
    pub fn effective_rate_bps(&self, price_tick: Tick) -> u16 {
        if price_tick == 0 || price_tick == 10000 {
            return 0;
        }

        let price = price_tick as f64 / 10000.0;
        let min_price = price.min(1.0 - price);
        let rate = self.fee_rate_bps as f64 / 10000.0;
        if price == 0.0 {
            return 0;
        }
        (rate * min_price / price * 10000.0).round() as u16
    }

    /// Calculate expected value for bundle arb (buying both YES and NO)
    /// Returns (raw_edge_bps, total_fee, net_edge) all in micro-units
    pub fn bundle_arb_ev(&self, ask_yes_tick: Tick, ask_no_tick: Tick, size: Size) -> BundleArbEv {
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
    pub fn record_maker_fill(
        &mut self,
        market_id: &str,
        token_id: &str,
        strategy_id: &str,
        size: Size,
    ) {
        let key = (
            market_id.to_string(),
            token_id.to_string(),
            strategy_id.to_string(),
        );
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
        // At price 0.50, fee should be 1000 bps × 0.50 × size = 0.05 × size
        // For 100 shares (100_000_000 micro): fee = 5_000_000 micro-USDC = $5.00
        let fee = model.calculate_fee(5000, 100_000_000);
        assert_eq!(fee, 5_000_000);
    }

    #[test]
    fn test_fee_at_10_percent() {
        let model = FeeModel::btc_15min();
        // At price 0.10, fee = 1000 bps × 0.10 × size = 0.01 × size
        // For 100 shares: fee = 1_000_000 micro-USDC = $1.00
        let fee = model.calculate_fee(1000, 100_000_000);
        assert_eq!(fee, 1_000_000);
    }

    #[test]
    fn test_fee_at_90_percent() {
        let model = FeeModel::btc_15min();
        // At price 0.90, fee = 1000 bps × 0.10 × size (symmetric with 10%)
        let fee = model.calculate_fee(9000, 100_000_000);
        assert_eq!(fee, 1_000_000);
    }

    #[test]
    fn test_fee_at_25_percent() {
        let model = FeeModel::btc_15min();
        // At price 0.25, fee = 1000 bps × 0.25 × size = 0.025 × size
        // For 100 shares: fee = 2_500_000 micro-USDC = $2.50
        let fee = model.calculate_fee(2500, 100_000_000);
        assert_eq!(fee, 2_500_000);
    }

    #[test]
    fn test_fee_symmetry() {
        let model = FeeModel::btc_15min();
        let size = 100_000_000u64;

        // Fee should be symmetric around 0.50
        assert_eq!(
            model.calculate_fee(1000, size),
            model.calculate_fee(9000, size)
        );
        assert_eq!(
            model.calculate_fee(2000, size),
            model.calculate_fee(8000, size)
        );
        assert_eq!(
            model.calculate_fee(3000, size),
            model.calculate_fee(7000, size)
        );
        assert_eq!(
            model.calculate_fee(4000, size),
            model.calculate_fee(6000, size)
        );
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
    fn test_fee_schedule_variants() {
        let parabolic = FeeSchedule::Parabolic { fee_rate_bps: 1000 };
        let zero = FeeSchedule::Zero;

        assert_eq!(parabolic.calculate_fee(5000, 100_000_000), 5_000_000);
        assert_eq!(zero.calculate_fee(5000, 100_000_000), 0);
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
        // Fees at 0.49: 1000 bps × 0.49 × size = 0.049 × size per leg
        // Total fees ≈ 0.098 × size
        // Raw edge = 0.02 × size = 2_000_000 micro-USDC
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

    #[test]
    fn test_fee_table_verification() {
        let model = FeeModel::btc_15min();
        let size = 100_000_000u64; // 100 shares

        // These are the expected values based on the min(price, 1-price) formula
        // fee = 1000 bps × min(price, 1-price) × 100
        struct TestPoint {
            price_tick: Tick,
            expected_fee_usdc: f64,
            tolerance: f64,
        }

        let test_points = [
            TestPoint {
                price_tick: 1000,
                expected_fee_usdc: 1.0,
                tolerance: 0.01,
            },
            TestPoint {
                price_tick: 2500,
                expected_fee_usdc: 2.5,
                tolerance: 0.01,
            },
            TestPoint {
                price_tick: 5000,
                expected_fee_usdc: 5.0,
                tolerance: 0.01,
            },
            TestPoint {
                price_tick: 7500,
                expected_fee_usdc: 2.5,
                tolerance: 0.01,
            },
            TestPoint {
                price_tick: 9000,
                expected_fee_usdc: 1.0,
                tolerance: 0.01,
            },
        ];

        for tp in test_points {
            let fee = model.calculate_fee(tp.price_tick, size);
            let fee_usdc = fee as f64 / 1_000_000.0;
            let diff = (fee_usdc - tp.expected_fee_usdc).abs();
            assert!(
                diff < tp.tolerance,
                "Fee at tick {} expected {:.3} USDC, got {:.3} (diff: {:.4})",
                tp.price_tick,
                tp.expected_fee_usdc,
                fee_usdc,
                diff
            );
        }
    }
}
