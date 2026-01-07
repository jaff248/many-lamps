//! Execution crate for order management.
//!
//! Provides:
//! - Order state machine with proper lifecycle tracking
//! - Self-trade prevention guard
//! - Cancel-before-cross workflow
//! - Idempotent order handling

pub mod error;
pub mod order;
pub mod self_trade_guard;
pub mod state_manager;

pub use error::ExecutionError;
pub use order::{Order, OrderId, OrderState, OrderType};
pub use self_trade_guard::SelfTradeGuard;
pub use state_manager::OrderStateManager;
