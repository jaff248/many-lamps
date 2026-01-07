//! Signal processing for strategies.

use mtrader_core::Tick;

/// A trading signal.
#[derive(Debug, Clone, Copy)]
pub struct Signal {
    /// Signal value (-1.0 to 1.0, negative = bearish, positive = bullish)
    pub value: f64,
    /// Signal strength/confidence (0.0 to 1.0)
    pub strength: f64,
    /// Signal timestamp (mono ns)
    pub timestamp_ns: u64,
}

impl Signal {
    pub fn new(value: f64, strength: f64, timestamp_ns: u64) -> Self {
        Self {
            value: value.clamp(-1.0, 1.0),
            strength: strength.clamp(0.0, 1.0),
            timestamp_ns,
        }
    }

    pub fn neutral(timestamp_ns: u64) -> Self {
        Self::new(0.0, 0.0, timestamp_ns)
    }

    /// Check if signal is bullish.
    pub fn is_bullish(&self) -> bool {
        self.value > 0.0 && self.strength > 0.1
    }

    /// Check if signal is bearish.
    pub fn is_bearish(&self) -> bool {
        self.value < 0.0 && self.strength > 0.1
    }

    /// Check if signal is strong.
    pub fn is_strong(&self) -> bool {
        self.strength > 0.5
    }
}

/// Signal processor with EMA smoothing.
pub struct SignalProcessor {
    /// EMA decay factor (0.0 to 1.0)
    alpha: f64,
    /// Current EMA value
    ema: Option<f64>,
    /// Last raw signal
    last_raw: Option<f64>,
    /// History for analysis
    history: Vec<Signal>,
    /// Max history size
    max_history: usize,
}

impl SignalProcessor {
    pub fn new(alpha: f64) -> Self {
        Self {
            alpha: alpha.clamp(0.01, 1.0),
            ema: None,
            last_raw: None,
            history: Vec::new(),
            max_history: 1000,
        }
    }

    /// Process a new raw signal value.
    pub fn process(&mut self, raw_value: f64, timestamp_ns: u64) -> Signal {
        let smoothed = match self.ema {
            Some(prev) => self.alpha * raw_value + (1.0 - self.alpha) * prev,
            None => raw_value,
        };
        self.ema = Some(smoothed);
        self.last_raw = Some(raw_value);

        // Calculate strength based on consistency
        let strength = self.calculate_strength(raw_value, smoothed);

        let signal = Signal::new(smoothed, strength, timestamp_ns);

        if self.history.len() >= self.max_history {
            self.history.remove(0);
        }
        self.history.push(signal);

        signal
    }

    fn calculate_strength(&self, raw: f64, smoothed: f64) -> f64 {
        // Strength based on:
        // 1. Absolute value (stronger signals = higher strength)
        // 2. Consistency (raw close to smoothed = higher strength)
        let magnitude = smoothed.abs();
        let consistency = 1.0 - (raw - smoothed).abs().min(1.0);

        (magnitude * 0.5 + consistency * 0.5).clamp(0.0, 1.0)
    }

    /// Get current smoothed value.
    pub fn current(&self) -> Option<f64> {
        self.ema
    }

    /// Get signal history.
    pub fn history(&self) -> &[Signal] {
        &self.history
    }

    /// Reset the processor.
    pub fn reset(&mut self) {
        self.ema = None;
        self.last_raw = None;
        self.history.clear();
    }
}

/// Price momentum signal generator.
pub struct MomentumSignal {
    /// Lookback window size
    window_size: usize,
    /// Price history
    prices: Vec<Tick>,
    /// Signal processor
    processor: SignalProcessor,
}

impl MomentumSignal {
    pub fn new(window_size: usize, ema_alpha: f64) -> Self {
        Self {
            window_size,
            prices: Vec::with_capacity(window_size + 1),
            processor: SignalProcessor::new(ema_alpha),
        }
    }

    /// Update with new price.
    pub fn update(&mut self, price_tick: Tick, timestamp_ns: u64) -> Signal {
        self.prices.push(price_tick);
        if self.prices.len() > self.window_size + 1 {
            self.prices.remove(0);
        }

        if self.prices.len() < 2 {
            return Signal::neutral(timestamp_ns);
        }

        // Simple momentum: (current - oldest) / oldest
        let oldest = self.prices[0] as f64;
        let current = *self.prices.last().unwrap() as f64;

        if oldest == 0.0 {
            return Signal::neutral(timestamp_ns);
        }

        let raw_momentum = (current - oldest) / oldest;
        // Scale to -1..1 range (assume max 10% move is extreme)
        let scaled = (raw_momentum * 10.0).clamp(-1.0, 1.0);

        self.processor.process(scaled, timestamp_ns)
    }

    /// Reset the signal generator.
    pub fn reset(&mut self) {
        self.prices.clear();
        self.processor.reset();
    }
}

/// Spread-based signal (wide spread = low confidence).
pub struct SpreadSignal {
    /// Normal spread in ticks
    normal_spread: u16,
}

impl SpreadSignal {
    pub fn new(normal_spread: u16) -> Self {
        Self { normal_spread }
    }

    /// Generate signal based on current spread.
    pub fn generate(&self, spread_ticks: u16, timestamp_ns: u64) -> Signal {
        // Wider spread = lower confidence/strength
        // Very wide spread might indicate adverse selection
        let ratio = spread_ticks as f64 / self.normal_spread as f64;

        let strength = if ratio <= 1.0 {
            1.0 // Normal or tight spread
        } else if ratio <= 2.0 {
            1.0 - (ratio - 1.0) * 0.5 // Gradually reduce
        } else {
            0.0 // Too wide, no confidence
        };

        // Spread itself doesn't give direction, just confidence
        Signal::new(0.0, strength, timestamp_ns)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_signal_creation() {
        let signal = Signal::new(0.5, 0.8, 1000);
        assert!(signal.is_bullish());
        assert!(signal.is_strong());

        let signal = Signal::new(-0.3, 0.6, 1000);
        assert!(signal.is_bearish());
        assert!(signal.is_strong());
    }

    #[test]
    fn test_signal_processor() {
        let mut processor = SignalProcessor::new(0.5);

        // First value sets the EMA
        let signal = processor.process(1.0, 1000);
        assert!((signal.value - 1.0).abs() < 0.01);

        // Second value smooths
        let signal = processor.process(0.0, 2000);
        assert!((signal.value - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_momentum_signal() {
        let mut momentum = MomentumSignal::new(5, 0.5);

        // Rising prices
        for i in 0..10 {
            let tick = 50 + i as u16;
            momentum.update(tick, i as u64 * 1000);
        }

        let signal = momentum.update(60, 10000);
        assert!(signal.is_bullish());
    }

    #[test]
    fn test_spread_signal() {
        let spread_signal = SpreadSignal::new(2);

        // Normal spread
        let signal = spread_signal.generate(2, 1000);
        assert!((signal.strength - 1.0).abs() < 0.01);

        // Wide spread
        let signal = spread_signal.generate(4, 2000);
        assert!(signal.strength < 1.0);

        // Very wide spread
        let signal = spread_signal.generate(10, 3000);
        assert!((signal.strength - 0.0).abs() < 0.01);
    }
}
