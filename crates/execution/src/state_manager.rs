//! Order state manager.
//!
//! Central component for tracking all orders and their lifecycle.
//! Handles:
//! - Order creation with idempotency
//! - State transitions based on exchange events
//! - Integration with self-trade guard
//! - Cancel-before-cross workflow

use crate::error::ExecutionError;
use crate::order::{ClientOrderId, Order, OrderId, OrderState, OrderType};
use crate::self_trade_guard::{SelfTradeBlock, SelfTradeConfig, SelfTradeGuard};
use mtrader_core::{OrderReason, Side, Size, Tick};
use std::collections::HashMap;

/// Configuration for order state manager.
#[derive(Debug, Clone)]
pub struct OrderManagerConfig {
    /// Self-trade prevention config
    pub self_trade: SelfTradeConfig,
    /// Maximum pending orders per side
    pub max_pending_per_side: usize,
    /// Cancel timeout in nanoseconds
    pub cancel_timeout_ns: u64,
}

impl Default for OrderManagerConfig {
    fn default() -> Self {
        Self {
            self_trade: SelfTradeConfig::default(),
            max_pending_per_side: 10,
            cancel_timeout_ns: 5_000_000_000, // 5 seconds
        }
    }
}

/// Manages order state and lifecycle.
pub struct OrderStateManager {
    config: OrderManagerConfig,
    /// All orders by exchange order ID
    orders_by_id: HashMap<OrderId, Order>,
    /// Map client order IDs to exchange order IDs for idempotency
    client_to_exchange_id: HashMap<ClientOrderId, OrderId>,
    /// Pending orders (PendingNew) by client ID
    pending_new: HashMap<ClientOrderId, Order>,
    /// Self-trade prevention guard
    self_trade_guard: SelfTradeGuard,
    /// Next client order ID sequence
    next_client_id: u64,
}

impl OrderStateManager {
    pub fn new(config: OrderManagerConfig) -> Self {
        Self {
            self_trade_guard: SelfTradeGuard::new(config.self_trade.clone()),
            config,
            orders_by_id: HashMap::new(),
            client_to_exchange_id: HashMap::new(),
            pending_new: HashMap::new(),
            next_client_id: 1,
        }
    }

    /// Generate a new client order ID.
    pub fn generate_client_id(&mut self) -> ClientOrderId {
        let id = format!("mtrader-{}", self.next_client_id);
        self.next_client_id += 1;
        id
    }

    /// Create a new order with self-trade check.
    ///
    /// Returns the order if it can be placed, or an error if blocked.
    pub fn create_order(
        &mut self,
        asset_id: String,
        side: Side,
        price_tick: Tick,
        size: Size,
        order_type: OrderType,
        reason: OrderReason,
        now_ns: u64,
    ) -> Result<Order, CreateOrderError> {
        // Check self-trade first
        if let Err(block) = self.self_trade_guard.check_order(side, price_tick, now_ns) {
            return Err(CreateOrderError::SelfTrade(block));
        }

        // Check pending order limits
        let pending_count = self.count_pending_orders(side);
        if pending_count >= self.config.max_pending_per_side {
            return Err(CreateOrderError::TooManyPending {
                count: pending_count,
                max: self.config.max_pending_per_side,
            });
        }

        let client_id = self.generate_client_id();
        let order = Order::new(
            client_id.clone(),
            asset_id,
            side,
            price_tick,
            size,
            order_type,
            reason,
            now_ns,
        );

        self.pending_new.insert(client_id, order.clone());
        Ok(order)
    }

    /// Handle order acknowledgment from exchange.
    pub fn on_order_ack(
        &mut self,
        client_order_id: &str,
        exchange_order_id: OrderId,
        now_ns: u64,
    ) -> Result<&Order, ExecutionError> {
        let mut order = self
            .pending_new
            .remove(client_order_id)
            .ok_or_else(|| ExecutionError::OrderNotFound(client_order_id.to_string()))?;

        order
            .acknowledge(exchange_order_id.clone(), now_ns)
            .map_err(|e| ExecutionError::InvalidStateTransition {
                from: format!("{:?}", e.from),
                to: format!("{:?}", e.to),
            })?;

        // Register with self-trade guard
        self.self_trade_guard.register_order(&order);

        // Store mappings
        self.client_to_exchange_id
            .insert(client_order_id.to_string(), exchange_order_id.clone());
        self.orders_by_id.insert(exchange_order_id.clone(), order);

        Ok(self.orders_by_id.get(&exchange_order_id).unwrap())
    }

    /// Handle order rejection from exchange.
    pub fn on_order_reject(
        &mut self,
        client_order_id: &str,
        _reason: &str,
        now_ns: u64,
    ) -> Result<Order, ExecutionError> {
        let mut order = self
            .pending_new
            .remove(client_order_id)
            .ok_or_else(|| ExecutionError::OrderNotFound(client_order_id.to_string()))?;

        order.reject(now_ns).map_err(|e| ExecutionError::InvalidStateTransition {
            from: format!("{:?}", e.from),
            to: format!("{:?}", e.to),
        })?;

        Ok(order)
    }

    /// Handle a fill event.
    pub fn on_fill(
        &mut self,
        order_id: &str,
        fill_size: Size,
        now_ns: u64,
    ) -> Result<&Order, ExecutionError> {
        let order = self
            .orders_by_id
            .get_mut(order_id)
            .ok_or_else(|| ExecutionError::OrderNotFound(order_id.to_string()))?;

        order.fill(fill_size, now_ns).map_err(|e| ExecutionError::InvalidStateTransition {
            from: format!("{:?}", e.from),
            to: format!("{:?}", e.to),
        })?;

        // Update self-trade guard
        self.self_trade_guard.update_order(order, now_ns);

        Ok(self.orders_by_id.get(order_id).unwrap())
    }

    /// Request to cancel an order.
    pub fn request_cancel(
        &mut self,
        order_id: &str,
        now_ns: u64,
    ) -> Result<&Order, ExecutionError> {
        let order = self
            .orders_by_id
            .get_mut(order_id)
            .ok_or_else(|| ExecutionError::OrderNotFound(order_id.to_string()))?;

        if order.state.is_terminal() {
            return Err(ExecutionError::InvalidStateTransition {
                from: format!("{:?}", order.state),
                to: "PendingCancel".to_string(),
            });
        }

        order.request_cancel(now_ns).map_err(|e| ExecutionError::InvalidStateTransition {
            from: format!("{:?}", e.from),
            to: format!("{:?}", e.to),
        })?;

        // Update self-trade guard
        self.self_trade_guard.update_order(order, now_ns);

        Ok(self.orders_by_id.get(order_id).unwrap())
    }

    /// Handle cancel confirmation.
    pub fn on_cancel_confirm(
        &mut self,
        order_id: &str,
        now_ns: u64,
    ) -> Result<&Order, ExecutionError> {
        let order = self
            .orders_by_id
            .get_mut(order_id)
            .ok_or_else(|| ExecutionError::OrderNotFound(order_id.to_string()))?;

        order.confirm_cancel(now_ns).map_err(|e| ExecutionError::InvalidStateTransition {
            from: format!("{:?}", e.from),
            to: format!("{:?}", e.to),
        })?;

        // Update self-trade guard
        self.self_trade_guard.update_order(order, now_ns);

        Ok(self.orders_by_id.get(order_id).unwrap())
    }

    /// Cancel-before-cross workflow.
    ///
    /// If we want to place an order that would cross with our own,
    /// this method identifies which orders to cancel first.
    pub fn orders_to_cancel_before_cross(
        &self,
        side: Side,
        tick: Tick,
    ) -> Vec<OrderId> {
        let mut to_cancel = Vec::new();

        // Find our orders that would be crossed
        let crossing_side = side.opposite();
        for (order_id, order) in &self.orders_by_id {
            if !order.state.is_live() {
                continue;
            }
            if order.side != crossing_side {
                continue;
            }

            let would_cross = match side {
                Side::Buy => order.price_tick <= tick,  // Our sell at or below buy price
                Side::Sell => order.price_tick >= tick, // Our buy at or above sell price
            };

            if would_cross {
                to_cancel.push(order_id.clone());
            }
        }

        to_cancel
    }

    /// Get an order by exchange ID.
    pub fn get_order(&self, order_id: &str) -> Option<&Order> {
        self.orders_by_id.get(order_id)
    }

    /// Get an order by client ID.
    pub fn get_order_by_client_id(&self, client_order_id: &str) -> Option<&Order> {
        // Check pending first
        if let Some(order) = self.pending_new.get(client_order_id) {
            return Some(order);
        }

        // Then check acknowledged orders
        self.client_to_exchange_id
            .get(client_order_id)
            .and_then(|id| self.orders_by_id.get(id))
    }

    /// Get all live orders.
    pub fn live_orders(&self) -> impl Iterator<Item = &Order> {
        self.orders_by_id.values().filter(|o| o.state.is_live())
    }

    /// Get all pending new orders.
    pub fn pending_orders(&self) -> impl Iterator<Item = &Order> {
        self.pending_new.values()
    }

    /// Count pending orders by side.
    fn count_pending_orders(&self, side: Side) -> usize {
        self.pending_new.values().filter(|o| o.side == side).count()
            + self.orders_by_id.values().filter(|o| o.state == OrderState::PendingNew && o.side == side).count()
    }

    /// Check for timed out cancel requests.
    pub fn check_cancel_timeouts(&self, now_ns: u64) -> Vec<OrderId> {
        self.orders_by_id
            .iter()
            .filter(|(_, order)| {
                order.state == OrderState::PendingCancel
                    && now_ns.saturating_sub(order.updated_at_ns) > self.config.cancel_timeout_ns
            })
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Cleanup stale data.
    pub fn cleanup(&mut self, now_ns: u64) {
        self.self_trade_guard.cleanup(now_ns);

        // Remove very old terminal orders (> 1 hour)
        let one_hour_ns = 3_600_000_000_000;
        self.orders_by_id.retain(|_, order| {
            !order.state.is_terminal()
                || now_ns.saturating_sub(order.updated_at_ns) < one_hour_ns
        });
    }
}

/// Error when creating an order.
#[derive(Debug)]
pub enum CreateOrderError {
    SelfTrade(SelfTradeBlock),
    TooManyPending { count: usize, max: usize },
}

impl std::fmt::Display for CreateOrderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SelfTrade(block) => write!(f, "{}", block),
            Self::TooManyPending { count, max } => {
                write!(f, "Too many pending orders: {} (max {})", count, max)
            }
        }
    }
}

impl std::error::Error for CreateOrderError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_order_lifecycle() {
        let mut mgr = OrderStateManager::new(OrderManagerConfig::default());

        // Create order
        let order = mgr
            .create_order(
                "asset-123".into(),
                Side::Buy,
                50,
                100_000,
                OrderType::Limit,
                OrderReason::MakerQuote,
                1000,
            )
            .unwrap();

        assert_eq!(order.state, OrderState::PendingNew);
        let client_id = order.client_order_id.clone();

        // Acknowledge
        let order = mgr
            .on_order_ack(&client_id, "exchange-order-1".into(), 2000)
            .unwrap();
        assert_eq!(order.state, OrderState::Open);

        // Fill
        let order = mgr.on_fill("exchange-order-1", 100_000, 3000).unwrap();
        assert_eq!(order.state, OrderState::Filled);
    }

    #[test]
    fn test_self_trade_prevention() {
        let mut mgr = OrderStateManager::new(OrderManagerConfig::default());

        // Create and ack a sell order at tick 55
        let order = mgr
            .create_order(
                "asset-123".into(),
                Side::Sell,
                55,
                100_000,
                OrderType::Limit,
                OrderReason::MakerQuote,
                1000,
            )
            .unwrap();
        let client_id = order.client_order_id.clone();
        mgr.on_order_ack(&client_id, "sell-1".into(), 2000).unwrap();

        // Try to create buy at tick 55 - should be blocked
        let result = mgr.create_order(
            "asset-123".into(),
            Side::Buy,
            55,
            100_000,
            OrderType::Limit,
            OrderReason::MakerQuote,
            3000,
        );
        assert!(matches!(result, Err(CreateOrderError::SelfTrade(_))));

        // Buy at tick 54 should be fine
        let result = mgr.create_order(
            "asset-123".into(),
            Side::Buy,
            54,
            100_000,
            OrderType::Limit,
            OrderReason::MakerQuote,
            3000,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_cancel_before_cross() {
        let mut mgr = OrderStateManager::new(OrderManagerConfig::default());

        // Create and ack sell orders at 55 and 57
        for (tick, id) in [(55, "sell-1"), (57, "sell-2")] {
            let order = mgr
                .create_order(
                    "asset-123".into(),
                    Side::Sell,
                    tick,
                    100_000,
                    OrderType::Limit,
                    OrderReason::MakerQuote,
                    1000,
                )
                .unwrap();
            mgr.on_order_ack(&order.client_order_id, id.into(), 2000)
                .unwrap();
        }

        // Check which orders need cancelling for a buy at 56
        let to_cancel = mgr.orders_to_cancel_before_cross(Side::Buy, 56);
        assert_eq!(to_cancel.len(), 1);
        assert!(to_cancel.contains(&"sell-1".to_string()));

        // Check for a buy at 58
        let to_cancel = mgr.orders_to_cancel_before_cross(Side::Buy, 58);
        assert_eq!(to_cancel.len(), 2);
    }

    #[test]
    fn test_idempotency() {
        let mut mgr = OrderStateManager::new(OrderManagerConfig::default());

        let order = mgr
            .create_order(
                "asset-123".into(),
                Side::Buy,
                50,
                100_000,
                OrderType::Limit,
                OrderReason::MakerQuote,
                1000,
            )
            .unwrap();
        let client_id = order.client_order_id.clone();

        // Ack the order
        mgr.on_order_ack(&client_id, "exchange-1".into(), 2000).unwrap();

        // Can retrieve by client ID
        let order = mgr.get_order_by_client_id(&client_id);
        assert!(order.is_some());
        assert_eq!(order.unwrap().order_id, "exchange-1");
    }
}
