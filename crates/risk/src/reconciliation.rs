//! Position reconciliation with auto-correction.
//!
//! Detects and auto-corrects discrepancies between expected (internal) and
//! actual (exchange) positions to prevent drift.

use mtrader_core::TokenId;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Configuration for position reconciliation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReconciliationConfig {
    /// How often to reconcile in milliseconds (default: 60000 = 1 minute)
    pub check_interval_ms: u64,
    /// Maximum acceptable position difference (default: 0.01)
    pub max_discrepancy: f64,
    /// Whether to automatically fix discrepancies
    pub auto_correct: bool,
    /// Whether to alert on any discrepancy
    pub alert_on_discrepancy: bool,
}

impl Default for ReconciliationConfig {
    fn default() -> Self {
        Self {
            check_interval_ms: 60_000,
            max_discrepancy: 0.01,
            auto_correct: true,
            alert_on_discrepancy: true,
        }
    }
}

/// Represents a discrepancy between expected and actual positions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PositionDiscrepancy {
    /// Token identifier
    pub token_id: TokenId,
    /// Expected position from internal tracking
    pub expected_position: f64,
    /// Actual position from exchange
    pub actual_position: f64,
    /// Difference (expected - actual)
    pub discrepancy: f64,
    /// Timestamp of detection (Unix epoch milliseconds)
    pub timestamp: i64,
}

impl PositionDiscrepancy {
    /// Create a new discrepancy record.
    pub fn new(token_id: TokenId, expected: f64, actual: f64, timestamp: i64) -> Self {
        let discrepancy = expected - actual;
        Self {
            token_id,
            expected_position: expected,
            actual_position: actual,
            discrepancy,
            timestamp,
        }
    }

    /// Check if discrepancy is within tolerance.
    pub fn is_within_tolerance(&self, max_discrepancy: f64) -> bool {
        self.discrepancy.abs() <= max_discrepancy
    }

    /// Get the absolute value of the discrepancy.
    pub fn abs_discrepancy(&self) -> f64 {
        self.discrepancy.abs()
    }
}

/// Actions to take when a discrepancy is detected.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ReconciliationAction {
    /// No action needed
    NoAction,
    /// Log a warning
    LogWarning,
    /// Correct the position by the adjustment amount
    CorrectPosition { adjustment: f64 },
    /// Emergency close due to large discrepancy
    EmergencyClose { reason: String },
}

/// The primary reconciliator for position consistency.
#[derive(Debug, Clone)]
pub struct PositionReconciliator {
    /// Configuration for reconciliation
    config: ReconciliationConfig,
    /// History of detected discrepancies (limited in size)
    discrepancy_history: Vec<PositionDiscrepancy>,
    /// Maximum history size
    max_history_size: usize,
    /// Timestamp of last reconciliation
    last_reconciliation_timestamp: Option<i64>,
}

impl PositionReconciliator {
    /// Create a new reconciliator with the given config.
    pub fn new(config: ReconciliationConfig) -> Self {
        Self {
            config,
            discrepancy_history: Vec::new(),
            max_history_size: 1000,
            last_reconciliation_timestamp: None,
        }
    }

    /// Create a reconciliator with default configuration.
    pub fn default() -> Self {
        Self::new(ReconciliationConfig::default())
    }

    /// Reconcile expected vs actual positions and return any discrepancies.
    ///
    /// # Arguments
    /// * `expected` - Map of token_id to expected position from internal tracking
    /// * `actual` - Map of token_id to actual position from exchange
    ///
    /// # Returns
    /// Vector of detected discrepancies
    pub fn reconcile(
        &mut self,
        expected: &HashMap<TokenId, f64>,
        actual: &HashMap<TokenId, f64>,
    ) -> Vec<PositionDiscrepancy> {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        let mut discrepancies = Vec::new();

        // Check all tokens in expected map
        for (token_id, &expected_pos) in expected {
            let actual_pos = actual.get(token_id).copied().unwrap_or(0.0);
            let discrepancy = expected_pos - actual_pos;

            if discrepancy.abs() > self.config.max_discrepancy {
                let disc = PositionDiscrepancy::new(
                    token_id.clone(),
                    expected_pos,
                    actual_pos,
                    timestamp,
                );
                discrepancies.push(disc.clone());
                self.add_discrepancy(disc);
            }
        }

        // Check for tokens in actual but not in expected
        for (token_id, &actual_pos) in actual {
            if !expected.contains_key(token_id) && actual_pos.abs() > self.config.max_discrepancy {
                let disc = PositionDiscrepancy::new(
                    token_id.clone(),
                    0.0,
                    actual_pos,
                    timestamp,
                );
                discrepancies.push(disc.clone());
                self.add_discrepancy(disc);
            }
        }

        self.last_reconciliation_timestamp = Some(timestamp);
        discrepancies
    }

    /// Determine the action to take for a given discrepancy.
    pub fn determine_action(&self, discrepancy: &PositionDiscrepancy) -> ReconciliationAction {
        let abs_discrepancy = discrepancy.abs_discrepancy();
        let max_disc = self.config.max_discrepancy;

        // Small discrepancy (< max_discrepancy): No action needed
        if abs_discrepancy <= max_disc {
            return ReconciliationAction::NoAction;
        }

        // Medium discrepancy (1x - 10x max): Log warning + correct if auto_correct enabled
        if abs_discrepancy <= max_disc * 10.0 {
            if self.config.auto_correct {
                return ReconciliationAction::CorrectPosition {
                    adjustment: discrepancy.discrepancy,
                };
            }
            return ReconciliationAction::LogWarning;
        }

        // Large discrepancy (> 10x max): Emergency close
        ReconciliationAction::EmergencyClose {
            reason: format!(
                "Large position discrepancy detected for token {}: expected={}, actual={}, diff={}",
                discrepancy.token_id,
                discrepancy.expected_position,
                discrepancy.actual_position,
                discrepancy.discrepancy
            ),
        }
    }

    /// Apply a correction for a discrepancy.
    ///
    /// In a real system, this would trigger position correction logic.
    /// For this implementation, we record the correction intent.
    pub fn apply_correction(
        &mut self,
        discrepancy: &PositionDiscrepancy,
    ) -> Result<(), String> {
        let action = self.determine_action(discrepancy);

        match action {
            ReconciliationAction::CorrectPosition { adjustment } => {
                // In a real system, this would:
                // 1. Create a correction order to adjust the position
                // 2. Log the correction event
                // 3. Update internal position tracking
                tracing::info!(
                    "Applying position correction for token {}: adjustment = {}",
                    discrepancy.token_id,
                    adjustment
                );
                Ok(())
            }
            ReconciliationAction::EmergencyClose { reason } => {
                // In a real system, this would:
                // 1. Trigger circuit breaker
                // 2. Close all positions for this token
                // 3. Send alerts
                tracing::error!("Emergency close triggered: {}", reason);
                Err(reason)
            }
            ReconciliationAction::LogWarning => {
                tracing::warn!(
                    "Position discrepancy warning for token {}: expected={}, actual={}",
                    discrepancy.token_id,
                    discrepancy.expected_position,
                    discrepancy.actual_position
                );
                Ok(())
            }
            ReconciliationAction::NoAction => {
                // No action needed, discrepancy is within tolerance
                Ok(())
            }
        }
    }

    /// Get reference to discrepancy history.
    pub fn discrepancy_history(&self) -> &[PositionDiscrepancy] {
        &self.discrepancy_history
    }

    /// Get the timestamp of the last reconciliation.
    pub fn last_reconciliation_timestamp(&self) -> Option<i64> {
        self.last_reconciliation_timestamp
    }

    /// Get the configuration.
    pub fn config(&self) -> &ReconciliationConfig {
        &self.config
    }

    /// Add a discrepancy to the history, maintaining max size.
    fn add_discrepancy(&mut self, discrepancy: PositionDiscrepancy) {
        if self.discrepancy_history.len() >= self.max_history_size {
            self.discrepancy_history.remove(0);
        }
        self.discrepancy_history.push(discrepancy);
    }

    /// Clear the discrepancy history.
    pub fn clear_history(&mut self) {
        self.discrepancy_history.clear();
    }

    /// Get the count of discrepancies in history.
    pub fn discrepancy_count(&self) -> usize {
        self.discrepancy_history.len()
    }

    /// Check if a discrepancy exists for the given token.
    pub fn has_discrepancy(&self, token_id: &TokenId) -> bool {
        self.discrepancy_history.iter().any(|d| &d.token_id == token_id)
    }

    /// Get the most recent discrepancy for a token.
    pub fn latest_discrepancy(&self, token_id: &TokenId) -> Option<&PositionDiscrepancy> {
        self.discrepancy_history
            .iter()
            .rev()
            .find(|d| &d.token_id == token_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mtrader_core::TokenId;

    fn token(id: &str) -> TokenId {
        TokenId(id.to_string())
    }

    #[test]
    fn test_reconciliation_no_discrepancy() {
        let mut reconciliator = PositionReconciliator::default();

        let mut expected = HashMap::new();
        expected.insert(token("token-1"), 100.0);
        expected.insert(token("token-2"), -50.0);

        let mut actual = HashMap::new();
        actual.insert(token("token-1"), 100.0);
        actual.insert(token("token-2"), -50.0);

        let discrepancies = reconciliator.reconcile(&expected, &actual);
        assert!(discrepancies.is_empty());
        assert_eq!(reconciliator.discrepancy_count(), 0);
    }

    #[test]
    fn test_reconciliation_small_discrepancy_within_tolerance() {
        let mut reconciliator = PositionReconciliator::default();

        let mut expected = HashMap::new();
        expected.insert(token("token-1"), 100.0);

        let mut actual = HashMap::new();
        actual.insert(token("token-1"), 100.005); // Within tolerance of 0.01

        let discrepancies = reconciliator.reconcile(&expected, &actual);
        assert!(discrepancies.is_empty());
    }

    #[test]
    fn test_reconciliation_detects_discrepancy() {
        let config = ReconciliationConfig {
            max_discrepancy: 0.01,
            auto_correct: true,
            ..Default::default()
        };
        let mut reconciliator = PositionReconciliator::new(config);

        let mut expected = HashMap::new();
        expected.insert(token("token-1"), 100.0);

        let mut actual = HashMap::new();
        actual.insert(token("token-1"), 99.98); // Discrepancy of 0.02

        let discrepancies = reconciliator.reconcile(&expected, &actual);
        assert_eq!(discrepancies.len(), 1);
        assert_eq!(discrepancies[0].token_id, token("token-1"));
        assert!((discrepancies[0].discrepancy - 0.02).abs() < 0.001);
    }

    #[test]
    fn test_reconciliation_missing_in_actual() {
        let mut reconciliator = PositionReconciliator::default();

        let mut expected = HashMap::new();
        expected.insert(token("token-1"), 100.0);

        let mut actual = HashMap::new();
        // token-1 not in actual

        let discrepancies = reconciliator.reconcile(&expected, &actual);
        assert_eq!(discrepancies.len(), 1);
        assert_eq!(discrepancies[0].discrepancy, 100.0);
    }

    #[test]
    fn test_reconciliation_missing_in_expected() {
        let mut reconciliator = PositionReconciliator::default();

        let mut expected = HashMap::new();

        let mut actual = HashMap::new();
        actual.insert(token("token-1"), 50.0);

        let discrepancies = reconciliator.reconcile(&expected, &actual);
        assert_eq!(discrepancies.len(), 1);
        assert_eq!(discrepancies[0].expected_position, 0.0);
        assert_eq!(discrepancies[0].actual_position, 50.0);
    }

    #[test]
    fn test_determine_action_no_action() {
        let reconciliator = PositionReconciliator::default();

        let discrepancy = PositionDiscrepancy::new(token("token-1"), 100.0, 100.005, 0);
        let action = reconciliator.determine_action(&discrepancy);
        assert_eq!(action, ReconciliationAction::NoAction);
    }

    #[test]
    fn test_determine_action_small_auto_correct() {
        let config = ReconciliationConfig {
            max_discrepancy: 0.01,
            auto_correct: true,
            ..Default::default()
        };
        let reconciliator = PositionReconciliator::new(config);

        let discrepancy = PositionDiscrepancy::new(token("token-1"), 100.0, 99.98, 0);
        let action = reconciliator.determine_action(&discrepancy);
        match action {
            ReconciliationAction::CorrectPosition { adjustment } => {
                assert!((adjustment - 0.02).abs() < 0.001);
            }
            _ => panic!("Expected CorrectPosition action"),
        }
    }

    #[test]
    fn test_determine_action_small_log_warning() {
        let config = ReconciliationConfig {
            max_discrepancy: 0.01,
            auto_correct: false,
            ..Default::default()
        };
        let reconciliator = PositionReconciliator::new(config);

        let discrepancy = PositionDiscrepancy::new(token("token-1"), 100.0, 99.98, 0);
        let action = reconciliator.determine_action(&discrepancy);
        assert_eq!(action, ReconciliationAction::LogWarning);
    }

    #[test]
    fn test_determine_action_large_emergency_close() {
        let reconciliator = PositionReconciliator::default();

        let discrepancy = PositionDiscrepancy::new(token("token-1"), 100.0, 50.0, 0);
        let action = reconciliator.determine_action(&discrepancy);
        match action {
            ReconciliationAction::EmergencyClose { reason } => {
                assert!(reason.contains("Large position discrepancy"));
            }
            _ => panic!("Expected EmergencyClose action"),
        }
    }

    #[test]
    fn test_apply_correction() {
        let config = ReconciliationConfig {
            max_discrepancy: 0.01,
            auto_correct: true,
            ..Default::default()
        };
        let mut reconciliator = PositionReconciliator::new(config);

        let discrepancy = PositionDiscrepancy::new(token("token-1"), 100.0, 99.98, 0);
        let result = reconciliator.apply_correction(&discrepancy);
        assert!(result.is_ok());
    }

    #[test]
    fn test_discrepancy_history() {
        let mut reconciliator = PositionReconciliator::default();

        let mut expected = HashMap::new();
        let mut actual = HashMap::new();

        expected.insert(token("token-1"), 100.0);
        actual.insert(token("token-1"), 99.98);
        reconciliator.reconcile(&expected, &actual);

        expected.insert(token("token-2"), 50.0);
        // Fix token-1 to be within tolerance to avoid adding another discrepancy
        actual.insert(token("token-1"), 100.005);
        actual.insert(token("token-2"), 50.05);
        reconciliator.reconcile(&expected, &actual);

        assert_eq!(reconciliator.discrepancy_count(), 2);
        assert!(reconciliator.has_discrepancy(&token("token-1")));
        assert!(reconciliator.has_discrepancy(&token("token-2")));
        assert!(!reconciliator.has_discrepancy(&token("token-3")));
    }

    #[test]
    fn test_latest_discrepancy() {
        let mut reconciliator = PositionReconciliator::default();

        let mut expected = HashMap::new();
        let mut actual = HashMap::new();

        expected.insert(token("token-1"), 100.0);
        actual.insert(token("token-1"), 99.98);
        reconciliator.reconcile(&expected, &actual);

        // Add slight delay and reconcile again
        std::thread::sleep(std::time::Duration::from_millis(10));
        expected.insert(token("token-1"), 100.0);
        actual.insert(token("token-1"), 99.95);
        reconciliator.reconcile(&expected, &actual);

        let latest = reconciliator.latest_discrepancy(&token("token-1"));
        assert!(latest.is_some());
        assert!((latest.unwrap().discrepancy - 0.05).abs() < 0.001);
    }

    #[test]
    fn test_clear_history() {
        let mut reconciliator = PositionReconciliator::default();

        let mut expected = HashMap::new();
        let mut actual = HashMap::new();
        expected.insert(token("token-1"), 100.0);
        actual.insert(token("token-1"), 99.98);
        reconciliator.reconcile(&expected, &actual);

        assert_eq!(reconciliator.discrepancy_count(), 1);

        reconciliator.clear_history();
        assert_eq!(reconciliator.discrepancy_count(), 0);
    }

    #[test]
    fn test_multiple_token_reconciliation() {
        let mut reconciliator = PositionReconciliator::default();

        let mut expected = HashMap::new();
        expected.insert(token("token-1"), 100.0);
        expected.insert(token("token-2"), -50.0);
        expected.insert(token("token-3"), 25.0);

        let mut actual = HashMap::new();
        actual.insert(token("token-1"), 100.0);           // No discrepancy
        actual.insert(token("token-2"), -49.98);          // Small discrepancy
        actual.insert(token("token-3"), 24.95);           // Larger discrepancy

        let discrepancies = reconciliator.reconcile(&expected, &actual);
        assert_eq!(discrepancies.len(), 2); // token-2 and token-3

        // Verify the discrepancies are for correct tokens
        let token_ids: Vec<_> = discrepancies.iter().map(|d| d.token_id.clone()).collect();
        assert!(token_ids.contains(&token("token-2")));
        assert!(token_ids.contains(&token("token-3")));
    }

    #[test]
    fn test_edge_case_zero_positions() {
        let mut reconciliator = PositionReconciliator::default();

        let mut expected = HashMap::new();
        expected.insert(token("token-1"), 0.0);

        let mut actual = HashMap::new();
        actual.insert(token("token-1"), 0.0);

        let discrepancies = reconciliator.reconcile(&expected, &actual);
        assert!(discrepancies.is_empty());
    }

    #[test]
    fn test_edge_case_opposite_positions() {
        let mut reconciliator = PositionReconciliator::default();

        let mut expected = HashMap::new();
        expected.insert(token("token-1"), 100.0);

        let mut actual = HashMap::new();
        actual.insert(token("token-1"), -100.0); // Completely opposite!

        let discrepancies = reconciliator.reconcile(&expected, &actual);
        assert_eq!(discrepancies.len(), 1);
        assert_eq!(discrepancies[0].discrepancy, 200.0);
    }

    #[test]
    fn test_discrepancy_is_within_tolerance() {
        let discrepancy = PositionDiscrepancy::new(token("token-1"), 100.0, 100.005, 0);
        assert!(discrepancy.is_within_tolerance(0.01));

        let discrepancy2 = PositionDiscrepancy::new(token("token-1"), 100.0, 100.02, 0);
        assert!(!discrepancy2.is_within_tolerance(0.01));
    }

    #[test]
    fn test_discrepancy_abs() {
        let discrepancy = PositionDiscrepancy::new(token("token-1"), 100.0, 99.98, 0);
        assert!((discrepancy.abs_discrepancy() - 0.02).abs() < 0.001);

        let discrepancy2 = PositionDiscrepancy::new(token("token-1"), 99.98, 100.0, 0);
        assert!((discrepancy2.abs_discrepancy() - 0.02).abs() < 0.001);
    }
}
