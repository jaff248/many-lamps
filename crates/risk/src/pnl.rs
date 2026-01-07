//! PnL tracking and calculation.

use crate::position::PositionTracker;
use mtrader_core::Tick;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// PnL snapshot at a point in time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PnLSnapshot {
    /// Timestamp (mono ns)
    pub timestamp_ns: u64,
    /// Realized PnL (micro-USDC)
    pub realized_pnl: i64,
    /// Unrealized PnL (micro-USDC)
    pub unrealized_pnl: i64,
    /// Total PnL (micro-USDC)
    pub total_pnl: i64,
    /// Total fees paid (micro-USDC)
    pub total_fees: i64,
    /// Net PnL after fees (micro-USDC)
    pub net_pnl: i64,
    /// High-water mark (micro-USDC)
    pub high_water_mark: i64,
    /// Current drawdown from high-water mark (micro-USDC)
    pub drawdown: i64,
    /// Drawdown percentage (basis points)
    pub drawdown_bps: i64,
}

/// PnL tracker with drawdown monitoring.
pub struct PnLTracker {
    /// High-water mark for drawdown calculation
    high_water_mark: i64,
    /// Total fees paid
    total_fees: i64,
    /// Starting capital (for drawdown percentage)
    starting_capital: i64,
    /// Historical snapshots
    snapshots: Vec<PnLSnapshot>,
    /// Maximum snapshots to keep
    max_snapshots: usize,
}

impl PnLTracker {
    pub fn new(starting_capital: i64) -> Self {
        Self {
            high_water_mark: 0,
            total_fees: 0,
            starting_capital,
            snapshots: Vec::new(),
            max_snapshots: 10_000,
        }
    }

    /// Record fees paid.
    pub fn add_fees(&mut self, fees: i64) {
        self.total_fees += fees;
    }

    /// Take a PnL snapshot.
    pub fn snapshot(
        &mut self,
        positions: &PositionTracker,
        prices: &HashMap<String, Tick>,
        timestamp_ns: u64,
    ) -> PnLSnapshot {
        let realized_pnl = positions.total_realized_pnl();
        let unrealized_pnl = positions.total_unrealized_pnl(prices);
        let total_pnl = realized_pnl + unrealized_pnl;
        let net_pnl = total_pnl - self.total_fees;

        // Update high-water mark
        if net_pnl > self.high_water_mark {
            self.high_water_mark = net_pnl;
        }

        // Calculate drawdown
        let drawdown = self.high_water_mark - net_pnl;
        let drawdown_bps = if self.starting_capital > 0 {
            (drawdown * 10_000) / self.starting_capital
        } else {
            0
        };

        let snapshot = PnLSnapshot {
            timestamp_ns,
            realized_pnl,
            unrealized_pnl,
            total_pnl,
            total_fees: self.total_fees,
            net_pnl,
            high_water_mark: self.high_water_mark,
            drawdown,
            drawdown_bps,
        };

        // Store snapshot
        if self.snapshots.len() >= self.max_snapshots {
            self.snapshots.remove(0);
        }
        self.snapshots.push(snapshot.clone());

        snapshot
    }

    /// Get the latest snapshot.
    pub fn latest(&self) -> Option<&PnLSnapshot> {
        self.snapshots.last()
    }

    /// Get current drawdown in basis points.
    pub fn current_drawdown_bps(&self) -> i64 {
        self.snapshots.last().map(|s| s.drawdown_bps).unwrap_or(0)
    }

    /// Get all snapshots.
    pub fn history(&self) -> &[PnLSnapshot] {
        &self.snapshots
    }

    /// Calculate Sharpe-like ratio from snapshots (rough approximation).
    pub fn returns_volatility(&self) -> Option<(f64, f64)> {
        if self.snapshots.len() < 2 {
            return None;
        }

        // Calculate returns between snapshots
        let returns: Vec<f64> = self
            .snapshots
            .windows(2)
            .map(|w| (w[1].net_pnl - w[0].net_pnl) as f64)
            .collect();

        if returns.is_empty() {
            return None;
        }

        let mean = returns.iter().sum::<f64>() / returns.len() as f64;
        let variance = returns
            .iter()
            .map(|r| (r - mean).powi(2))
            .sum::<f64>()
            / returns.len() as f64;
        let std_dev = variance.sqrt();

        Some((mean, std_dev))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mtrader_core::Side;

    #[test]
    fn test_pnl_tracking() {
        let mut tracker = PnLTracker::new(1_000_000_000); // $1000 starting
        let mut positions = PositionTracker::new();

        // Open position
        positions.on_fill("asset-1", Side::Buy, 50, 100_000);

        // Take snapshot at profit
        let prices = HashMap::from([("asset-1".to_string(), 55u16)]);
        let snap = tracker.snapshot(&positions, &prices, 1000);

        assert!(snap.unrealized_pnl > 0);
        assert_eq!(snap.realized_pnl, 0);
        assert_eq!(snap.drawdown, 0); // At high water mark

        // Price drops - drawdown
        let prices = HashMap::from([("asset-1".to_string(), 45u16)]);
        let snap = tracker.snapshot(&positions, &prices, 2000);

        assert!(snap.unrealized_pnl < 0);
        assert!(snap.drawdown > 0);
    }

    #[test]
    fn test_fee_tracking() {
        let mut tracker = PnLTracker::new(1_000_000_000);
        let positions = PositionTracker::new();
        let prices = HashMap::new();

        tracker.add_fees(100_000); // $0.10 in fees

        let snap = tracker.snapshot(&positions, &prices, 1000);
        assert_eq!(snap.total_fees, 100_000);
        assert_eq!(snap.net_pnl, -100_000);
    }
}
