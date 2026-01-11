//! Online Feature Extraction Pipeline
//!
//! Real-time computation of technical features from normalized market data.
//! Designed for low-latency, online processing with sliding windows per token.
//!
//! # Architecture
//!
//! ```text
//! MarketDataEvent (from gateway)
//!   → FeatureExtractor (per-token state)
//!   → SlidingWindowBuffers (price, spread, volume)
//!   → TechnicalIndicators (EMA, volatility, momentum)
//!   → FeatureVector (for ML inference)
//! ```
//!
//! # Features Extracted
//!
//! - **Price Features**: mid_price, returns (1m, 5m, 15m), momentum
//! - **Spread Features**: spread, spread_volatility, relative_spread
//! - **Volume Features**: bid_size, ask_size, imbalance, volume_rate
//! - **Technical Indicators**: EMA(price), EMA(spread), ATR, Bollinger Bands
//! - **Order Flow**: trade_rate, delta_imbalance

use many_lamps_core::{TokenId, NormalizedMarketData};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};

/// Feature index constants for named access to ExtractedFeatureVector
pub mod idx {
    pub const MID_PRICE: usize = 0;
    pub const PRICE_EMA: usize = 1;
    pub const PRICE_MOMENTUM_5: usize = 2;
    pub const PRICE_MOMENTUM_15: usize = 3;
    pub const PRICE_MOMENTUM_30: usize = 4;
    pub const PRICE_VOLATILITY: usize = 5;
    pub const SPREAD: usize = 6;
    pub const SPREAD_EMA: usize = 7;
    pub const SPREAD_VOLATILITY: usize = 8;
    pub const RELATIVE_SPREAD: usize = 9;
    pub const BID_SIZE: usize = 10;
    pub const ASK_SIZE: usize = 11;
    pub const BID_ASK_RATIO: usize = 12;
    pub const IMBALANCE: usize = 13;
    pub const IMBALANCE_EMA: usize = 14;
    pub const VOLUME_EMA: usize = 15;
    pub const VOLUME_RATE: usize = 16;
    pub const TRADE_RATE: usize = 17;
    pub const PRICE_RETURN_1M: usize = 18;
    pub const PRICE_RETURN_5M: usize = 19;
    pub const SPREAD_RANK: usize = 20;
    pub const VOLUME_IMBALANCE: usize = 21;
    pub const BID_SIZE_NORM: usize = 22;
    pub const ASK_SIZE_NORM: usize = 23;
}

/// Feature names for debugging/visualization
pub const FEATURE_NAMES: [&'static str; 24] = [
    "mid_price",
    "price_ema",
    "price_momentum_5",
    "price_momentum_15",
    "price_momentum_30",
    "price_volatility",
    "spread",
    "spread_ema",
    "spread_volatility",
    "relative_spread",
    "bid_size",
    "ask_size",
    "bid_ask_ratio",
    "imbalance",
    "imbalance_ema",
    "volume_ema",
    "volume_rate",
    "trade_rate",
    "price_return_1m",
    "price_return_5m",
    "spread_rank",
    "volume_imbalance",
    "bid_size_norm",
    "ask_size_norm",
];

/// Configuration for feature extraction pipeline
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureExtractorConfig {
    /// Price history window size (number of ticks)
    pub price_window: usize,
    /// Spread history window size
    pub spread_window: usize,
    /// Volume history window size
    pub volume_window: usize,
    /// EMA alpha for price (higher = more responsive)
    pub ema_alpha_price: f64,
    /// EMA alpha for spread
    pub ema_alpha_spread: f64,
    /// Volatility lookback period
    pub volatility_window: usize,
    /// Momentum lookback periods (in ticks)
    pub momentum_periods: Vec<usize>,
    /// Enable debug logging
    pub debug: bool,
}

impl Default for FeatureExtractorConfig {
    fn default() -> Self {
        Self {
            price_window: 100,
            spread_window: 50,
            volume_window: 20,
            ema_alpha_price: 0.1,
            ema_alpha_spread: 0.2,
            volatility_window: 20,
            momentum_periods: vec![5, 15, 30],
            debug: false,
        }
    }
}

/// Sliding window buffer for time-series data
#[derive(Debug, Clone)]
pub struct SlidingWindow<T> {
    values: VecDeque<T>,
    max_size: usize,
}

impl<T: Clone> SlidingWindow<T> {
    pub fn new(max_size: usize) -> Self {
        Self {
            values: VecDeque::with_capacity(max_size),
            max_size,
        }
    }

    /// Push a new value, evict oldest if at capacity
    #[inline]
    pub fn push(&mut self, value: T) {
        self.values.push_back(value);
        if self.values.len() > self.max_size {
            self.values.pop_front();
        }
    }

    /// Get all values as a slice
    #[inline]
    pub fn as_slice(&mut self) -> &[T] {
        self.values.make_contiguous()
    }

    /// Get the most recent value
    #[inline]
    pub fn latest(&self) -> Option<&T> {
        self.values.back()
    }

    /// Get the n-th most recent value
    #[inline]
    pub fn nth_latest(&self, n: usize) -> Option<&T> {
        self.values.iter().rev().nth(n)
    }

    /// Current number of elements
    #[inline]
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// Check if empty
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Clear all values
    #[inline]
    pub fn clear(&mut self) {
        self.values.clear();
    }
}

/// Rolling statistics computed online
#[derive(Debug, Clone, Default)]
pub struct RollingStats {
    sum: f64,
    sum_sq: f64,
    count: usize,
}

impl RollingStats {
    #[inline]
    pub fn reset(&mut self) {
        self.sum = 0.0;
        self.sum_sq = 0.0;
        self.count = 0;
    }

    #[inline]
    pub fn add(&mut self, value: f64) {
        self.sum += value;
        self.sum_sq += value * value;
        self.count += 1;
    }

    #[inline]
    pub fn from_values(values: &[f64]) -> Self {
        let mut stats = Self::default();
        for &v in values {
            stats.add(v);
        }
        stats
    }

    #[inline]
    pub fn mean(&self) -> f64 {
        if self.count == 0 {
            0.0
        } else {
            self.sum / self.count as f64
        }
    }

    #[inline]
    pub fn variance(&self) -> f64 {
        if self.count < 2 {
            0.0
        } else {
            let mean = self.mean();
            (self.sum_sq / self.count as f64) - (mean * mean)
        }
    }

    #[inline]
    pub fn std(&self) -> f64 {
        self.variance().sqrt()
    }

    #[inline]
    pub fn count(&self) -> usize {
        self.count
    }
}

/// Exponential Moving Average with online update
#[derive(Debug, Clone, Default)]
pub struct EMA {
    value: f64,
    alpha: f64,
    initialized: bool,
}

impl EMA {
    pub fn new(alpha: f64) -> Self {
        Self {
            value: 0.0,
            alpha,
            initialized: false,
        }
    }

    #[inline]
    pub fn update(&mut self, new_value: f64) -> f64 {
        if !self.initialized {
            self.value = new_value;
            self.initialized = true;
        } else {
            self.value = self.alpha * new_value + (1.0 - self.alpha) * self.value;
        }
        self.value
    }

    #[inline]
    pub fn get(&self) -> f64 {
        self.value
    }

    #[inline]
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }

    #[inline]
    pub fn reset(&mut self) {
        self.value = 0.0;
        self.initialized = false;
    }
}

/// Token-specific feature extraction state
#[derive(Debug, Clone)]
pub struct TokenFeatureState {
    /// Token identifier
    pub token_id: TokenId,
    /// Configuration (shared reference)
    config: FeatureExtractorConfig,
    /// Price history (normalized)
    price_window: SlidingWindow<f64>,
    /// Spread history
    spread_window: SlidingWindow<f64>,
    /// Volume history (bid + ask sizes)
    volume_window: SlidingWindow<f64>,
    /// Bid size history
    bid_size_window: SlidingWindow<f64>,
    /// Ask size history
    ask_size_window: SlidingWindow<f64>,
    /// Trade count for rate calculation
    trade_count: usize,
    /// Last update timestamp
    last_update_ts: i64,
    // EMAs
    ema_price: EMA,
    ema_spread: EMA,
    ema_volume: EMA,
    // Volatility
    price_volatility: f64,
    spread_volatility: f64,
}

impl TokenFeatureState {
    pub fn new(token_id: TokenId, config: &FeatureExtractorConfig) -> Self {
        Self {
            token_id,
            config: config.clone(),
            price_window: SlidingWindow::new(config.price_window),
            spread_window: SlidingWindow::new(config.spread_window),
            volume_window: SlidingWindow::new(config.volume_window),
            bid_size_window: SlidingWindow::new(config.volume_window),
            ask_size_window: SlidingWindow::new(config.volume_window),
            trade_count: 0,
            last_update_ts: 0,
            ema_price: EMA::new(config.ema_alpha_price),
            ema_spread: EMA::new(config.ema_alpha_spread),
            ema_volume: EMA::new(0.15),
            price_volatility: 0.0,
            spread_volatility: 0.0,
        }
    }

    /// Update state with new market data
    #[inline]
    pub fn update(&mut self, data: &NormalizedMarketData) {
        self.last_update_ts = data.timestamp_us;

        // Push to windows
        self.price_window.push(data.mid_price);
        self.spread_window.push(data.spread);
        self.bid_size_window.push(data.bid_size);
        self.ask_size_window.push(data.ask_size);
        self.volume_window.push(data.bid_size + data.ask_size);

        // Update EMAs
        self.ema_price.update(data.mid_price);
        self.ema_spread.update(data.spread);
        self.ema_volume.update(data.bid_size + data.ask_size);

        // Compute volatility from recent price changes
        self.compute_volatility();

        if self.config.debug {
            tracing::debug!(
                token_id = %self.token_id,
                mid_price = data.mid_price,
                spread = data.spread,
                imbalance = data.imbalance,
                "TokenFeatureState updated"
            );
        }
    }

    /// Compute rolling volatility
    #[inline]
    fn compute_volatility(&mut self) {
        let window = self.config.volatility_window;
        let prices = self.price_window.as_slice();

        if prices.len() < 2 {
            return;
        }

        // Compute returns
        let mut returns = Vec::with_capacity(prices.len() - 1);
        for i in 1..prices.len() {
            let prev = prices[i - 1];
            let curr = prices[i];
            if prev > 0.0 {
                returns.push((curr - prev) / prev);
            }
        }

        if returns.len() < window {
            return;
        }

        // Compute standard deviation of returns
        let stats = RollingStats::from_values(&returns[returns.len().saturating_sub(window)..]);
        self.price_volatility = stats.std() * 100.0; // Annualize-ish

        // Spread volatility
        let spreads = self.spread_window.as_slice();
        if spreads.len() >= window {
            let recent_spreads: Vec<f64> = spreads[spreads.len().saturating_sub(window)..]
                .iter()
                .copied()
                .collect();
            let stats = RollingStats::from_values(&recent_spreads);
            self.spread_volatility = stats.std();
        }
    }

    /// Get momentum for a given period
    #[inline]
    pub fn momentum(&mut self, period: usize) -> f64 {
        let prices = self.price_window.as_slice();
        if prices.len() <= period {
            return 0.0;
        }

        let recent = prices[prices.len() - 1];
        let old = prices[prices.len() - 1 - period];
        if old > 0.0 {
            (recent - old) / old
        } else {
            0.0
        }
    }

    /// Get current order book imbalance
    #[inline]
    pub fn current_imbalance(&mut self) -> f64 {
        let bid = self.bid_size_window.latest().copied().unwrap_or(0.0);
        let ask = self.ask_size_window.latest().copied().unwrap_or(0.0);
        let total = bid + ask;
        if total > 0.0 {
            (bid - ask) / total
        } else {
            0.0
        }
    }

    /// Get relative spread (spread / mid_price)
    #[inline]
    pub fn relative_spread(&mut self) -> f64 {
        let spread = self.spread_window.latest().copied().unwrap_or(0.0);
        let mid = self.ema_price.get();
        if mid > 0.0 {
            spread / mid
        } else {
            0.0
        }
    }

    /// Increment trade counter
    #[inline]
    pub fn on_trade(&mut self) {
        self.trade_count += 1;
    }

    /// Get trade rate (trades per window)
    #[inline]
    pub fn trade_rate(&self) -> f64 {
        self.trade_count as f64
    }

    /// Reset trade counter
    #[inline]
    pub fn reset_trade_counter(&mut self) {
        self.trade_count = 0;
    }
}

/// Feature vector output from the extraction pipeline
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedFeatureVector {
    /// Timestamp in microseconds
    pub timestamp_us: i64,
    /// Token identifier
    pub token_id: TokenId,
    /// Feature values [f64; 24]
    pub features: [f64; 24],
}

impl ExtractedFeatureVector {
    /// Total number of features
    pub const FEATURE_COUNT: usize = 24;

    /// Create a new feature vector
    #[inline]
    pub fn new(timestamp_us: i64, token_id: TokenId) -> Self {
        Self {
            timestamp_us,
            token_id,
            features: [0.0; Self::FEATURE_COUNT],
        }
    }

    /// Get mid price feature
    #[inline]
    pub fn mid_price(&self) -> f64 {
        self.features[idx::MID_PRICE]
    }

    /// Get spread feature
    #[inline]
    pub fn spread(&self) -> f64 {
        self.features[idx::SPREAD]
    }

    /// Get imbalance feature
    #[inline]
    pub fn imbalance(&self) -> f64 {
        self.features[idx::IMBALANCE]
    }

    /// Get volatility feature
    #[inline]
    pub fn volatility(&self) -> f64 {
        self.features[idx::PRICE_VOLATILITY]
    }
}

/// Main feature extractor managing multiple token states
#[derive(Debug)]
pub struct FeatureExtractor {
    /// Configuration
    config: FeatureExtractorConfig,
    /// Per-token feature states
    token_states: HashMap<TokenId, TokenFeatureState>,
}

impl FeatureExtractor {
    /// Create a new feature extractor
    pub fn new(config: &FeatureExtractorConfig) -> Self {
        Self {
            config: config.clone(),
            token_states: HashMap::new(),
        }
    }

    /// Get or create token state
    fn get_or_create_token_state(&mut self, token_id: &TokenId) -> &mut TokenFeatureState {
        self.token_states
            .entry(token_id.clone())
            .or_insert_with(|| TokenFeatureState::new(token_id.clone(), &self.config))
    }

    /// Process normalized market data and return feature vector
    ///
    /// Returns `None` if insufficient data for feature extraction.
    #[inline]
    pub fn process(&mut self, data: &NormalizedMarketData) -> Option<ExtractedFeatureVector> {
        let token_state = self.get_or_create_token_state(&data.token_id);

        // Check if we have enough data
        if token_state.price_window.len() < 5 {
            // Initialize state but don't emit features yet
            token_state.update(data);
            return None;
        }

        token_state.update(data);

        // Build feature vector
        let mut features = ExtractedFeatureVector::new(data.timestamp_us, data.token_id.clone());

        // Price features
        features.features[idx::MID_PRICE] = data.mid_price;
        features.features[idx::PRICE_EMA] = token_state.ema_price.get();
        features.features[idx::PRICE_MOMENTUM_5] = token_state.momentum(5);
        features.features[idx::PRICE_MOMENTUM_15] = token_state.momentum(15);
        features.features[idx::PRICE_MOMENTUM_30] = token_state.momentum(30);
        features.features[idx::PRICE_VOLATILITY] = token_state.price_volatility;

        // Spread features
        features.features[idx::SPREAD] = data.spread;
        features.features[idx::SPREAD_EMA] = token_state.ema_spread.get();
        features.features[idx::SPREAD_VOLATILITY] = token_state.spread_volatility;
        features.features[idx::RELATIVE_SPREAD] = token_state.relative_spread();

        // Volume features
        features.features[idx::BID_SIZE] = data.bid_size;
        features.features[idx::ASK_SIZE] = data.ask_size;
        let bid_ask_ratio = if data.ask_size > 0.0 {
            data.bid_size / data.ask_size
        } else {
            1.0
        };
        features.features[idx::BID_ASK_RATIO] = bid_ask_ratio;

        // Imbalance features
        features.features[idx::IMBALANCE] = data.imbalance;
        features.features[idx::IMBALANCE_EMA] = token_state.current_imbalance();
        features.features[idx::BID_SIZE_NORM] = data.bid_size / (data.bid_size + data.ask_size + 1e-9);
        features.features[idx::ASK_SIZE_NORM] = data.ask_size / (data.bid_size + data.ask_size + 1e-9);

        // Volume rate
        features.features[idx::VOLUME_EMA] = token_state.ema_volume.get();
        features.features[idx::VOLUME_RATE] = token_state.trade_rate();

        // Trade features
        features.features[idx::TRADE_RATE] = token_state.trade_rate() / token_state.volume_window.len() as f64;

        // Price returns
        let prices = token_state.price_window.as_slice();
        if prices.len() >= 2 {
            let current = prices[prices.len() - 1];
            let prev_1m_idx = prices.len().saturating_sub(2);
            let prev_1m = if prev_1m_idx < prices.len() {
                prices[prev_1m_idx]
            } else {
                current
            };
            features.features[idx::PRICE_RETURN_1M] = if prev_1m > 0.0 {
                (current - prev_1m) / prev_1m
            } else {
                0.0
            };
        }

        if prices.len() >= 6 {
            let current = prices[prices.len() - 1];
            let prev_5m = prices[prices.len() - 6];
            features.features[idx::PRICE_RETURN_5M] = if prev_5m > 0.0 {
                (current - prev_5m) / prev_5m
            } else {
                0.0
            };
        }

        // Spread rank (percentile within window)
        let spread_rank = Self::compute_percentile(
            data.spread,
            token_state.spread_window.as_slice(),
        );
        features.features[idx::SPREAD_RANK] = spread_rank;

        // Volume imbalance
        let volume_imbalance = data.bid_size - data.ask_size;
        features.features[idx::VOLUME_IMBALANCE] = volume_imbalance;

        Some(features)
    }

    /// Compute percentile of value within a slice
    #[inline]
    fn compute_percentile(value: f64, slice: &[f64]) -> f64 {
        if slice.is_empty() {
            return 0.5;
        }

        let count = slice.len();
        let below = slice.iter().filter(|&&v| v < value).count();
        below as f64 / count as f64
    }

    /// Get state for a specific token
    pub fn token_state(&self, token_id: &TokenId) -> Option<&TokenFeatureState> {
        self.token_states.get(token_id)
    }

    /// Remove state for a token (e.g., when unsubscribing)
    pub fn remove_token(&mut self, token_id: &TokenId) {
        self.token_states.remove(token_id);
    }

    /// Clear all token states
    pub fn clear(&mut self) {
        self.token_states.clear();
    }

    /// Get number of tracked tokens
    #[inline]
    pub fn num_tokens(&self) -> usize {
        self.token_states.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use many_lamps_core::TokenId;

    fn create_test_market_data(
        timestamp_us: i64,
        token_id: &TokenId,
        mid_price: f64,
        spread: f64,
        bid_size: f64,
        ask_size: f64,
    ) -> NormalizedMarketData {
        NormalizedMarketData::new(
            timestamp_us,
            token_id.clone(),
            mid_price,
            spread,
            bid_size,
            ask_size,
            0.001,
            Some(mid_price - spread / 2.0),
            Some(mid_price + spread / 2.0),
        )
    }

    #[test]
    fn test_sliding_window() {
        let mut window = SlidingWindow::new(3);
        assert!(window.is_empty());

        window.push(1.0);
        window.push(2.0);
        window.push(3.0);
        assert_eq!(window.len(), 3);

        window.push(4.0); // Should evict 1.0
        assert_eq!(window.len(), 3);
        assert_eq!(*window.latest().unwrap(), 4.0);
    }

    #[test]
    fn test_ema() {
        let mut ema = EMA::new(0.5);
        assert!(!ema.is_initialized());

        assert!((ema.update(10.0) - 10.0).abs() < 1e-10);
        assert!((ema.update(20.0) - 15.0).abs() < 1e-10);
        assert!((ema.update(10.0) - 12.5).abs() < 1e-10);
    }

    #[test]
    fn test_rolling_stats() {
        let values = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let stats = RollingStats::from_values(&values);

        assert!((stats.mean() - 3.0).abs() < 1e-10);
        assert!((stats.variance() - 2.0).abs() < 1e-10);
        assert!((stats.std() - 1.41421356).abs() < 1e-5);
    }

    #[test]
    fn test_token_feature_state() {
        let token_id = TokenId("test-token".to_string());
        let config = FeatureExtractorConfig::default();
        let mut state = TokenFeatureState::new(token_id.clone(), &config);

        let data = create_test_market_data(1000, &token_id, 0.55, 0.001, 100.0, 100.0);
        state.update(&data);
        assert_eq!(state.price_window.len(), 1);
        assert!((state.ema_price.get() - 0.55).abs() < 1e-10);
    }

    #[test]
    fn test_feature_extractor() {
        let token_id = TokenId("test-token".to_string());
        let config = FeatureExtractorConfig::default();
        let mut extractor = FeatureExtractor::new(&config);

        // Initial data - should not emit features yet
        let data1 = create_test_market_data(1000, &token_id, 0.55, 0.001, 100.0, 100.0);
        assert!(extractor.process(&data1).is_none());

        // Add more data
        for i in 1..10 {
            let data = create_test_market_data(
                1000 + i as i64 * 100,
                &token_id,
                0.55 + i as f64 * 0.001,
                0.001 + i as f64 * 0.0001,
                100.0 + i as f64 * 10.0,
                100.0 - i as f64 * 5.0,
            );
            let features = extractor.process(&data);
            if let Some(fv) = features {
                assert_eq!(fv.timestamp_us, 1000 + i as i64 * 100);
                assert_eq!(fv.token_id, token_id);
                assert!(fv.mid_price() > 0.0);
                assert!(fv.spread() >= 0.0);
            }
        }

        assert_eq!(extractor.num_tokens(), 1);
    }

    #[test]
    fn test_momentum_calculation() {
        let token_id = TokenId("test-token".to_string());
        let config = FeatureExtractorConfig::default();
        let mut state = TokenFeatureState::new(token_id.clone(), &config);

        // Build up price history
        for i in 0..40 {
            let data = create_test_market_data(
                i as i64 * 1000,
                &token_id,
                0.55 + i as f64 * 0.001,
                0.001,
                100.0,
                100.0,
            );
            state.update(&data);
        }

        // Momentum should be positive (prices increasing)
        let mom_5 = state.momentum(5);
        let mom_15 = state.momentum(15);

        assert!(mom_5 > 0.0, "5-period momentum should be positive");
        assert!(mom_15 > 0.0, "15-period momentum should be positive");
    }

    #[test]
    fn test_imbalance_calculation() {
        let token_id = TokenId("test-token".to_string());
        let config = FeatureExtractorConfig::default();
        let mut state = TokenFeatureState::new(token_id.clone(), &config);

        // Buy pressure
        let buy_data = create_test_market_data(1000, &token_id, 0.55, 0.001, 150.0, 50.0);
        state.update(&buy_data);
        assert!(state.current_imbalance() > 0.0);

        // Sell pressure
        let sell_data = create_test_market_data(2000, &token_id, 0.56, 0.001, 50.0, 150.0);
        state.update(&sell_data);
        assert!(state.current_imbalance() < 0.0);
    }

    #[test]
    fn test_percentile_computation() {
        let slice = vec![0.1, 0.2, 0.3, 0.4, 0.5];
        assert!((FeatureExtractor::compute_percentile(0.25, &slice) - 0.4).abs() < 0.1);
        assert!((FeatureExtractor::compute_percentile(0.5, &slice) - 0.8).abs() < 0.1);
        assert!((FeatureExtractor::compute_percentile(0.9, &slice) - 1.0).abs() < 0.1);
    }

    #[test]
    fn test_feature_vector_indices() {
        assert_eq!(idx::MID_PRICE, 0);
        assert_eq!(idx::SPREAD, 6);
        assert_eq!(idx::IMBALANCE, 13);
        assert_eq!(idx::PRICE_VOLATILITY, 5);
        assert_eq!(ExtractedFeatureVector::FEATURE_COUNT, 24);
    }

    #[test]
    fn test_multitoken_tracking() {
        let config = FeatureExtractorConfig::default();
        let mut extractor = FeatureExtractor::new(&config);

        let token1 = TokenId("token1".to_string());
        let token2 = TokenId("token2".to_string());

        // Seed both tokens
        for i in 0..10 {
            let d1 = create_test_market_data(
                1000 + i as i64 * 100,
                &token1,
                0.55 + i as f64 * 0.001,
                0.001,
                100.0,
                100.0,
            );
            let d2 = create_test_market_data(
                1000 + i as i64 * 100,
                &token2,
                0.75 + i as f64 * 0.001,
                0.002,
                200.0,
                150.0,
            );
            extractor.process(&d1);
            extractor.process(&d2);
        }

        assert_eq!(extractor.num_tokens(), 2);

        // Remove token1
        extractor.remove_token(&token1);
        assert_eq!(extractor.num_tokens(), 1);
    }

    #[test]
    fn test_serialization_roundtrip() {
        let token_id = TokenId("test-token".to_string());
        let features = ExtractedFeatureVector::new(1000000, token_id.clone());
        
        let json = serde_json::to_string(&features).unwrap();
        let decoded: ExtractedFeatureVector = serde_json::from_str(&json).unwrap();
        
        assert_eq!(features.timestamp_us, decoded.timestamp_us);
        assert_eq!(features.token_id, decoded.token_id);
        assert_eq!(features.features, decoded.features);
    }
}
