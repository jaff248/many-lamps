//! Monotonic clock for consistent event ordering.
//!
//! All internal timestamps use monotonic time to ensure correct ordering
//! regardless of wall clock adjustments. Exchange timestamps are preserved
//! separately for analytics.

use std::time::Instant;

/// Monotonic clock for local timestamps
#[derive(Debug, Clone)]
pub struct MonotonicClock {
    start: Instant,
}

impl MonotonicClock {
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
        }
    }

    /// Get current monotonic timestamp in nanoseconds since clock start
    pub fn now_ns(&self) -> i64 {
        self.start.elapsed().as_nanos() as i64
    }

    /// Get current monotonic timestamp in milliseconds since clock start
    pub fn now_ms(&self) -> i64 {
        self.start.elapsed().as_millis() as i64
    }

    /// Get current monotonic timestamp in microseconds since clock start
    pub fn now_us(&self) -> i64 {
        self.start.elapsed().as_micros() as i64
    }
}

impl Default for MonotonicClock {
    fn default() -> Self {
        Self::new()
    }
}

/// Wall clock for exchange timestamp comparison
#[derive(Debug, Clone)]
pub struct WallClock;

impl WallClock {
    /// Get current wall clock time in milliseconds since Unix epoch
    pub fn now_ms() -> i64 {
        chrono::Utc::now().timestamp_millis()
    }

    /// Get current wall clock time in seconds since Unix epoch
    pub fn now_secs() -> i64 {
        chrono::Utc::now().timestamp()
    }
}

/// Combined timing context for events
#[derive(Debug, Clone, Copy)]
pub struct TimingContext {
    /// Monotonic timestamp when created
    pub mono_ns: i64,
    /// Wall clock timestamp when created
    pub wall_ms: i64,
}

impl TimingContext {
    pub fn new(clock: &MonotonicClock) -> Self {
        Self {
            mono_ns: clock.now_ns(),
            wall_ms: WallClock::now_ms(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn test_monotonic_clock_increases() {
        let clock = MonotonicClock::new();
        let t1 = clock.now_ns();
        thread::sleep(Duration::from_millis(1));
        let t2 = clock.now_ns();
        assert!(t2 > t1);
    }

    #[test]
    fn test_monotonic_clock_units() {
        let clock = MonotonicClock::new();
        thread::sleep(Duration::from_millis(10));
        
        let ns = clock.now_ns();
        let us = clock.now_us();
        let ms = clock.now_ms();
        
        // Should be roughly consistent (with some tolerance for timing)
        assert!(ns >= 10_000_000); // At least 10ms in ns
        assert!(us >= 10_000);     // At least 10ms in us
        assert!(ms >= 10);         // At least 10ms
    }

    #[test]
    fn test_wall_clock_reasonable() {
        let now = WallClock::now_ms();
        // Should be after Jan 1, 2020 (1577836800000 ms)
        assert!(now > 1577836800000);
        // Should be before Jan 1, 2100 (4102444800000 ms)
        assert!(now < 4102444800000);
    }
}
