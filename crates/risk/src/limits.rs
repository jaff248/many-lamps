//! Position and order limits.

use serde::{Deserialize, Serialize};

/// Position limit configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PositionLimits {
    /// Maximum position per asset (micro-shares)
    pub max_position_per_asset: i64,
    /// Maximum gross position across all assets (micro-shares)
    pub max_gross_position: i64,
    /// Maximum order size (micro-shares)
    pub max_order_size: u64,
    /// Minimum order size (micro-shares)
    pub min_order_size: u64,
    /// Maximum daily volume (micro-shares)
    pub max_daily_volume: u64,
    /// Maximum open orders per side
    pub max_open_orders_per_side: usize,
}

impl Default for PositionLimits {
    fn default() -> Self {
        Self {
            // 200 shares max position per asset
            max_position_per_asset: 200_000_000, // 200 shares
            // 1000 shares max gross position
            max_gross_position: 1_000_000_000, // 1000 shares
            // 20 shares max single order
            max_order_size: 20_000_000, // 20 shares
            // 0.2 shares min order
            min_order_size: 200_000, // 0.2 shares
            // 2000 shares daily volume limit
            max_daily_volume: 2_000_000_000, // 2000 shares
            // Max 5 open orders per side
            max_open_orders_per_side: 5,
        }
    }
}

/// Result of a limit check.
#[derive(Debug, Clone)]
pub enum LimitCheck {
    Ok,
    PositionLimitExceeded {
        current: i64,
        limit: i64,
        would_be: i64,
    },
    GrossPositionExceeded {
        current: i64,
        limit: i64,
        would_be: i64,
    },
    OrderSizeTooLarge {
        size: u64,
        limit: u64,
    },
    OrderSizeTooSmall {
        size: u64,
        limit: u64,
    },
    DailyVolumeExceeded {
        current: u64,
        limit: u64,
        would_be: u64,
    },
    TooManyOpenOrders {
        current: usize,
        limit: usize,
    },
}

impl LimitCheck {
    pub fn is_ok(&self) -> bool {
        matches!(self, Self::Ok)
    }
}

impl PositionLimits {
    /// Check if an order can be placed.
    pub fn check_order(
        &self,
        order_size: u64,
        current_position: i64,
        is_buy: bool,
        gross_position: i64,
        daily_volume: u64,
        open_orders: usize,
    ) -> LimitCheck {
        // Order size bounds
        if order_size > self.max_order_size {
            return LimitCheck::OrderSizeTooLarge {
                size: order_size,
                limit: self.max_order_size,
            };
        }
        if order_size < self.min_order_size {
            return LimitCheck::OrderSizeTooSmall {
                size: order_size,
                limit: self.min_order_size,
            };
        }

        // Daily volume
        let would_be_volume = daily_volume + order_size;
        if would_be_volume > self.max_daily_volume {
            return LimitCheck::DailyVolumeExceeded {
                current: daily_volume,
                limit: self.max_daily_volume,
                would_be: would_be_volume,
            };
        }

        // Open orders
        if open_orders >= self.max_open_orders_per_side {
            return LimitCheck::TooManyOpenOrders {
                current: open_orders,
                limit: self.max_open_orders_per_side,
            };
        }

        // Position limits
        let position_delta = if is_buy {
            order_size as i64
        } else {
            -(order_size as i64)
        };
        let would_be_position = current_position + position_delta;

        if would_be_position.abs() > self.max_position_per_asset {
            return LimitCheck::PositionLimitExceeded {
                current: current_position,
                limit: self.max_position_per_asset,
                would_be: would_be_position,
            };
        }

        // Gross position
        // Approximate: add order size (worst case)
        let would_be_gross = gross_position + order_size as i64;
        if would_be_gross > self.max_gross_position {
            return LimitCheck::GrossPositionExceeded {
                current: gross_position,
                limit: self.max_gross_position,
                would_be: would_be_gross,
            };
        }

        LimitCheck::Ok
    }

    /// Get the maximum order size that would fit within limits.
    pub fn max_allowed_order_size(
        &self,
        current_position: i64,
        is_buy: bool,
        gross_position: i64,
        daily_volume: u64,
    ) -> u64 {
        let mut max_size = self.max_order_size;

        // Position limit constraint
        let position_headroom = if is_buy {
            (self.max_position_per_asset - current_position).max(0) as u64
        } else {
            (self.max_position_per_asset + current_position).max(0) as u64
        };
        max_size = max_size.min(position_headroom);

        // Gross position constraint
        let gross_headroom = (self.max_gross_position - gross_position).max(0) as u64;
        max_size = max_size.min(gross_headroom);

        // Daily volume constraint
        let volume_headroom = self.max_daily_volume.saturating_sub(daily_volume);
        max_size = max_size.min(volume_headroom);

        max_size
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_order_size_limits() {
        let limits = PositionLimits {
            max_order_size: 100_000,
            min_order_size: 1_000,
            ..Default::default()
        };

        // Too large
        let check = limits.check_order(150_000, 0, true, 0, 0, 0);
        assert!(matches!(check, LimitCheck::OrderSizeTooLarge { .. }));

        // Too small
        let check = limits.check_order(500, 0, true, 0, 0, 0);
        assert!(matches!(check, LimitCheck::OrderSizeTooSmall { .. }));

        // Just right
        let check = limits.check_order(50_000, 0, true, 0, 0, 0);
        assert!(check.is_ok());
    }

    #[test]
    fn test_position_limit() {
        let limits = PositionLimits {
            max_position_per_asset: 100_000,
            ..Default::default()
        };

        // Would exceed long limit
        let check = limits.check_order(50_000, 80_000, true, 80_000, 0, 0);
        assert!(matches!(check, LimitCheck::PositionLimitExceeded { .. }));

        // Would be fine (reducing position)
        let check = limits.check_order(50_000, 80_000, false, 80_000, 0, 0);
        assert!(check.is_ok());
    }

    #[test]
    fn test_max_allowed_order_size() {
        let limits = PositionLimits {
            max_position_per_asset: 100_000,
            max_gross_position: 200_000,
            max_order_size: 50_000,
            max_daily_volume: 500_000,
            ..Default::default()
        };

        // With no existing position
        let max = limits.max_allowed_order_size(0, true, 0, 0);
        assert_eq!(max, 50_000); // Limited by max_order_size

        // With large existing position
        let max = limits.max_allowed_order_size(90_000, true, 90_000, 0);
        assert_eq!(max, 10_000); // Limited by position headroom

        // With high daily volume
        let max = limits.max_allowed_order_size(0, true, 0, 480_000);
        assert_eq!(max, 20_000); // Limited by volume headroom
    }
}
