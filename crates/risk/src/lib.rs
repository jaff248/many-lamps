//! Risk management crate.
//!
//! Provides:
//! - Position limits (per-market, total)
//! - PnL tracking (realized, unrealized)
//! - Circuit breakers
//! - Drawdown monitoring
//! - Position reconciliation with auto-correction

pub mod circuit_breaker;
pub mod limits;
pub mod pnl;
pub mod position;
pub mod reconciliation;

pub use circuit_breaker::{CircuitBreaker, CircuitBreakerConfig, TripReason};
pub use limits::{LimitCheck, PositionLimits};
pub use pnl::{PnLSnapshot, PnLTracker};
pub use position::{Position, PositionTracker};
pub use reconciliation::{PositionDiscrepancy, ReconciliationAction, ReconciliationConfig, PositionReconciliator};
