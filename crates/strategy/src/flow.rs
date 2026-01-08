//! Flow-based signals for strategy decisions.

/// Raw flow metrics used to derive alpha vs impact signals.
#[derive(Debug, Clone, Copy)]
pub struct FlowMetrics {
    /// Signed net aggressive flow (positive = buy pressure).
    pub net_flow: f64,
    /// Rolling traded volume proxy.
    pub rolling_volume: f64,
    /// Market capacity proxy (open interest, collateral, depth).
    pub market_capacity: f64,
    /// Timestamp (mono ns).
    pub timestamp_ns: u64,
}

impl FlowMetrics {
    pub fn new(net_flow: f64, rolling_volume: f64, market_capacity: f64, timestamp_ns: u64) -> Self {
        Self {
            net_flow,
            rolling_volume,
            market_capacity,
            timestamp_ns,
        }
    }
}

/// Configuration for normalizing flow metrics into signals.
#[derive(Debug, Clone, Copy)]
pub struct FlowSignalConfig {
    /// Minimum rolling volume required to trust impact normalization.
    pub min_volume: f64,
    /// Minimum market capacity required to trust alpha normalization.
    pub min_capacity: f64,
    /// Maximum absolute alpha flow value after normalization.
    pub max_abs_alpha: f64,
    /// Maximum absolute impact flow value after normalization.
    pub max_abs_impact: f64,
}

impl Default for FlowSignalConfig {
    fn default() -> Self {
        Self {
            min_volume: 1.0,
            min_capacity: 1.0,
            max_abs_alpha: 1.0,
            max_abs_impact: 1.0,
        }
    }
}

/// Normalized flow signals.
#[derive(Debug, Clone, Copy)]
pub struct FlowSignal {
    /// Alpha-focused flow (Q / M).
    pub alpha_flow: f64,
    /// Impact-focused flow (Q / V).
    pub impact_flow: f64,
    /// Strength for alpha flow signal (0.0 to 1.0).
    pub alpha_strength: f64,
    /// Strength for impact flow signal (0.0 to 1.0).
    pub impact_strength: f64,
    /// Timestamp (mono ns).
    pub timestamp_ns: u64,
}

impl FlowSignal {
    pub fn neutral(timestamp_ns: u64) -> Self {
        Self {
            alpha_flow: 0.0,
            impact_flow: 0.0,
            alpha_strength: 0.0,
            impact_strength: 0.0,
            timestamp_ns,
        }
    }

    pub fn from_metrics(metrics: FlowMetrics, config: FlowSignalConfig) -> Self {
        let alpha_strength = if metrics.market_capacity >= config.min_capacity && config.min_capacity > 0.0
        {
            (metrics.market_capacity / config.min_capacity).min(1.0)
        } else {
            0.0
        };

        let impact_strength = if metrics.rolling_volume >= config.min_volume && config.min_volume > 0.0
        {
            (metrics.rolling_volume / config.min_volume).min(1.0)
        } else {
            0.0
        };

        let alpha_flow = if metrics.market_capacity > 0.0 {
            (metrics.net_flow / metrics.market_capacity)
                .clamp(-config.max_abs_alpha, config.max_abs_alpha)
        } else {
            0.0
        };

        let impact_flow = if metrics.rolling_volume > 0.0 {
            (metrics.net_flow / metrics.rolling_volume)
                .clamp(-config.max_abs_impact, config.max_abs_impact)
        } else {
            0.0
        };

        Self {
            alpha_flow,
            impact_flow,
            alpha_strength,
            impact_strength,
            timestamp_ns: metrics.timestamp_ns,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_flow_signal_normalization() {
        let metrics = FlowMetrics::new(100.0, 200.0, 500.0, 42);
        let signal = FlowSignal::from_metrics(metrics, FlowSignalConfig::default());

        assert!(signal.alpha_flow.abs() <= 1.0);
        assert!(signal.impact_flow.abs() <= 1.0);
        assert!(signal.alpha_strength > 0.0);
        assert!(signal.impact_strength > 0.0);
    }

    #[test]
    fn test_flow_signal_strength_gate() {
        let metrics = FlowMetrics::new(100.0, 0.5, 0.5, 42);
        let signal = FlowSignal::from_metrics(
            metrics,
            FlowSignalConfig {
                min_volume: 1.0,
                min_capacity: 1.0,
                max_abs_alpha: 1.0,
                max_abs_impact: 1.0,
            },
        );

        assert_eq!(signal.alpha_strength, 0.0);
        assert_eq!(signal.impact_strength, 0.0);
    }
}
