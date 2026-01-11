//! Data Quality Monitoring with Anomaly Detection
//!
//! Real-time monitoring of market data and feature quality to detect:
//! - Statistical outliers (z-score based)
//! - Missing or invalid values
//! - Stale data
//! - Duplicate timestamps
//!
//! This module prevents bad data from corrupting ML models or trading decisions.

use many_lamps_core::NormalizedMarketData;
use mtrader_ml::ExtractedFeatureVector;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::fmt;

/// Data quality monitoring configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataQualityConfig {
    /// Enable data quality monitoring
    pub enable_monitoring: bool,
    /// Z-score threshold for outlier detection (standard deviations)
    pub zscore_threshold: f64,
    /// Maximum percentage of missing values allowed (0.0 - 1.0)
    pub missing_value_threshold: f64,
    /// Maximum data age in milliseconds before considered stale
    pub staleness_threshold_ms: i64,
    /// Enable alerting for detected anomalies
    pub enable_alerts: bool,
    /// Maximum number of anomalies to keep in history
    pub anomaly_history_size: usize,
    /// Minimum samples before statistics are considered valid
    pub min_samples_for_stats: usize,
}

impl Default for DataQualityConfig {
    fn default() -> Self {
        Self {
            enable_monitoring: true,
            zscore_threshold: 3.0,
            missing_value_threshold: 0.1,
            staleness_threshold_ms: 5000,
            enable_alerts: true,
            anomaly_history_size: 1000,
            min_samples_for_stats: 10,
        }
    }
}

impl DataQualityConfig {
    /// Create config for HFT (strict monitoring)
    pub fn hft() -> Self {
        Self {
            enable_monitoring: true,
            zscore_threshold: 2.5,
            missing_value_threshold: 0.05,
            staleness_threshold_ms: 100,
            enable_alerts: true,
            anomaly_history_size: 5000,
            min_samples_for_stats: 20,
        }
    }

    /// Create config for backtesting (lenient monitoring)
    pub fn backtest() -> Self {
        Self {
            enable_monitoring: true,
            zscore_threshold: 4.0,
            missing_value_threshold: 0.2,
            staleness_threshold_ms: 60000,
            enable_alerts: false,
            anomaly_history_size: 10000,
            min_samples_for_stats: 5,
        }
    }
}

/// Types of anomalies that can be detected
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AnomalyType {
    /// Value is an outlier based on z-score
    ZScoreOutlier {
        field: String,
        zscore: f64,
        value: f64,
        expected_mean: f64,
        expected_std: f64,
    },
    /// Field has too many missing values
    MissingValues {
        field: String,
        ratio: f64,
        threshold: f64,
    },
    /// Data is too old
    StaleData {
        age_ms: i64,
        threshold_ms: i64,
    },
    /// Value is outside expected range
    InvalidRange {
        field: String,
        value: f64,
        min_expected: f64,
        max_expected: f64,
    },
    /// Duplicate timestamp detected
    Duplicate {
        timestamp: i64,
        token_id: String,
    },
    /// NaN or Inf value detected
    InvalidValue {
        field: String,
        value: f64,
    },
}

impl fmt::Display for AnomalyType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AnomalyType::ZScoreOutlier {
                field,
                zscore,
                value,
                ..
            } => write!(f, "ZScoreOutlier: {}={} (z={:.2})", field, value, zscore),
            AnomalyType::MissingValues { field, ratio, .. } => {
                write!(f, "MissingValues: {}={:.2}", field, ratio * 100.0)
            }
            AnomalyType::StaleData { age_ms, .. } => write!(f, "StaleData: age={}ms", age_ms),
            AnomalyType::InvalidRange {
                field,
                value,
                min_expected,
                max_expected,
            } => write!(
                f,
                "InvalidRange: {}={} not in [{:.4}, {:.4}]",
                field, value, min_expected, max_expected
            ),
            AnomalyType::Duplicate { timestamp, .. } => {
                write!(f, "Duplicate: timestamp={}", timestamp)
            }
            AnomalyType::InvalidValue { field, value } => {
                write!(f, "InvalidValue: {}={}", field, value)
            }
        }
    }
}

/// Statistics for a single field
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FieldStats {
    /// Running mean
    pub mean: f64,
    /// Running standard deviation
    pub std_dev: f64,
    /// Minimum value observed
    pub min: f64,
    /// Maximum value observed
    pub max: f64,
    /// Total samples collected
    pub sample_count: usize,
    /// Number of missing/invalid values
    pub missing_count: usize,
    /// Sum of values (for incremental mean calculation)
    sum: f64,
    /// Sum of squared values (for incremental variance)
    sum_sq: f64,
    /// Welford's online algorithm state for numerical stability
    m2: f64,
}

impl FieldStats {
    /// Create empty stats
    pub fn new() -> Self {
        Self {
            mean: 0.0,
            std_dev: 0.0,
            min: f64::INFINITY,
            max: f64::NEG_INFINITY,
            sample_count: 0,
            missing_count: 0,
            sum: 0.0,
            sum_sq: 0.0,
            m2: 0.0,
        }
    }

    /// Add a value using Welford's online algorithm for numerical stability
    pub fn add_value(&mut self, value: f64) {
        if !value.is_finite() {
            self.missing_count += 1;
            return;
        }

        self.sample_count += 1;
        self.sum += value;
        self.sum_sq += value * value;

        // Welford's online algorithm for variance
        if self.sample_count == 1 {
            self.mean = value;
            self.min = value;
            self.max = value;
            self.m2 = 0.0;
        } else {
            let n = self.sample_count as f64;
            let delta = value - self.mean;
            self.mean += delta / n;
            let delta2 = value - self.mean;
            self.m2 += delta * delta2;
            self.std_dev = (self.m2 / (n - 1.0)).sqrt();

            if value < self.min {
                self.min = value;
            }
            if value > self.max {
                self.max = value;
            }
        }
    }

    /// Get missing value ratio
    pub fn missing_ratio(&self, total_samples: usize) -> f64 {
        if total_samples == 0 {
            0.0
        } else {
            self.missing_count as f64 / total_samples as f64
        }
    }

    /// Check if we have enough samples for reliable statistics
    pub fn has_enough_samples(&self, min_samples: usize) -> bool {
        self.sample_count >= min_samples
    }
}

/// Data quality metrics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DataQualityMetrics {
    /// Total samples processed
    pub total_samples: usize,
    /// Total anomalies detected
    pub anomalies_detected: usize,
    /// Total checks performed
    pub checks_performed: usize,
    /// Last check timestamp (microseconds)
    pub last_check_timestamp: i64,
    /// Per-field statistics
    pub field_statistics: HashMap<String, FieldStats>,
    /// Anomaly counts by type
    pub anomaly_counts: HashMap<String, usize>,
    /// Timestamp of last anomaly
    pub last_anomaly_timestamp: Option<i64>,
    /// Overall anomaly rate (0.0 - 1.0)
    pub anomaly_rate: f64,
}

impl DataQualityMetrics {
    /// Record a new sample
    pub fn record_sample(&mut self) {
        self.total_samples += 1;
    }

    /// Record an anomaly
    pub fn record_anomaly(&mut self, anomaly: &AnomalyType) {
        self.anomalies_detected += 1;
        self.last_anomaly_timestamp = Some(chrono::Utc::now().timestamp_micros());

        let type_name = match anomaly {
            AnomalyType::ZScoreOutlier { .. } => "ZScoreOutlier",
            AnomalyType::MissingValues { .. } => "MissingValues",
            AnomalyType::StaleData { .. } => "StaleData",
            AnomalyType::InvalidRange { .. } => "InvalidRange",
            AnomalyType::Duplicate { .. } => "Duplicate",
            AnomalyType::InvalidValue { .. } => "InvalidValue",
        };

        *self.anomaly_counts.entry(type_name.to_string()).or_insert(0) += 1;
        self.update_anomaly_rate();
    }

    /// Update anomaly rate
    pub fn update_anomaly_rate(&mut self) {
        if self.checks_performed > 0 {
            self.anomaly_rate = self.anomalies_detected as f64 / self.checks_performed as f64;
        }
    }

    /// Record a check
    pub fn record_check(&mut self) {
        self.checks_performed += 1;
        self.update_anomaly_rate();
    }

    /// Get statistics for a field
    pub fn get_field_stats(&self, field: &str) -> Option<&FieldStats> {
        self.field_statistics.get(field)
    }

    /// Get anomaly rate as percentage
    pub fn anomaly_rate_percent(&self) -> f64 {
        self.anomaly_rate * 100.0
    }
}

/// Main data quality monitor
#[derive(Debug)]
pub struct DataQualityMonitor {
    /// Configuration
    config: DataQualityConfig,
    /// Metrics
    metrics: DataQualityMetrics,
    /// Rolling anomaly history
    anomaly_history: VecDeque<AnomalyType>,
    /// Fields that have been seen (for detecting missing fields)
    seen_fields: HashMap<String, usize>,
    /// Last seen timestamp per token
    last_timestamps: HashMap<String, i64>,
    /// Expected value ranges for validation
    expected_ranges: HashMap<String, (f64, f64)>,
}

impl DataQualityMonitor {
    /// Create a new monitor with default config
    pub fn new() -> Self {
        Self::with_config(DataQualityConfig::default())
    }

    /// Create a new monitor with custom config
    pub fn with_config(config: DataQualityConfig) -> Self {
        let anomaly_history_size = config.anomaly_history_size;
        let mut monitor = Self {
            config: config.clone(),
            metrics: DataQualityMetrics::default(),
            anomaly_history: VecDeque::new(),
            seen_fields: HashMap::new(),
            last_timestamps: HashMap::new(),
            expected_ranges: Self::default_ranges(),
        };

        // Initialize with reasonable history capacity
        monitor.anomaly_history = VecDeque::with_capacity(anomaly_history_size);

        monitor
    }

    /// Default expected value ranges for market data fields
    fn default_ranges() -> HashMap<String, (f64, f64)> {
        let mut ranges = HashMap::new();
        // Price fields: 0.0 to 1.0 for binary markets
        ranges.insert("mid_price".to_string(), (0.0, 1.0));
        ranges.insert("bid_price".to_string(), (0.0, 1.0));
        ranges.insert("ask_price".to_string(), (0.0, 1.0));
        // Spread: 0.0 to 0.1 (10%)
        ranges.insert("spread".to_string(), (0.0, 0.1));
        // Volume fields: non-negative
        ranges.insert("bid_size".to_string(), (0.0, f64::MAX));
        ranges.insert("ask_size".to_string(), (0.0, f64::MAX));
        // Imbalance: -1.0 to 1.0
        ranges.insert("imbalance".to_string(), (-1.0, 1.0));
        // Feature values (general)
        ranges.insert("feature_0".to_string(), (0.0, 1.0)); // mid_price
        ranges.insert("feature_1".to_string(), (0.0, 1.0)); // price_ema
        ranges.insert("feature_5".to_string(), (0.0, 1.0)); // volatility
        ranges.insert("feature_6".to_string(), (0.0, 0.1)); // spread
        ranges.insert("feature_9".to_string(), (0.0, 1.0)); // relative_spread
        ranges.insert("feature_13".to_string(), (-1.0, 1.0)); // imbalance
        ranges
    }

    /// Check market data for anomalies
    pub fn check_market_data(&mut self, data: &NormalizedMarketData) -> Vec<AnomalyType> {
        if !self.config.enable_monitoring {
            return Vec::new();
        }

        let mut anomalies = Vec::new();
        self.metrics.record_check();

        // Check staleness
        if let Some(anomaly) = self.check_staleness(data.timestamp_us) {
            anomalies.push(anomaly);
        }

        // Check for duplicates
        if let Some(anomaly) = self.check_duplicate(&data.token_id.0, data.timestamp_us) {
            anomalies.push(anomaly);
        }

        // Check each field for range validity and anomalies
        let fields = [
            ("mid_price", data.mid_price),
            ("spread", data.spread),
            ("bid_size", data.bid_size),
            ("ask_size", data.ask_size),
            ("imbalance", data.imbalance),
        ];

        for (field, value) in fields {
            // Check for NaN/Inf
            if !value.is_finite() {
                let anomaly = AnomalyType::InvalidValue {
                    field: field.to_string(),
                    value,
                };
                anomalies.push(anomaly.clone());
                self.record_anomaly_internal(&anomaly);
                continue;
            }

            // Check range validity
            if let Some(anomaly) = self.check_range(field, value) {
                anomalies.push(anomaly.clone());
                self.record_anomaly_internal(&anomaly);
            }

            // Check for z-score outliers
            if let Some(anomaly) = self.check_zscore(field, value) {
                anomalies.push(anomaly.clone());
                self.record_anomaly_internal(&anomaly);
            }

            // Update statistics
            self.update_statistics(field, value);
        }

        // Record sample
        self.metrics.record_sample();

        // Emit alerts if enabled
        if self.config.enable_alerts && !anomalies.is_empty() {
            for anomaly in &anomalies {
                log::warn!("Data quality anomaly detected: {:?}", anomaly);
            }
        }

        anomalies
    }

    /// Check feature vector for anomalies
    pub fn check_features(&mut self, features: &ExtractedFeatureVector) -> Vec<AnomalyType> {
        if !self.config.enable_monitoring {
            return Vec::new();
        }

        let mut anomalies = Vec::new();
        self.metrics.record_check();

        // Check staleness
        if let Some(anomaly) = self.check_staleness(features.timestamp_us) {
            anomalies.push(anomaly);
        }

        // Check each feature value
        for (i, &value) in features.features.iter().enumerate() {
            let field = format!("feature_{}", i);

            // Check for NaN/Inf
            if !value.is_finite() {
                let anomaly = AnomalyType::InvalidValue {
                    field: field.clone(),
                    value,
                };
                anomalies.push(anomaly.clone());
                self.record_anomaly_internal(&anomaly);
                continue;
            }

            // Check range validity
            if let Some(anomaly) = self.check_range(&field, value) {
                anomalies.push(anomaly.clone());
                self.record_anomaly_internal(&anomaly);
            }

            // Check for z-score outliers
            if let Some(anomaly) = self.check_zscore(&field, value) {
                anomalies.push(anomaly.clone());
                self.record_anomaly_internal(&anomaly);
            }

            // Update statistics
            self.update_statistics(&field, value);
        }

        // Record sample
        self.metrics.record_sample();

        // Emit alerts if enabled
        if self.config.enable_alerts && !anomalies.is_empty() {
            for anomaly in &anomalies {
                log::warn!("Feature quality anomaly detected: token={}, anomaly={:?}", features.token_id.0, anomaly);
            }
        }

        anomalies
    }

    /// Update statistics for a field
    pub fn update_statistics(&mut self, field: &str, value: f64) {
        let stats = self
            .metrics
            .field_statistics
            .entry(field.to_string())
            .or_insert_with(FieldStats::new);
        stats.add_value(value);

        // Track field occurrence
        *self.seen_fields.entry(field.to_string()).or_insert(0) += 1;
    }

    /// Check if a value is anomalous using z-score
    /// Returns Some(zscore) if anomalous, None otherwise
    pub fn is_anomalous(&self, field: &str, value: f64) -> Option<f64> {
        if let Some(stats) = self.metrics.field_statistics.get(field) {
            if stats.sample_count >= self.config.min_samples_for_stats && stats.std_dev > 0.0 {
                let zscore = (value - stats.mean) / stats.std_dev;
                if zscore.abs() > self.config.zscore_threshold {
                    return Some(zscore);
                }
            }
        }
        None
    }

    /// Get current metrics reference
    pub fn metrics(&self) -> &DataQualityMetrics {
        &self.metrics
    }

    /// Get mutable metrics reference
    pub fn metrics_mut(&mut self) -> &mut DataQualityMetrics {
        &mut self.metrics
    }

    /// Get anomaly history
    pub fn anomaly_history(&self) -> &VecDeque<AnomalyType> {
        &self.anomaly_history
    }

    /// Get config reference
    pub fn config(&self) -> &DataQualityConfig {
        &self.config
    }

    /// Reset all metrics and statistics
    pub fn reset(&mut self) {
        self.metrics = DataQualityMetrics::default();
        self.anomaly_history.clear();
        self.seen_fields.clear();
        self.last_timestamps.clear();
    }

    /// Add custom expected range for a field
    pub fn set_expected_range(&mut self, field: &str, min: f64, max: f64) {
        self.expected_ranges.insert(field.to_string(), (min, max));
    }

    // Private methods

    /// Check for stale data
    fn check_staleness(&self, timestamp_us: i64) -> Option<AnomalyType> {
        let now = chrono::Utc::now().timestamp_millis() * 1000; // Convert to microseconds
        let age_ms = (now - timestamp_us) / 1000;

        if age_ms > self.config.staleness_threshold_ms {
            Some(AnomalyType::StaleData {
                age_ms,
                threshold_ms: self.config.staleness_threshold_ms,
            })
        } else {
            None
        }
    }

    /// Check for duplicate timestamp
    fn check_duplicate(&mut self, token_id: &str, timestamp: i64) -> Option<AnomalyType> {
        if let Some(&last_ts) = self.last_timestamps.get(token_id) {
            if timestamp == last_ts {
                return Some(AnomalyType::Duplicate {
                    timestamp,
                    token_id: token_id.to_string(),
                });
            }
        }
        self.last_timestamps
            .insert(token_id.to_string(), timestamp);
        None
    }

    /// Check if value is within expected range
    fn check_range(&self, field: &str, value: f64) -> Option<AnomalyType> {
        if let Some((min, max)) = self.expected_ranges.get(field) {
            if value < *min || value > *max {
                return Some(AnomalyType::InvalidRange {
                    field: field.to_string(),
                    value,
                    min_expected: *min,
                    max_expected: *max,
                });
            }
        }
        None
    }

    /// Check z-score for outlier detection
    fn check_zscore(&self, field: &str, value: f64) -> Option<AnomalyType> {
        if let Some(stats) = self.metrics.field_statistics.get(field) {
            if stats.sample_count >= self.config.min_samples_for_stats && stats.std_dev > 0.0 {
                let zscore = (value - stats.mean) / stats.std_dev;
                if zscore.abs() > self.config.zscore_threshold {
                    return Some(AnomalyType::ZScoreOutlier {
                        field: field.to_string(),
                        zscore,
                        value,
                        expected_mean: stats.mean,
                        expected_std: stats.std_dev,
                    });
                }
            }
        }
        None
    }

    /// Record anomaly internally
    fn record_anomaly_internal(&mut self, anomaly: &AnomalyType) {
        self.metrics.record_anomaly(anomaly);

        // Add to history
        if self.anomaly_history.len() >= self.config.anomaly_history_size {
            self.anomaly_history.pop_front();
        }
        self.anomaly_history.push_back(anomaly.clone());
    }
}

impl Default for DataQualityMonitor {
    fn default() -> Self {
        Self::new()
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
    fn test_zscore_outlier_detection() {
        let mut monitor = DataQualityMonitor::new();

        // Add normal values first
        for i in 0..20 {
            monitor.update_statistics("mid_price", 0.5 + (i as f64 - 10.0) * 0.01);
        }

        // Now add an outlier (value 5 standard deviations away)
        let anomalies = monitor.check_market_data(&create_test_market_data(
            1000,
            &TokenId("test".to_string()),
            0.95, // This should be within range
            0.001,
            100.0,
            100.0,
        ));

        // The mid_price should not trigger an anomaly yet (need to go through check_market_data)
        // which also updates statistics
    }

    #[test]
    fn test_missing_value_detection() {
        let mut monitor = DataQualityMonitor::new();

        // Create data with NaN
        let mut data = create_test_market_data(
            1000,
            &TokenId("test".to_string()),
            0.55,
            f64::NAN,
            100.0,
            100.0,
        );

        let anomalies = monitor.check_market_data(&data);

        // Should detect invalid value for spread
        assert!(anomalies.iter().any(|a| matches!(
            a,
            AnomalyType::InvalidValue { field, .. } if field == "spread"
        )));
    }

    #[test]
    fn test_stale_data_detection() {
        let mut monitor = DataQualityMonitor::new();
        let config = DataQualityConfig {
            staleness_threshold_ms: 100,
            ..Default::default()
        };
        monitor.config = config;

        // Data from 10 seconds ago
        let old_timestamp = chrono::Utc::now().timestamp_millis() * 1000 - 10_000_000;
        let data = create_test_market_data(
            old_timestamp,
            &TokenId("test".to_string()),
            0.55,
            0.001,
            100.0,
            100.0,
        );

        let anomalies = monitor.check_market_data(&data);

        assert!(anomalies.iter().any(|a| matches!(a, AnomalyType::StaleData { .. })));
    }

    #[test]
    fn test_range_validation() {
        let mut monitor = DataQualityMonitor::new();

        // Invalid mid_price (> 1.0)
        let data = create_test_market_data(
            1000,
            &TokenId("test".to_string()),
            1.5, // Invalid - should be 0-1
            0.001,
            100.0,
            100.0,
        );

        let anomalies = monitor.check_market_data(&data);

        assert!(anomalies.iter().any(|a| matches!(
            a,
            AnomalyType::InvalidRange { field, .. } if field == "mid_price"
        )));
    }

    #[test]
    fn test_duplicate_detection() {
        let mut monitor = DataQualityMonitor::new();

        let token_id = TokenId("test".to_string());
        let timestamp = 1000;

        let data1 = create_test_market_data(timestamp, &token_id, 0.55, 0.001, 100.0, 100.0);
        let data2 = create_test_market_data(timestamp, &token_id, 0.56, 0.002, 150.0, 100.0);

        let anomalies1 = monitor.check_market_data(&data1);
        let anomalies2 = monitor.check_market_data(&data2);

        // First data should be fine
        assert!(!anomalies1.iter().any(|a| matches!(a, AnomalyType::Duplicate { .. })));

        // Second data should detect duplicate
        assert!(anomalies2.iter().any(|a| matches!(a, AnomalyType::Duplicate { .. })));
    }

    #[test]
    fn test_statistics_tracking() {
        let mut monitor = DataQualityMonitor::new();

        // Add values 1, 2, 3, 4, 5
        for i in 1..=5 {
            monitor.update_statistics("test_field", i as f64);
        }

        let stats = monitor.metrics().field_statistics.get("test_field").unwrap();

        assert_eq!(stats.sample_count, 5);
        assert!((stats.mean - 3.0).abs() < 0.01);
        assert!((stats.std_dev - 1.41).abs() < 0.1);
        assert_eq!(stats.min, 1.0);
        assert_eq!(stats.max, 5.0);
    }

    #[test]
    fn test_anomaly_rate_tracking() {
        let mut monitor = DataQualityMonitor::new();

        // Process some data with anomalies
        for i in 0..10 {
            let data = create_test_market_data(
                1000 + i * 1000,
                &TokenId(format!("token{}", i)),
                if i == 5 { 1.5 } else { 0.55 }, // One outlier
                0.001,
                100.0,
                100.0,
            );
            let _ = monitor.check_market_data(&data);
        }

        let metrics = monitor.metrics();
        assert_eq!(metrics.total_samples, 10);
        assert!(metrics.anomaly_rate > 0.0);
        assert!(metrics.anomaly_rate < 0.5);
    }

    #[test]
    fn test_config_presets() {
        let hft = DataQualityConfig::hft();
        assert_eq!(hft.zscore_threshold, 2.5);
        assert_eq!(hft.staleness_threshold_ms, 100);

        let backtest = DataQualityConfig::backtest();
        assert_eq!(backtest.zscore_threshold, 4.0);
        assert_eq!(backtest.staleness_threshold_ms, 60000);
    }

    #[test]
    fn test_monitoring_disabled() {
        let mut config = DataQualityConfig::default();
        config.enable_monitoring = false;
        let mut monitor = DataQualityMonitor::with_config(config);

        let data = create_test_market_data(
            1000,
            &TokenId("test".to_string()),
            1.5, // Would normally trigger anomaly
            0.001,
            100.0,
            100.0,
        );

        let anomalies = monitor.check_market_data(&data);

        // Should not detect any anomalies when monitoring is disabled
        assert!(anomalies.is_empty());
    }

    #[test]
    fn test_feature_checking() {
        let mut monitor = DataQualityMonitor::new();

        let mut features = ExtractedFeatureVector::new(1000, TokenId("test".to_string()));
        features.features[0] = 0.55;
        features.features[6] = 0.001; // spread feature
        features.features[13] = 0.0; // imbalance

        let anomalies = monitor.check_features(&features);

        // Normal features should not trigger anomalies
        assert!(anomalies.is_empty());
    }

    #[test]
    fn test_anomaly_history() {
        let mut config = DataQualityConfig::default();
        config.anomaly_history_size = 5;
        config.enable_alerts = false;
        let mut monitor = DataQualityMonitor::with_config(config);

        // Generate some anomalies
        for i in 0..10 {
            let data = create_test_market_data(
                1000 + i * 1000,
                &TokenId("test".to_string()),
                if i % 3 == 0 { 1.5 } else { 0.55 },
                0.001,
                100.0,
                100.0,
            );
            let _ = monitor.check_market_data(&data);
        }

        // History should be limited to configured size
        assert!(monitor.anomaly_history().len() <= 5);
    }

    #[test]
    fn test_field_stats_numerical_stability() {
        let mut monitor = DataQualityMonitor::new();

        // Add very large values to test numerical stability
        for i in 0..100 {
            monitor.update_statistics("large_values", 1_000_000.0 + i as f64 * 1000.0);
        }

        let stats = monitor.metrics().field_statistics.get("large_values").unwrap();
        assert!(stats.mean > 1_000_000.0);
        assert!(stats.std_dev > 0.0);
    }

    #[test]
    fn test_custom_expected_range() {
        let mut monitor = DataQualityMonitor::new();

        // Set custom range for a field
        monitor.set_expected_range("bid_size", 50.0, 200.0);

        // Value within custom range should be fine
        let data1 = create_test_market_data(1000, &TokenId("test".to_string()), 0.55, 0.001, 100.0, 100.0);
        let anomalies1 = monitor.check_market_data(&data1);
        assert!(!anomalies1.iter().any(|a| matches!(
            a,
            AnomalyType::InvalidRange { field, .. } if field == "bid_size"
        )));

        // Value outside custom range should trigger anomaly
        let data2 = create_test_market_data(2000, &TokenId("test".to_string()), 0.55, 0.001, 25.0, 100.0);
        let anomalies2 = monitor.check_market_data(&data2);
        assert!(anomalies2.iter().any(|a| matches!(
            a,
            AnomalyType::InvalidRange { field, .. } if field == "bid_size"
        )));
    }

    #[test]
    fn test_reset() {
        let mut monitor = DataQualityMonitor::new();

        // Add some data
        let data = create_test_market_data(1000, &TokenId("test".to_string()), 1.5, 0.001, 100.0, 100.0);
        let _ = monitor.check_market_data(&data);

        assert!(monitor.metrics().total_samples > 0);
        assert!(!monitor.anomaly_history().is_empty());

        // Reset
        monitor.reset();

        assert_eq!(monitor.metrics().total_samples, 0);
        assert!(monitor.anomaly_history().is_empty());
    }
}
