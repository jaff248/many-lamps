//! Uncertainty Quantification for Position Sizing
//!
//! This module implements Monte Carlo dropout for epistemic uncertainty estimation,
//! enabling better position sizing via the Kelly criterion.
//!
//! # Kelly Criterion
//!
//! The Kelly criterion calculates the optimal fraction of bankroll to bet:
//! ```
//! f* = (p * b - q) / b
//! ```
//! Where:
//! - p = probability of winning
//! - q = 1 - p = probability of losing
//! - b = odds received on the bet (profit/loss ratio)
//!
//! For binary outcomes with equal odds (b=1):
//! ```
//! f* = 2p - 1
//! ```
//!
//! We apply uncertainty-aware adjustments:
//! - Higher uncertainty → smaller position (safety multiplier)
//! - Confidence threshold filters low-conviction signals
//! - Quarter-Kelly (0.25x) provides conservative sizing

use rand::Rng;
use serde::{Deserialize, Serialize};

/// Configuration for uncertainty quantification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UncertaintyConfig {
    /// Number of Monte Carlo samples (default: 30)
    pub num_samples: usize,
    /// Dropout probability during inference (default: 0.1)
    pub dropout_rate: f64,
    /// Minimum confidence threshold for actionable signals (default: 0.5)
    pub min_confidence: f64,
    /// Kelly fraction multiplier (default: 0.25 for quarter-Kelly)
    pub kelly_fraction: f64,
    /// Confidence level for intervals (default: 0.95 for 95% CI)
    pub confidence_level: f64,
}

impl Default for UncertaintyConfig {
    fn default() -> Self {
        Self {
            num_samples: 30,
            dropout_rate: 0.1,
            min_confidence: 0.5,
            kelly_fraction: 0.25,
            confidence_level: 0.95,
        }
    }
}

impl UncertaintyConfig {
    /// Validate configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.num_samples == 0 {
            return Err("num_samples must be greater than 0".to_string());
        }
        if self.dropout_rate < 0.0 || self.dropout_rate >= 1.0 {
            return Err("dropout_rate must be in [0, 1)".to_string());
        }
        if self.min_confidence < 0.0 || self.min_confidence > 1.0 {
            return Err("min_confidence must be in [0, 1]".to_string());
        }
        if self.kelly_fraction <= 0.0 || self.kelly_fraction > 1.0 {
            return Err("kelly_fraction must be in (0, 1]".to_string());
        }
        if self.confidence_level <= 0.0 || self.confidence_level >= 1.0 {
            return Err("confidence_level must be in (0, 1)".to_string());
        }
        Ok(())
    }
}

/// Result of uncertainty estimation
#[derive(Debug, Clone)]
pub struct UncertaintyResult {
    /// Mean prediction across MC samples
    pub mean_prediction: f64,
    /// Standard deviation (epistemic uncertainty)
    pub std_deviation: f64,
    /// Lower bound of confidence interval
    pub ci_lower: f64,
    /// Upper bound of confidence interval
    pub ci_upper: f64,
    /// Raw Monte Carlo samples
    pub samples: Vec<f64>,
    /// Coefficient of variation (normalized uncertainty)
    pub coefficient_of_variation: f64,
    /// Effective sample size for confidence
    pub effective_confidence: f64,
}

impl UncertaintyResult {
    /// Create a neutral result with zero uncertainty
    pub fn neutral() -> Self {
        Self {
            mean_prediction: 0.0,
            std_deviation: 0.0,
            ci_lower: 0.0,
            ci_upper: 0.0,
            samples: Vec::new(),
            coefficient_of_variation: 0.0,
            effective_confidence: 0.0,
        }
    }

    /// Check if uncertainty is within acceptable bounds
    pub fn is_uncertain(&self, threshold: f64) -> bool {
        self.coefficient_of_variation > threshold
    }

    /// Get the uncertainty-adjusted confidence (1 - normalized std)
    pub fn adjusted_confidence(&self) -> f64 {
        if self.mean_prediction.abs() < 1e-10 {
            0.0
        } else {
            (self.coefficient_of_variation + 1.0).recip()
        }
    }
}

/// Monte Carlo Dropout for epistemic uncertainty estimation
///
/// Applies random dropout masks during inference and aggregates predictions
/// to estimate model uncertainty.
#[derive(Debug, Clone)]
pub struct MonteCarloDropout {
    dropout_rate: f64,
    feature_dim: usize,
}

impl MonteCarloDropout {
    /// Create a new Monte Carlo dropout wrapper
    pub fn new(dropout_rate: f64, feature_dim: usize) -> Self {
        Self {
            dropout_rate,
            feature_dim,
        }
    }

    /// Generate a dropout mask for the given feature dimension
    ///
    /// Each element is retained with probability (1 - dropout_rate)
    /// and zeroed with probability dropout_rate.
    pub fn generate_mask(&self) -> Vec<bool> {
        let mut rng = rand::thread_rng();
        (0..self.feature_dim)
            .map(|_| rng.gen_bool(1.0 - self.dropout_rate))
            .collect()
    }

    /// Apply dropout mask to features
    ///
    /// Returns a new feature vector with masked elements set to zero.
    pub fn apply_mask(&self, features: &[f64], mask: &[bool]) -> Vec<f64> {
        features
            .iter()
            .zip(mask.iter())
            .map(|(&f, &m)| if m { f } else { 0.0 })
            .collect()
    }

    /// Run multiple forward passes with different dropout masks
    ///
    /// Returns a vector of predictions from each forward pass.
    pub fn monte_carlo_samples<F>(&self, features: &[f64], model: &F, num_samples: usize) -> Vec<f64>
    where
        F: Fn(&[f64]) -> f64,
    {
        let mut samples = Vec::with_capacity(num_samples);

        for _ in 0..num_samples {
            let mask = self.generate_mask();
            let masked_features = self.apply_mask(features, &mask);
            let prediction = model(&masked_features);
            samples.push(prediction);
        }

        samples
    }
}

/// Main uncertainty estimator combining MC dropout with Kelly criterion
#[derive(Debug, Clone)]
pub struct UncertaintyEstimator {
    config: UncertaintyConfig,
    dropout: MonteCarloDropout,
}

impl UncertaintyEstimator {
    /// Create a new uncertainty estimator
    pub fn new(config: UncertaintyConfig) -> Self {
        config.validate().expect("Invalid uncertainty config");
        Self {
            config: config.clone(),
            dropout: MonteCarloDropout::new(config.dropout_rate, 0), // feature_dim set per-call
        }
    }

    /// Estimate uncertainty for a given model and features
    ///
    /// Runs multiple forward passes with Monte Carlo dropout and aggregates
    /// the predictions to compute mean, standard deviation, and confidence intervals.
    pub fn estimate<F>(&mut self, features: &[f64], model: &F) -> UncertaintyResult
    where
        F: Fn(&[f64]) -> f64,
    {
        // Initialize dropout with correct feature dimension
        self.dropout = MonteCarloDropout::new(self.config.dropout_rate, features.len());

        // Run Monte Carlo sampling
        let samples = self.dropout.monte_carlo_samples(features, model, self.config.num_samples);

        // Compute statistics
        let n = samples.len() as f64;
        let mean: f64 = samples.iter().sum::<f64>() / n;

        // Standard deviation (population std)
        let variance: f64 = samples.iter().map(|s| (s - mean).powi(2)).sum::<f64>() / n;
        let std_dev = variance.sqrt();

        // Confidence interval (using normal approximation for large n)
        let ci = Self::confidence_interval(&samples, mean, std_dev, self.config.confidence_level);

        // Coefficient of variation (normalized uncertainty)
        let cv = if mean.abs() > 1e-10 {
            std_dev / mean.abs()
        } else {
            0.0
        };

        // Effective confidence based on uncertainty
        let effective_confidence = Self::effective_confidence(mean, std_dev);

        UncertaintyResult {
            mean_prediction: mean,
            std_deviation: std_dev,
            ci_lower: ci.0,
            ci_upper: ci.1,
            samples,
            coefficient_of_variation: cv,
            effective_confidence,
        }
    }

    /// Compute confidence interval using normal approximation
    fn confidence_interval(
        samples: &[f64],
        mean: f64,
        std_dev: f64,
        confidence_level: f64,
    ) -> (f64, f64) {
        if samples.is_empty() || std_dev == 0.0 {
            return (mean, mean);
        }

        // Z-score for confidence level (approximation)
        // Using the inverse CDF of standard normal
        let z = Self::z_score_for_confidence(confidence_level);
        let margin = z * std_dev / (samples.len() as f64).sqrt();

        (mean - margin, mean + margin)
    }

    /// Approximate z-score for confidence level
    fn z_score_for_confidence(level: f64) -> f64 {
        // Common confidence levels
        match level {
            l if (l - 0.90).abs() < 0.01 => 1.645,
            l if (l - 0.95).abs() < 0.01 => 1.96,
            l if (l - 0.99).abs() < 0.01 => 2.576,
            _ => {
                // For other levels, use approximation
                // This is a simplified inverse error function approximation
                let alpha = 1.0 - level;
                (-2.0 * alpha.ln()).sqrt()
            }
        }
    }

    /// Compute effective confidence from mean and std deviation
    fn effective_confidence(mean: f64, std_dev: f64) -> f64 {
        // Higher mean and lower std → higher confidence
        let signal_strength = mean.abs().clamp(0.0, 1.0);
        let uncertainty_penalty = (std_dev + 1.0).recip();

        signal_strength * uncertainty_penalty
    }

    /// Convert uncertainty to position sizing multiplier
    ///
    /// Higher uncertainty → smaller position size
    /// Uses exponential decay based on coefficient of variation.
    pub fn uncertainty_multiplier(&self, result: &UncertaintyResult) -> f64 {
        // Exponential decay: multiplier = exp(-k * CV)
        let k = 2.0; // decay constant
        let multiplier = (-k * result.coefficient_of_variation).exp();

        // Clamp to reasonable bounds [0, 1]
        multiplier.clamp(0.0, 1.0)
    }

    /// Risk-adjusted signal strength
    ///
    /// Combines signal magnitude with uncertainty to produce a risk-weighted score.
    pub fn risk_adjusted_signal(&self, result: &UncertaintyResult) -> f64 {
        // Signal strength weighted by uncertainty multiplier
        result.mean_prediction * self.uncertainty_multiplier(result)
    }

    /// Kelly criterion position sizing
    ///
    /// Calculates the optimal position fraction using Kelly criterion,
    /// adjusted for uncertainty and limited by kelly_fraction.
    ///
    /// Kelly formula: f* = (bp - q) / b
    /// Where p = win probability, q = 1 - p, b = odds
    ///
    /// For binary prediction (direction ∈ [-1, 1]):
    /// - Convert direction to implied probability
    /// - Apply uncertainty adjustment
    /// - Limit to kelly_fraction for safety
    pub fn kelly_position(&self, signal: f64, uncertainty: &UncertaintyResult, bankroll: f64) -> f64 {
        // Convert signal direction to probability estimate
        // Signal ∈ [-1, 1] → probability ∈ [0, 1]
        let p = (signal + 1.0) / 2.0;

        // Kelly fraction without odds (b=1 for simplicity)
        // f* = 2p - 1 for binary outcome with equal odds
        let kelly_raw = 2.0 * p - 1.0;

        // Adjust for uncertainty (safety multiplier)
        let uncertainty_multiplier = self.uncertainty_multiplier(uncertainty);

        // Apply Kelly fraction limit (e.g., quarter-Kelly = 0.25)
        let adjusted_kelly = kelly_raw.abs() * self.config.kelly_fraction * uncertainty_multiplier;

        // Position size = adjusted_kelly * bankroll
        // Only take the trade if direction matches signal
        let direction = signal.signum();
        if direction > 0.0 {
            adjusted_kelly * bankroll
        } else if direction < 0.0 {
            -adjusted_kelly * bankroll
        } else {
            0.0
        }
    }

    /// Calculate win probability from prediction mean
    ///
    /// Interprets the mean prediction as a probability of favorable outcome.
    pub fn win_probability(&self, result: &UncertaintyResult) -> f64 {
        // Map mean prediction to probability space
        // Assuming prediction is centered at 0 with some spread
        let mean = result.mean_prediction;
        let std = result.std_deviation.max(0.1); // Prevent division by zero

        // Use sigmoid-like transformation for probability
        // P(win) = 1 / (1 + exp(-mean/std))
        1.0 / (1.0 + (-mean / std).exp())
    }

    /// Filter signal by confidence threshold
    ///
    /// Returns true if the signal meets the minimum confidence requirement.
    pub fn passes_confidence_filter(&self, result: &UncertaintyResult) -> bool {
        result.effective_confidence >= self.config.min_confidence
    }

    /// Get the configuration
    pub fn config(&self) -> &UncertaintyConfig {
        &self.config
    }
}

/// Position sizing result combining signal, uncertainty, and Kelly calculation
#[derive(Debug, Clone)]
pub struct PositionSizingResult {
    /// Raw Kelly position (before risk adjustments)
    pub raw_kelly: f64,
    /// Uncertainty-adjusted position
    pub adjusted_position: f64,
    /// Confidence multiplier applied
    pub confidence_multiplier: f64,
    /// Whether the signal passed confidence filter
    pub is_actionable: bool,
    /// The uncertainty result used
    pub uncertainty: UncertaintyResult,
}

/// Extended position sizing with full details
impl PositionSizingResult {
    /// Create a neutral result (no position)
    pub fn neutral(uncertainty: UncertaintyResult) -> Self {
        Self {
            raw_kelly: 0.0,
            adjusted_position: 0.0,
            confidence_multiplier: 0.0,
            is_actionable: false,
            uncertainty,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Simple linear model for testing
    fn linear_model(weights: &[f64]) -> impl Fn(&[f64]) -> f64 {
        let weights = weights.to_vec();
        move |inputs: &[f64]| {
            inputs
                .iter()
                .zip(weights.iter())
                .map(|(i, w)| i * w)
                .sum()
        }
    }

    #[test]
    fn test_uncertainty_config_default() {
        let config = UncertaintyConfig::default();
        assert_eq!(config.num_samples, 30);
        assert_eq!(config.dropout_rate, 0.1);
        assert_eq!(config.min_confidence, 0.5);
        assert_eq!(config.kelly_fraction, 0.25);
        assert_eq!(config.confidence_level, 0.95);
    }

    #[test]
    fn test_uncertainty_config_validation() {
        let mut config = UncertaintyConfig::default();
        assert!(config.validate().is_ok());

        config.num_samples = 0;
        assert!(config.validate().is_err());

        config = UncertaintyConfig::default();
        config.dropout_rate = 1.5;
        assert!(config.validate().is_err());

        config = UncertaintyConfig::default();
        config.kelly_fraction = 0.0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_monte_carlo_dropout_mask() {
        let dropout = MonteCarloDropout::new(0.5, 10);
        let mask = dropout.generate_mask();

        // Mask should have ~50% true values (retained)
        let retained_count: usize = mask.iter().filter(|&&m| m).count();
        assert!(retained_count >= 2 && retained_count <= 8); // Reasonable range for 10 elements
    }

    #[test]
    fn test_monte_carlo_dropout_apply() {
        let dropout = MonteCarloDropout::new(0.5, 4);
        let features = vec![1.0, 2.0, 3.0, 4.0];
        let mask = vec![true, false, true, false];

        let masked = dropout.apply_mask(&features, &mask);
        assert_eq!(masked, vec![1.0, 0.0, 3.0, 0.0]);
    }

    #[test]
    fn test_monte_carlo_samples() {
        let dropout = MonteCarloDropout::new(0.2, 5);
        let weights = vec![0.5; 5];
        let model = linear_model(&weights);
        let features = vec![1.0; 5];

        let samples = dropout.monte_carlo_samples(&features, &model, 10);

        assert_eq!(samples.len(), 10);
        // Expected value: 1.0 * 5 * 0.5 = 2.5 (on average, with 80% retention)
        let mean: f64 = samples.iter().sum::<f64>() / samples.len() as f64;
        assert!((mean - 2.5).abs() < 1.0); // Allow variance due to dropout
    }

    #[test]
    fn test_uncertainty_estimation() {
        let config = UncertaintyConfig {
            num_samples: 100,
            dropout_rate: 0.1,
            min_confidence: 0.5,
            kelly_fraction: 0.25,
            confidence_level: 0.95,
        };

        let mut estimator = UncertaintyEstimator::new(config);
        let weights = vec![1.0; 10];
        let model = linear_model(&weights);
        let features = vec![1.0; 10];

        let result = estimator.estimate(&features, &model);

        assert!(result.samples.len() == 100);
        assert!(result.mean_prediction > 0.0);
        assert!(result.std_deviation > 0.0); // Should have some variance
        assert!(result.ci_lower <= result.mean_prediction);
        assert!(result.ci_upper >= result.mean_prediction);
    }

    #[test]
    fn test_uncertainty_result_neutral() {
        let result = UncertaintyResult::neutral();
        assert_eq!(result.mean_prediction, 0.0);
        assert_eq!(result.std_deviation, 0.0);
        assert!(result.samples.is_empty());
    }

    #[test]
    fn test_uncertainty_multiplier() {
        let config = UncertaintyConfig::default();
        let estimator = UncertaintyEstimator::new(config);

        // Low uncertainty → high multiplier
        let low_uncertainty = UncertaintyResult {
            mean_prediction: 0.8,
            std_deviation: 0.1,
            ci_lower: 0.7,
            ci_upper: 0.9,
            samples: Vec::new(),
            coefficient_of_variation: 0.125,
            effective_confidence: 0.8,
        };
        let multiplier = estimator.uncertainty_multiplier(&low_uncertainty);
        assert!(multiplier > 0.5);

        // High uncertainty → low multiplier
        let high_uncertainty = UncertaintyResult {
            mean_prediction: 0.5,
            std_deviation: 0.5,
            ci_lower: 0.0,
            ci_upper: 1.0,
            samples: Vec::new(),
            coefficient_of_variation: 1.0,
            effective_confidence: 0.3,
        };
        let multiplier = estimator.uncertainty_multiplier(&high_uncertainty);
        assert!(multiplier < 0.5);
    }

    #[test]
    fn test_kelly_position_sizing() {
        let config = UncertaintyConfig {
            num_samples: 30,
            dropout_rate: 0.1,
            min_confidence: 0.5,
            kelly_fraction: 0.25,
            confidence_level: 0.95,
        };

        let estimator = UncertaintyEstimator::new(config);

        let uncertainty = UncertaintyResult {
            mean_prediction: 0.7,
            std_deviation: 0.1,
            ci_lower: 0.6,
            ci_upper: 0.8,
            samples: Vec::new(),
            coefficient_of_variation: 0.14,
            effective_confidence: 0.7,
        };

        // Positive signal
        let position = estimator.kelly_position(0.8, &uncertainty, 1000.0);
        assert!(position > 0.0);
        assert!(position < 250.0); // Kelly limited to 25% of bankroll max

        // Negative signal
        let position = estimator.kelly_position(-0.8, &uncertainty, 1000.0);
        assert!(position < 0.0);

        // Neutral signal
        let position = estimator.kelly_position(0.0, &uncertainty, 1000.0);
        assert_eq!(position, 0.0);
    }

    #[test]
    fn test_kelly_formula_properties() {
        // Verify Kelly formula: f* = 2p - 1
        // p = 0.5 → f* = 0 (no edge)
        // p = 0.6 → f* = 0.2
        // p = 0.75 → f* = 0.5
        // p = 0.9 → f* = 0.8

        // Use zero uncertainty to test pure Kelly
        let config = UncertaintyConfig {
            num_samples: 30,
            dropout_rate: 0.0, // No dropout = no variance
            min_confidence: 0.5,
            kelly_fraction: 1.0, // Full Kelly for testing
            confidence_level: 0.95,
        };

        let estimator = UncertaintyEstimator::new(config);
        let zero_uncertainty = UncertaintyResult {
            mean_prediction: 1.0, // Maximum confidence
            std_deviation: 0.0,
            ci_lower: 1.0,
            ci_upper: 1.0,
            samples: Vec::new(),
            coefficient_of_variation: 0.0,
            effective_confidence: 1.0,
        };

        let pos_60 = estimator.kelly_position(0.2, &zero_uncertainty, 1000.0); // p = 0.6
        let pos_75 = estimator.kelly_position(0.5, &zero_uncertainty, 1000.0); // p = 0.75
        let pos_90 = estimator.kelly_position(0.8, &zero_uncertainty, 1000.0); // p = 0.9

        assert!((pos_60 - 200.0).abs() < 1.0); // 0.2 * 1000
        assert!((pos_75 - 500.0).abs() < 1.0); // 0.5 * 1000
        assert!((pos_90 - 800.0).abs() < 1.0); // 0.8 * 1000
    }

    #[test]
    fn test_confidence_interval() {
        let _config = UncertaintyConfig::default();

        let config_95 = UncertaintyConfig {
            num_samples: 100,
            dropout_rate: 0.1,
            min_confidence: 0.5,
            kelly_fraction: 0.25,
            confidence_level: 0.95,
        };
        let mut estimator_95 = UncertaintyEstimator::new(config_95);

        let config_90 = UncertaintyConfig {
            num_samples: 100,
            dropout_rate: 0.1,
            min_confidence: 0.5,
            kelly_fraction: 0.25,
            confidence_level: 0.90,
        };
        let mut estimator_90 = UncertaintyEstimator::new(config_90);

        let weights = vec![1.0; 10];
        let model = linear_model(&weights);
        let features = vec![1.0; 10];

        let result_95 = estimator_95.estimate(&features, &model);
        let result_90 = estimator_90.estimate(&features, &model);

        let width_95 = result_95.ci_upper - result_95.ci_lower;
        let width_90 = result_90.ci_upper - result_90.ci_lower;

        assert!(width_95 > width_90);
    }

    #[test]
    fn test_risk_adjusted_signal() {
        let config = UncertaintyConfig::default();
        let estimator = UncertaintyEstimator::new(config);

        let low_uncertainty = UncertaintyResult {
            mean_prediction: 0.8,
            std_deviation: 0.1,
            ci_lower: 0.7,
            ci_upper: 0.9,
            samples: Vec::new(),
            coefficient_of_variation: 0.125,
            effective_confidence: 0.8,
        };

        let high_uncertainty = UncertaintyResult {
            mean_prediction: 0.8,
            std_deviation: 0.8,
            ci_lower: 0.0,
            ci_upper: 1.6,
            samples: Vec::new(),
            coefficient_of_variation: 1.0,
            effective_confidence: 0.4,
        };

        let risk_low = estimator.risk_adjusted_signal(&low_uncertainty);
        let risk_high = estimator.risk_adjusted_signal(&high_uncertainty);

        // Lower uncertainty should give higher risk-adjusted signal
        assert!(risk_low > risk_high);
        // Both should be less than raw mean due to uncertainty penalty
        assert!(risk_low < 0.8);
        assert!(risk_high < 0.8);
    }

    #[test]
    fn test_confidence_filter() {
        let high_threshold = UncertaintyConfig {
            min_confidence: 0.8,
            ..UncertaintyConfig::default()
        };
        let estimator_high = UncertaintyEstimator::new(high_threshold);

        // Use effective_confidence of 0.1 for low_conf
        let low_threshold = UncertaintyConfig {
            min_confidence: 0.05,
            ..UncertaintyConfig::default()
        };
        let estimator_low = UncertaintyEstimator::new(low_threshold);

        let high_conf = UncertaintyResult {
            mean_prediction: 0.9,
            std_deviation: 0.1,
            ci_lower: 0.8,
            ci_upper: 1.0,
            samples: Vec::new(),
            coefficient_of_variation: 0.11,
            effective_confidence: 0.9,
        };

        // effective_confidence = 0.1 * (1.0 / 1.1) ≈ 0.09
        let low_conf = UncertaintyResult {
            mean_prediction: 0.1,
            std_deviation: 0.1,
            ci_lower: 0.0,
            ci_upper: 0.2,
            samples: Vec::new(),
            coefficient_of_variation: 1.0,
            effective_confidence: 0.09,
        };

        assert!(estimator_high.passes_confidence_filter(&high_conf));
        assert!(!estimator_high.passes_confidence_filter(&low_conf));
        assert!(estimator_low.passes_confidence_filter(&low_conf));
    }

    #[test]
    fn test_high_uncertainty_edge_case() {
        // Test that high uncertainty leads to small positions
        let config = UncertaintyConfig {
            num_samples: 30,
            dropout_rate: 0.1,
            min_confidence: 0.5,
            kelly_fraction: 0.25,
            confidence_level: 0.95,
        };

        let estimator = UncertaintyEstimator::new(config);

        // Very high uncertainty (coefficient of variation = 10)
        let high_uncertainty = UncertaintyResult {
            mean_prediction: 0.5,
            std_deviation: 5.0,
            ci_lower: -4.5,
            ci_upper: 5.5,
            samples: Vec::new(),
            coefficient_of_variation: 10.0,
            effective_confidence: 0.05,
        };

        let position = estimator.kelly_position(0.8, &high_uncertainty, 1000.0);

        // Should be very small due to high uncertainty
        assert!(position < 50.0); // Less than 5% of bankroll
    }

    #[test]
    fn test_zero_variance() {
        // Test handling of zero variance
        let config = UncertaintyConfig::default();
        let estimator = UncertaintyEstimator::new(config);

        let zero_variance = UncertaintyResult {
            mean_prediction: 0.5,
            std_deviation: 0.0,
            ci_lower: 0.5,
            ci_upper: 0.5,
            samples: vec![0.5, 0.5, 0.5],
            coefficient_of_variation: 0.0,
            effective_confidence: 1.0,
        };

        let multiplier = estimator.uncertainty_multiplier(&zero_variance);
        assert_eq!(multiplier, 1.0); // No uncertainty penalty

        let position = estimator.kelly_position(0.5, &zero_variance, 1000.0);
        assert!(position > 0.0);
    }

    #[test]
    fn test_z_score_for_confidence() {
        assert!((UncertaintyEstimator::z_score_for_confidence(0.90) - 1.645).abs() < 0.1);
        assert!((UncertaintyEstimator::z_score_for_confidence(0.95) - 1.96).abs() < 0.1);
        assert!((UncertaintyEstimator::z_score_for_confidence(0.99) - 2.576).abs() < 0.1);
    }

    #[test]
    fn test_effective_confidence() {
        // High mean, low std → high confidence
        let high_conf = UncertaintyEstimator::effective_confidence(0.9, 0.1);
        // Low mean, high std → low confidence
        let low_conf = UncertaintyEstimator::effective_confidence(0.1, 0.5);

        assert!(high_conf > low_conf);
        assert!(high_conf > 0.5);
        assert!(low_conf < 0.5);
    }

    #[test]
    fn test_position_sizing_result_neutral() {
        let uncertainty = UncertaintyResult::neutral();
        let result = PositionSizingResult::neutral(uncertainty.clone());

        assert_eq!(result.raw_kelly, 0.0);
        assert_eq!(result.adjusted_position, 0.0);
        assert_eq!(result.confidence_multiplier, 0.0);
        assert!(!result.is_actionable);
        assert_eq!(result.uncertainty.mean_prediction, 0.0);
    }

    #[test]
    fn test_adjusted_confidence() {
        let result = UncertaintyResult {
            mean_prediction: 0.5,
            std_deviation: 0.25,
            ci_lower: 0.25,
            ci_upper: 0.75,
            samples: Vec::new(),
            coefficient_of_variation: 0.5,
            effective_confidence: 0.5,
        };

        let conf = result.adjusted_confidence();
        assert!((conf - 2.0 / 3.0).abs() < 0.01); // 1 / (0.5 + 1) = 2/3
    }

    #[test]
    fn test_is_uncertain() {
        let result = UncertaintyResult {
            mean_prediction: 0.5,
            std_deviation: 0.5,
            ci_lower: 0.0,
            ci_upper: 1.0,
            samples: Vec::new(),
            coefficient_of_variation: 1.0,
            effective_confidence: 0.3,
        };

        assert!(result.is_uncertain(0.5));
        assert!(!result.is_uncertain(1.5));
    }

    #[test]
    fn test_dropout_rate_effect() {
        // Higher dropout should produce higher variance
        let mut low_dropout = UncertaintyEstimator::new(UncertaintyConfig {
            dropout_rate: 0.05,
            num_samples: 100,
            ..UncertaintyConfig::default()
        });

        let mut high_dropout = UncertaintyEstimator::new(UncertaintyConfig {
            dropout_rate: 0.3,
            num_samples: 100,
            ..UncertaintyConfig::default()
        });

        let weights = vec![1.0; 20];
        let model = linear_model(&weights);
        let features = vec![1.0; 20];

        let result_low = low_dropout.estimate(&features, &model);
        let result_high = high_dropout.estimate(&features, &model);

        // High dropout should have higher variance
        assert!(result_high.std_deviation > result_low.std_deviation);
    }

    #[test]
    fn test_sample_count_effect() {
        // More samples should give more stable estimates
        let mut few_samples = UncertaintyEstimator::new(UncertaintyConfig {
            num_samples: 10,
            dropout_rate: 0.1,
            ..UncertaintyConfig::default()
        });

        let mut many_samples = UncertaintyEstimator::new(UncertaintyConfig {
            num_samples: 100,
            dropout_rate: 0.1,
            ..UncertaintyConfig::default()
        });

        let weights = vec![1.0; 10];
        let model = linear_model(&weights);
        let features = vec![1.0; 10];

        let result_few = few_samples.estimate(&features, &model);
        let result_many = many_samples.estimate(&features, &model);

        // More samples should give more precise CI (narrower)
        let width_few = result_few.ci_upper - result_few.ci_lower;
        let width_many = result_many.ci_upper - result_many.ci_lower;

        assert!(width_many < width_few);
    }
}
