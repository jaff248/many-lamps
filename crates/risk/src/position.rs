//! Position tracking.

use mtrader_core::{Side, Size, Tick};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A position in a single asset.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Position {
    /// Net position in micro-shares (positive = long, negative = short)
    pub net_size: i64,
    /// Volume-weighted average entry price (in ticks)
    pub avg_entry_tick: Option<Tick>,
    /// Total bought volume
    pub total_bought: Size,
    /// Total sold volume
    pub total_sold: Size,
    /// Realized PnL in micro-USDC
    pub realized_pnl_micro_usdc: i64,
}

impl Position {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a fill.
    pub fn on_fill(&mut self, side: Side, price_tick: Tick, size: Size) {
        let signed_size = match side {
            Side::Buy => size as i64,
            Side::Sell => -(size as i64),
        };

        // Update totals
        match side {
            Side::Buy => self.total_bought += size,
            Side::Sell => self.total_sold += size,
        }

        // Calculate PnL and update position
        if self.net_size == 0 {
            // Opening a new position
            self.net_size = signed_size;
            self.avg_entry_tick = Some(price_tick);
        } else if (self.net_size > 0 && signed_size > 0) || (self.net_size < 0 && signed_size < 0) {
            // Adding to existing position
            let old_size = self.net_size.unsigned_abs();
            let new_size = size;
            let total_size = old_size + new_size;

            // Update average entry price
            if let Some(old_avg) = self.avg_entry_tick {
                let weighted_avg =
                    (old_avg as u64 * old_size + price_tick as u64 * new_size) / total_size;
                self.avg_entry_tick = Some(weighted_avg as Tick);
            }

            self.net_size += signed_size;
        } else {
            // Reducing or flipping position
            let closing_size = size.min(self.net_size.unsigned_abs());

            // Calculate realized PnL
            if let Some(entry) = self.avg_entry_tick {
                let pnl_per_share = if self.net_size > 0 {
                    // Was long, now selling
                    price_tick as i64 - entry as i64
                } else {
                    // Was short, now buying
                    entry as i64 - price_tick as i64
                };
                // PnL in micro-USDC: (tick_diff / 10000) * micro-shares
                self.realized_pnl_micro_usdc +=
                    (pnl_per_share as i128 * closing_size as i128 / 10000) as i64;
            }

            // Update position
            self.net_size += signed_size;

            // If position flipped, reset average entry
            if (self.net_size > 0 && signed_size > 0) || (self.net_size < 0 && signed_size < 0) {
                self.avg_entry_tick = Some(price_tick);
            } else if self.net_size == 0 {
                self.avg_entry_tick = None;
            }
        }
    }

    /// Calculate unrealized PnL at current price.
    pub fn unrealized_pnl_micro_usdc(&self, current_tick: Tick) -> i64 {
        if self.net_size == 0 {
            return 0;
        }

        let entry = match self.avg_entry_tick {
            Some(t) => t,
            None => return 0,
        };

        let pnl_per_share = if self.net_size > 0 {
            current_tick as i64 - entry as i64
        } else {
            entry as i64 - current_tick as i64
        };

        (pnl_per_share as i128 * self.net_size.abs() as i128 / 10000) as i64
    }

    /// Check if position is long.
    pub fn is_long(&self) -> bool {
        self.net_size > 0
    }

    /// Check if position is short.
    pub fn is_short(&self) -> bool {
        self.net_size < 0
    }

    /// Check if position is flat.
    pub fn is_flat(&self) -> bool {
        self.net_size == 0
    }
}

/// Tracks positions across multiple assets.
pub struct PositionTracker {
    positions: HashMap<String, Position>,
}

impl PositionTracker {
    pub fn new() -> Self {
        Self {
            positions: HashMap::new(),
        }
    }

    /// Get or create position for an asset.
    pub fn get_or_create(&mut self, asset_id: &str) -> &mut Position {
        self.positions
            .entry(asset_id.to_string())
            .or_insert_with(Position::new)
    }

    /// Get position for an asset (if exists).
    pub fn get(&self, asset_id: &str) -> Option<&Position> {
        self.positions.get(asset_id)
    }

    /// Record a fill.
    pub fn on_fill(&mut self, asset_id: &str, side: Side, price_tick: Tick, size: Size) {
        let position = self.get_or_create(asset_id);
        position.on_fill(side, price_tick, size);
    }

    /// Calculate total unrealized PnL across all positions.
    pub fn total_unrealized_pnl(&self, prices: &HashMap<String, Tick>) -> i64 {
        self.positions
            .iter()
            .filter_map(|(asset_id, pos)| {
                prices
                    .get(asset_id)
                    .map(|&price| pos.unrealized_pnl_micro_usdc(price))
            })
            .sum()
    }

    /// Calculate total realized PnL.
    pub fn total_realized_pnl(&self) -> i64 {
        self.positions
            .values()
            .map(|p| p.realized_pnl_micro_usdc)
            .sum()
    }

    /// Get all positions.
    pub fn all_positions(&self) -> impl Iterator<Item = (&str, &Position)> {
        self.positions.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// Get gross position (sum of absolute positions).
    pub fn gross_position(&self) -> i64 {
        self.positions.values().map(|p| p.net_size.abs()).sum()
    }

    /// Get net position (sum of signed positions).
    pub fn net_position(&self) -> i64 {
        self.positions.values().map(|p| p.net_size).sum()
    }
}

impl Default for PositionTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_position_long_open_close() {
        let mut pos = Position::new();

        // Buy 1000 shares at tick 5000 (0.50)
        pos.on_fill(Side::Buy, 5000, 100_000_000);
        assert_eq!(pos.net_size, 100_000_000);
        assert_eq!(pos.avg_entry_tick, Some(5000));

        // Sell 1000 shares at tick 5500 (0.55) - profit!
        pos.on_fill(Side::Sell, 5500, 100_000_000);
        assert_eq!(pos.net_size, 0);
        assert!(pos.is_flat());

        // PnL: 500 ticks * 100_000_000 micro-shares / 10000 = 5_000_000 micro-USDC = $5
        assert_eq!(pos.realized_pnl_micro_usdc, 5_000_000);
    }

    #[test]
    fn test_position_short_open_close() {
        let mut pos = Position::new();

        // Sell 1000 shares at tick 6000 (0.60)
        pos.on_fill(Side::Sell, 6000, 100_000_000);
        assert_eq!(pos.net_size, -100_000_000);
        assert!(pos.is_short());

        // Buy 1000 shares at tick 5500 (0.55) - profit on short!
        pos.on_fill(Side::Buy, 5500, 100_000_000);
        assert!(pos.is_flat());

        // PnL: 500 ticks * 100_000_000 micro-shares / 10000 = 5_000_000 micro-USDC = $5
        assert_eq!(pos.realized_pnl_micro_usdc, 5_000_000);
    }

    #[test]
    fn test_position_add_to_long() {
        let mut pos = Position::new();

        // Buy 500 at 50c
        pos.on_fill(Side::Buy, 5000, 50_000_000);
        // Buy 500 at 52c
        pos.on_fill(Side::Buy, 5200, 50_000_000);

        assert_eq!(pos.net_size, 100_000_000);
        // Average entry: (5000 * 50_000_000 + 5200 * 50_000_000) / 100_000_000 = 5100
        assert_eq!(pos.avg_entry_tick, Some(5100));
    }

    #[test]
    fn test_unrealized_pnl() {
        let mut pos = Position::new();
        pos.on_fill(Side::Buy, 5000, 100_000_000);

        // Price at 55: unrealized profit
        let pnl = pos.unrealized_pnl_micro_usdc(5500);
        assert_eq!(pnl, 5_000_000); // $5 profit

        // Price at 45: unrealized loss
        let pnl = pos.unrealized_pnl_micro_usdc(4500);
        assert_eq!(pnl, -5_000_000); // $5 loss
    }

    #[test]
    fn test_position_tracker() {
        let mut tracker = PositionTracker::new();

        tracker.on_fill("asset-1", Side::Buy, 5000, 100_000_000);
        tracker.on_fill("asset-2", Side::Sell, 6000, 50_000_000);

        assert_eq!(tracker.gross_position(), 150_000_000);
        assert_eq!(tracker.net_position(), 50_000_000); // 100M - 50M
    }
}
