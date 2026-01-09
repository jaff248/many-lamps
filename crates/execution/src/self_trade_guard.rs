//! Self-trade prevention guard.
//!
//! Prevents self-trades by tracking our own orders and blocking
//! new orders that would cross with them. Accounts for cancel latency.
//!
//! Key insight: A cancel request doesn't immediately remove the order
//! from the book. We must wait for confirmation before placing a
//! crossing order.

use crate::order::{Order, OrderKind, OrderState};
use mtrader_core::{Side, Tick};
use std::collections::{HashMap, HashSet};

/// Configuration for self-trade prevention.
#[derive(Debug, Clone)]
pub struct SelfTradeConfig {
    /// Expected cancel latency in nanoseconds
    /// Orders in PendingCancel state are still considered live
    /// until this time has passed since cancel was requested
    pub cancel_latency_ns: u64,

    /// Safety margin to add to cancel latency
    pub cancel_margin_ns: u64,
}

impl Default for SelfTradeConfig {
    fn default() -> Self {
        Self {
            // Assume 100ms cancel latency
            cancel_latency_ns: 100_000_000,
            // Add 50ms safety margin
            cancel_margin_ns: 50_000_000,
        }
    }
}

/// Tracks our orders to prevent self-trades.
pub struct SelfTradeGuard {
    config: SelfTradeConfig,
    /// Our buy orders by tick
    buy_ticks: HashMap<Tick, HashSet<String>>,
    /// Our sell orders by tick
    sell_ticks: HashMap<Tick, HashSet<String>>,
    /// Orders pending cancel with their cancel request time
    pending_cancels: HashMap<String, PendingCancel>,
}

#[derive(Debug, Clone)]
struct PendingCancel {
    tick: Tick,
    side: Side,
    cancel_requested_ns: u64,
}

impl SelfTradeGuard {
    pub fn new(config: SelfTradeConfig) -> Self {
        Self {
            config,
            buy_ticks: HashMap::new(),
            sell_ticks: HashMap::new(),
            pending_cancels: HashMap::new(),
        }
    }

    /// Register a new order with the guard.
    pub fn register_order(&mut self, order: &Order) {
        if !order.state.is_live() && order.state != OrderState::PendingNew {
            return;
        }

        let OrderKind::Limit { price_tick, .. } = order.kind else {
            return;
        };

        let ticks = match order.side {
            Side::Buy => &mut self.buy_ticks,
            Side::Sell => &mut self.sell_ticks,
        };

        ticks
            .entry(price_tick)
            .or_default()
            .insert(order.order_id.clone());
    }

    /// Update order state (especially for cancels).
    pub fn update_order(&mut self, order: &Order, now_ns: u64) {
        match order.state {
            OrderState::PendingCancel => {
                let OrderKind::Limit { price_tick, .. } = order.kind else {
                    return;
                };
                // Track that cancel is pending
                self.pending_cancels.insert(
                    order.order_id.clone(),
                    PendingCancel {
                        tick: price_tick,
                        side: order.side,
                        cancel_requested_ns: now_ns,
                    },
                );
            }
            OrderState::Cancelled | OrderState::Filled | OrderState::Rejected => {
                // Remove from tracking
                if let OrderKind::Limit { price_tick, .. } = order.kind {
                    self.remove_order(&order.order_id, price_tick, order.side);
                }
                self.pending_cancels.remove(&order.order_id);
            }
            _ => {}
        }
    }

    /// Remove an order from tracking.
    fn remove_order(&mut self, order_id: &str, tick: Tick, side: Side) {
        let ticks = match side {
            Side::Buy => &mut self.buy_ticks,
            Side::Sell => &mut self.sell_ticks,
        };

        if let Some(orders) = ticks.get_mut(&tick) {
            orders.remove(order_id);
            if orders.is_empty() {
                ticks.remove(&tick);
            }
        }
    }

    /// Check if placing a new order would cause a self-trade.
    ///
    /// Returns `Err(SelfTradeBlock)` if the order would cross with our own.
    pub fn check_order(&self, side: Side, tick: Tick, now_ns: u64) -> Result<(), SelfTradeBlock> {
        // A buy order crosses if there's our sell at or below this tick
        // A sell order crosses if there's our buy at or above this tick
        let crossing_ticks = match side {
            Side::Buy => self.find_crossing_sells(tick, now_ns),
            Side::Sell => self.find_crossing_buys(tick, now_ns),
        };

        if let Some(crossing_tick) = crossing_ticks {
            return Err(SelfTradeBlock {
                our_tick: crossing_tick,
                blocked_tick: tick,
            });
        }

        Ok(())
    }

    /// Find our sell orders at or below the given tick that would cross.
    fn find_crossing_sells(&self, buy_tick: Tick, now_ns: u64) -> Option<Tick> {
        let mut lowest_sell = None;

        for (&tick, orders) in &self.sell_ticks {
            // Skip if tick is above our buy price (won't cross)
            if tick > buy_tick {
                continue;
            }

            // Check if any orders at this tick are still effectively live
            for order_id in orders {
                if self.is_order_effectively_live(order_id, now_ns) {
                    match lowest_sell {
                        None => lowest_sell = Some(tick),
                        Some(prev) if tick < prev => lowest_sell = Some(tick),
                        _ => {}
                    }
                    break;
                }
            }
        }

        lowest_sell
    }

    /// Find our buy orders at or above the given tick that would cross.
    fn find_crossing_buys(&self, sell_tick: Tick, now_ns: u64) -> Option<Tick> {
        let mut highest_buy = None;

        for (&tick, orders) in &self.buy_ticks {
            // Skip if tick is below our sell price (won't cross)
            if tick < sell_tick {
                continue;
            }

            // Check if any orders at this tick are still effectively live
            for order_id in orders {
                if self.is_order_effectively_live(order_id, now_ns) {
                    match highest_buy {
                        None => highest_buy = Some(tick),
                        Some(prev) if tick > prev => highest_buy = Some(tick),
                        _ => {}
                    }
                    break;
                }
            }
        }

        highest_buy
    }

    /// Check if an order is effectively live (not cancelled or cancel not yet processed).
    fn is_order_effectively_live(&self, order_id: &str, now_ns: u64) -> bool {
        if let Some(pending) = self.pending_cancels.get(order_id) {
            // Order is pending cancel - check if enough time has passed
            let effective_cancel_time =
                self.config.cancel_latency_ns + self.config.cancel_margin_ns;
            let elapsed = now_ns.saturating_sub(pending.cancel_requested_ns);

            // Still considered live if not enough time has passed
            elapsed < effective_cancel_time
        } else {
            // Order is definitely live
            true
        }
    }

    /// Clean up stale pending cancels that should have completed by now.
    pub fn cleanup(&mut self, now_ns: u64) {
        let effective_cancel_time = self.config.cancel_latency_ns + self.config.cancel_margin_ns;

        let stale: Vec<_> = self
            .pending_cancels
            .iter()
            .filter(|(_, pc)| {
                now_ns.saturating_sub(pc.cancel_requested_ns) > effective_cancel_time * 2
            })
            .map(|(id, pc)| (id.clone(), pc.tick, pc.side))
            .collect();

        for (order_id, tick, side) in stale {
            self.remove_order(&order_id, tick, side);
            self.pending_cancels.remove(&order_id);
        }
    }

    /// Get best bid tick from our orders.
    pub fn our_best_bid(&self) -> Option<Tick> {
        self.buy_ticks.keys().max().copied()
    }

    /// Get best ask tick from our orders.
    pub fn our_best_ask(&self) -> Option<Tick> {
        self.sell_ticks.keys().min().copied()
    }
}

/// Self-trade would be blocked.
#[derive(Debug, Clone)]
pub struct SelfTradeBlock {
    /// Our existing order's tick
    pub our_tick: Tick,
    /// The tick we tried to place at
    pub blocked_tick: Tick,
}

impl std::fmt::Display for SelfTradeBlock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Self-trade blocked: our order at tick {} would cross with new order at tick {}",
            self.our_tick, self.blocked_tick
        )
    }
}

impl std::error::Error for SelfTradeBlock {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::order::{Order, OrderKind, OrderType};
    use mtrader_core::{ClientOrderId, OrderReason};

    fn make_order(order_id: &str, side: Side, tick: Tick, state: OrderState) -> Order {
        let mut order = Order::new(
            ClientOrderId(format!("client-{}", order_id)),
            "asset-123".into(),
            side,
            OrderKind::Limit {
                price_tick: tick,
                size_shares: 100_000,
            },
            OrderType::Limit,
            OrderReason::MakerQuote,
            0,
        );
        order.order_id = order_id.into();
        order.state = state;
        order
    }

    #[test]
    fn test_no_self_trade_without_orders() {
        let guard = SelfTradeGuard::new(SelfTradeConfig::default());
        assert!(guard.check_order(Side::Buy, 5000, 0).is_ok());
        assert!(guard.check_order(Side::Sell, 5000, 0).is_ok());
    }

    #[test]
    fn test_self_trade_blocked() {
        let mut guard = SelfTradeGuard::new(SelfTradeConfig::default());

        // We have a sell at tick 55
        let sell_order = make_order("sell-1", Side::Sell, 5500, OrderState::Open);
        guard.register_order(&sell_order);

        // Buying at 55 or higher would cross
        assert!(guard.check_order(Side::Buy, 5500, 0).is_err());
        assert!(guard.check_order(Side::Buy, 6000, 0).is_err());

        // Buying at 54 or lower is fine
        assert!(guard.check_order(Side::Buy, 5400, 0).is_ok());
        assert!(guard.check_order(Side::Buy, 5000, 0).is_ok());
    }

    #[test]
    fn test_self_trade_with_pending_cancel() {
        let config = SelfTradeConfig {
            cancel_latency_ns: 100_000_000, // 100ms
            cancel_margin_ns: 50_000_000,   // 50ms
        };
        let mut guard = SelfTradeGuard::new(config);

        // We have a sell at tick 55
        let mut sell_order = make_order("sell-1", Side::Sell, 5500, OrderState::Open);
        guard.register_order(&sell_order);

        // Request cancel at t=0
        sell_order.state = OrderState::PendingCancel;
        guard.update_order(&sell_order, 0);

        // At t=50ms, still blocked (within cancel latency + margin)
        let t_50ms = 50_000_000;
        assert!(guard.check_order(Side::Buy, 5500, t_50ms).is_err());

        // At t=200ms, should be allowed (past cancel latency + margin)
        let t_200ms = 200_000_000;
        assert!(guard.check_order(Side::Buy, 5500, t_200ms).is_ok());
    }

    #[test]
    fn test_order_removal_on_fill() {
        let mut guard = SelfTradeGuard::new(SelfTradeConfig::default());

        let mut buy_order = make_order("buy-1", Side::Buy, 5000, OrderState::Open);
        guard.register_order(&buy_order);

        // Initially blocked
        assert!(guard.check_order(Side::Sell, 5000, 0).is_err());

        // Order fills
        buy_order.state = OrderState::Filled;
        guard.update_order(&buy_order, 1000);

        // Now allowed
        assert!(guard.check_order(Side::Sell, 5000, 1000).is_ok());
    }

    #[test]
    fn test_our_best_bid_ask() {
        let mut guard = SelfTradeGuard::new(SelfTradeConfig::default());

        let buy1 = make_order("buy-1", Side::Buy, 4800, OrderState::Open);
        let buy2 = make_order("buy-2", Side::Buy, 5000, OrderState::Open);
        let sell1 = make_order("sell-1", Side::Sell, 5200, OrderState::Open);
        let sell2 = make_order("sell-2", Side::Sell, 5500, OrderState::Open);

        guard.register_order(&buy1);
        guard.register_order(&buy2);
        guard.register_order(&sell1);
        guard.register_order(&sell2);

        assert_eq!(guard.our_best_bid(), Some(5000));
        assert_eq!(guard.our_best_ask(), Some(5200));
    }
}
