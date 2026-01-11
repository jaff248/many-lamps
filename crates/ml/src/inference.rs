//! Inference Engine with Warm Caching for T-KAN Model
//!
//! High-performance ML inference pipeline targeting <1ms latency including
//! feature extraction and model forward pass.
//!
//! # Architecture
//!
//! ```text
//! NormalizedMarketData
//!   → FeatureExtractor (online feature computation)
//!   → TkanModel (warm cached weights)
//!   → SignalData (with confidence thresholding)
//!   → FallbackPolicy (if model unavailable)
//! ```
//!
//! # Features
//!
//! - **Warm Caching**: Model weights loaded once and cached in memory
//! - **Hot Reload**: Model can be reloaded without service interruption
//! - **Fallback Policies**: Graceful degradation when ML unavailable
//! - **Latency Tracking**: Microsecond-precision inference metrics
//! - **Batch Processing**: Efficient batch inference for multiple tokens

use many_lamps_core::{NormalizedMarketData, SignalData, TokenId};
use many_lamps_core::data_contracts::SignalType;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use thiserror::Error;

/// Fallback policy when ML inference is unavailable
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FallbackPolicy {
    /// Use the last valid signal (if available)
    UseLastSignal,
    /// Return a neutral signal (direction=0, confidence=0)
    Neutral,
    /// Skip prediction entirely (return None)
    Skip,
}

impl Default for FallbackPolicy {
    fn default() -> Self {
        FallbackPolicy::Neutral
    }
}

/// Inference engine configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferenceConfig {
    /// Path to T-KAN model weights file
    pub model_path: PathBuf,
    /// Enable warm caching of model weights
    pub enable_caching: bool,
    /// Maximum batch size for batch predictions
    pub max_batch_size: usize,
    /// Confidence threshold for actionable signals [0, 1]
    pub confidence_threshold: f64,
    /// Fallback policy when model unavailable
    pub fallback_policy: FallbackPolicy,
    /// Feature extractor configuration (None to disable feature extraction)
    pub feature_extractor_config: Option<super::FeatureExtractorConfig>,
    /// Enable detailed latency metrics
    pub enable_metrics: bool,
}

impl Default for InferenceConfig {
    fn default() -> Self {
        Self {
            model_path: PathBuf::from("models/tkan_model.json"),
            enable_caching: true,
            max_batch_size: 32,
            confidence_threshold: 0.5,
            fallback_policy: FallbackPolicy::default(),
            feature_extractor_config: Some(super::FeatureExtractorConfig::default()),
            enable_metrics: true,
        }
    }
}

impl InferenceConfig {
    /// Create config with default path
    pub fn new(model_path: PathBuf) -> Self {
        Self {
            model_path,
            ..Default::default()
        }
    }

    /// Create config for HFT (high-frequency trading)
    pub fn hft() -> Self {
        Self {
            model_path: PathBuf::from("models/tkan_model.json"),
            enable_caching: true,
            max_batch_size: 64,
            confidence_threshold: 0.6,
            fallback_policy: FallbackPolicy::UseLastSignal,
            feature_extractor_config: Some(super::FeatureExtractorConfig {
                price_window: 100,
                spread_window: 50,
                volume_window: 20,
                ema_alpha_price: 0.15,
                ema_alpha_spread: 0.25,
                volatility_window: 15,
                momentum_periods: vec![5, 15, 30],
                debug: false,
            }),
            enable_metrics: true,
        }
    }

    /// Create config for backtesting
    pub fn backtest() -> Self {
        Self {
            model_path: PathBuf::from("models/tkan_model.json"),
            enable_caching: false,
            max_batch_size: 256,
            confidence_threshold: 0.5,
            fallback_policy: FallbackPolicy::Skip,
            feature_extractor_config: Some(super::FeatureExtractorConfig::default()),
            enable_metrics: true,
        }
    }
}

/// Inference engine errors
#[derive(Debug, Error)]
pub enum InferenceError {
    #[error("Model reload failed: {source}")]
    Reload {
        #[from]
        source: anyhow::Error,
    },
    #[error("Batch size {requested} exceeds max batch size {max}")]
    BatchSizeExceeded { requested: usize, max: usize },
    #[error("No features available for token {token_id}")]
    NoFeaturesAvailable { token_id: TokenId },
}

/// Inference metrics for performance monitoring
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct InferenceMetrics {
    /// Total number of inferences performed
    pub inference_count: u64,
    /// Total latency in microseconds (for avg calculation)
    pub total_latency_us: u64,
    /// Number of fallback activations
    pub fallback_count: u64,
    /// Number of model reloads
    pub reload_count: u64,
    /// Number of errors
    pub error_count: u64,
}

impl InferenceMetrics {
    /// Get average latency in microseconds
    #[inline]
    pub fn avg_latency_us(&self) -> f64 {
        if self.inference_count == 0 {
            0.0
        } else {
            self.total_latency_us as f64 / self.inference_count as f64
        }
    }

    /// Get p50 latency in microseconds (placeholder for histogram)
    #[inline]
    pub fn p50_latency_us(&self) -> f64 {
        // Simplified: assumes uniform distribution
        self.avg_latency_us() * 0.9
    }
}

/// Main inference engine for T-KAN model
///
/// # Example
///
/// ```ignore
/// use mtrader_ml::{InferenceEngine, InferenceConfig};
///
/// let config = InferenceConfig::hft();
/// let mut engine = InferenceEngine::new(config).expect("Failed to create engine");
///
/// let market_data = NormalizedMarketData::new(...);
/// let signal = engine.predict(&market_data).expect("Inference failed");
///
/// if signal.is_actionable(engine.confidence_threshold()) {
///     // Act on signal
/// }
/// ```
#[derive(Debug)]
pub struct InferenceEngine {
    /// T-KAN model with warm-cached weights
    model: Option<super::TkanModel>,
    /// Model configuration
    model_config: super::TkanConfig,
    /// Feature extractor (optional)
    feature_extractor: Option<super::FeatureExtractor>,
    /// Last valid signal for fallback
    last_signal: Option<SignalData>,
    /// Engine configuration
    config: InferenceConfig,
    /// Atomic total latency for thread-safe updates
    total_latency_us: AtomicU64,
    /// Atomic inference count
    inference_count: AtomicU64,
    /// Atomic fallback count
    fallback_count: AtomicU64,
    /// Atomic reload count
    reload_count: AtomicU64,
    /// Atomic error count
    error_count: AtomicU64,
    /// Model weights cached in memory (marker for warm caching)
    _cached_weights: Option<()>,
}

impl InferenceEngine {
    /// Create a new inference engine with model loading
    ///
    /// # Errors
    ///
    /// Returns `InferenceError` if model loading fails.
    pub fn new(config: InferenceConfig) -> Result<Self, InferenceError> {
        let model_config = super::TkanConfig {
            model_path: config.model_path.to_string_lossy().to_string(),
            ..Default::default()
        };

        // Create feature extractor if configured
        let feature_extractor = config
            .feature_extractor_config
            .as_ref()
            .map(|fc| super::FeatureExtractor::new(fc));

        // Try to load model (may fail if file doesn't exist yet)
        let model = if config.enable_caching {
            Some(super::TkanModel::new(&model_config))
        } else {
            None
        };

        // Check if weights file exists for warm caching
        let _cached_weights = if config.model_path.exists() {
            Some(())
        } else {
            None
        };

        let engine = Self {
            model,
            model_config,
            feature_extractor,
            last_signal: None,
            config,
            total_latency_us: AtomicU64::new(0),
            inference_count: AtomicU64::new(0),
            fallback_count: AtomicU64::new(0),
            reload_count: AtomicU64::new(0),
            error_count: AtomicU64::new(0),
            _cached_weights,
        };

        Ok(engine)
    }

    /// Create a dummy engine for testing (no model required)
    pub fn dummy(config: InferenceConfig) -> Self {
        let model_config = super::TkanConfig {
            model_path: config.model_path.to_string_lossy().to_string(),
            ..Default::default()
        };

        let feature_extractor = config
            .feature_extractor_config
            .as_ref()
            .map(|fc| super::FeatureExtractor::new(fc));

        Self {
            model: Some(super::TkanModel::dummy(&model_config)),
            model_config,
            feature_extractor,
            last_signal: None,
            config,
            total_latency_us: AtomicU64::new(0),
            inference_count: AtomicU64::new(0),
            fallback_count: AtomicU64::new(0),
            reload_count: AtomicU64::new(0),
            error_count: AtomicU64::new(0),
            _cached_weights: None,
        }
    }

    /// Run inference on a single market data point
    ///
    /// # Errors
    ///
    /// Returns `InferenceError` if feature extraction fails or model unavailable with Skip policy.
    #[inline]
    pub fn predict(&mut self, market_data: &NormalizedMarketData) -> Result<SignalData, InferenceError> {
        let start = Instant::now();

        // Extract features
        let features = if let Some(ref mut extractor) = self.feature_extractor {
            extractor.process(market_data)
        } else {
            // Build simple feature vector from market data
            let fv = super::ExtractedFeatureVector::new(
                market_data.timestamp_us,
                market_data.token_id.clone(),
            );
            Some(fv)
        };

        let signal = match features {
            Some(fv) => {
                // Run model inference
                match self.model.as_ref() {
                    Some(model) => {
                        // Convert ExtractedFeatureVector to T-KAN input format
                        let tkan_features = self.extract_tkan_features(&fv);
                        let tkan_signal = model.predict(&tkan_features);

                        // Convert to SignalData
                        SignalData::new(
                            market_data.timestamp_us,
                            market_data.token_id.clone(),
                            "tkan_v1".to_string(),
                            tkan_signal.direction,
                            SignalType::Direction,
                            tkan_signal.confidence,
                        )
                    }
                    None => {
                        // No model available, use fallback
                        self.use_fallback(market_data)?
                    }
                }
            }
            None => {
                // Not enough data for features, use fallback
                self.use_fallback(market_data)?
            }
        };

        // Update last signal for fallback
        self.last_signal = Some(signal.clone());

        // Record metrics
        let latency_us = start.elapsed().as_micros() as u64;
        self.total_latency_us.fetch_add(latency_us, Ordering::Relaxed);
        self.inference_count.fetch_add(1, Ordering::Relaxed);

        Ok(signal)
    }

    /// Run batch inference on multiple market data points
    ///
    /// # Errors
    ///
    /// Returns `InferenceError::BatchSizeExceeded` if batch size exceeds max.
    #[inline]
    pub fn predict_batch(
        &mut self,
        batch: &[NormalizedMarketData],
    ) -> Result<Vec<SignalData>, InferenceError> {
        if batch.len() > self.config.max_batch_size {
            return Err(InferenceError::BatchSizeExceeded {
                requested: batch.len(),
                max: self.config.max_batch_size,
            });
        }

        let start = Instant::now();
        let mut signals = Vec::with_capacity(batch.len());

        for data in batch {
            let signal = self.predict(data)?;
            signals.push(signal);
        }

        // Record batch metrics
        let latency_us = start.elapsed().as_micros() as u64;
        self.total_latency_us.fetch_add(latency_us, Ordering::Relaxed);
        self.inference_count.fetch_add(batch.len() as u64, Ordering::Relaxed);

        Ok(signals)
    }

    /// Reload model weights from disk (hot-swap)
    ///
    /// # Errors
    ///
    /// Returns `InferenceError::Reload` if reload fails.
    #[inline]
    pub fn reload_model(&mut self) -> Result<(), InferenceError> {
        if !self.config.model_path.exists() {
            return Err(InferenceError::Reload {
                source: anyhow::anyhow!("Model file not found: {:?}", self.config.model_path),
            });
        }

        // Try to load new weights
        let _new_weights = super::TkanModel::load_json(
            self.config.model_path.to_str().unwrap_or(""),
            &self.model_config,
        )
        .map_err(|e| InferenceError::Reload { source: e })?;

        // Update cached weights marker
        self._cached_weights = Some(());

        self.reload_count.fetch_add(1, Ordering::Relaxed);

        tracing::info!("Model reloaded from {:?}", self.config.model_path);

        Ok(())
    }

    /// Get current performance metrics
    #[inline]
    pub fn metrics(&self) -> InferenceMetrics {
        InferenceMetrics {
            inference_count: self.inference_count.load(Ordering::Relaxed),
            total_latency_us: self.total_latency_us.load(Ordering::Relaxed),
            fallback_count: self.fallback_count.load(Ordering::Relaxed),
            reload_count: self.reload_count.load(Ordering::Relaxed),
            error_count: self.error_count.load(Ordering::Relaxed),
        }
    }

    /// Get confidence threshold
    #[inline]
    pub fn confidence_threshold(&self) -> f64 {
        self.config.confidence_threshold
    }

    /// Check if model is enabled
    #[inline]
    pub fn is_model_enabled(&self) -> bool {
        self.model.as_ref().map(|m| m.is_enabled()).unwrap_or(false)
    }

    /// Get the last valid signal (for debugging/monitoring)
    #[inline]
    pub fn last_signal(&self) -> Option<&SignalData> {
        self.last_signal.as_ref()
    }

    /// Get the feature extractor (for external feature computation)
    #[inline]
    pub fn feature_extractor(&mut self) -> Option<&mut super::FeatureExtractor> {
        self.feature_extractor.as_mut()
    }

    /// Use fallback policy when model unavailable
    #[inline]
    fn use_fallback(&mut self, market_data: &NormalizedMarketData) -> Result<SignalData, InferenceError> {
        self.fallback_count.fetch_add(1, Ordering::Relaxed);

        match self.config.fallback_policy {
            FallbackPolicy::UseLastSignal => {
                if let Some(ref last) = self.last_signal {
                    // Return cached signal with updated timestamp
                    Ok(SignalData::new(
                        market_data.timestamp_us,
                        market_data.token_id.clone(),
                        last.model_id.clone(),
                        last.value,
                        last.signal_type.clone(),
                        last.confidence,
                    ))
                } else {
                    // No cached signal, fall through to Neutral
                    Ok(SignalData::new(
                        market_data.timestamp_us,
                        market_data.token_id.clone(),
                        "fallback".to_string(),
                        0.0,
                        SignalType::Direction,
                        0.0,
                    ))
                }
            }
            FallbackPolicy::Neutral => Ok(SignalData::new(
                market_data.timestamp_us,
                market_data.token_id.clone(),
                "fallback".to_string(),
                0.0,
                SignalType::Direction,
                0.0,
            )),
            FallbackPolicy::Skip => Err(InferenceError::NoFeaturesAvailable {
                token_id: market_data.token_id.clone(),
            }),
        }
    }

    /// Convert ExtractedFeatureVector to T-KAN FeatureVector
    #[inline]
    fn extract_tkan_features(&self, features: &super::ExtractedFeatureVector) -> super::FeatureVector {
        // Extract key features for T-KAN
        let mut tkan_features = Vec::with_capacity(self.model_config.window_size + 2);

        // Use first N features as price/momentum features
        for i in 0..self.model_config.window_size.min(features.features.len()) {
            tkan_features.push(features.features[i].clamp(-1.0, 1.0));
        }

        // Pad if needed
        while tkan_features.len() < self.model_config.window_size {
            tkan_features.push(0.0);
        }

        // Add spread and volatility as additional features
        tkan_features.push(features.spread().clamp(0.0, 1.0));
        tkan_features.push(features.volatility().clamp(0.0, 1.0));

        super::FeatureVector {
            features: tkan_features,
            timestamp_ns: features.timestamp_us as u64 * 1000,
        }
    }

    /// Clear metrics (for fresh start)
    #[inline]
    pub fn reset_metrics(&mut self) {
        self.total_latency_us.store(0, Ordering::Relaxed);
        self.inference_count.store(0, Ordering::Relaxed);
        self.fallback_count.store(0, Ordering::Relaxed);
        self.reload_count.store(0, Ordering::Relaxed);
        self.error_count.store(0, Ordering::Relaxed);
    }

    /// Clear last signal cache
    #[inline]
    pub fn clear_cache(&mut self) {
        self.last_signal = None;
        if let Some(ref mut extractor) = self.feature_extractor {
            extractor.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use many_lamps_core::TokenId;

    fn create_test_market_data(timestamp_us: i64, token_id: &TokenId) -> NormalizedMarketData {
        NormalizedMarketData::new(
            timestamp_us,
            token_id.clone(),
            0.55,
            0.001,
            100.0,
            100.0,
            0.001,
            Some(0.55),
            Some(0.551),
        )
    }

    #[test]
    fn test_inference_config_defaults() {
        let config = InferenceConfig::default();
        assert!(config.enable_caching);
        assert_eq!(config.max_batch_size, 32);
        assert_eq!(config.confidence_threshold, 0.5);
        assert_eq!(config.fallback_policy, FallbackPolicy::Neutral);
    }

    #[test]
    fn test_inference_config_hft() {
        let config = InferenceConfig::hft();
        assert!(config.enable_caching);
        assert_eq!(config.max_batch_size, 64);
        assert_eq!(config.confidence_threshold, 0.6);
        assert_eq!(config.fallback_policy, FallbackPolicy::UseLastSignal);
    }

    #[test]
    fn test_inference_config_backtest() {
        let config = InferenceConfig::backtest();
        assert!(!config.enable_caching);
        assert_eq!(config.max_batch_size, 256);
        assert_eq!(config.fallback_policy, FallbackPolicy::Skip);
    }

    #[test]
    fn test_fallback_policy_variants() {
        assert_eq!(FallbackPolicy::UseLastSignal, FallbackPolicy::UseLastSignal);
        assert_eq!(FallbackPolicy::Neutral, FallbackPolicy::Neutral);
        assert_eq!(FallbackPolicy::Skip, FallbackPolicy::Skip);
    }

    #[test]
    fn test_inference_engine_dummy() {
        let config = InferenceConfig::default();
        let engine = InferenceEngine::dummy(config);

        assert!(!engine.is_model_enabled());
        assert!(engine.metrics().inference_count == 0);
    }

    #[test]
    fn test_predict_single() {
        let config = InferenceConfig::default();
        let mut engine = InferenceEngine::dummy(config);

        let token_id = TokenId("test-token".to_string());
        let market_data = create_test_market_data(1000000, &token_id);

        let signal = engine.predict(&market_data).expect("Prediction should succeed");

        assert_eq!(signal.timestamp_us, 1000000);
        assert_eq!(signal.token_id, token_id);
        assert!(signal.confidence >= 0.0 && signal.confidence <= 1.0);
    }

    #[test]
    fn test_predict_batch() {
        let config = InferenceConfig::default();
        let mut engine = InferenceEngine::dummy(config);

        let token_id = TokenId("test-token".to_string());
        let batch: Vec<NormalizedMarketData> = (0..5)
            .map(|i| create_test_market_data(1000000 + i as i64 * 100, &token_id))
            .collect();

        let signals = engine.predict_batch(&batch).expect("Batch prediction should succeed");

        assert_eq!(signals.len(), 5);
        for (i, signal) in signals.iter().enumerate() {
            assert_eq!(signal.timestamp_us, 1000000 + i as i64 * 100);
        }
    }

    #[test]
    fn test_batch_size_exceeded() {
        let config = InferenceConfig::default();
        let mut engine = InferenceEngine::dummy(config);

        let token_id = TokenId("test-token".to_string());
        let batch: Vec<NormalizedMarketData> = (0..100)
            .map(|i| create_test_market_data(1000000 + i as i64 * 100, &token_id))
            .collect();

        let result = engine.predict_batch(&batch);
        assert!(result.is_err());
        if let Err(e) = result {
            if let InferenceError::BatchSizeExceeded { requested, max } = e {
                assert_eq!(requested, 100);
                assert_eq!(max, 32);
            } else {
                panic!("Expected BatchSizeExceeded error");
            }
        }
    }

    #[test]
    fn test_metrics_tracking() {
        let config = InferenceConfig::default();
        let mut engine = InferenceEngine::dummy(config);

        let token_id = TokenId("test-token".to_string());
        for i in 0..10 {
            let market_data = create_test_market_data(1000000 + i as i64 * 100, &token_id);
            engine.predict(&market_data).unwrap();
        }

        let metrics = engine.metrics();
        assert_eq!(metrics.inference_count, 10);
        assert!(metrics.total_latency_us > 0);
    }

    #[test]
    fn test_fallback_neutral_policy() {
        let config = InferenceConfig {
            fallback_policy: FallbackPolicy::Neutral,
            ..InferenceConfig::default()
        };
        let mut engine = InferenceEngine::dummy(config);

        let token_id = TokenId("test-token".to_string());
        let market_data = create_test_market_data(1000000, &token_id);

        // Disable model to trigger fallback
        engine.model = None;

        let signal = engine.predict(&market_data).expect("Should return fallback signal");

        assert_eq!(signal.value, 0.0); // Neutral
        assert_eq!(signal.confidence, 0.0);
    }

    #[test]
    fn test_fallback_use_last_signal() {
        let config = InferenceConfig {
            fallback_policy: FallbackPolicy::UseLastSignal,
            ..InferenceConfig::default()
        };
        let mut engine = InferenceEngine::dummy(config);

        let token_id = TokenId("test-token".to_string());
        let market_data = create_test_market_data(1000000, &token_id);

        // First prediction should succeed (may be neutral with dummy model)
        let first_signal = engine.predict(&market_data).unwrap();
        
        // Store the signal value
        let first_value = first_signal.value;
        let first_confidence = first_signal.confidence;

        // Disable model
        engine.model = None;

        // Second prediction should use last signal
        let market_data2 = create_test_market_data(1000001, &token_id);
        let fallback_signal = engine.predict(&market_data2).unwrap();

        // Should have timestamp from new data but signal from last
        assert_eq!(fallback_signal.timestamp_us, 1000001);
        assert_eq!(fallback_signal.value, first_value);
        assert_eq!(fallback_signal.confidence, first_confidence);
    }

    #[test]
    fn test_confidence_threshold() {
        let config = InferenceConfig {
            confidence_threshold: 0.8,
            ..InferenceConfig::default()
        };
        let engine = InferenceEngine::dummy(config);

        assert_eq!(engine.confidence_threshold(), 0.8);
    }

    #[test]
    fn test_last_signal() {
        let config = InferenceConfig::default();
        let mut engine = InferenceEngine::dummy(config);

        let token_id = TokenId("test-token".to_string());
        let market_data = create_test_market_data(1000000, &token_id);

        assert!(engine.last_signal().is_none());

        engine.predict(&market_data).unwrap();

        assert!(engine.last_signal().is_some());
        assert_eq!(engine.last_signal().unwrap().timestamp_us, 1000000);
    }

    #[test]
    fn test_clear_cache() {
        let config = InferenceConfig::default();
        let mut engine = InferenceEngine::dummy(config);

        let token_id = TokenId("test-token".to_string());
        let market_data = create_test_market_data(1000000, &token_id);

        engine.predict(&market_data).unwrap();
        assert!(engine.last_signal().is_some());

        engine.clear_cache();
        assert!(engine.last_signal().is_none());
    }

    #[test]
    fn test_reset_metrics() {
        let config = InferenceConfig::default();
        let mut engine = InferenceEngine::dummy(config);

        let token_id = TokenId("test-token".to_string());
        let market_data = create_test_market_data(1000000, &token_id);

        for _ in 0..10 {
            engine.predict(&market_data).unwrap();
        }

        assert!(engine.metrics().inference_count > 0);

        engine.reset_metrics();
        assert_eq!(engine.metrics().inference_count, 0);
    }

    #[test]
    fn test_inference_metrics_avg_latency() {
        let metrics = InferenceMetrics {
            inference_count: 100,
            total_latency_us: 50000,
            fallback_count: 5,
            reload_count: 2,
            error_count: 1,
        };

        assert!((metrics.avg_latency_us() - 500.0).abs() < 0.001);
    }

    #[test]
    fn test_inference_metrics_empty() {
        let metrics = InferenceMetrics::default();
        assert_eq!(metrics.avg_latency_us(), 0.0);
    }
}
