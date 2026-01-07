//! Risk management crate.
//!
//! Provides:
//! - Position limits (per-market, total)
//! - PnL tracking (realized, unrealized)
//! - Circuit breakers
//! - Drawdown monitoring

pub mod circuit_breaker;
pub mod limits;
pub mod pnl;
pub mod position;

pub use circuit_breaker::{CircuitBreaker, CircuitBreakerConfig, TripReason};
pub use limits::{PositionLimits, LimitCheck};
pub use pnl::{PnLTracker, PnLSnapshot};
pub use position::{Position, PositionTracker};
