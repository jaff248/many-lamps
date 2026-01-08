//! Order types and state machine.
//!
//! Orders follow a strict state machine:
//! - PendingNew -> Open | Rejected
//! - Open -> PartiallyFilled | Filled | PendingCancel | Cancelled
//! - PartiallyFilled -> Filled | PendingCancel | Cancelled
//! - PendingCancel -> Cancelled | Filled
//!
//! The state machine ensures we never lose track of orders.

use mtrader_core::{ClientOrderId, OrderReason, Side, Size, Tick, UsdcAmount};
use serde::{Deserialize, Serialize};

/// Unique order identifier.
pub type OrderId = String;

/// Order sizing semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderKind {
    /// Limit order with price tick and size in shares.
    Limit { price_tick: Tick, size_shares: Size },
    /// Market buy sized in USDC.
    MarketBuy { usdc_amount: UsdcAmount },
    /// Market sell sized in shares.
    MarketSell { size_shares: Size },
}

/// Order state in the lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OrderState {
    /// Order submitted, awaiting acknowledgment
    PendingNew,
    /// Order is live on the book
    Open,
    /// Order partially filled
    PartiallyFilled,
    /// Order completely filled
    Filled,
    /// Cancel request sent, awaiting confirmation
    PendingCancel,
    /// Order cancelled
    Cancelled,
    /// Order rejected by exchange
    Rejected,
}

impl OrderState {
    /// Check if this state is terminal (no more transitions possible).
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Filled | Self::Cancelled | Self::Rejected)
    }

    /// Check if this order is still live (can be filled).
    pub fn is_live(&self) -> bool {
        matches!(self, Self::Open | Self::PartiallyFilled | Self::PendingCancel)
    }

    /// Check if a cancel is pending.
    pub fn is_cancel_pending(&self) -> bool {
        matches!(self, Self::PendingCancel)
    }

    /// Validate state transition.
    pub fn can_transition_to(&self, next: OrderState) -> bool {
        match (self, next) {
            // From PendingNew
            (Self::PendingNew, Self::Open) => true,
            (Self::PendingNew, Self::Rejected) => true,
            (Self::PendingNew, Self::Filled) => true, // Immediate fill

            // From Open
            (Self::Open, Self::PartiallyFilled) => true,
            (Self::Open, Self::Filled) => true,
            (Self::Open, Self::PendingCancel) => true,
            (Self::Open, Self::Cancelled) => true, // Exchange-initiated

            // From PartiallyFilled
            (Self::PartiallyFilled, Self::PartiallyFilled) => true, // More fills
            (Self::PartiallyFilled, Self::Filled) => true,
            (Self::PartiallyFilled, Self::PendingCancel) => true,
            (Self::PartiallyFilled, Self::Cancelled) => true,

            // From PendingCancel
            (Self::PendingCancel, Self::Cancelled) => true,
            (Self::PendingCancel, Self::Filled) => true, // Raced with fill
            (Self::PendingCancel, Self::PartiallyFilled) => true, // Fill during cancel

            _ => false,
        }
    }
}

/// Order type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderType {
    /// Good-til-cancelled limit order
    Limit,
    /// Fill-or-kill (immediate full fill or cancel)
    FOK,
    /// Good-til-date
    GTD { expires_at_ms: u64 },
}

/// An order in the system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Order {
    /// Exchange-assigned order ID
    pub order_id: OrderId,
    /// Client-assigned ID for idempotency
    pub client_order_id: ClientOrderId,
    /// Asset being traded
    pub asset_id: String,
    /// Buy or Sell
    pub side: Side,
    /// Order kind (limit/market semantics)
    pub kind: OrderKind,
    /// Original order size in micro-shares (0 for market buys)
    pub original_size: Size,
    /// Remaining unfilled size
    pub remaining_size: Size,
    /// Cumulative filled size
    pub filled_size: Size,
    /// Order type
    pub order_type: OrderType,
    /// Current state
    pub state: OrderState,
    /// Why this order was placed
    pub reason: OrderReason,
    /// Timestamp when order was created (mono ns)
    pub created_at_ns: u64,
    /// Timestamp of last state change (mono ns)
    pub updated_at_ns: u64,
}

impl Order {
    /// Create a new pending order.
    pub fn new(
        client_order_id: ClientOrderId,
        asset_id: String,
        side: Side,
        kind: OrderKind,
        order_type: OrderType,
        reason: OrderReason,
        now_ns: u64,
    ) -> Self {
        let size = match kind {
            OrderKind::Limit { size_shares, .. } => size_shares,
            OrderKind::MarketSell { size_shares } => size_shares,
            OrderKind::MarketBuy { .. } => 0,
        };

        Self {
            order_id: String::new(), // Set when ack received
            client_order_id,
            asset_id,
            side,
            kind,
            original_size: size,
            remaining_size: size,
            filled_size: 0,
            order_type,
            state: OrderState::PendingNew,
            reason,
            created_at_ns: now_ns,
            updated_at_ns: now_ns,
        }
    }

    /// Acknowledge the order (transition to Open).
    pub fn acknowledge(&mut self, order_id: OrderId, now_ns: u64) -> Result<(), InvalidTransition> {
        self.transition_to(OrderState::Open, now_ns)?;
        self.order_id = order_id;
        Ok(())
    }

    /// Reject the order.
    pub fn reject(&mut self, now_ns: u64) -> Result<(), InvalidTransition> {
        self.transition_to(OrderState::Rejected, now_ns)
    }

    /// Record a fill.
    pub fn fill(&mut self, fill_size: Size, now_ns: u64) -> Result<(), InvalidTransition> {
        if fill_size > self.remaining_size {
            // Overfill - should not happen but handle gracefully
            self.remaining_size = 0;
            self.filled_size = self.original_size;
        } else {
            self.remaining_size -= fill_size;
            self.filled_size += fill_size;
        }

        let new_state = if self.remaining_size == 0 {
            OrderState::Filled
        } else {
            OrderState::PartiallyFilled
        };

        self.transition_to(new_state, now_ns)
    }

    /// Request cancellation.
    pub fn request_cancel(&mut self, now_ns: u64) -> Result<(), InvalidTransition> {
        self.transition_to(OrderState::PendingCancel, now_ns)
    }

    /// Confirm cancellation.
    pub fn confirm_cancel(&mut self, now_ns: u64) -> Result<(), InvalidTransition> {
        self.transition_to(OrderState::Cancelled, now_ns)
    }

    fn transition_to(&mut self, new_state: OrderState, now_ns: u64) -> Result<(), InvalidTransition> {
        if !self.state.can_transition_to(new_state) {
            return Err(InvalidTransition {
                from: self.state,
                to: new_state,
            });
        }
        self.state = new_state;
        self.updated_at_ns = now_ns;
        Ok(())
    }
}

impl Order {
    pub fn price_tick(&self) -> Tick {
        match self.kind {
            OrderKind::Limit { price_tick, .. } => price_tick,
            _ => 0,
        }
    }

    pub fn size_shares(&self) -> Option<Size> {
        match self.kind {
            OrderKind::Limit { size_shares, .. } => Some(size_shares),
            OrderKind::MarketSell { size_shares } => Some(size_shares),
            OrderKind::MarketBuy { .. } => None,
        }
    }
}

/// Error for invalid state transitions.
#[derive(Debug, Clone)]
pub struct InvalidTransition {
    pub from: OrderState,
    pub to: OrderState,
}

impl std::fmt::Display for InvalidTransition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Invalid transition: {:?} -> {:?}", self.from, self.to)
    }
}

impl std::error::Error for InvalidTransition {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_order_lifecycle_normal() {
        let mut order = Order::new(
            ClientOrderId("client-1".into()),
            "asset-123".into(),
            Side::Buy,
            OrderKind::Limit {
                price_tick: 5000,
                size_shares: 100_000,
            },
            OrderType::Limit,
            OrderReason::MakerQuote,
            1000,
        );

        assert_eq!(order.state, OrderState::PendingNew);

        // Acknowledge
        order.acknowledge("order-xyz".into(), 2000).unwrap();
        assert_eq!(order.state, OrderState::Open);
        assert_eq!(order.order_id, "order-xyz");

        // Partial fill
        order.fill(50_000, 3000).unwrap();
        assert_eq!(order.state, OrderState::PartiallyFilled);
        assert_eq!(order.remaining_size, 50_000);

        // Complete fill
        order.fill(50_000, 4000).unwrap();
        assert_eq!(order.state, OrderState::Filled);
        assert_eq!(order.remaining_size, 0);
        assert!(order.state.is_terminal());
    }

    #[test]
    fn test_order_cancel_flow() {
        let mut order = Order::new(
            ClientOrderId("client-2".into()),
            "asset-123".into(),
            Side::Sell,
            OrderKind::Limit {
                price_tick: 5500,
                size_shares: 100_000,
            },
            OrderType::Limit,
            OrderReason::MakerQuote,
            1000,
        );

        order.acknowledge("order-abc".into(), 2000).unwrap();
        order.request_cancel(3000).unwrap();
        assert_eq!(order.state, OrderState::PendingCancel);

        order.confirm_cancel(4000).unwrap();
        assert_eq!(order.state, OrderState::Cancelled);
        assert!(order.state.is_terminal());
    }

    #[test]
    fn test_cancel_race_with_fill() {
        let mut order = Order::new(
            ClientOrderId("client-3".into()),
            "asset-123".into(),
            Side::Buy,
            OrderKind::Limit {
                price_tick: 5000,
                size_shares: 100_000,
            },
            OrderType::Limit,
            OrderReason::MakerQuote,
            1000,
        );

        order.acknowledge("order-def".into(), 2000).unwrap();
        order.request_cancel(3000).unwrap();

        // Fill comes in during cancel
        order.fill(100_000, 3500).unwrap();
        assert_eq!(order.state, OrderState::Filled);
    }

    #[test]
    fn test_invalid_transition() {
        let mut order = Order::new(
            ClientOrderId("client-4".into()),
            "asset-123".into(),
            Side::Buy,
            OrderKind::Limit {
                price_tick: 5000,
                size_shares: 100_000,
            },
            OrderType::Limit,
            OrderReason::MakerQuote,
            1000,
        );

        // Cannot go directly from PendingNew to Cancelled
        let result = order.confirm_cancel(2000);
        assert!(result.is_err());
    }

    #[test]
    fn test_state_queries() {
        assert!(OrderState::Filled.is_terminal());
        assert!(OrderState::Cancelled.is_terminal());
        assert!(OrderState::Rejected.is_terminal());
        assert!(!OrderState::Open.is_terminal());

        assert!(OrderState::Open.is_live());
        assert!(OrderState::PartiallyFilled.is_live());
        assert!(OrderState::PendingCancel.is_live());
        assert!(!OrderState::PendingNew.is_live());

        assert!(OrderState::PendingCancel.is_cancel_pending());
    }
}
