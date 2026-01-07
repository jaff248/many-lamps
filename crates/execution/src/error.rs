//! Execution error types.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ExecutionError {
    #[error("Order not found: {0}")]
    OrderNotFound(String),

    #[error("Invalid state transition: {from:?} -> {to:?}")]
    InvalidStateTransition { from: String, to: String },

    #[error("Duplicate order ID: {0}")]
    DuplicateOrderId(String),

    #[error("Self-trade would occur at tick {tick}")]
    SelfTrade { tick: u16 },

    #[error("Order rejected: {reason}")]
    Rejected { reason: String },

    #[error("Cancel timed out for order {order_id}")]
    CancelTimeout { order_id: String },

    #[error("Order expired")]
    Expired,
}
