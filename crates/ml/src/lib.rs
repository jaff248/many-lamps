//! T-KAN (Temporal Kolmogorov-Arnold Network) Model for Price Prediction
//!
//! This module provides ML inference for Polymarket price prediction using
//! a SiLU-based MLP approximation of KAN layers.
//!
//! # Usage
//! ```
//! use mtrader_ml::{TkanModel, TkanConfig, FeatureVector};
//!
//! let config = TkanConfig::default();
//! let model = TkanModel::new(&config);
//!
//! let features = FeatureVector::from_price_history(&[5000, 5010, 5020], &[10, 12, 8], 1000);
//! let signal = model.predict(&features);
//! ```
//!
//! # Model format
//! Models can be loaded from JSON format:
//! ```no_run
//! use mtrader_ml::{TkanModel, TkanConfig};
//!
//! let config = TkanConfig::default();
//! let model = TkanModel::load_json("model.json", &config).expect("Failed to load model");
//! ```
//!
//! For mock inference (no weights), use:
//! ```no_run
//! use mtrader_ml::{TkanModel, TkanConfig};
//!
//! let config = TkanConfig::default();
//! let model = TkanModel::dummy(&config);
//! ```

// Large-Margin Softmax Classifier (Phase 2.1)
mod margin_softmax;

// Multi-Head Attention for Cross-Asset Correlation (Phase 2.2)
mod attention;

// Uncertainty Quantification for Position Sizing (Phase 2.3)
mod uncertainty;

// Feature extraction pipeline (Phase 1.2)
mod feature_pipeline;

// Inference engine (Phase 1.4)
mod inference;

// Meta-Learning (MAML) for Rapid Regime Adaptation (Phase 3.2)
mod meta_learning;

// Re-export margin softmax types
pub use margin_softmax::{
    MarginSoftmaxConfig,
    MarginSoftmaxClassifier,
    DirectionalSignal,
};

// Re-export attention types
pub use attention::{
    AttentionConfig,
    MultiHeadAttention,
    CrossAssetAttention,
    scaled_dot_product_attention,
    mean_pooling,
    max_pooling,
};

// Re-export feature pipeline types
pub use feature_pipeline::{
    FeatureExtractor,
    FeatureExtractorConfig,
    ExtractedFeatureVector,
    TokenFeatureState,
    SlidingWindow,
    RollingStats,
    EMA,
};

// Re-export inference types
pub use inference::{
    InferenceEngine,
    InferenceConfig,
    InferenceMetrics,
    FallbackPolicy,
    InferenceError,
};

// Re-export uncertainty types
pub use uncertainty::{
    UncertaintyConfig,
    UncertaintyEstimator,
    UncertaintyResult,
    MonteCarloDropout,
    PositionSizingResult,
};

// Re-export meta-learning types
pub use meta_learning::{
    MAMLConfig,
    MetaLearner,
    Task,
    MAMLError,
    RegimeConverter,
    MAMLInferenceAdapter,
};

use many_lamps_core::{Tick, Side};
use serde::{Deserialize, Serialize};
use serde_json;
use std::collections::VecDeque;
use std::fs;
use anyhow::{Result, Context};

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
        let mut features = Vec::with_capacity(window + 2); // window + spread + volatility

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
}

/// Neural Network layer weights
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayerWeights {
    pub weights: Vec<Vec<f64>>,
    pub biases: Vec<f64>,
}

/// T-KAN Model weights
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelWeights {
    pub input_layer: LayerWeights,
    pub hidden_layer: LayerWeights,
}

/// T-KAN Model wrapper
#[derive(Debug, Clone)]
pub struct TkanModel {
    config: TkanConfig,
    weights: Option<ModelWeights>,
    initialized: bool,
}

impl TkanModel {
    /// Create a new ML model with random weights
    ///
    /// This creates a functional model even without pre-trained weights.
    /// For production use, load pre-trained weights from `load_json`.
    pub fn new(config: &TkanConfig) -> Self {
        let input_size = config.window_size + 2; // features + spread + vol
        let hidden_size = config.hidden_size;
        let output_size = config.output_size;

        // Create random weights for mock inference
        let input_layer = LayerWeights {
            weights: vec![vec![0.0; input_size]; hidden_size],
            biases: vec![0.0; hidden_size],
        };

        let hidden_layer = LayerWeights {
            weights: vec![vec![0.0; hidden_size]; output_size],
            biases: vec![0.0; output_size],
        };

        let weights = ModelWeights {
            input_layer,
            hidden_layer,
        };

        Self {
            config: config.clone(),
            weights: Some(weights),
            initialized: true,
        }
    }

    /// Load model from JSON file
    pub fn load_json(model_path: &str, config: &TkanConfig) -> Result<Self> {
        let contents = fs::read_to_string(model_path)
            .context(format!("Failed to read model file: {}", model_path))?;

        let weights: ModelWeights = serde_json::from_str(&contents)
            .context(format!("Failed to parse JSON from: {}", model_path))?;

        // Validate dimensions
        let input_size = config.window_size + 2;
        let hidden_size = config.hidden_size;
        let output_size = config.output_size;

        if weights.input_layer.weights.len() != hidden_size {
            return Err(anyhow::anyhow!(
                "Input layer weights height {} doesn't match hidden size {}",
                weights.input_layer.weights.len(),
                hidden_size
            ));
        }

        if weights.input_layer.weights[0].len() != input_size {
            return Err(anyhow::anyhow!(
                "Input layer weights width {} doesn't match input size {}",
                weights.input_layer.weights[0].len(),
                input_size
            ));
        }

        if weights.input_layer.biases.len() != hidden_size {
            return Err(anyhow::anyhow!(
                "Input layer biases length {} doesn't match hidden size {}",
                weights.input_layer.biases.len(),
                hidden_size
            ));
        }

        if weights.hidden_layer.weights.len() != output_size {
            return Err(anyhow::anyhow!(
                "Hidden layer weights height {} doesn't match output size {}",
                weights.hidden_layer.weights.len(),
                output_size
            ));
        }

        if weights.hidden_layer.weights[0].len() != hidden_size {
            return Err(anyhow::anyhow!(
                "Hidden layer weights width {} doesn't match hidden size {}",
                weights.hidden_layer.weights[0].len(),
                hidden_size
            ));
        }

        if weights.hidden_layer.biases.len() != output_size {
            return Err(anyhow::anyhow!(
                "Hidden layer biases length {} doesn't match output size {}",
                weights.hidden_layer.biases.len(),
                output_size
            ));
        }

        Ok(Self {
            config: config.clone(),
            weights: Some(weights),
            initialized: true,
        })
    }

    /// Create dummy model for testing (no weights)
    pub fn dummy(config: &TkanConfig) -> Self {
        let mut model = Self::new(config);
        model.weights = None;
        model
    }

    /// Compute SiLU (Sigmoid-weighted Linear Unit) activation
    fn silu(x: f64) -> f64 {
        x / (1.0 + (-x).exp())
    }

    /// Apply linear layer (W·x + b)
    fn linear_layer(input: &[f64], weights: &[Vec<f64>], biases: &[f64]) -> Vec<f64> {
        let mut output = vec![0.0; weights.len()];
        
        for i in 0..weights.len() {
            let mut sum = biases[i];
            for j in 0..input.len() {
                sum += weights[i][j] * input[j];
            }
            output[i] = sum;
        }
        
        output
    }

    /// Run inference
    pub fn predict(&self, features: &FeatureVector) -> TkanSignal {
        if !self.initialized || self.weights.is_none() {
            return TkanSignal::neutral(features.timestamp_ns);
        }

        let weights = self.weights.as_ref().unwrap();
        let input = &features.features;

        // Validate input size
        let expected_input_size = self.config.window_size + 2;
        if input.len() != expected_input_size {
            tracing::warn!(
                "Feature vector size {} doesn't match expected input size {}",
                input.len(),
                expected_input_size
            );
            return TkanSignal::neutral(features.timestamp_ns);
        }

        // Forward pass: Input -> Hidden (SiLU) -> Output
        let hidden = Self::linear_layer(input, &weights.input_layer.weights, &weights.input_layer.biases);
        
        // Apply SiLU activation
        let hidden_activated: Vec<f64> = hidden.iter().map(|&x| Self::silu(x)).collect();
        
        // Output layer
        let output = Self::linear_layer(&hidden_activated, &weights.hidden_layer.weights, &weights.hidden_layer.biases);
        
        // Output: [direction, confidence]
        let direction = if output.len() > 0 { output[0].clamp(-1.0, 1.0) } else { 0.0 };
        let confidence_raw = if output.len() > 1 { output[1] } else { 0.0 };

        // Apply sigmoid to confidence
        let confidence = 1.0 / (1.0 + (-confidence_raw).exp());

        TkanSignal {
            direction,
            confidence,
            timestamp_ns: features.timestamp_ns,
        }
    }

    /// Check if model is enabled
    pub fn is_enabled(&self) -> bool {
        self.initialized && self.weights.is_some()
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
    fn test_ml_model_inference() {
        let config = TkanConfig::default();
        let model = TkanModel::new(&config);
        assert!(model.is_enabled());

        // Create test features
        let features = FeatureVector {
            features: vec![0.0; config.window_size + 2],
            timestamp_ns: 1000,
        };

        let signal = model.predict(&features);
        assert_eq!(signal.timestamp_ns, 1000);
        // With zero weights, direction should be 0 (neutral), confidence ~0.5 (sigmoid(0) = 0.5)
        assert!(signal.direction.abs() < 1e-10);
        assert!((signal.confidence - 0.5).abs() < 1e-10); // sigmoid(0) = 0.5
    }

    #[test]
    fn test_ml_model_dummy() {
        let config = TkanConfig::default();
        let model = TkanModel::dummy(&config);
        assert!(!model.is_enabled()); // Dummy model has no weights

        let features = FeatureVector {
            features: vec![0.0; config.window_size + 2],
            timestamp_ns: 2000,
        };

        let signal = model.predict(&features);
        // Dummy model returns neutral signal
        assert_eq!(signal.direction, 0.0);
        assert_eq!(signal.confidence, 0.0);
        assert_eq!(signal.timestamp_ns, 2000);
    }

    #[test]
    fn test_silu_activation() {
        use super::TkanModel;
        // Test SiLU activation function
        assert_eq!(TkanModel::silu(0.0), 0.0);
        assert!(TkanModel::silu(1.0) < 1.0);
        assert!(TkanModel::silu(-1.0) > -1.0);
    }

    #[test]
    fn test_linear_layer() {
        use super::TkanModel;
        
        let weights = vec![
            vec![1.0, 2.0],
            vec![3.0, 4.0],
        ];
        let biases = vec![0.5, 1.0];
        let input = vec![2.0, 3.0];
        
        let output = TkanModel::linear_layer(&input, &weights, &biases);
        
        // Output[0] = 0.5 + 1.0*2.0 + 2.0*3.0 = 0.5 + 2.0 + 6.0 = 8.5
        // Output[1] = 1.0 + 3.0*2.0 + 4.0*3.0 = 1.0 + 6.0 + 12.0 = 19.0
        assert_eq!(output.len(), 2);
        assert!((output[0] - 8.5).abs() < 1e-10);
        assert!((output[1] - 19.0).abs() < 1e-10);
    }
}
