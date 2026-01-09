//! Circuit breaker for risk management.
//!
//! Automatically halts trading when risk thresholds are exceeded.

use serde::{Deserialize, Serialize};

/// Circuit breaker configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CircuitBreakerConfig {
    /// Maximum drawdown before tripping (basis points)
    pub max_drawdown_bps: i64,
    /// Maximum loss per period before tripping (micro-USDC)
    pub max_loss_per_period: i64,
    /// Period length for loss calculation (nanoseconds)
    pub loss_period_ns: u64,
    /// Maximum consecutive losses before tripping
    pub max_consecutive_losses: u32,
    /// Maximum position delta per minute (micro-shares)
    pub max_position_delta_per_minute: i64,
    /// Cooldown period after trip (nanoseconds)
    pub cooldown_ns: u64,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            // 5% max drawdown
            max_drawdown_bps: 500,
            // $100 max loss per hour
            max_loss_per_period: 100_000_000,
            // 1 hour period
            loss_period_ns: 3_600_000_000_000,
            // 10 consecutive losses
            max_consecutive_losses: 10,
            // 1 share per minute position change
            max_position_delta_per_minute: 1_000_000,
            // 5 minute cooldown
            cooldown_ns: 300_000_000_000,
        }
    }
}

/// Reason the circuit breaker tripped.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TripReason {
    MaxDrawdownExceeded { current_bps: i64, limit_bps: i64 },
    MaxLossExceeded { loss: i64, limit: i64 },
    ConsecutiveLosses { count: u32, limit: u32 },
    PositionDeltaExceeded { delta: i64, limit: i64 },
    ManualTrip { reason: String },
}

impl std::fmt::Display for TripReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MaxDrawdownExceeded {
                current_bps,
                limit_bps,
            } => {
                write!(
                    f,
                    "Max drawdown exceeded: {}bps > {}bps",
                    current_bps, limit_bps
                )
            }
            Self::MaxLossExceeded { loss, limit } => {
                write!(f, "Max loss exceeded: {} > {}", loss, limit)
            }
            Self::ConsecutiveLosses { count, limit } => {
                write!(f, "Consecutive losses: {} >= {}", count, limit)
            }
            Self::PositionDeltaExceeded { delta, limit } => {
                write!(f, "Position delta exceeded: {} > {}", delta, limit)
            }
            Self::ManualTrip { reason } => {
                write!(f, "Manual trip: {}", reason)
            }
        }
    }
}

/// Circuit breaker state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakerState {
    /// Normal operation
    Closed,
    /// Trading halted
    Open,
    /// Cooldown period after trip
    Cooling,
}

/// Circuit breaker for risk management.
pub struct CircuitBreaker {
    config: CircuitBreakerConfig,
    state: BreakerState,
    /// When the breaker was tripped
    tripped_at_ns: Option<u64>,
    /// Why the breaker was tripped
    trip_reason: Option<TripReason>,
    /// Consecutive losing trades
    consecutive_losses: u32,
    /// PnL at start of current period
    period_start_pnl: i64,
    /// Start of current period
    period_start_ns: u64,
    /// Position at last check (for delta calculation)
    last_position: i64,
    /// Time of last position check
    last_position_check_ns: u64,
}

impl CircuitBreaker {
    pub fn new(config: CircuitBreakerConfig) -> Self {
        Self {
            config,
            state: BreakerState::Closed,
            tripped_at_ns: None,
            trip_reason: None,
            consecutive_losses: 0,
            period_start_pnl: 0,
            period_start_ns: 0,
            last_position: 0,
            last_position_check_ns: 0,
        }
    }

    /// Get current state.
    pub fn state(&self) -> BreakerState {
        self.state
    }

    /// Check if trading is allowed.
    pub fn is_trading_allowed(&self) -> bool {
        self.state == BreakerState::Closed
    }

    /// Get trip reason if tripped.
    pub fn trip_reason(&self) -> Option<&TripReason> {
        self.trip_reason.as_ref()
    }

    /// Check drawdown and potentially trip.
    pub fn check_drawdown(&mut self, drawdown_bps: i64, now_ns: u64) -> bool {
        if drawdown_bps > self.config.max_drawdown_bps {
            self.trip(
                TripReason::MaxDrawdownExceeded {
                    current_bps: drawdown_bps,
                    limit_bps: self.config.max_drawdown_bps,
                },
                now_ns,
            );
            return true;
        }
        false
    }

    /// Check PnL and potentially trip.
    pub fn check_pnl(&mut self, current_pnl: i64, now_ns: u64) -> bool {
        // Check if we need to start a new period
        if now_ns - self.period_start_ns > self.config.loss_period_ns {
            self.period_start_ns = now_ns;
            self.period_start_pnl = current_pnl;
            return false;
        }

        let period_loss = self.period_start_pnl - current_pnl;
        if period_loss > self.config.max_loss_per_period {
            self.trip(
                TripReason::MaxLossExceeded {
                    loss: period_loss,
                    limit: self.config.max_loss_per_period,
                },
                now_ns,
            );
            return true;
        }
        false
    }

    /// Record a trade result.
    pub fn on_trade_pnl(&mut self, pnl: i64, now_ns: u64) -> bool {
        if pnl < 0 {
            self.consecutive_losses += 1;
            if self.consecutive_losses >= self.config.max_consecutive_losses {
                self.trip(
                    TripReason::ConsecutiveLosses {
                        count: self.consecutive_losses,
                        limit: self.config.max_consecutive_losses,
                    },
                    now_ns,
                );
                return true;
            }
        } else {
            self.consecutive_losses = 0;
        }
        false
    }

    /// Check position velocity.
    pub fn check_position_velocity(&mut self, current_position: i64, now_ns: u64) -> bool {
        let elapsed_ns = now_ns.saturating_sub(self.last_position_check_ns);
        if elapsed_ns == 0 {
            return false;
        }

        let delta = (current_position - self.last_position).abs();
        let minute_ns = 60_000_000_000u64;

        // Extrapolate to per-minute rate
        let delta_per_minute = (delta as u128 * minute_ns as u128 / elapsed_ns as u128) as i64;

        self.last_position = current_position;
        self.last_position_check_ns = now_ns;

        if delta_per_minute > self.config.max_position_delta_per_minute {
            self.trip(
                TripReason::PositionDeltaExceeded {
                    delta: delta_per_minute,
                    limit: self.config.max_position_delta_per_minute,
                },
                now_ns,
            );
            return true;
        }
        false
    }

    /// Manually trip the breaker.
    pub fn manual_trip(&mut self, reason: String, now_ns: u64) {
        self.trip(TripReason::ManualTrip { reason }, now_ns);
    }

    /// Trip the breaker.
    fn trip(&mut self, reason: TripReason, now_ns: u64) {
        self.state = BreakerState::Open;
        self.tripped_at_ns = Some(now_ns);
        self.trip_reason = Some(reason);
    }

    /// Update state based on time (check cooldown).
    pub fn update(&mut self, now_ns: u64) {
        match self.state {
            BreakerState::Open => {
                // Start cooldown
                self.state = BreakerState::Cooling;
            }
            BreakerState::Cooling => {
                if let Some(tripped_at) = self.tripped_at_ns {
                    if now_ns - tripped_at > self.config.cooldown_ns {
                        self.reset();
                    }
                }
            }
            BreakerState::Closed => {}
        }
    }

    /// Reset the breaker (after cooldown or manual reset).
    pub fn reset(&mut self) {
        self.state = BreakerState::Closed;
        self.tripped_at_ns = None;
        self.trip_reason = None;
        self.consecutive_losses = 0;
    }

    /// Force reset (use with caution).
    pub fn force_reset(&mut self) {
        self.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_drawdown_trip() {
        let mut breaker = CircuitBreaker::new(CircuitBreakerConfig {
            max_drawdown_bps: 500,
            ..Default::default()
        });

        assert!(breaker.is_trading_allowed());

        // Small drawdown - ok
        assert!(!breaker.check_drawdown(300, 1000));
        assert!(breaker.is_trading_allowed());

        // Large drawdown - trip
        assert!(breaker.check_drawdown(600, 2000));
        assert!(!breaker.is_trading_allowed());
        assert!(matches!(
            breaker.trip_reason(),
            Some(TripReason::MaxDrawdownExceeded { .. })
        ));
    }

    #[test]
    fn test_consecutive_losses() {
        let mut breaker = CircuitBreaker::new(CircuitBreakerConfig {
            max_consecutive_losses: 3,
            ..Default::default()
        });

        // Two losses - ok
        assert!(!breaker.on_trade_pnl(-100, 1000));
        assert!(!breaker.on_trade_pnl(-100, 2000));
        assert!(breaker.is_trading_allowed());

        // Third loss - trip
        assert!(breaker.on_trade_pnl(-100, 3000));
        assert!(!breaker.is_trading_allowed());

        // Reset on profit
        breaker.reset();
        breaker.on_trade_pnl(100, 4000);
        assert_eq!(breaker.consecutive_losses, 0);
    }

    #[test]
    fn test_cooldown() {
        let mut breaker = CircuitBreaker::new(CircuitBreakerConfig {
            cooldown_ns: 1000,
            max_drawdown_bps: 100,
            ..Default::default()
        });

        breaker.check_drawdown(200, 0);
        assert!(!breaker.is_trading_allowed());

        // Move to cooling
        breaker.update(500);
        assert_eq!(breaker.state, BreakerState::Cooling);

        // Still cooling
        breaker.update(800);
        assert_eq!(breaker.state, BreakerState::Cooling);

        // Cooldown complete
        breaker.update(1500);
        assert_eq!(breaker.state, BreakerState::Closed);
        assert!(breaker.is_trading_allowed());
    }
}
