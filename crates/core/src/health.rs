//! System health state machine with SAFE_MODE.
//!
//! SAFE_MODE hard-disables all order placement (only cancels allowed).
//! This is the non-negotiable safety layer that protects against:
//! - Book desync
//! - Network issues
//! - Parser errors
//! - Clock anomalies
//! - Repeated failures

use crate::types::TokenId;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Reasons for entering SAFE_MODE
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SafeModeReason {
    /// Book is not in Synced state
    BookNotSynced { token_id: TokenId },
    /// WebSocket lag exceeds threshold
    WsLagExceeded { lag_ms: u64, threshold_ms: u64 },
    /// REST reconciliation found mismatch
    RestReconciliationMismatch {
        token_id: TokenId,
        mismatch_type: ReconciliationMismatch,
    },
    /// Tick size change observed (must resnapshot)
    TickSizeChange {
        token_id: TokenId,
        old_tick_size: u16,
        new_tick_size: u16,
    },
    /// Parser error rate exceeded threshold
    ParserErrorRateExceeded {
        error_count: u32,
        window_ms: u64,
        threshold: u32,
    },
    /// Repeated order rejects/timeouts
    OrderRejectRateExceeded {
        reject_count: u32,
        window_ms: u64,
        threshold: u32,
    },
    /// Clock anomaly (exchange timestamps moving backward)
    ClockAnomaly {
        expected_min_ts: u64,
        received_ts: u64,
        tolerance_ms: u64,
    },
    /// Manual kill switch activated
    ManualKill { reason: String },
    /// Risk limit breached
    RiskLimitBreached { limit_type: String },
    /// WS disconnected
    WsDisconnected,
}

/// Types of reconciliation mismatches
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReconciliationMismatch {
    BestBidMismatch {
        local: Option<u16>,
        remote: Option<u16>,
    },
    BestAskMismatch {
        local: Option<u16>,
        remote: Option<u16>,
    },
    LevelSizeMismatch {
        tick: u16,
        local: u64,
        remote: u64,
    },
    TickSizeMismatch {
        local: u16,
        remote: u16,
    },
}

/// System health state
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HealthState {
    /// System is healthy, trading allowed
    Healthy,
    /// System is in SAFE_MODE, only cancels allowed
    SafeMode { reasons: Vec<SafeModeReason> },
}

impl HealthState {
    pub fn is_healthy(&self) -> bool {
        matches!(self, HealthState::Healthy)
    }

    pub fn is_safe_mode(&self) -> bool {
        matches!(self, HealthState::SafeMode { .. })
    }
}

/// Configuration for health monitoring thresholds
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthConfig {
    /// Maximum WS lag before SAFE_MODE (milliseconds)
    pub max_ws_lag_ms: u64,
    /// Parser error threshold (count per window)
    pub parser_error_threshold: u32,
    /// Parser error window (milliseconds)
    pub parser_error_window_ms: u64,
    /// Order reject threshold (count per window)
    pub order_reject_threshold: u32,
    /// Order reject window (milliseconds)
    pub order_reject_window_ms: u64,
    /// Clock backward tolerance (milliseconds)
    pub clock_backward_tolerance_ms: u64,
    /// REST reconciliation interval (milliseconds)
    pub reconciliation_interval_ms: u64,
    /// Number of top levels to compare in reconciliation
    pub reconciliation_depth: usize,
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self {
            max_ws_lag_ms: 500,
            parser_error_threshold: 5,
            parser_error_window_ms: 60_000,
            order_reject_threshold: 10,
            order_reject_window_ms: 60_000,
            clock_backward_tolerance_ms: 1_000,
            reconciliation_interval_ms: 30_000,
            reconciliation_depth: 20,
        }
    }
}

/// Rolling window counter for rate limiting
#[derive(Debug, Clone)]
struct RollingCounter {
    events: Vec<u64>, // timestamps of events
    window_ms: u64,
}

impl RollingCounter {
    fn new(window_ms: u64) -> Self {
        Self {
            events: Vec::new(),
            window_ms,
        }
    }

    fn record(&mut self, now_ms: u64) {
        self.events.push(now_ms);
        self.prune(now_ms);
    }

    fn count(&mut self, now_ms: u64) -> u32 {
        self.prune(now_ms);
        self.events.len() as u32
    }

    fn prune(&mut self, now_ms: u64) {
        let cutoff = now_ms.saturating_sub(self.window_ms);
        self.events.retain(|&ts| ts > cutoff);
    }

    fn clear(&mut self) {
        self.events.clear();
    }
}

/// Per-token health state
#[derive(Debug)]
struct TokenHealth {
    book_synced: bool,
    last_exchange_ts: Option<u64>,
}

impl Default for TokenHealth {
    fn default() -> Self {
        Self {
            book_synced: false,
            last_exchange_ts: None,
        }
    }
}

/// System health monitor
#[derive(Debug)]
pub struct SystemHealth {
    config: HealthConfig,
    state: HealthState,
    /// Per-token health tracking
    token_health: HashMap<TokenId, TokenHealth>,
    /// Parser error counter
    parser_errors: RollingCounter,
    /// Order reject counter  
    order_rejects: RollingCounter,
    /// Current WS lag
    ws_lag_ms: u64,
    /// WS connected status
    ws_connected: bool,
    /// Active SAFE_MODE reasons (may have multiple)
    active_reasons: Vec<SafeModeReason>,
}

impl SystemHealth {
    pub fn new(config: HealthConfig) -> Self {
        Self {
            parser_errors: RollingCounter::new(config.parser_error_window_ms),
            order_rejects: RollingCounter::new(config.order_reject_window_ms),
            config,
            state: HealthState::Healthy,
            token_health: HashMap::new(),
            ws_lag_ms: 0,
            ws_connected: false,
            active_reasons: Vec::new(),
        }
    }

    /// Get current health state
    pub fn state(&self) -> &HealthState {
        &self.state
    }

    /// Check if new order placement is allowed
    pub fn can_place_orders(&self) -> bool {
        self.state.is_healthy()
    }

    /// Check if cancel orders are allowed (always true, even in SAFE_MODE)
    pub fn can_cancel_orders(&self) -> bool {
        true
    }

    /// Record that a token's book is now synced
    pub fn set_book_synced(&mut self, token_id: &TokenId, synced: bool, now_ms: u64) {
        let health = self.token_health.entry(token_id.clone()).or_default();
        health.book_synced = synced;

        if !synced {
            self.add_reason(SafeModeReason::BookNotSynced {
                token_id: token_id.clone(),
            });
        } else {
            self.remove_reason_matching(
                |r| matches!(r, SafeModeReason::BookNotSynced { token_id: t } if t == token_id),
            );
        }

        self.update_state(now_ms);
    }

    /// Record WS connection status
    pub fn set_ws_connected(&mut self, connected: bool, now_ms: u64) {
        self.ws_connected = connected;

        if !connected {
            self.add_reason(SafeModeReason::WsDisconnected);
        } else {
            self.remove_reason_matching(|r| matches!(r, SafeModeReason::WsDisconnected));
        }

        self.update_state(now_ms);
    }

    /// Record WS lag measurement
    pub fn record_ws_lag(&mut self, lag_ms: u64, now_ms: u64) {
        self.ws_lag_ms = lag_ms;

        if lag_ms > self.config.max_ws_lag_ms {
            self.add_reason(SafeModeReason::WsLagExceeded {
                lag_ms,
                threshold_ms: self.config.max_ws_lag_ms,
            });
        } else {
            self.remove_reason_matching(|r| matches!(r, SafeModeReason::WsLagExceeded { .. }));
        }

        self.update_state(now_ms);
    }

    /// Record a parser error
    pub fn record_parser_error(&mut self, now_ms: u64) {
        self.parser_errors.record(now_ms);
        let count = self.parser_errors.count(now_ms);

        if count >= self.config.parser_error_threshold {
            self.add_reason(SafeModeReason::ParserErrorRateExceeded {
                error_count: count,
                window_ms: self.config.parser_error_window_ms,
                threshold: self.config.parser_error_threshold,
            });
        }

        self.update_state(now_ms);
    }

    /// Record an order reject/timeout
    pub fn record_order_reject(&mut self, now_ms: u64) {
        self.order_rejects.record(now_ms);
        let count = self.order_rejects.count(now_ms);

        if count >= self.config.order_reject_threshold {
            self.add_reason(SafeModeReason::OrderRejectRateExceeded {
                reject_count: count,
                window_ms: self.config.order_reject_window_ms,
                threshold: self.config.order_reject_threshold,
            });
        }

        self.update_state(now_ms);
    }

    /// Record tick size change (always triggers SAFE_MODE until resnapshot)
    pub fn record_tick_size_change(
        &mut self,
        token_id: &TokenId,
        old_tick_size: u16,
        new_tick_size: u16,
        now_ms: u64,
    ) {
        // Mark book as not synced
        self.set_book_synced(token_id, false, now_ms);

        self.add_reason(SafeModeReason::TickSizeChange {
            token_id: token_id.clone(),
            old_tick_size,
            new_tick_size,
        });

        self.update_state(now_ms);
    }

    /// Clear tick size change reason after successful resnapshot
    pub fn clear_tick_size_change(&mut self, token_id: &TokenId, now_ms: u64) {
        self.remove_reason_matching(
            |r| matches!(r, SafeModeReason::TickSizeChange { token_id: t, .. } if t == token_id),
        );
        self.update_state(now_ms);
    }

    /// Record REST reconciliation mismatch
    pub fn record_reconciliation_mismatch(
        &mut self,
        token_id: &TokenId,
        mismatch: ReconciliationMismatch,
        now_ms: u64,
    ) {
        self.set_book_synced(token_id, false, now_ms);

        self.add_reason(SafeModeReason::RestReconciliationMismatch {
            token_id: token_id.clone(),
            mismatch_type: mismatch,
        });

        self.update_state(now_ms);
    }

    /// Check exchange timestamp for clock anomaly
    pub fn check_clock(&mut self, token_id: &TokenId, exchange_ts: u64, now_ms: u64) -> bool {
        let health = self.token_health.entry(token_id.clone()).or_default();

        if let Some(last_ts) = health.last_exchange_ts {
            // Allow some tolerance for clock skew
            let min_expected = last_ts.saturating_sub(self.config.clock_backward_tolerance_ms);

            if exchange_ts < min_expected {
                self.add_reason(SafeModeReason::ClockAnomaly {
                    expected_min_ts: min_expected,
                    received_ts: exchange_ts,
                    tolerance_ms: self.config.clock_backward_tolerance_ms,
                });
                self.update_state(now_ms);
                return false;
            }
        }

        health.last_exchange_ts = Some(exchange_ts);
        true
    }

    /// Manually trigger SAFE_MODE
    pub fn trigger_kill_switch(&mut self, reason: String, now_ms: u64) {
        self.add_reason(SafeModeReason::ManualKill { reason });
        self.update_state(now_ms);
    }

    /// Record risk limit breach
    pub fn record_risk_breach(&mut self, limit_type: String, now_ms: u64) {
        self.add_reason(SafeModeReason::RiskLimitBreached { limit_type });
        self.update_state(now_ms);
    }

    /// Attempt to recover from SAFE_MODE (call after resolving issues)
    pub fn attempt_recovery(&mut self, now_ms: u64) -> bool {
        // Prune time-based counters
        self.parser_errors.prune(now_ms);
        self.order_rejects.prune(now_ms);

        // Check if parser errors are now below threshold
        if self.parser_errors.count(now_ms) < self.config.parser_error_threshold {
            self.remove_reason_matching(|r| {
                matches!(r, SafeModeReason::ParserErrorRateExceeded { .. })
            });
        }

        // Check if order rejects are now below threshold
        if self.order_rejects.count(now_ms) < self.config.order_reject_threshold {
            self.remove_reason_matching(|r| {
                matches!(r, SafeModeReason::OrderRejectRateExceeded { .. })
            });
        }

        // Check WS lag
        if self.ws_lag_ms <= self.config.max_ws_lag_ms {
            self.remove_reason_matching(|r| matches!(r, SafeModeReason::WsLagExceeded { .. }));
        }

        self.update_state(now_ms);
        self.state.is_healthy()
    }

    /// Get current SAFE_MODE reasons (empty if healthy)
    pub fn safe_mode_reasons(&self) -> &[SafeModeReason] {
        &self.active_reasons
    }

    fn add_reason(&mut self, reason: SafeModeReason) {
        // Avoid duplicates of same type
        if !self
            .active_reasons
            .iter()
            .any(|r| std::mem::discriminant(r) == std::mem::discriminant(&reason))
        {
            self.active_reasons.push(reason);
        }
    }

    fn remove_reason_matching<F>(&mut self, predicate: F)
    where
        F: Fn(&SafeModeReason) -> bool,
    {
        self.active_reasons.retain(|r| !predicate(r));
    }

    fn update_state(&mut self, _now_ms: u64) {
        if self.active_reasons.is_empty() {
            self.state = HealthState::Healthy;
        } else {
            self.state = HealthState::SafeMode {
                reasons: self.active_reasons.clone(),
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_health() -> SystemHealth {
        SystemHealth::new(HealthConfig {
            max_ws_lag_ms: 100,
            parser_error_threshold: 3,
            parser_error_window_ms: 1000,
            order_reject_threshold: 5,
            order_reject_window_ms: 1000,
            clock_backward_tolerance_ms: 100,
            reconciliation_interval_ms: 30_000,
            reconciliation_depth: 20,
        })
    }

    #[test]
    fn test_initial_state_healthy() {
        let health = make_health();
        assert!(health.state().is_healthy());
        assert!(health.can_place_orders());
        assert!(health.can_cancel_orders());
    }

    #[test]
    fn test_book_not_synced_triggers_safe_mode() {
        let mut health = make_health();
        let token = TokenId("123".to_string());

        health.set_book_synced(&token, false, 1000);

        assert!(health.state().is_safe_mode());
        assert!(!health.can_place_orders());
        assert!(health.can_cancel_orders()); // Cancels always allowed
    }

    #[test]
    fn test_book_synced_clears_safe_mode() {
        let mut health = make_health();
        let token = TokenId("123".to_string());

        health.set_book_synced(&token, false, 1000);
        assert!(health.state().is_safe_mode());

        health.set_book_synced(&token, true, 2000);
        assert!(health.state().is_healthy());
    }

    #[test]
    fn test_ws_lag_triggers_safe_mode() {
        let mut health = make_health();

        health.record_ws_lag(50, 1000); // Below threshold
        assert!(health.state().is_healthy());

        health.record_ws_lag(150, 2000); // Above threshold
        assert!(health.state().is_safe_mode());

        health.record_ws_lag(50, 3000); // Back below
        health.attempt_recovery(3000);
        assert!(health.state().is_healthy());
    }

    #[test]
    fn test_parser_errors_trigger_safe_mode() {
        let mut health = make_health();

        health.record_parser_error(1000);
        health.record_parser_error(1001);
        assert!(health.state().is_healthy()); // Below threshold (3)

        health.record_parser_error(1002);
        assert!(health.state().is_safe_mode()); // At threshold
    }

    #[test]
    fn test_parser_errors_expire() {
        let mut health = make_health();

        health.record_parser_error(1000);
        health.record_parser_error(1001);
        health.record_parser_error(1002);
        assert!(health.state().is_safe_mode());

        // After window expires
        health.attempt_recovery(3000); // 2 seconds later, window is 1s
        assert!(health.state().is_healthy());
    }

    #[test]
    fn test_tick_size_change_triggers_safe_mode() {
        let mut health = make_health();
        let token = TokenId("123".to_string());

        // First sync the book
        health.set_book_synced(&token, true, 1000);
        assert!(health.state().is_healthy());

        // Tick size change
        health.record_tick_size_change(&token, 100, 10, 2000);
        assert!(health.state().is_safe_mode());

        // After resnapshot
        health.clear_tick_size_change(&token, 3000);
        health.set_book_synced(&token, true, 3000);
        assert!(health.state().is_healthy());
    }

    #[test]
    fn test_clock_anomaly() {
        let mut health = make_health();
        let token = TokenId("123".to_string());

        // First timestamp
        assert!(health.check_clock(&token, 1000, 1000));

        // Normal forward movement
        assert!(health.check_clock(&token, 1500, 1500));

        // Small backward (within tolerance)
        assert!(health.check_clock(&token, 1450, 2000));

        // Large backward (outside tolerance)
        assert!(!health.check_clock(&token, 1000, 2500));
        assert!(health.state().is_safe_mode());
    }

    #[test]
    fn test_multiple_reasons_accumulate() {
        let mut health = make_health();
        let token = TokenId("123".to_string());

        health.set_book_synced(&token, false, 1000);
        health.record_ws_lag(150, 1000);

        assert!(health.state().is_safe_mode());
        assert_eq!(health.safe_mode_reasons().len(), 2);

        // Fix one
        health.set_book_synced(&token, true, 2000);
        assert!(health.state().is_safe_mode()); // Still in safe mode
        assert_eq!(health.safe_mode_reasons().len(), 1);

        // Fix the other
        health.record_ws_lag(50, 3000);
        health.attempt_recovery(3000);
        assert!(health.state().is_healthy());
    }

    #[test]
    fn test_manual_kill_switch() {
        let mut health = make_health();

        health.trigger_kill_switch("Testing".to_string(), 1000);
        assert!(health.state().is_safe_mode());

        // Manual kill requires explicit clearing (not automatic recovery)
        health.attempt_recovery(2000);
        assert!(health.state().is_safe_mode());
    }

    #[test]
    fn test_ws_disconnect() {
        let mut health = make_health();

        health.set_ws_connected(true, 1000);
        assert!(health.state().is_healthy());

        health.set_ws_connected(false, 2000);
        assert!(health.state().is_safe_mode());

        health.set_ws_connected(true, 3000);
        assert!(health.state().is_healthy());
    }
}
