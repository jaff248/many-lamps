//! Data contracts for normalized market data, feature vectors, and ML signals.
//!
//! This module provides structured data types for:
//! - Normalized market data (price, spread, imbalance, volatility)
//! - Feature vectors for ML pipelines
//! - Signal data for strategy integration
//!
//! All types are designed for low-latency access and serialization support.

use crate::events::MarketDataEvent;
use crate::types::{Tick, Size, TokenId};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Feature index constants for named access to FeatureVector
pub mod idx {
    pub const MID_PRICE: usize = 0;
    pub const SPREAD: usize = 1;
    pub const BID_SIZE: usize = 2;
    pub const ASK_SIZE: usize = 3;
    pub const IMBALANCE: usize = 4;
    pub const VOLATILITY: usize = 5;
    pub const BID_SIZE_NORM: usize = 6;
    pub const ASK_SIZE_NORM: usize = 7;
}

/// Normalized market data extracted from raw order book events.
///
/// This struct provides a consistent representation of market state
/// suitable for feature extraction and ML pipelines. All prices are
/// normalized to [0, 1] range with high precision.
///
/// # Latency Considerations
/// - All fields are Copy types where possible
/// - Volatility is computed incrementally
/// - No allocations in hot path
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NormalizedMarketData {
    /// Timestamp in microseconds since epoch (monotonic conversion)
    pub timestamp_us: i64,
    /// Token/market identifier
    pub token_id: TokenId,
    /// Mid price in normalized [0, 1] range
    pub mid_price: f64,
    /// Spread in normalized price units (ask - bid)
    pub spread: f64,
    /// Total size at best bid
    pub bid_size: f64,
    /// Total size at best ask
    pub ask_size: f64,
    /// Order book imbalance: (bid_size - ask_size) / (bid_size + ask_size)
    /// Range: [-1.0, 1.0] where positive = buy pressure
    pub imbalance: f64,
    /// Volatility estimate (rolling, in normalized price units)
    pub volatility: f64,
    /// Optional: best bid price in normalized units
    pub best_bid: Option<f64>,
    /// Optional: best ask price in normalized units
    pub best_ask: Option<f64>,
}

impl NormalizedMarketData {
    /// Create new normalized market data from components
    #[inline]
    pub fn new(
        timestamp_us: i64,
        token_id: TokenId,
        mid_price: f64,
        spread: f64,
        bid_size: f64,
        ask_size: f64,
        volatility: f64,
        best_bid: Option<f64>,
        best_ask: Option<f64>,
    ) -> Self {
        let total_size = bid_size + ask_size;
        let imbalance = if total_size > 0.0 {
            (bid_size - ask_size) / total_size
        } else {
            0.0
        };

        Self {
            timestamp_us,
            token_id,
            mid_price,
            spread,
            bid_size,
            ask_size,
            imbalance,
            volatility,
            best_bid,
            best_ask,
        }
    }

    /// Quick imbalance check for directional bias
    #[inline]
    pub fn has_buy_pressure(&self) -> bool {
        self.imbalance > 0.01
    }

    /// Quick imbalance check for directional bias
    #[inline]
    pub fn has_sell_pressure(&self) -> bool {
        self.imbalance < -0.01
    }

    /// Check if spread is wide (potential inefficiency)
    #[inline]
    pub fn is_wide_spread(&self, threshold: f64) -> bool {
        self.spread > threshold
    }
}

/// Feature vector for ML model input.
///
/// Provides named accessor methods for feature indices to avoid
/// magic numbers in downstream code. Features are stored as Vec<f64>
/// for compatibility with ML libraries.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FeatureVector {
    /// Timestamp in microseconds
    pub timestamp_us: i64,
    /// Token identifier
    pub token_id: TokenId,
    /// Feature values
    pub features: Vec<f64>,
}

impl FeatureVector {
    /// Create a new feature vector with pre-allocated capacity
    #[inline]
    pub fn new(timestamp_us: i64, token_id: TokenId, capacity: usize) -> Self {
        Self {
            timestamp_us,
            token_id,
            features: Vec::with_capacity(capacity),
        }
    }

    /// Create from NormalizedMarketData with standard feature set
    #[inline]
    pub fn from_market_data(data: &NormalizedMarketData) -> Self {
        let total_size = data.bid_size + data.ask_size;
        let bid_norm = if total_size > 0.0 { data.bid_size / total_size } else { 0.5 };
        let ask_norm = if total_size > 0.0 { data.ask_size / total_size } else { 0.5 };

        Self {
            timestamp_us: data.timestamp_us,
            token_id: data.token_id.clone(),
            features: vec![
                data.mid_price,
                data.spread,
                data.bid_size,
                data.ask_size,
                data.imbalance,
                data.volatility,
                bid_norm,
                ask_norm,
            ],
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
        self.features[idx::VOLATILITY]
    }

    /// Get bid size normalized
    #[inline]
    pub fn bid_size_norm(&self) -> f64 {
        self.features[idx::BID_SIZE_NORM]
    }

    /// Get ask size normalized
    #[inline]
    pub fn ask_size_norm(&self) -> f64 {
        self.features[idx::ASK_SIZE_NORM]
    }
}

/// Signal data from ML model output.
///
/// Wraps model predictions with metadata for strategy integration.
/// Supports various signal types (direction, probability, confidence).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SignalData {
    /// Timestamp of signal generation
    pub timestamp_us: i64,
    /// Token identifier
    pub token_id: TokenId,
    /// Model identifier
    pub model_id: String,
    /// Primary signal value (interpretation depends on signal_type)
    pub value: f64,
    /// Signal type for interpretation
    pub signal_type: SignalType,
    /// Model confidence in [0, 1] range
    pub confidence: f64,
    /// Optional: feature importance scores
    pub feature_importance: Option<Vec<(String, f64)>>,
}

impl SignalData {
    /// Create a new signal
    #[inline]
    pub fn new(
        timestamp_us: i64,
        token_id: TokenId,
        model_id: String,
        value: f64,
        signal_type: SignalType,
        confidence: f64,
    ) -> Self {
        Self {
            timestamp_us,
            token_id,
            model_id,
            value,
            signal_type,
            confidence,
            feature_importance: None,
        }
    }

    /// Check if signal is actionable (high confidence)
    #[inline]
    pub fn is_actionable(&self, threshold: f64) -> bool {
        self.confidence >= threshold
    }
}

/// Types of ML signals for interpretation
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SignalType {
    /// Directional prediction: positive = long, negative = short
    Direction,
    /// Probability of price move up
    ProbabilityUp,
    /// Probability of price move down
    ProbabilityDown,
    /// Volatility forecast (annualized)
    VolatilityForecast,
    /// Expected return (normalized)
    ExpectedReturn,
    /// Custom signal with string identifier
    Custom(String),
}

impl fmt::Display for SignalType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SignalType::Direction => write!(f, "Direction"),
            SignalType::ProbabilityUp => write!(f, "ProbabilityUp"),
            SignalType::ProbabilityDown => write!(f, "ProbabilityDown"),
            SignalType::VolatilityForecast => write!(f, "VolatilityForecast"),
            SignalType::ExpectedReturn => write!(f, "ExpectedReturn"),
            SignalType::Custom(s) => write!(f, "Custom({})", s),
        }
    }
}

/// Conversion traits from existing event types

impl TryFrom<MarketDataEvent> for NormalizedMarketData {
    type Error = ConversionError;

    fn try_from(event: MarketDataEvent) -> Result<Self, Self::Error> {
        match event {
            MarketDataEvent::BestBidAsk { market_id: _, token_id, best_bid, best_ask, spread: _, timestamps } => {
                let best_bid_f = best_bid.map(tick_to_price);
                let best_ask_f = best_ask.map(tick_to_price);

                let (mid_price, spread) = match (best_bid_f, best_ask_f) {
                    (Some(bid), Some(ask)) => {
                        let mid = (bid + ask) / 2.0;
                        let spread = ask - bid;
                        (mid, spread)
                    }
                    (Some(bid), None) => (bid, 0.0),
                    (None, Some(ask)) => (ask, 0.0),
                    (None, None) => return Err(ConversionError::NoBestPrices),
                };

                Ok(NormalizedMarketData::new(
                    timestamps.ts_exchange_ms * 1000,
                    token_id,
                    mid_price,
                    spread,
                    0.0,
                    0.0,
                    0.0,
                    best_bid_f,
                    best_ask_f,
                ))
            }
            MarketDataEvent::BookSnapshot { market_id: _, token_id, bids, asks, tick_size: _, snapshot_hash: _, timestamps } => {
                let best_bid = bids.first().map(|(tick, _)| tick_to_price(*tick));
                let best_ask = asks.first().map(|(tick, _)| tick_to_price(*tick));

                let (mid_price, spread) = match (best_bid, best_ask) {
                    (Some(bid), Some(ask)) => {
                        let mid = (bid + ask) / 2.0;
                        let spread = ask - bid;
                        (mid, spread)
                    }
                    _ => return Err(ConversionError::EmptyBook),
                };

                let bid_size = bids.first()
                    .map(|(_, size)| *size as f64 / 1_000_000.0)
                    .unwrap_or(0.0);
                let ask_size = asks.first()
                    .map(|(_, size)| *size as f64 / 1_000_000.0)
                    .unwrap_or(0.0);

                Ok(NormalizedMarketData::new(
                    timestamps.ts_exchange_ms * 1000,
                    token_id,
                    mid_price,
                    spread,
                    bid_size,
                    ask_size,
                    0.0,
                    best_bid,
                    best_ask,
                ))
            }
            MarketDataEvent::BookDelta { market_id: _, token_id, side: _, tick: _, new_size, best_bid, best_ask, order_hash: _, timestamps } => {
                let best_bid_f = best_bid.map(tick_to_price);
                let best_ask_f = best_ask.map(tick_to_price);

                let (mid_price, spread) = match (best_bid_f, best_ask_f) {
                    (Some(bid), Some(ask)) => {
                        let mid = (bid + ask) / 2.0;
                        let spread = ask - bid;
                        (mid, spread)
                    }
                    (Some(bid), None) => (bid, 0.0),
                    (None, Some(ask)) => (ask, 0.0),
                    (None, None) => return Err(ConversionError::NoBestPrices),
                };

                let size = new_size as f64 / 1_000_000.0;

                Ok(NormalizedMarketData::new(
                    timestamps.ts_exchange_ms * 1000,
                    token_id,
                    mid_price,
                    spread,
                    size,
                    size,
                    0.0,
                    best_bid_f,
                    best_ask_f,
                ))
            }
            _ => Err(ConversionError::UnsupportedEventType),
        }
    }
}

/// Conversion errors
#[derive(Debug, thiserror::Error)]
pub enum ConversionError {
    #[error("No best bid/ask prices available")]
    NoBestPrices,
    #[error("Order book is empty")]
    EmptyBook,
    #[error("Unsupported event type for conversion")]
    UnsupportedEventType,
}

/// Convert tick to normalized price
#[inline]
pub fn tick_to_price(tick: Tick) -> f64 {
    tick as f64 / 10000.0
}

/// Convert normalized price to tick (with rounding)
#[inline]
pub fn price_to_tick(price: f64) -> Tick {
    (price * 10000.0).round() as Tick
}

/// Convert size in micro-shares to floating point
#[inline]
pub fn micro_size_to_float(size: Size) -> f64 {
    size as f64 / 1_000_000.0
}

/// Convert float size to micro-shares
#[inline]
pub fn float_to_micro_size(size: f64) -> Size {
    (size * 1_000_000.0).round() as Size
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::EventTimestamps;
    use crate::types::{Side, MarketId};

    fn create_test_book_snapshot() -> MarketDataEvent {
        MarketDataEvent::BookSnapshot {
            market_id: MarketId("test-market".to_string()),
            token_id: TokenId("0x123".to_string()),
            bids: vec![(5500, 100_000), (5499, 200_000), (5498, 300_000)],
            asks: vec![(5501, 150_000), (5502, 250_000)],
            tick_size: 100,
            snapshot_hash: "hash123".to_string(),
            timestamps: EventTimestamps::new(1_000_000_000, 1_000_500_000),
        }
    }

    fn create_test_best_bid_ask() -> MarketDataEvent {
        MarketDataEvent::BestBidAsk {
            market_id: MarketId("test-market".to_string()),
            token_id: TokenId("0x123".to_string()),
            best_bid: Some(5500),
            best_ask: Some(5501),
            spread: Some(1),
            timestamps: EventTimestamps::new(1_000_000_000, 1_000_500_000),
        }
    }

    fn create_test_book_delta() -> MarketDataEvent {
        MarketDataEvent::BookDelta {
            market_id: MarketId("test-market".to_string()),
            token_id: TokenId("0x123".to_string()),
            side: Side::Buy,
            tick: 5501,
            new_size: 100_000,
            best_bid: Some(5500),
            best_ask: Some(5502),
            order_hash: "order123".to_string(),
            timestamps: EventTimestamps::new(1_000_000_000, 1_000_500_000),
        }
    }

    #[test]
    fn test_normalized_market_data_creation() {
        let data = NormalizedMarketData::new(
            1_000_000_000_000,
            TokenId("test".to_string()),
            0.5501,
            0.0001,
            1.5,
            2.0,
            0.001,
            Some(0.55),
            Some(0.5501),
        );

        assert_eq!(data.timestamp_us, 1_000_000_000_000);
        assert_eq!(data.mid_price, 0.5501);
        assert_eq!(data.spread, 0.0001);
        // Imbalance = (1.5 - 2.0) / (1.5 + 2.0) = -0.5 / 3.5 = -0.1428...
        assert!((data.imbalance - (-0.142857)).abs() < 0.0001);
    }

    #[test]
    fn test_imbalance_calculations() {
        // Balanced book
        let balanced = NormalizedMarketData::new(
            0,
            TokenId("test".to_string()),
            0.5,
            0.01,
            100.0,
            100.0,
            0.0,
            None,
            None,
        );
        assert!((balanced.imbalance - 0.0).abs() < 0.0001);

        // Buy pressure
        let buy = NormalizedMarketData::new(
            0,
            TokenId("test".to_string()),
            0.5,
            0.01,
            150.0,
            50.0,
            0.0,
            None,
            None,
        );
        assert!((buy.imbalance - 0.5).abs() < 0.0001);

        // Sell pressure
        let sell = NormalizedMarketData::new(
            0,
            TokenId("test".to_string()),
            0.5,
            0.01,
            50.0,
            150.0,
            0.0,
            None,
            None,
        );
        assert!((sell.imbalance - (-0.5)).abs() < 0.0001);

        // Empty book
        let empty = NormalizedMarketData::new(
            0,
            TokenId("test".to_string()),
            0.5,
            0.01,
            0.0,
            0.0,
            0.0,
            None,
            None,
        );
        assert_eq!(empty.imbalance, 0.0);
    }

    #[test]
    fn test_book_snapshot_conversion() {
        let snapshot = create_test_book_snapshot();
        let normalized: NormalizedMarketData = snapshot.try_into().unwrap();

        assert_eq!(normalized.timestamp_us, 1_000_000_000_000); // ms * 1000
        assert_eq!(normalized.token_id, TokenId("0x123".to_string()));
        // Mid price: (0.55 + 0.5501) / 2 = 0.55005
        assert!((normalized.mid_price - 0.55005).abs() < 0.00001);
        // Spread: 0.5501 - 0.55 = 0.0001
        assert!((normalized.spread - 0.0001).abs() < 0.00001);
        // Best bid: 5500 / 10000 = 0.55
        assert_eq!(normalized.best_bid, Some(0.55));
        // Best ask: 5501 / 10000 = 0.5501
        assert_eq!(normalized.best_ask, Some(0.5501));
    }

    #[test]
    fn test_best_bid_ask_conversion() {
        let bba = create_test_best_bid_ask();
        let normalized: NormalizedMarketData = bba.try_into().unwrap();

        assert_eq!(normalized.timestamp_us, 1_000_000_000_000);
        assert_eq!(normalized.mid_price, 0.55005);
        // Use approximate comparison for floating point
        assert!((normalized.spread - 0.0001).abs() < 1e-10);
        assert_eq!(normalized.best_bid, Some(0.55));
        assert_eq!(normalized.best_ask, Some(0.5501));
    }

    #[test]
    fn test_book_delta_conversion() {
        let delta = create_test_book_delta();
        let normalized: NormalizedMarketData = delta.try_into().unwrap();

        assert_eq!(normalized.timestamp_us, 1_000_000_000_000);
        // Mid price: (0.55 + 0.5502) / 2 = 0.5501
        assert!((normalized.mid_price - 0.5501).abs() < 0.00001);
        // Spread: 0.5502 - 0.55 = 0.0002
        assert!((normalized.spread - 0.0002).abs() < 0.00001);
    }

    #[test]
    fn test_feature_vector_from_market_data() {
        let data = NormalizedMarketData::new(
            1_000_000_000_000,
            TokenId("test".to_string()),
            0.55,
            0.001,
            150.0,
            100.0,
            0.01,
            Some(0.55),
            Some(0.551),
        );

        let features = FeatureVector::from_market_data(&data);

        assert_eq!(features.timestamp_us, 1_000_000_000_000);
        assert_eq!(features.token_id, TokenId("test".to_string()));
        assert_eq!(features.features.len(), 8);
        assert_eq!(features.mid_price(), 0.55);
        assert_eq!(features.spread(), 0.001);
        // Imbalance: (150 - 100) / 250 = 0.2
        assert!((features.imbalance() - 0.2).abs() < 0.0001);
        // Bid normalized: 150 / 250 = 0.6
        assert!((features.bid_size_norm() - 0.6).abs() < 0.0001);
        // Ask normalized: 100 / 250 = 0.4
        assert!((features.ask_size_norm() - 0.4).abs() < 0.0001);
    }

    #[test]
    fn test_signal_data_actionable() {
        let signal = SignalData::new(
            1_000_000_000_000,
            TokenId("test".to_string()),
            "model_v1".to_string(),
            0.75,
            SignalType::ProbabilityUp,
            0.85,
        );

        assert!(signal.is_actionable(0.8));
        assert!(!signal.is_actionable(0.9));
    }

    #[test]
    fn test_tick_price_conversion() {
        assert!((tick_to_price(5500) - 0.55).abs() < 0.00001);
        assert!((tick_to_price(1) - 0.0001).abs() < 0.000001);
        assert!((tick_to_price(10000) - 1.0).abs() < 0.00001);

        assert_eq!(price_to_tick(0.55), 5500);
        assert_eq!(price_to_tick(0.0001), 1);
        assert_eq!(price_to_tick(1.0), 10000);
    }

    #[test]
    fn test_micro_size_conversion() {
        assert!((micro_size_to_float(1_000_000) - 1.0).abs() < 0.000001);
        assert!((micro_size_to_float(500_000) - 0.5).abs() < 0.000001);
        assert_eq!(float_to_micro_size(1.0), 1_000_000);
        assert_eq!(float_to_micro_size(0.5), 500_000);
    }

    #[test]
    fn test_market_data_event_conversion() {
        let event = create_test_best_bid_ask();
        let normalized: NormalizedMarketData = event.try_into().unwrap();
        assert_eq!(normalized.mid_price, 0.55005);
    }

    #[test]
    fn test_pressure_detection() {
        let buy_pressure = NormalizedMarketData::new(
            0,
            TokenId("test".to_string()),
            0.5,
            0.01,
            100.0,
            50.0,
            0.0,
            None,
            None,
        );
        assert!(buy_pressure.has_buy_pressure());
        assert!(!buy_pressure.has_sell_pressure());

        let sell_pressure = NormalizedMarketData::new(
            0,
            TokenId("test".to_string()),
            0.5,
            0.01,
            50.0,
            100.0,
            0.0,
            None,
            None,
        );
        assert!(!sell_pressure.has_buy_pressure());
        assert!(sell_pressure.has_sell_pressure());
    }

    #[test]
    fn test_wide_spread_detection() {
        let tight = NormalizedMarketData::new(
            0,
            TokenId("test".to_string()),
            0.5,
            0.0005,
            100.0,
            100.0,
            0.0,
            None,
            None,
        );
        assert!(!tight.is_wide_spread(0.001));

        let wide = NormalizedMarketData::new(
            0,
            TokenId("test".to_string()),
            0.5,
            0.005,
            100.0,
            100.0,
            0.0,
            None,
            None,
        );
        assert!(wide.is_wide_spread(0.001));
    }

    #[test]
    fn test_serialization_roundtrip() {
        let data = NormalizedMarketData::new(
            1_000_000_000_000,
            TokenId("test".to_string()),
            0.55,
            0.001,
            150.0,
            100.0,
            0.01,
            Some(0.55),
            Some(0.551),
        );

        let json = serde_json::to_string(&data).unwrap();
        let decoded: NormalizedMarketData = serde_json::from_str(&json).unwrap();

        assert_eq!(data, decoded);
    }

    #[test]
    fn test_feature_index_constants() {
        assert_eq!(idx::MID_PRICE, 0);
        assert_eq!(idx::SPREAD, 1);
        assert_eq!(idx::BID_SIZE, 2);
        assert_eq!(idx::ASK_SIZE, 3);
        assert_eq!(idx::IMBALANCE, 4);
        assert_eq!(idx::VOLATILITY, 5);
        assert_eq!(idx::BID_SIZE_NORM, 6);
        assert_eq!(idx::ASK_SIZE_NORM, 7);
    }
}
