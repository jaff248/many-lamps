//! Flow normalization helpers for Polymarket-style markets.
//!
//! Implements the "participation rate fallacy" guidance:
//! - Alpha flow normalized by capacity proxy (Q / M)
//! - Impact flow normalized by volume proxy (Q / V)

use std::collections::VecDeque;

/// Configuration for flow normalization.
#[derive(Debug, Clone, Copy)]
pub struct FlowSignalConfig {
    /// Rolling window size in nanoseconds.
    pub window_ns: u64,
    /// Minimum capacity used to avoid division by zero.
    pub min_capacity: f64,
    /// Minimum volume used to avoid division by zero.
    pub min_volume: f64,
}

impl Default for FlowSignalConfig {
    fn default() -> Self {
        Self {
            window_ns: 30_000_000_000, // 30s
            min_capacity: 1.0,
            min_volume: 1.0,
        }
    }
}

/// Snapshot of flow state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlowSnapshot {
    /// Timestamp (mono ns).
    pub timestamp_ns: u64,
    /// Rolling signed notional flow Q(t).
    pub signed_flow: f64,
    /// Rolling volume V(t).
    pub volume: f64,
    /// Capacity proxy M(t).
    pub capacity: f64,
    /// Alpha flow signal Q / M.
    pub alpha_flow: f64,
    /// Impact flow signal Q / V.
    pub impact_flow: f64,
}

/// Rolling flow calculator for Q / M and Q / V signals.
pub struct FlowSignal {
    config: FlowSignalConfig,
    signed_flow_window: RollingWindow,
    volume_window: RollingWindow,
    last_capacity: f64,
}

impl FlowSignal {
    pub fn new(config: FlowSignalConfig) -> Self {
        Self {
            config,
            signed_flow_window: RollingWindow::new(config.window_ns),
            volume_window: RollingWindow::new(config.window_ns),
            last_capacity: config.min_capacity,
        }
    }

    /// Update the flow windows with a new trade.
    ///
    /// `signed_notional` should be positive for aggressive buys of YES
    /// (or equivalent bullish flow) and negative for aggressive sells.
    /// `trade_notional` should be the absolute notional for the trade.
    pub fn update(
        &mut self,
        signed_notional: f64,
        trade_notional: f64,
        capacity_proxy: f64,
        timestamp_ns: u64,
    ) -> FlowSnapshot {
        let signed_flow = self
            .signed_flow_window
            .push(timestamp_ns, signed_notional);
        let volume = self.volume_window.push(timestamp_ns, trade_notional.abs());

        if capacity_proxy.is_finite() && capacity_proxy > 0.0 {
            self.last_capacity = capacity_proxy;
        }

        let capacity = self.last_capacity.max(self.config.min_capacity);
        let safe_volume = volume.max(self.config.min_volume);

        let alpha_flow = signed_flow / capacity;
        let impact_flow = signed_flow / safe_volume;

        FlowSnapshot {
            timestamp_ns,
            signed_flow,
            volume,
            capacity,
            alpha_flow,
            impact_flow,
        }
    }
}

#[derive(Debug)]
struct RollingWindow {
    window_ns: u64,
    entries: VecDeque<(u64, f64)>,
    sum: f64,
}

impl RollingWindow {
    fn new(window_ns: u64) -> Self {
        Self {
            window_ns,
            entries: VecDeque::new(),
            sum: 0.0,
        }
    }

    fn push(&mut self, timestamp_ns: u64, value: f64) -> f64 {
        self.entries.push_back((timestamp_ns, value));
        self.sum += value;
        self.expire(timestamp_ns);
        self.sum
    }

    fn expire(&mut self, now_ns: u64) {
        let cutoff = now_ns.saturating_sub(self.window_ns);
        while let Some((ts, value)) = self.entries.front().copied() {
            if ts < cutoff {
                self.entries.pop_front();
                self.sum -= value;
            } else {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_flow_snapshot_basic() {
        let mut flow = FlowSignal::new(FlowSignalConfig {
            window_ns: 1_000,
            min_capacity: 1.0,
            min_volume: 1.0,
        });

        let snapshot = flow.update(100.0, 200.0, 1000.0, 10);
        assert_eq!(snapshot.signed_flow, 100.0);
        assert_eq!(snapshot.volume, 200.0);
        assert_eq!(snapshot.capacity, 1000.0);
        assert!((snapshot.alpha_flow - 0.1).abs() < 1e-9);
        assert!((snapshot.impact_flow - 0.5).abs() < 1e-9);
    }

    #[test]
    fn test_flow_window_rolloff() {
        let mut flow = FlowSignal::new(FlowSignalConfig {
            window_ns: 100,
            min_capacity: 1.0,
            min_volume: 1.0,
        });

        flow.update(50.0, 100.0, 1000.0, 0);
        let snapshot = flow.update(-20.0, 40.0, 1000.0, 50);
        assert_eq!(snapshot.signed_flow, 30.0);
        assert_eq!(snapshot.volume, 140.0);

        let snapshot = flow.update(0.0, 0.0, 1000.0, 200);
        assert_eq!(snapshot.signed_flow, 0.0);
        assert_eq!(snapshot.volume, 0.0);
        assert_eq!(snapshot.capacity, 1000.0);
    }

    #[test]
    fn test_flow_minimums() {
        let mut flow = FlowSignal::new(FlowSignalConfig {
            window_ns: 100,
            min_capacity: 10.0,
            min_volume: 5.0,
        });

        let snapshot = flow.update(5.0, 0.0, 0.0, 1);
        assert_eq!(snapshot.capacity, 10.0);
        assert_eq!(snapshot.volume, 0.0);
        assert!((snapshot.alpha_flow - 0.5).abs() < 1e-9);
        assert!((snapshot.impact_flow - 1.0).abs() < 1e-9);
    }
}
