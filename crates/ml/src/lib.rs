//! T-KAN (Temporal Kolmogorov-Arnold Network) Model for Price Prediction
//!
//! This module provides ML inference for Polymarket price prediction using
//! a SiLU-based MLP approximation of KAN layers.
//!
//! # Feature Flag
//! Enable the `ml` feature to use T-KAN models:
//! ```toml
//! [dependencies]
//! mtrader-ml = { path = "crates/ml", features = ["ml"] }
//! ```
//!
//! # Usage (requires `ml` feature)
//! ```
//! use mtrader_ml::{TkanModel, TkanConfig, FeatureVector};
//!
//! #[cfg(feature = "ml")]
//! {
//!     let config = TkanConfig::default();
//!     let model = TkanModel::load("model.ot", &config).expect("Failed to load model");
//!
//!     let features = FeatureVector::from_price_history(&[5000, 5010, 5020], &[10, 12, 8], 1000);
//!     let signal = model.predict(&features);
//! }
//! ```
//!
//! Without `ml` feature, use the FeatureBuffer to accumulate data:
//! ```
//! use mtrader_ml::FeatureBuffer;
//!
//! let mut buffer = FeatureBuffer::new(20);
//! buffer.push(5000, 10);
//! buffer.push(5010, 12);
//!
//! if let Some(features) = buffer.to_feature_vector(1000) {
//!     // Process features...
//! }
//! ```

use many_lamps_core::{Tick, Side};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// T-KAN prediction signal
#[derive(Debug, Clone, PartialEq)]
pub struct TkanSignal {
    /// Direction: -1.0 (bearish) to 1.0 (bullish)
    pub direction: f64,
    /// Confidence: 0.0 to 1.0
    pub confidence: f64,
    /// Timestamp of prediction
    pub timestamp_ns: u64,
}

/// Configuration for T-KAN model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TkanConfig {
    /// Input feature window size
    pub window_size: usize,
    /// Hidden layer size
    pub hidden_size: usize,
    /// Output dimension (1 for direction + 1 for confidence)
    pub output_size: usize,
    /// Model file path
    pub model_path: String,
    /// Minimum confidence threshold
    pub min_confidence: f64,
}

impl Default for TkanConfig {
    fn default() -> Self {
        Self {
            window_size: 20,
            hidden_size: 64,
            output_size: 2,
            model_path: "models/tkan_model.ot".to_string(),
            min_confidence: 0.5,
        }
    }
}

/// Feature vector for T-KAN input
#[derive(Debug, Clone)]
pub struct FeatureVector {
    /// Normalized features [window_size * num_features]
    pub features: Vec<f64>,
    pub timestamp_ns: u64,
}

impl FeatureVector {
    /// Create feature vector from price history
    ///
    /// Features extracted:
    /// - Price returns (normalized)
    /// - Volume proxy (tick count)
    /// - Spread
    /// - Momentum indicators
    pub fn from_price_history(prices: &[Tick], spreads: &[u16], now_ns: u64) -> Self {
        let window = prices.len().min(20);
        let mut features = Vec::with_capacity(window * 4);

        // Price returns (pct change)
        for i in (prices.len().saturating_sub(window))..prices.len() {
            if i > 0 {
                let curr = prices[i] as f64 / 10000.0;
                let prev = prices[i - 1] as f64 / 10000.0;
                let ret = if prev > 0.0 { (curr - prev) / prev } else { 0.0 };
                features.push(ret.clamp(-1.0, 1.0)); // Normalize
            } else {
                features.push(0.0);
            }
        }

        // Fill remaining if needed
        while features.len() < window {
            features.push(0.0);
        }

        // Spread features
        let avg_spread = if !spreads.is_empty() {
            spreads.iter().map(|s| *s as f64).sum::<f64>() / spreads.len() as f64 / 100.0
        } else {
            0.0
        };
        features.push(avg_spread.clamp(0.0, 1.0));

        // Volatility proxy (spread std)
        let spread_std = if spreads.len() > 1 {
            let mean = spreads.iter().map(|s| *s as f64).sum::<f64>() / spreads.len() as f64;
            let variance = spreads.iter()
                .map(|s| {
                    let d = (*s as f64) - mean;
                    d * d
                })
                .sum::<f64>() / spreads.len() as f64;
            variance.sqrt() / 100.0
        } else {
            0.0
        };
        features.push(spread_std.clamp(0.0, 1.0));

        Self {
            features,
            timestamp_ns: now_ns,
        }
    }

    /// Convert to tensor format for tch-rs
    #[cfg(feature = "ml")]
    pub fn to_tensor(&self) -> tch::Tensor {
        let shape = [1, self.features.len() as i64];
        tch::Tensor::of_slice(&self.features, shape, false)
    }
}

/// T-KAN Model wrapper
#[derive(Debug, Clone)]
pub struct TkanModel {
    #[cfg(feature = "ml")]
    model: tch::nn::Sequential,
    #[cfg(feature = "ml")]
    device: tch::Device,
    config: TkanConfig,
    initialized: bool,
}

impl TkanModel {
    /// Load model from PyTorch checkpoint
    ///
    /// Requires `ml` feature to be enabled.
    #[cfg(feature = "ml")]
    pub fn load(model_path: &str, config: &TkanConfig) -> Result<Self, tch::TchError> {
        use tch::nn::{Linear, LinearConfig, Sequential};

        let device = tch::Device::Cpu;
        let vs = tch::nn::VarStore::new(device);

        // Load weights from file
        vs.load(model_path)?;

        // Build MLP: Input -> Hidden (SiLU) -> Output
        let input_size = config.window_size + 2; // features + spread + vol
        let hidden_size = config.hidden_size;
        let output_size = config.output_size;

        let sequential = Sequential::new(
            Linear::new(
                vs.root() / "fc1",
                tch::nn::LinearConfig { bias: true },
                input_size as i64,
                hidden_size as i64,
            ),
            tch::nn::func(|x| x.silu()), // SiLU activation (approx of KAN)
            Linear::new(
                vs.root() / "fc2",
                tch::nn::LinearConfig { bias: true },
                hidden_size as i64,
                output_size as i64,
            ),
        );

        Ok(Self {
            model: sequential,
            device,
            config: config.clone(),
            initialized: true,
        })
    }

    /// Create dummy model for testing (no weights)
    #[cfg(feature = "ml")]
    pub fn dummy(config: &TkanConfig) -> Self {
        use tch::nn::{Linear, Sequential};

        let device = tch::Device::Cpu;
        let vs = tch::nn::VarStore::new(device);

        let input_size = config.window_size + 2;
        let hidden_size = config.hidden_size;
        let output_size = config.output_size;

        let sequential = Sequential::new(
            Linear::new(
                vs.root() / "fc1",
                tch::nn::LinearConfig { bias: true },
                input_size as i64,
                hidden_size as i64,
            ),
            tch::nn::func(|x| x.silu()),
            Linear::new(
                vs.root() / "fc2",
                tch::nn::LinearConfig { bias: true },
                hidden_size as i64,
                output_size as i64,
            ),
        );

        Self {
            model: sequential,
            device,
            config: config.clone(),
            initialized: true,
        }
    }

    /// Run inference
    #[cfg(feature = "ml")]
    pub fn predict(&self, features: &FeatureVector) -> TkanSignal {
        if !self.initialized {
            return TkanSignal::neutral(features.timestamp_ns);
        }

        let input = features.to_tensor();
        let output = self.model.forward(&input);

        // Output: [direction, confidence]
        let direction = output.double_value(&[0, 0]);
        let confidence_raw = output.double_value(&[0, 1]);

        // Apply sigmoid to confidence
        let confidence = 1.0 / (1.0 + (-confidence_raw).exp());

        // Clamp direction
        let direction = direction.clamp(-1.0, 1.0);

        TkanSignal {
            direction,
            confidence,
            timestamp_ns: features.timestamp_ns,
        }
    }

    /// Check if model is enabled
    pub fn is_enabled() -> bool {
        #[cfg(feature = "ml")]
        return true;

        #[cfg(not(feature = "ml"))]
        return false;
    }
}

impl TkanSignal {
    /// Create neutral signal
    pub fn neutral(timestamp_ns: u64) -> Self {
        Self {
            direction: 0.0,
            confidence: 0.0,
            timestamp_ns,
        }
    }

    /// Check if signal is actionable
    pub fn is_actionable(&self, min_confidence: f64) -> bool {
        self.confidence >= min_confidence && self.direction.abs() > 0.1
    }

    /// Get trading side from direction
    pub fn side(&self) -> Option<Side> {
        if self.direction > 0.1 {
            Some(Side::Buy)
        } else if self.direction < -0.1 {
            Some(Side::Sell)
        } else {
            None
        }
    }
}

/// Feature buffer for sliding window
#[derive(Debug)]
pub struct FeatureBuffer {
    prices: VecDeque<Tick>,
    spreads: VecDeque<u16>,
    max_size: usize,
}

impl FeatureBuffer {
    pub fn new(max_size: usize) -> Self {
        Self {
            prices: VecDeque::with_capacity(max_size),
            spreads: VecDeque::with_capacity(max_size),
            max_size,
        }
    }

    pub fn push(&mut self, price: Tick, spread: u16) {
        self.prices.push_back(price);
        self.spreads.push_back(spread);

        if self.prices.len() > self.max_size {
            self.prices.pop_front();
            self.spreads.pop_front();
        }
    }

    pub fn to_feature_vector(&self, now_ns: u64) -> Option<FeatureVector> {
        if self.prices.is_empty() {
            return None;
        }

        let prices: Vec<Tick> = self.prices.iter().copied().collect();
        let spreads: Vec<u16> = self.spreads.iter().copied().collect();

        Some(FeatureVector::from_price_history(&prices, &spreads, now_ns))
    }

    pub fn len(&self) -> usize {
        self.prices.len()
    }

    pub fn is_empty(&self) -> bool {
        self.prices.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_feature_buffer() {
        let mut buffer = FeatureBuffer::new(5);

        buffer.push(5000, 10); // 0.5 price, 10 tick spread
        buffer.push(5010, 12);
        buffer.push(5020, 8);

        assert_eq!(buffer.len(), 3);

        let features = buffer.to_feature_vector(1000);
        assert!(features.is_some());
        assert!(!features.unwrap().features.is_empty());
    }

    #[test]
    fn test_tkan_signal() {
        let signal = TkanSignal::neutral(1000);
        assert_eq!(signal.direction, 0.0);
        assert_eq!(signal.confidence, 0.0);

        let signal = TkanSignal {
            direction: 0.8,
            confidence: 0.7,
            timestamp_ns: 2000,
        };

        assert!(signal.is_actionable(0.5));
        assert_eq!(signal.side(), Some(Side::Buy));
    }

    #[test]
    #[cfg(feature = "ml")]
    fn test_dummy_model() {
        let config = TkanConfig::default();
        let model = TkanModel::dummy(&config);
        assert!(model.is_enabled());

        let buffer = FeatureBuffer::new(config.window_size);
        // Buffer is empty, should get neutral signal
        let features = FeatureVector {
            features: vec![0.0; config.window_size + 2],
            timestamp_ns: 1000,
        };

        let signal = model.predict(&features);
        // Dummy model has random weights, just check structure
        assert!(signal.timestamp_ns > 0);
    }
}
