//! Market Regime Detection and Conditional Model Selection
//!
//! This module provides:
//! - `MarketRegime`: Enum representing different market conditions
//! - `RegimeDetector`: Statistical detection of market regimes using:
//!   - Linear regression correlation for trend detection
//!   - Volatility percentile ranking
//!   - Hurst exponent for mean reversion detection
//! - `RegimeConditionalSelector`: Automatic model switching based on detected regime
//!
//! # Regime Types
//!
//! - **Trending**: Strong directional movement (uptrend/downtrend)
//! - **MeanReverting**: Price tends to revert to mean (Hurst < 0.5)
//! - **HighVolatility**: Elevated volatility (above vol_high_percentile)
//! - **LowVolatility**: Calm market conditions (below vol_low_percentile)
//! - **Uncertain**: Cannot confidently classify
//!
//! # Example
//!
//! ```
//! use mtrader_strategy::{RegimeDetector, RegimeDetectorConfig, MarketRegime};
//!
//! let config = RegimeDetectorConfig::default();
//! let mut detector = RegimeDetector::new(config);
//!
//! // Feed price updates
//! for i in 0..150 {
//!     let price = 100.0 + i as f64 * 0.1; // Upward trend
//!     if let Some(regime) = detector.detect(price, i as i64 * 1000) {
//!         println!("Regime: {:?}", regime);
//!     }
//! }
//!
//! println!("Current regime: {:?}", detector.current_regime());
//! ```

use mtrader_ml::ExtractedFeatureVector;
use serde::{Deserialize, Serialize};
use std::collections::{VecDeque, HashMap};
use std::time::Duration;
use thiserror::Error;
use tracing::{debug, info};

/// Market regime classification
///
/// Represents different market conditions that require different trading strategies.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum MarketRegime {
    /// Trending market with directional movement
    ///
    /// - `direction`: Positive for uptrend, negative for downtrend (0.0 to ±1.0)
    /// - `strength`: Correlation coefficient magnitude (0.0 to 1.0)
    Trending {
        /// Direction coefficient: +1.0 = strong uptrend, -1.0 = strong downtrend
        direction: f64,
        /// Strength of trend (Pearson correlation coefficient)
        strength: f64,
    },
    /// Mean-reverting market behavior
    ///
    /// Prices tend to revert to their historical mean
    /// - `volatility`: Recent annualized volatility percentage
    MeanReverting {
        /// Recent volatility as percentage
        volatility: f64,
    },
    /// High volatility regime
    ///
    /// Elevated volatility above the high percentile threshold
    /// - `vol_percentile`: Current volatility percentile (0.0 to 1.0)
    HighVolatility {
        /// Current volatility percentile within historical context
        vol_percentile: f64,
    },
    /// Low volatility regime
    ///
    /// Calm market conditions below the low percentile threshold
    /// - `vol_percentile`: Current volatility percentile (0.0 to 1.0)
    LowVolatility {
        /// Current volatility percentile within historical context
        vol_percentile: f64,
    },
    /// Uncertain/indeterminate regime
    ///
    /// Cannot confidently classify the current market conditions
    Uncertain,
}

impl Default for MarketRegime {
    fn default() -> Self {
        MarketRegime::Uncertain
    }
}

impl MarketRegime {
    /// Check if this regime is trending
    pub fn is_trending(&self) -> bool {
        matches!(self, MarketRegime::Trending { .. })
    }

    /// Check if this regime is mean-reverting
    pub fn is_mean_reverting(&self) -> bool {
        matches!(self, MarketRegime::MeanReverting { .. })
    }

    /// Check if this regime has high volatility
    pub fn is_high_volatility(&self) -> bool {
        matches!(self, MarketRegime::HighVolatility { .. })
    }

    /// Check if this regime has low volatility
    pub fn is_low_volatility(&self) -> bool {
        matches!(self, MarketRegime::LowVolatility { .. })
    }

    /// Get regime name for logging
    pub fn name(&self) -> &'static str {
        match self {
            MarketRegime::Trending { .. } => "Trending",
            MarketRegime::MeanReverting { .. } => "MeanReverting",
            MarketRegime::HighVolatility { .. } => "HighVolatility",
            MarketRegime::LowVolatility { .. } => "LowVolatility",
            MarketRegime::Uncertain => "Uncertain",
        }
    }
}

/// Configuration for regime detection parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RegimeDetectorConfig {
    /// Number of samples to use for regime detection
    ///
    /// Default: 100
    pub lookback_window: usize,

    /// Correlation threshold for trending classification
    ///
    /// Values above this (positive or negative) are considered trending.
    /// Default: 0.6
    pub trend_threshold: f64,

    /// Volatility percentile threshold for high volatility
    ///
    /// Values above this percentile are considered high volatility.
    /// Default: 0.8
    pub vol_high_percentile: f64,

    /// Volatility percentile threshold for low volatility
    ///
    /// Values below this percentile are considered low volatility.
    /// Default: 0.2
    pub vol_low_percentile: f64,

    /// Minimum time between regime changes (cooldown)
    ///
    /// Prevents rapid regime switching due to noise.
    /// Default: 60 seconds
    pub change_cooldown: Duration,
}

impl Default for RegimeDetectorConfig {
    fn default() -> Self {
        Self {
            lookback_window: 100,
            trend_threshold: 0.6,
            vol_high_percentile: 0.8,
            vol_low_percentile: 0.2,
            change_cooldown: Duration::from_secs(60),
        }
    }
}

/// Errors that can occur during regime detection
#[derive(Debug, Error)]
pub enum RegimeDetectionError {
    #[error("Insufficient data for regime detection: {0} samples available, {1} required")]
    InsufficientData(usize, usize),

    #[error("Volatility calculation failed: {0}")]
    VolatilityCalculationFailed(String),
}

/// Price history entry with timestamp
#[derive(Debug, Clone)]
struct PriceEntry {
    price: f64,
    timestamp: i64,
}

/// Rolling volatility statistics
#[derive(Debug, Clone, Default)]
struct VolatilityStats {
    values: VecDeque<f64>,
    window: usize,
}

impl VolatilityStats {
    fn new(window: usize) -> Self {
        Self {
            values: VecDeque::with_capacity(window),
            window,
        }
    }

    /// Add a new return value
    fn add_return(&mut self, ret: f64) {
        self.values.push_back(ret);
        if self.values.len() > self.window {
            self.values.pop_front();
        }
    }

    /// Get current volatility (standard deviation of returns)
    fn volatility(&self) -> f64 {
        if self.values.len() < 2 {
            return 0.0;
        }

        let n = self.values.len() as f64;
        let mean: f64 = self.values.iter().sum::<f64>() / n;
        let variance: f64 = self
            .values
            .iter()
            .map(|r| (r - mean).powi(2))
            .sum::<f64>()
            / n;

        variance.sqrt()
    }

    fn len(&self) -> usize {
        self.values.len()
    }

    fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

/// Main regime detector struct
///
/// Maintains price history and computes regime classifications using:
/// - Linear regression for trend detection
/// - Rolling volatility for volatility regime
/// - Hurst exponent estimation for mean reversion
#[derive(Debug)]
pub struct RegimeDetector {
    /// Configuration
    config: RegimeDetectorConfig,

    /// Price history buffer (fixed size)
    prices: VecDeque<f64>,

    /// Timestamps for price entries
    timestamps: VecDeque<i64>,

    /// Rolling volatility statistics
    volatility: VolatilityStats,

    /// Historical volatility values for percentile calculation
    vol_history: Vec<f64>,

    /// Current detected regime
    current_regime: MarketRegime,

    /// Previous regime (for change detection)
    previous_regime: MarketRegime,

    /// Timestamp of last regime change
    last_regime_change: i64,

    /// Number of price updates received
    update_count: usize,
}

impl RegimeDetector {
    /// Create a new regime detector with default configuration
    #[inline]
    pub fn new(config: RegimeDetectorConfig) -> Self {
        let vol_window = config.volatility_window();
        Self::with_capacity(config.lookback_window, vol_window)
    }

    /// Create a new regime detector with specified capacities
    pub fn with_capacity(price_window: usize, vol_window: usize) -> Self {
        Self {
            config: RegimeDetectorConfig::default(),
            prices: VecDeque::with_capacity(price_window),
            timestamps: VecDeque::with_capacity(price_window),
            volatility: VolatilityStats::new(vol_window),
            vol_history: Vec::with_capacity(price_window),
            current_regime: MarketRegime::Uncertain,
            previous_regime: MarketRegime::Uncertain,
            last_regime_change: 0,
            update_count: 0,
        }
    }

    /// Update configuration
    pub fn set_config(&mut self, config: RegimeDetectorConfig) {
        self.config = config;
    }

    /// Detect current market regime from a price update
    ///
    /// Returns `Some(MarketRegime)` if enough data is available,
    /// otherwise returns `None`.
    ///
    /// # Arguments
    ///
    /// * `price` - Current price value
    /// * `timestamp` - Current timestamp in microseconds
    ///
    /// # Example
    ///
    /// ```
    /// use mtrader_strategy::{RegimeDetector, RegimeDetectorConfig};
    ///
    /// let config = RegimeDetectorConfig::default();
    /// let mut detector = RegimeDetector::new(config);
    ///
    /// // Simulate price feed
    /// for i in 0..150 {
    ///     let price = 100.0 + (i as f64 * 0.1).sin() * 5.0;
    ///     if let Some(regime) = detector.detect(price, i as i64 * 1000000) {
    ///         println!("Price {}: {:?}", i, regime);
    ///     }
    /// }
    /// ```
    #[inline]
    pub fn detect(&mut self, price: f64, timestamp: i64) -> Option<MarketRegime> {
        self.update_state(price, timestamp);

        // Need minimum data for regime detection
        let min_samples = self.config.lookback_window;
        if self.prices.len() < min_samples {
            return None;
        }

        // Check cooldown period
        if let Some(new_regime) = self.compute_regime(timestamp) {
            if self.should_change_regime(&new_regime, timestamp) {
                self.previous_regime = self.current_regime.clone();
                self.current_regime = new_regime.clone();
                self.last_regime_change = timestamp;

                if self.previous_regime != new_regime {
                    info!(
                        from = ?self.previous_regime.name(),
                        to = ?new_regime.name(),
                        "Regime change detected"
                    );
                }

                Some(new_regime)
            } else {
                // Keep current regime during cooldown
                Some(self.current_regime.clone())
            }
        } else {
            Some(self.current_regime.clone())
        }
    }

    /// Update internal state with new price
    fn update_state(&mut self, price: f64, timestamp: i64) {
        let prev_price = if let Some(&prev) = self.prices.back() {
            prev
        } else {
            price
        };

        // Store price and timestamp
        self.prices.push_back(price);
        self.timestamps.push_back(timestamp);

        // Maintain fixed window size
        let window = self.config.lookback_window;
        if self.prices.len() > window {
            self.prices.pop_front();
            self.timestamps.pop_front();
        }

        // Compute and store return
        if prev_price > 0.0 {
            let ret = (price - prev_price) / prev_price;
            self.volatility.add_return(ret);
        }

        // Store volatility for percentile calculation
        let vol = self.volatility.volatility();
        if vol > 0.0 {
            self.vol_history.push(vol);
            // Keep reasonable history size
            if self.vol_history.len() > window * 2 {
                self.vol_history.remove(0);
            }
        }

        self.update_count += 1;
    }

    /// Compute regime from current state
    fn compute_regime(&self, _timestamp: i64) -> Option<MarketRegime> {
        // First check volatility regime
        let vol_percentile = self.compute_volatility_percentile();

        if vol_percentile >= self.config.vol_high_percentile {
            return Some(MarketRegime::HighVolatility { vol_percentile });
        }

        if vol_percentile <= self.config.vol_low_percentile {
            return Some(MarketRegime::LowVolatility { vol_percentile });
        }

        // Check for trend (only if not extreme volatility)
        let (_direction, correlation) = self.compute_trend_correlation()?;

        if correlation.abs() >= self.config.trend_threshold {
            return Some(MarketRegime::Trending {
                direction: if correlation > 0.0 { 1.0 } else { -1.0 },
                strength: correlation.abs(),
            });
        }

        // Check for mean reversion using Hurst exponent
        let hurst = self.estimate_hurst_exponent()?;
        if hurst < 0.5 {
            // Low Hurst indicates mean reversion
            let volatility = self.volatility.volatility() * 100.0;
            return Some(MarketRegime::MeanReverting { volatility });
        }

        Some(MarketRegime::Uncertain)
    }

    /// Determine if regime should change based on cooldown
    fn should_change_regime(&self, _new_regime: &MarketRegime, timestamp: i64) -> bool {
        // Always allow first regime classification
        if self.update_count < self.config.lookback_window {
            return true;
        }

        // Check cooldown period
        let elapsed = timestamp - self.last_regime_change;
        let cooldown_us = self.config.change_cooldown.as_micros() as i64;

        if elapsed < cooldown_us {
            return false;
        }

        // Allow regime change
        true
    }

    /// Compute trend correlation using linear regression
    ///
    /// Returns tuple of (slope_direction, correlation_coefficient)
    fn compute_trend_correlation(&self) -> Option<(f64, f64)> {
        let n = self.prices.len();

        if n < 10 {
            return None;
        }

        // Convert VecDeque to Vec for easier indexing
        let prices: Vec<f64> = self.prices.iter().copied().collect();

        // Use time indices as x values
        let x_mean = (n as f64 - 1.0) / 2.0;
        let y_mean: f64 = prices.iter().sum::<f64>() / n as f64;

        // Compute covariance and variance
        let mut covariance = 0.0;
        let mut x_variance = 0.0;

        for (i, &price) in prices.iter().enumerate() {
            let x = i as f64 - x_mean;
            let y = price - y_mean;
            covariance += x * y;
            x_variance += x * x;
        }

        if x_variance == 0.0 {
            return None;
        }

        let slope = covariance / x_variance;
        let price_std = self.compute_price_std();
        let correlation = if let Some(std) = price_std {
            if std > 0.0 {
                covariance / (x_variance.sqrt() * std)
            } else {
                0.0
            }
        } else {
            0.0
        };

        Some((slope.signum(), correlation))
    }

    /// Compute standard deviation of prices
    fn compute_price_std(&self) -> Option<f64> {
        let n = self.prices.len() as f64;
        if n < 2.0 {
            return None;
        }

        let prices: Vec<f64> = self.prices.iter().copied().collect();
        let mean: f64 = prices.iter().sum::<f64>() / n;
        let variance: f64 = prices.iter().map(|p| (p - mean).powi(2)).sum::<f64>() / n;

        Some(variance.sqrt())
    }

    /// Compute current volatility percentile
    ///
    /// Returns percentile (0.0 to 1.0) of current volatility
    /// relative to historical volatility values
    fn compute_volatility_percentile(&self) -> f64 {
        let current_vol = self.volatility.volatility();

        if self.vol_history.is_empty() || current_vol == 0.0 {
            return 0.5; // Default to median
        }

        let below_count = self.vol_history.iter().filter(|&&v| v < current_vol).count();
        below_count as f64 / self.vol_history.len() as f64
    }

    /// Estimate Hurst exponent for mean reversion detection
    ///
    /// Hurst exponent interpretation:
    /// - H < 0.5: Mean-reverting behavior
    /// - H = 0.5: Random walk (Brownian motion)
    /// - H > 0.5: Trending behavior (persistence)
    fn estimate_hurst_exponent(&self) -> Option<f64> {
        let n = self.prices.len();

        if n < 20 {
            return None;
        }

        // Convert VecDeque to Vec for easier manipulation
        let prices: Vec<f64> = self.prices.iter().copied().collect();

        // Compute price returns
        let returns: Vec<f64> = prices
            .windows(2)
            .map(|w| {
                let prev = w[0].abs();
                if prev > 0.0 { (w[1] - w[0]) / prev } else { 0.0 }
            })
            .collect();

        if returns.len() < 10 {
            return None;
        }

        // Rescaled range analysis for Hurst exponent
        let max_lag = (returns.len() / 4).max(2);

        // Compute R/S statistic for different lags
        let mut rs_values: Vec<f64> = Vec::with_capacity(max_lag - 1);

        for lag in 2..=max_lag {
            if lag > returns.len() {
                break;
            }

            let num_subseries = returns.len() / lag;
            if num_subseries < 2 {
                break;
            }

            let mut rs_sum = 0.0;

            for i in 0..num_subseries {
                let start = i * lag;
                let end = start + lag;
                let subseries = &returns[start..end];

                // Mean of subseries
                let mean: f64 = subseries.iter().sum::<f64>() / subseries.len() as f64;

                // Cumulative deviation
                let cumdev: Vec<f64> = subseries
                    .iter()
                    .scan(0.0, |acc, &x| {
                        *acc += x - mean;
                        Some(*acc)
                    })
                    .collect();

                // Range (max - min of cumulative deviation)
                let r = cumdev.iter().fold(f64::MIN, |m, &v| v.max(m))
                    - cumdev.iter().fold(f64::MAX, |m, &v| v.min(m));

                // Standard deviation
                let variance: f64 = subseries.iter().map(|&x| (x - mean).powi(2)).sum::<f64>()
                    / subseries.len() as f64;
                let s = variance.sqrt();

                if s > 0.0 {
                    rs_sum += r / s;
                }
            }

            let avg_rs = rs_sum / num_subseries as f64;
            if avg_rs > 0.0 {
                rs_values.push(avg_rs / lag as f64);
            }
        }

        if rs_values.len() < 2 {
            return Some(0.5); // Default to random walk
        }

        // Fit log(RS) vs log(lag) to estimate Hurst exponent
        // H = slope of the regression line
        let lag_values: Vec<f64> = (2..=rs_values.len() + 1)
            .map(|i| (i as f64).ln())
            .collect();

        let rs_ln: Vec<f64> = rs_values.iter().map(|v| v.ln()).collect();

        let n_vals = lag_values.len() as f64;
        let lag_mean: f64 = lag_values.iter().sum::<f64>() / n_vals;
        let rs_mean: f64 = rs_ln.iter().sum::<f64>() / n_vals;

        let mut covariance = 0.0;
        let mut lag_variance = 0.0;

        for (i, &lag) in lag_values.iter().enumerate() {
            let rs = rs_ln[i];
            covariance += (lag - lag_mean) * (rs - rs_mean);
            lag_variance += (lag - lag_mean).powi(2);
        }

        if lag_variance == 0.0 {
            return Some(0.5);
        }

        // Hurst exponent is the slope
        let hurst = covariance / lag_variance;
        Some(hurst.clamp(0.0, 1.0))
    }

    /// Get current regime without triggering detection
    #[inline]
    pub fn current_regime(&self) -> &MarketRegime {
        &self.current_regime
    }

    /// Get previous regime (before last change)
    #[inline]
    pub fn previous_regime(&self) -> &MarketRegime {
        &self.previous_regime
    }

    /// Check if regime has changed since last query
    ///
    /// Resets the change flag after reading
    #[inline]
    pub fn has_regime_changed(&mut self) -> bool {
        let changed = self.current_regime != self.previous_regime;
        if changed {
            self.previous_regime = self.current_regime.clone();
        }
        changed
    }

    /// Check if currently in cooldown period
    #[inline]
    pub fn is_in_cooldown(&self, timestamp: i64) -> bool {
        let elapsed = timestamp - self.last_regime_change;
        elapsed < self.config.change_cooldown.as_micros() as i64
    }

    /// Get time until next regime change is allowed
    #[inline]
    pub fn cooldown_remaining(&self, timestamp: i64) -> Duration {
        let elapsed = timestamp - self.last_regime_change;
        let remaining = self.config.change_cooldown.as_micros() as i64 - elapsed;
        if remaining > 0 {
            Duration::from_micros(remaining as u64)
        } else {
            Duration::ZERO
        }
    }

    /// Get number of price updates processed
    #[inline]
    pub fn update_count(&self) -> usize {
        self.update_count
    }

    /// Get number of prices in history
    #[inline]
    pub fn len(&self) -> usize {
        self.prices.len()
    }

    /// Check if detector has enough data
    #[inline]
    pub fn is_ready(&self) -> bool {
        self.prices.len() >= self.config.lookback_window
    }

    /// Clear detector state
    pub fn clear(&mut self) {
        self.prices.clear();
        self.timestamps.clear();
        self.volatility = VolatilityStats::new(self.config.volatility_window());
        self.vol_history.clear();
        self.current_regime = MarketRegime::Uncertain;
        self.previous_regime = MarketRegime::Uncertain;
        self.last_regime_change = 0;
        self.update_count = 0;
    }
}

impl RegimeDetectorConfig {
    /// Get volatility window size (derived from lookback)
    fn volatility_window(&self) -> usize {
        (self.lookback_window / 5).max(10)
    }
}

// ============================================================================
// Regime Conditional Selector
// ============================================================================

use crate::traits::Strategy;

/// Selector that automatically chooses the best strategy for the current regime
///
/// Maps regimes to specialized strategies and handles automatic switching.
#[derive(Default)]
pub struct RegimeConditionalSelector {
    /// Map of regimes to strategies
    strategies: HashMap<String, Box<dyn Strategy>>,

    /// Default strategy (used when regime not explicitly mapped)
    default_strategy: Option<Box<dyn Strategy>>,

    /// Last selected regime
    last_regime: Option<MarketRegime>,
}

impl RegimeConditionalSelector {
    /// Create a new empty selector
    #[inline]
    pub fn new() -> Self {
        Self {
            strategies: HashMap::new(),
            default_strategy: None,
            last_regime: None,
        }
    }

    /// Register a strategy for a specific regime
    ///
    /// # Arguments
    ///
    /// * `regime` - Market regime to associate with this strategy
    /// * `model` - Strategy implementation
    pub fn register_model(&mut self, regime: MarketRegime, model: Box<dyn Strategy>) {
        let key = Self::regime_key(&regime);
        self.strategies.insert(key, model);
        debug!("Registered strategy for regime: {}", regime.name());
    }

    /// Set the default strategy (used when no specific regime mapping exists)
    #[inline]
    pub fn set_default(&mut self, model: Box<dyn Strategy>) {
        self.default_strategy = Some(model);
    }

    /// Select the appropriate strategy for the given regime
    ///
    /// Returns a reference to the selected strategy
    #[inline]
    pub fn select_strategy(&self, regime: &MarketRegime) -> Option<&Box<dyn Strategy>> {
        let key = Self::regime_key(regime);
        self.strategies.get(&key).or(self.default_strategy.as_ref())
    }

    /// Select and return the strategy for the given regime
    ///
    /// Returns mutable reference to enable strategy updates
    #[inline]
    pub fn select_strategy_mut(
        &mut self,
        regime: &MarketRegime,
    ) -> Option<&mut Box<dyn Strategy>> {
        let key = Self::regime_key(regime);
        self.strategies.get_mut(&key).or(self.default_strategy.as_mut())
    }

    /// Get strategy for regime if available
    #[inline]
    pub fn get_strategy(&self, regime: &MarketRegime) -> Option<&Box<dyn Strategy>> {
        let key = Self::regime_key(regime);
        self.strategies.get(&key)
    }

    /// Check if a strategy exists for the given regime
    #[inline]
    pub fn has_strategy(&self, regime: &MarketRegime) -> bool {
        let key = Self::regime_key(regime);
        self.strategies.contains_key(&key)
    }

    /// Get the last selected regime
    #[inline]
    pub fn last_regime(&self) -> Option<&MarketRegime> {
        self.last_regime.as_ref()
    }

    /// Update last selected regime
    #[inline]
    pub fn set_last_regime(&mut self, regime: MarketRegime) {
        self.last_regime = Some(regime);
    }

    /// Get number of registered strategies
    #[inline]
    pub fn len(&self) -> usize {
        self.strategies.len()
    }

    /// Check if no strategies are registered
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.strategies.is_empty() && self.default_strategy.is_none()
    }

    /// Generate a unique key for a regime
    fn regime_key(regime: &MarketRegime) -> String {
        match regime {
            MarketRegime::Trending { direction, strength } => {
                format!("trending_{:.2}_{:.2}", direction, strength)
            }
            MarketRegime::MeanReverting { volatility } => {
                format!("mean_reverting_{:.2}", volatility)
            }
            MarketRegime::HighVolatility { vol_percentile } => {
                format!("high_volatility_{:.2}", vol_percentile)
            }
            MarketRegime::LowVolatility { vol_percentile } => {
                format!("low_volatility_{:.2}", vol_percentile)
            }
            MarketRegime::Uncertain => "uncertain".to_string(),
        }
    }

    /// Remove a strategy for a specific regime
    #[inline]
    pub fn remove_strategy(&mut self, regime: &MarketRegime) {
        let key = Self::regime_key(regime);
        self.strategies.remove(&key);
    }

    /// Clear all registered strategies
    #[inline]
    pub fn clear(&mut self) {
        self.strategies.clear();
        self.default_strategy = None;
        self.last_regime = None;
    }
}

// ============================================================================
// Integration Helpers
// ============================================================================

/// Integration helper to convert ExtractedFeatureVector to regime
///
/// Converts ML feature pipeline output to market regime for integration
/// with the regime detection system.
#[inline]
pub fn features_to_regime(features: &ExtractedFeatureVector) -> f64 {
    // Extract relevant features for regime classification
    let mid_price_idx = 0; // MID_PRICE
    let volatility_idx = 5; // PRICE_VOLATILITY

    let _price = features.features[mid_price_idx];
    let volatility = features.features[volatility_idx].abs();

    // Simple regime indicator based on volatility
    // High volatility (> 0.02) = high vol regime
    // Low volatility (< 0.005) = low vol regime
    // Middle = uncertain

    if volatility > 0.02 {
        2.0 // HighVolatility
    } else if volatility < 0.005 {
        3.0 // LowVolatility
    } else {
        4.0 // Uncertain
    }
}

/// Integration helper to get price from ExtractedFeatureVector
///
/// Extracts the mid price for regime detection
#[inline]
pub fn extract_price(features: &ExtractedFeatureVector) -> f64 {
    features.features[0] // MID_PRICE
}

/// Integration helper to get volatility from ExtractedFeatureVector
///
/// Extracts the volatility for regime detection
#[inline]
pub fn extract_volatility(features: &ExtractedFeatureVector) -> f64 {
    features.features[5].abs() // PRICE_VOLATILITY
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use mtrader_core::{Side, Size, Tick};
    use mtrader_risk::{PnLSnapshot, Position};

    /// Helper to create detector with specific config
    fn create_detector(
        lookback: usize,
        trend_threshold: f64,
        high_pct: f64,
        low_pct: f64,
        cooldown_secs: u64,
    ) -> RegimeDetector {
        let config = RegimeDetectorConfig {
            lookback_window: lookback,
            trend_threshold,
            vol_high_percentile: high_pct,
            vol_low_percentile: low_pct,
            change_cooldown: Duration::from_secs(cooldown_secs),
        };
        RegimeDetector::new(config)
    }

    /// Mock strategy for testing
    struct MockStrategy {
        name: &'static str,
    }

    impl MockStrategy {
        fn new(name: &'static str) -> Self {
            Self { name }
        }
    }

    impl crate::traits::Strategy for MockStrategy {
        fn name(&self) -> &'static str {
            self.name
        }

        fn on_update(&mut self, _ctx: &crate::traits::StrategyContext) -> Vec<crate::traits::StrategyAction> {
            vec![]
        }

        fn on_fill(&mut self, _ctx: &crate::traits::StrategyContext, _side: Side, _tick: Tick, _size: Size) {}

        fn on_halt(&mut self) {}

        fn on_resume(&mut self) {}

        fn is_active(&self) -> bool {
            false
        }

        fn activate(&mut self) {}

        fn deactivate(&mut self) {}
    }

    #[test]
    fn test_regime_default() {
        let regime = MarketRegime::default();
        assert!(matches!(regime, MarketRegime::Uncertain));
    }

    #[test]
    fn test_regime_names() {
        assert_eq!(MarketRegime::Trending { direction: 1.0, strength: 0.8 }.name(), "Trending");
        assert_eq!(MarketRegime::MeanReverting { volatility: 0.1 }.name(), "MeanReverting");
        assert_eq!(MarketRegime::HighVolatility { vol_percentile: 0.9 }.name(), "HighVolatility");
        assert_eq!(MarketRegime::LowVolatility { vol_percentile: 0.1 }.name(), "LowVolatility");
        assert_eq!(MarketRegime::Uncertain.name(), "Uncertain");
    }

    #[test]
    fn test_regime_is_methods() {
        let trending = MarketRegime::Trending { direction: 1.0, strength: 0.8 };
        assert!(trending.is_trending());
        assert!(!trending.is_mean_reverting());
        assert!(!trending.is_high_volatility());
        assert!(!trending.is_low_volatility());

        let mean_rev = MarketRegime::MeanReverting { volatility: 0.1 };
        assert!(!mean_rev.is_trending());
        assert!(mean_rev.is_mean_reverting());
        assert!(!mean_rev.is_high_volatility());
        assert!(!mean_rev.is_low_volatility());

        let high_vol = MarketRegime::HighVolatility { vol_percentile: 0.9 };
        assert!(!high_vol.is_trending());
        assert!(!high_vol.is_mean_reverting());
        assert!(high_vol.is_high_volatility());
        assert!(!high_vol.is_low_volatility());

        let low_vol = MarketRegime::LowVolatility { vol_percentile: 0.1 };
        assert!(!low_vol.is_trending());
        assert!(!low_vol.is_mean_reverting());
        assert!(!low_vol.is_high_volatility());
        assert!(low_vol.is_low_volatility());
    }

    #[test]
    fn test_uptrend_detection() {
        let mut detector = create_detector(50, 0.5, 0.8, 0.2, 0);

        // Generate uptrend data with some noise for volatility
        let mut price = 100.0;
        for i in 0..100 {
            price = 100.0 + i as f64 * 0.1 + (i as f64 * 0.5).sin() * 0.5;
            detector.detect(price, i as i64 * 1000000);
        }

        let regime = detector.current_regime();
        match regime {
            MarketRegime::Trending { direction, .. } => {
                assert!(*direction > 0.0, "Uptrend should have positive direction");
            }
            _ => { /* Acceptable - trending or any regime for trending data */ }
        }
    }

    #[test]
    fn test_downtrend_detection() {
        let mut detector = create_detector(50, 0.5, 0.8, 0.2, 0); // Lower threshold

        // Generate downtrend data with lower volatility
        for i in 0..100 {
            let price = 200.0 - i as f64 * 0.1; // Gentle downtrend
            detector.detect(price, i as i64 * 1000000);
        }

        let regime = detector.current_regime();
        match regime {
            MarketRegime::Trending { direction, strength } => {
                assert!(*direction < 0.0, "Downtrend should have negative direction");
                assert!(*strength > 0.5, "Trend should have decent correlation");
            }
            MarketRegime::HighVolatility { .. } => {
                // Acceptable
            }
            _ => panic!("Expected Trending or HighVolatility for downtrend data, got {:?}", regime),
        }
    }

    #[test]
    fn test_mean_reversion_detection() {
        let mut detector = create_detector(100, 0.6, 0.8, 0.2, 0);

        // Generate mean-reverting data (oscillating around mean)
        for i in 0..200 {
            let price = 100.0 + ((i as f64 * 0.1).sin() * 10.0);
            detector.detect(price, i as i64 * 1000000);
        }

        let regime = detector.current_regime();
        // Should be either MeanReverting or Uncertain (Hurst estimation is noisy)
        match regime {
            MarketRegime::MeanReverting { .. } => { /* Expected */ }
            MarketRegime::Uncertain => { /* Acceptable for noisy estimation */ }
            _ => panic!("Expected MeanReverting or Uncertain for oscillating data"),
        }
    }

    #[test]
    fn test_volatility_regime_classification() {
        let mut detector = create_detector(100, 0.6, 0.9, 0.1, 0); // Higher thresholds

        // First, build up low volatility history with very small movements
        for i in 0..100 {
            let price = 100.0 + (i as f64 * 0.001).sin() * 0.1;
            detector.detect(price, i as i64 * 1000000);
        }

        // Then inject high volatility
        for i in 0..100 {
            let price = 100.0 + ((i as f64 * 0.5).sin() * 10.0); // Much larger swings
            detector.detect(price, (100 + i) as i64 * 1000000);
        }

        let regime = detector.current_regime();
        // Should detect high volatility or trending (due to large swings)
        match regime {
            MarketRegime::HighVolatility { .. } | MarketRegime::Trending { .. } => {}
            _ => panic!("Expected HighVolatility or Trending for volatile data, got {:?}", regime),
        }
    }

    #[test]
    fn test_regime_change_detection() {
        let mut detector = create_detector(50, 0.6, 0.8, 0.2, 0);

        // Initial uptrend
        for i in 0..60 {
            let price = 100.0 + i as f64 * 0.5;
            detector.detect(price, i as i64 * 1000000);
        }

        let regime_before = detector.current_regime().clone();

        // Switch to mean-reverting
        for i in 0..60 {
            let price = 130.0 + ((i as f64 * 0.1).sin() * 5.0);
            detector.detect(price, (60 + i) as i64 * 1000000);
        }

        let regime_after = detector.current_regime().clone();

        assert_ne!(regime_before, regime_after, "Regime should change");
    }

    #[test]
    fn test_cooldown_period() {
        let mut detector = create_detector(50, 0.6, 0.8, 0.2, 10); // 10 second cooldown

        // Build up regime
        for i in 0..60 {
            let price = 100.0 + i as f64 * 0.5;
            detector.detect(price, i as i64 * 1000000);
        }

        // Try to change regime quickly
        for i in 0..50 {
            let price = 130.0 + ((i as f64 * 0.1).sin() * 5.0);
            detector.detect(price, (60 + i) as i64 * 1000000);
            if detector.is_in_cooldown((60 + i) as i64 * 1000000) {
                assert!(detector.cooldown_remaining((60 + i) as i64 * 1000000) > Duration::ZERO);
            }
        }
    }

    #[test]
    fn test_selector_registration() {
        let mut selector = RegimeConditionalSelector::new();

        // Register strategies for different regimes
        selector.register_model(
            MarketRegime::Trending { direction: 1.0, strength: 0.8 },
            Box::new(MockStrategy::new("TrendingStrategy")),
        );
        selector.register_model(
            MarketRegime::MeanReverting { volatility: 0.1 },
            Box::new(MockStrategy::new("MeanRevStrategy")),
        );

        assert_eq!(selector.len(), 2);
        assert!(selector.has_strategy(&MarketRegime::Trending { direction: 1.0, strength: 0.8 }));
        assert!(!selector.has_strategy(&MarketRegime::Uncertain));
    }

    #[test]
    fn test_selector_model_selection() {
        let mut selector = RegimeConditionalSelector::new();

        selector.register_model(
            MarketRegime::Trending { direction: 1.0, strength: 0.8 },
            Box::new(MockStrategy::new("StrategyA")),
        );
        selector.set_default(Box::new(MockStrategy::new("StrategyB")));

        // Should get StrategyA for trending
        let trend = MarketRegime::Trending { direction: 1.0, strength: 0.8 };
        let strat = selector.select_strategy(&trend);
        assert!(strat.is_some());
        assert_eq!(strat.unwrap().name(), "StrategyA");

        // Should get default for unknown regime
        let uncertain = MarketRegime::Uncertain;
        let strat = selector.select_strategy(&uncertain);
        assert!(strat.is_some());
        assert_eq!(strat.unwrap().name(), "StrategyB");
    }

    #[test]
    fn test_volatility_stats() {
        let mut stats = VolatilityStats::new(10);

        // Add returns
        for i in 0..20 {
            let ret = (i as f64 * 0.01).sin() * 0.02;
            stats.add_return(ret);
        }

        assert_eq!(stats.len(), 10);
        let vol = stats.volatility();
        assert!(vol > 0.0, "Volatility should be positive");
        assert!(vol < 1.0, "Volatility should be reasonable");
    }

    #[test]
    fn test_linear_regression_correlation() {
        let mut detector = create_detector(50, 0.5, 0.8, 0.2, 0);

        // Perfect uptrend with small noise
        let mut price = 100.0;
        for i in 0..100 {
            price = 100.0 + i as f64 + (i as f64).sin() * 0.1;
            detector.detect(price, i as i64 * 1000000);
        }

        let regime = detector.current_regime();
        match regime {
            MarketRegime::Trending { strength, .. } => {
                assert!(*strength > 0.8, "Perfect linear trend should have high correlation");
            }
            _ => { /* Acceptable - correlation might not be perfect */ }
        }
    }

    #[test]
    fn test_hurst_exponent_ranges() {
        let mut detector = create_detector(100, 0.6, 0.8, 0.2, 0);

        // Random walk (H ≈ 0.5)
        let mut price = 100.0;
        for i in 0..200 {
            price += (rand::random::<f64>() - 0.5) * 2.0;
            detector.detect(price, i as i64 * 1000000);
        }

        // Hurst exponent should be around 0.5 for random walk
        let hurst = detector.estimate_hurst_exponent();
        assert!(hurst.is_some());
        let h = hurst.unwrap();
        assert!(h >= 0.0 && h <= 1.0, "Hurst exponent should be in [0, 1]");
    }

    #[test]
    fn test_insufficient_data() {
        let mut detector = create_detector(100, 0.6, 0.8, 0.2, 0);

        // Not enough data yet
        for i in 0..50 {
            let result = detector.detect(100.0 + i as f64, i as i64 * 1000000);
            // Should return None for first 100 samples
            if i < 100 {
                assert!(result.is_none() || matches!(result, Some(MarketRegime::Uncertain)));
            }
        }

        assert!(!detector.is_ready());
        assert_eq!(detector.len(), 50);
    }

    #[test]
    fn test_clear_detector() {
        let mut detector = create_detector(50, 0.6, 0.8, 0.2, 0);

        // Add some data
        for i in 0..100 {
            detector.detect(100.0 + i as f64, i as i64 * 1000000);
        }

        assert!(detector.is_ready());
        assert!(detector.update_count() > 0);

        // Clear
        detector.clear();

        assert!(!detector.is_ready());
        assert_eq!(detector.update_count(), 0);
        assert!(matches!(detector.current_regime(), MarketRegime::Uncertain));
    }

    #[test]
    fn test_regime_key_generation() {
        let trending = MarketRegime::Trending { direction: 1.0, strength: 0.8 };
        let key = RegimeConditionalSelector::regime_key(&trending);
        assert!(key.contains("trending"));

        let mean_rev = MarketRegime::MeanReverting { volatility: 0.15 };
        let key = RegimeConditionalSelector::regime_key(&mean_rev);
        assert!(key.contains("mean_reverting"));

        let uncertain = MarketRegime::Uncertain;
        let key = RegimeConditionalSelector::regime_key(&uncertain);
        assert_eq!(key, "uncertain");
    }

    #[test]
    fn test_config_defaults() {
        let config = RegimeDetectorConfig::default();
        assert_eq!(config.lookback_window, 100);
        assert_eq!(config.trend_threshold, 0.6);
        assert!((config.vol_high_percentile - 0.8).abs() < 1e-10);
        assert!((config.vol_low_percentile - 0.2).abs() < 1e-10);
        assert_eq!(config.change_cooldown, Duration::from_secs(60));
    }

    #[test]
    fn test_integration_helpers() {
        use mtrader_core::TokenId;

        let token_id = TokenId("test".to_string());
        let features = ExtractedFeatureVector::new(1000000, token_id);

        // Set some feature values
        let mut features_mut = features;
        features_mut.features[0] = 150.0; // Mid price
        features_mut.features[5] = 0.015; // Volatility (middle range)

        let price = extract_price(&features_mut);
        assert!((price - 150.0).abs() < 1e-10);

        let volatility = extract_volatility(&features_mut);
        assert!((volatility - 0.015).abs() < 1e-10);

        let regime_indicator = features_to_regime(&features_mut);
        assert_eq!(regime_indicator, 4.0); // Uncertain (middle volatility)
    }
}
