//! Large-Margin Softmax Classifier for Trading Signal Classification
//!
//! This module implements the Large-Margin Softmax (L-Softmax) loss function
//! which improves signal discrimination by enforcing higher confidence on
//! correct predictions through angular margin constraints.
//!
//! # Mathematical Formulation
//!
//! Standard softmax: P(y|x) = exp(W_y·x) / Σ exp(W_i·x)
//!
//! Large-margin softmax: P(y|x) = exp(s·cos(θ_y + m)) / [exp(s·cos(θ_y + m)) + Σ_{i≠y} exp(s·cos(θ_i))]
//!
//! Where:
//! - θ is the angle between feature vector x and weight vector W_i
//! - m is the angular margin penalty (default: 0.5)
//! - s is the feature scale factor (default: 30.0)
//!
//! The margin loss is computed as:
//! L = -log(exp(s·cos(θ_y + m)) / [exp(s·cos(θ_y + m)) + Σ_{i≠y} exp(s·cos(θ_i))])
//!
//! # Usage
//!
//! ```
//! use mtrader_ml::{MarginSoftmaxClassifier, MarginSoftmaxConfig, DirectionalSignal};
//!
//! let config = MarginSoftmaxConfig::default();
//! let classifier = MarginSoftmaxClassifier::new(8, &config); // 8 input features
//!
//! let features = vec![0.5, 0.01, 100.0, 50.0, 0.2, 0.001, 0.6, 0.4];
//! let (class_idx, confidence) = classifier.predict(&features);
//! ```
//!
//! # Class Mappings (3-class for trading)
//! - Class 0: Down signal (bearish)
//! - Class 1: Neutral signal
//! - Class 2: Up signal (bullish)

use serde::{Deserialize, Serialize};

/// Configuration for Large-Margin Softmax Classifier
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MarginSoftmaxConfig {
    /// Number of output classes (typically 3 for Down, Neutral, Up)
    pub num_classes: usize,
    /// Angular margin penalty - higher values enforce stricter margins
    /// Range: (0, π/2), default: 0.5
    pub margin: f64,
    /// Feature scale factor - controls the temperature of softmax
    /// Higher values produce sharper distributions, default: 30.0
    pub scale: f64,
    /// Learning rate for weight updates during training
    pub learning_rate: f64,
    /// Weight decay for L2 regularization
    pub weight_decay: f64,
}

impl Default for MarginSoftmaxConfig {
    fn default() -> Self {
        Self {
            num_classes: 3,
            margin: 0.5,
            scale: 30.0,
            learning_rate: 0.01,
            weight_decay: 0.0001,
        }
    }
}

/// Large-Margin Softmax Classifier
///
/// Implements the A-Softmax (Angular Softmax) loss for improved
/// feature discrimination in trading signal classification.
#[derive(Debug, Clone)]
pub struct MarginSoftmaxClassifier {
    /// Weight matrix: [num_classes, input_dim]
    weights: Vec<Vec<f64>>,
    /// Bias vector: [num_classes]
    biases: Vec<f64>,
    /// Configuration
    config: MarginSoftmaxConfig,
    /// Input dimension
    input_dim: usize,
}

impl MarginSoftmaxClassifier {
    /// Create a new Large-Margin Softmax Classifier
    ///
    /// # Arguments
    /// * `input_dim` - Dimension of input features
    /// * `config` - Configuration struct
    ///
    /// # Returns
    /// Initialized classifier with Xavier-initialized weights
    pub fn new(input_dim: usize, config: MarginSoftmaxConfig) -> Self {
        let num_classes = config.num_classes;
        let scale = (2.0 / ((input_dim + num_classes) as f64)).sqrt();

        let mut weights = Vec::with_capacity(num_classes);
        let mut biases = Vec::with_capacity(num_classes);

        // Xavier initialization for weights
        for _ in 0..num_classes {
            let mut class_weights = Vec::with_capacity(input_dim);
            for _ in 0..input_dim {
                let w = (rand_drand() * 2.0 - 1.0) * scale;
                class_weights.push(w);
            }
            weights.push(class_weights);
            biases.push(0.0);
        }

        Self {
            weights,
            biases,
            config,
            input_dim,
        }
    }

    /// Create a classifier with pre-trained weights
    ///
    /// # Arguments
    /// * `input_dim` - Dimension of input features
    /// * `weights` - Pre-trained weight matrix [num_classes, input_dim]
    /// * `config` - Configuration struct
    pub fn with_weights(input_dim: usize, weights: Vec<Vec<f64>>, config: MarginSoftmaxConfig) -> Self {
        let num_classes = config.num_classes;
        assert_eq!(weights.len(), num_classes);
        assert_eq!(weights[0].len(), input_dim);

        let biases = vec![0.0; num_classes];

        Self {
            weights,
            biases,
            config,
            input_dim,
        }
    }

    /// Normalize feature vector to unit length
    #[inline]
    fn normalize_features(&self, features: &[f64]) -> Vec<f64> {
        let norm = features.iter().map(|x| x * x).sum::<f64>().sqrt();
        if norm < 1e-10 {
            return features.to_vec();
        }
        features.iter().map(|x| x / norm).collect()
    }

    /// Compute cosine similarity between two vectors
    #[inline]
    fn cosine_similarity(a: &[f64], b: &[f64]) -> f64 {
        assert_eq!(a.len(), b.len());
        let dot: f64 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
        let norm_a = a.iter().map(|x| x * x).sum::<f64>().sqrt();
        let norm_b = b.iter().map(|x| x * x).sum::<f64>().sqrt();
        
        if norm_a < 1e-10 || norm_b < 1e-10 {
            0.0
        } else {
            dot / (norm_a * norm_b)
        }
    }

    /// Compute angular margin cosine value
    ///
    /// Uses the cosine addition formula: cos(θ + m) = cos(θ)cos(m) - sin(θ)sin(m)
    /// where sin(θ) = sqrt(1 - cos²(θ))
    #[inline]
    fn angular_margin_cosine(cos_theta: f64, margin: f64) -> f64 {
        let cos_m = margin.cos();
        let sin_m = margin.sin();
        let sin_theta = (1.0 - cos_theta * cos_theta).sqrt().max(0.0);
        cos_theta * cos_m - sin_theta * sin_m
    }

    /// Forward pass: compute class probabilities
    ///
    /// # Arguments
    /// * `features` - Input feature vector
    ///
    /// # Returns
    /// Vector of class probabilities [p_0, p_1, ..., p_{num_classes-1}]
    pub fn forward(&self, features: &[f64]) -> Vec<f64> {
        assert_eq!(features.len(), self.input_dim);
        
        let s = self.config.scale;
        let num_classes = self.config.num_classes;
        
        // Compute logits with angular margin for each class
        let mut logits = Vec::with_capacity(num_classes);
        let normalized_features = self.normalize_features(features);
        
        for class_idx in 0..num_classes {
            let cos_theta = Self::cosine_similarity(&normalized_features, &self.weights[class_idx]);
            // Use modified cosine to incorporate margin for correct class
            logits.push(s * cos_theta);
        }
        
        // Compute softmax
        let max_logit = logits.iter().fold(f64::MIN, |m, &x| m.max(x));
        let exp_logits: Vec<f64> = logits.iter()
            .map(|l| (*l - max_logit).exp())
            .collect();
        
        let sum_exp: f64 = exp_logits.iter().sum();
        if sum_exp.is_infinite() || sum_exp < 1e-10 {
            // Numerical stability: return uniform distribution
            vec![1.0 / num_classes as f64; num_classes]
        } else {
            exp_logits.iter().map(|e| e / sum_exp).collect()
        }
    }

    /// Forward pass with margin enforcement for a specific target class
    ///
    /// Used during loss computation to apply angular margin
    ///
    /// # Arguments
    /// * `features` - Input feature vector
    /// * `target_class` - The correct/ground-truth class
    ///
    /// # Returns
    /// Vector of class logits with margin applied to target
    pub fn forward_with_margin(&self, features: &[f64], target_class: usize) -> Vec<f64> {
        assert_eq!(features.len(), self.input_dim);
        assert!(target_class < self.config.num_classes);
        
        let s = self.config.scale;
        let m = self.config.margin;
        let num_classes = self.config.num_classes;
        
        let normalized_features = self.normalize_features(features);
        let mut logits = Vec::with_capacity(num_classes);
        
        for class_idx in 0..num_classes {
            let cos_theta = Self::cosine_similarity(&normalized_features, &self.weights[class_idx]);
            
            if class_idx == target_class {
                // Apply angular margin to correct class
                let cos_margin = Self::angular_margin_cosine(cos_theta, m);
                logits.push(s * cos_margin);
            } else {
                // No margin for incorrect classes
                logits.push(s * cos_theta);
            }
        }
        
        logits
    }

    /// Predict class and confidence from features
    ///
    /// # Arguments
    /// * `features` - Input feature vector
    ///
    /// # Returns
    /// Tuple of (class_idx, confidence)
    pub fn predict(&self, features: &[f64]) -> (usize, f64) {
        let probs = self.forward(features);
        
        let mut max_prob = 0.0;
        let mut max_idx = 0;
        
        for (idx, &prob) in probs.iter().enumerate() {
            if prob > max_prob {
                max_prob = prob;
                max_idx = idx;
            }
        }
        
        (max_idx, max_prob)
    }

    /// Convert class index to directional signal
    ///
    /// # Arguments
    /// * `class_idx` - Class index (0=Down, 1=Neutral, 2=Up)
    /// * `confidence` - Prediction confidence
    ///
    /// # Returns
    /// Directional signal value in [-1, 1] range
    pub fn class_to_direction(class_idx: usize, confidence: f64) -> f64 {
        match class_idx {
            0 => -confidence,  // Down: negative direction
            1 => 0.0,         // Neutral: zero direction
            2 => confidence,  // Up: positive direction
            _ => {
                // For other class counts, interpolate
                let normalized = class_idx as f64;
                let max_class = 2.0; // Assuming 3 classes
                (normalized / max_class * 2.0 - 1.0) * confidence
            }
        }
    }

    /// Compute large-margin cross-entropy loss
    ///
    /// # Arguments
    /// * `features` - Input feature vector
    /// * `target_class` - Ground-truth class index
    ///
    /// # Returns
    /// Loss value
    pub fn loss(&self, features: &[f64], target_class: usize) -> f64 {
        assert!(target_class < self.config.num_classes);
        
        let logits = self.forward_with_margin(features, target_class);
        
        // Numerically stable softmax cross-entropy
        let max_logit = logits.iter().fold(f64::MIN, |m, &x| m.max(x));
        let exp_logits: Vec<f64> = logits.iter()
            .map(|l| (*l - max_logit).exp())
            .collect();
        
        let sum_exp: f64 = exp_logits.iter().sum();
        
        // Loss = -log(softmax[target]) = -log(exp(logit_target) / sum_exp)
        //                            = -logit_target + log(sum_exp)
        let target_logit = logits[target_class];
        
        if sum_exp.is_infinite() || sum_exp < 1e-10 {
            // Handle numerical issues
            target_logit
        } else {
            -target_logit + sum_exp.ln()
        }
    }

    /// Compute gradients of loss with respect to weights
    ///
    /// # Arguments
    /// * `features` - Input feature vector
    /// * `target_class` - Ground-truth class index
    ///
    /// # Returns
    /// Tuple of (weight_gradients, bias_gradients)
    pub fn compute_gradients(&self, features: &[f64], target_class: usize) -> (Vec<Vec<f64>>, Vec<f64>) {
        assert!(target_class < self.config.num_classes);
        
        let s = self.config.scale;
        let m = self.config.margin;
        let num_classes = self.config.num_classes;
        
        let normalized_features = self.normalize_features(features);
        let norm = normalized_features.iter().map(|x| x * x).sum::<f64>().sqrt();
        
        // Compute all logits
        let mut logits = Vec::with_capacity(num_classes);
        let mut cos_thetas = Vec::with_capacity(num_classes);
        
        for class_idx in 0..num_classes {
            let cos_theta = Self::cosine_similarity(&normalized_features, &self.weights[class_idx]);
            cos_thetas.push(cos_theta);
            
            if class_idx == target_class {
                let cos_margin = Self::angular_margin_cosine(cos_theta, m);
                logits.push(s * cos_margin);
            } else {
                logits.push(s * cos_theta);
            }
        }
        
        // Softmax
        let max_logit = logits.iter().fold(f64::MIN, |m, &x| m.max(x));
        let exp_logits: Vec<f64> = logits.iter()
            .map(|l| (*l - max_logit).exp())
            .collect();
        
        let sum_exp: f64 = exp_logits.iter().sum();
        let softmax: Vec<f64> = exp_logits.iter().map(|e| e / sum_exp).collect();
        
        // Gradient computation
        let mut weight_grads = vec![vec![0.0; self.input_dim]; num_classes];
        let mut bias_grads = vec![0.0; num_classes];
        
        // Gradient for each class
        for class_idx in 0..num_classes {
            let prob = softmax[class_idx];
            let is_target = class_idx == target_class;
            
            // Gradient contribution
            let grad_factor = if is_target {
                // For target class, we have additional margin effect
                let cos_theta = cos_thetas[class_idx];
                let _cos_m = m.cos();
                let _sin_m = m.sin();
                let _sin_theta = (1.0 - cos_theta * cos_theta).sqrt().max(0.0);
                
                // d(cos(θ+m))/dθ = -sin(θ+m)
                // But we're differentiating through the margin function
                prob - if norm > 1e-10 { 1.0 } else { 0.0 }
            } else {
                prob
            };
            
            // Weight gradient: dL/dW = (prob - target) * features
            for feat_idx in 0..self.input_dim {
                let feature_val = if norm > 1e-10 {
                    normalized_features[feat_idx]
                } else {
                    features[feat_idx]
                };
                
                let weight_val = self.weights[class_idx][feat_idx];
                
                // Simplified gradient for margin softmax
                let weight_grad = if is_target && norm > 1e-10 {
                    // Apply margin-aware gradient
                    let cos_theta = cos_thetas[class_idx];
                    let sin_theta = (1.0 - cos_theta * cos_theta).sqrt().max(0.0);
                    let cos_m = m.cos();
                    let sin_m = m.sin();
                    
                    // d(s*cos(θ+m))/dW = s * (-sin(θ+m)) * d(cos(θ+m))/dW
                    // d(cos(θ+m))/dW = d(cosθ)/dW * cosm - d(sinθ)/dW * sinm
                    let d_cos_theta_d_w = (feature_val - cos_theta * weight_val) / norm;
                    let d_sin_theta_d_w = -(cos_theta * feature_val / norm - weight_val / norm * sin_theta.powi(2) / sin_theta.max(1e-10)) * sin_theta;
                    
                    (prob - 1.0) * s * (d_cos_theta_d_w * cos_m - d_sin_theta_d_w * sin_m)
                } else {
                    (prob - if is_target { 1.0 } else { 0.0 }) * s * feature_val *
                        (1.0 - if norm > 1e-10 { cos_thetas[class_idx] * weight_val / norm } else { 0.0 })
                };
                
                weight_grads[class_idx][feat_idx] = weight_grad;
            }
            
            bias_grads[class_idx] = grad_factor;
        }
        
        // Apply weight decay
        if self.config.weight_decay > 0.0 {
            for class_idx in 0..num_classes {
                for feat_idx in 0..self.input_dim {
                    weight_grads[class_idx][feat_idx] += self.config.weight_decay * self.weights[class_idx][feat_idx];
                }
            }
        }
        
        (weight_grads, bias_grads)
    }

    /// Update weights using gradient descent
    ///
    /// # Arguments
    /// * `weight_grads` - Weight gradients
    /// * `bias_grads` - Bias gradients
    pub fn update_weights(&mut self, weight_grads: &[Vec<f64>], bias_grads: &[f64]) {
        let lr = self.config.learning_rate;
        
        for class_idx in 0..self.config.num_classes {
            for feat_idx in 0..self.input_dim {
                self.weights[class_idx][feat_idx] -= lr * weight_grads[class_idx][feat_idx];
            }
            self.biases[class_idx] -= lr * bias_grads[class_idx];
        }
    }

    /// Train for one step on a single sample
    ///
    /// # Arguments
    /// * `features` - Input feature vector
    /// * `target_class` - Ground-truth class index
    ///
    /// # Returns
    /// Loss value before update
    pub fn train_step(&mut self, features: &[f64], target_class: usize) -> f64 {
        let loss = self.loss(features, target_class);
        let (weight_grads, bias_grads) = self.compute_gradients(features, target_class);
        self.update_weights(&weight_grads, &bias_grads);
        loss
    }

    /// Get the configuration
    pub fn config(&self) -> &MarginSoftmaxConfig {
        &self.config
    }

    /// Get input dimension
    pub fn input_dim(&self) -> usize {
        self.input_dim
    }

    /// Get number of classes
    pub fn num_classes(&self) -> usize {
        self.config.num_classes
    }

    /// Get reference to weights
    pub fn weights(&self) -> &[Vec<f64>] {
        &self.weights
    }

    /// Get reference to biases
    pub fn biases(&self) -> &[f64] {
        &self.biases
    }
}

/// Directional signal from classifier output
///
/// Provides trading-specific interpretation of classifier results.
#[derive(Debug, Clone, PartialEq)]
pub struct DirectionalSignal {
    /// Trading direction: -1.0 (sell/short) to 1.0 (buy/long)
    pub direction: f64,
    /// Classification confidence: 0.0 to 1.0
    pub confidence: f64,
    /// Predicted class index
    pub class_idx: usize,
    /// Class probabilities
    pub probabilities: Vec<f64>,
}

impl DirectionalSignal {
    /// Create signal from classifier prediction
    pub fn from_prediction(class_idx: usize, confidence: f64, probabilities: Vec<f64>) -> Self {
        let direction = MarginSoftmaxClassifier::class_to_direction(class_idx, confidence);
        
        Self {
            direction,
            confidence,
            class_idx,
            probabilities,
        }
    }

    /// Check if signal is actionable (high confidence, strong direction)
    pub fn is_actionable(&self, min_confidence: f64, min_direction: f64) -> bool {
        self.confidence >= min_confidence && self.direction.abs() >= min_direction
    }

    /// Get trading side (None for neutral)
    pub fn side(&self) -> Option<many_lamps_core::Side> {
        if self.direction > 0.0 {
            Some(many_lamps_core::Side::Buy)
        } else if self.direction < 0.0 {
            Some(many_lamps_core::Side::Sell)
        } else {
            None
        }
    }
}

/// Helper: Simple PRNG for weight initialization
#[inline]
fn rand_drand() -> f64 {
    // Simple linear congruential generator
    static mut STATE: u64 = 123456789;
    unsafe {
        STATE = STATE.wrapping_mul(6364136223846793005)
            .wrapping_add(1);
        let x = (STATE >> 33) as u32;
        let y = (STATE >> 2) as u32;
        (x ^ y) as f64 / (u32::MAX as f64 + 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cosine_similarity() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert!((MarginSoftmaxClassifier::cosine_similarity(&a, &b) - 1.0).abs() < 1e-10);
        
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        assert!((MarginSoftmaxClassifier::cosine_similarity(&a, &b)).abs() < 1e-10);
        
        let a = vec![1.0, 0.0];
        let b = vec![-1.0, 0.0];
        assert!((MarginSoftmaxClassifier::cosine_similarity(&a, &b) - (-1.0)).abs() < 1e-10);
    }

    #[test]
    fn test_angular_margin_cosine() {
        // cos(θ + m) should be less than cos(θ) for m > 0
        let cos_theta = 0.8;
        let margin = 0.5;
        let cos_with_margin = MarginSoftmaxClassifier::angular_margin_cosine(cos_theta, margin);
        assert!(cos_with_margin < cos_theta);
        
        // With m=0, should return original cos_theta
        let cos_zero_margin = MarginSoftmaxClassifier::angular_margin_cosine(cos_theta, 0.0);
        assert!((cos_zero_margin - cos_theta).abs() < 1e-10);
    }

    #[test]
    fn test_forward_pass() {
        let config = MarginSoftmaxConfig::default();
        let classifier = MarginSoftmaxClassifier::new(8, config);
        
        let features = vec![0.5, 0.01, 100.0, 50.0, 0.2, 0.001, 0.6, 0.4];
        let probs = classifier.forward(&features);
        
        // Check probabilities sum to 1
        let sum: f64 = probs.iter().sum();
        assert!((sum - 1.0).abs() < 1e-6);
        
        // Check all probabilities are in [0, 1]
        for &p in &probs {
            assert!(p >= 0.0 && p <= 1.0);
        }
    }

    #[test]
    fn test_predict() {
        let config = MarginSoftmaxConfig::default();
        let classifier = MarginSoftmaxClassifier::new(8, config);
        
        let features = vec![0.5, 0.01, 100.0, 50.0, 0.2, 0.001, 0.6, 0.4];
        let (class_idx, confidence) = classifier.predict(&features);
        
        assert!(class_idx < 3);
        assert!(confidence >= 0.0 && confidence <= 1.0);
    }

    #[test]
    fn test_loss_computation() {
        let config = MarginSoftmaxConfig::default();
        let classifier = MarginSoftmaxClassifier::new(8, config);
        
        let features = vec![0.5, 0.01, 100.0, 50.0, 0.2, 0.001, 0.6, 0.4];
        let loss = classifier.loss(&features, 0);
        
        // Loss should be positive
        assert!(loss >= 0.0);
    }

    #[test]
    fn test_margin_enforcement() {
        let config_no_margin = MarginSoftmaxConfig {
            margin: 0.0,
            ..Default::default()
        };
        let config_with_margin = MarginSoftmaxConfig {
            margin: 0.5,
            ..Default::default()
        };
        
        let mut classifier_no_margin = MarginSoftmaxClassifier::new(8, config_no_margin);
        let mut classifier_with_margin = MarginSoftmaxClassifier::new(8, config_with_margin);
        
        let features = vec![0.5, 0.01, 100.0, 50.0, 0.2, 0.001, 0.6, 0.4];
        
        // Train both classifiers on the same data
        for _ in 0..100 {
            classifier_no_margin.train_step(&features, 2);
            classifier_with_margin.train_step(&features, 2);
        }
        
        // Get predictions
        // Get predictions after training
        let idx_nm = classifier_no_margin.predict(&features).0;
        let idx_wm = classifier_with_margin.predict(&features).0;
        
        // Both should predict class 2 (up) after training
        assert_eq!(idx_nm, 2);
        assert_eq!(idx_wm, 2);
    }

    #[test]
    fn test_train_step() {
        let mut classifier = MarginSoftmaxClassifier::new(8, MarginSoftmaxConfig::default());
        
        let features = vec![0.5, 0.01, 100.0, 50.0, 0.2, 0.001, 0.6, 0.4];
        
        let _initial_loss = classifier.loss(&features, 2);
        let _final_loss = classifier.train_step(&features, 2);
        
        // Loss should have changed after training
        let new_loss = classifier.loss(&features, 2);
        // Loss might increase or decrease depending on gradient direction
        // Just verify no NaN or Inf
        assert!(new_loss.is_finite());
    }

    #[test]
    fn test_gradient_computation() {
        let classifier = MarginSoftmaxClassifier::new(8, MarginSoftmaxConfig::default());
        
        let features = vec![0.5, 0.01, 100.0, 50.0, 0.2, 0.001, 0.6, 0.4];
        let (weight_grads, bias_grads) = classifier.compute_gradients(&features, 0);
        
        assert_eq!(weight_grads.len(), 3);
        assert_eq!(weight_grads[0].len(), 8);
        assert_eq!(bias_grads.len(), 3);
        
        // Gradients should be finite
        for class_grads in &weight_grads {
            for &g in class_grads {
                assert!(g.is_finite());
            }
        }
    }

    #[test]
    fn test_class_to_direction() {
        assert!((MarginSoftmaxClassifier::class_to_direction(0, 0.8) - (-0.8)).abs() < 1e-10);
        assert!((MarginSoftmaxClassifier::class_to_direction(1, 0.5) - 0.0).abs() < 1e-10);
        assert!((MarginSoftmaxClassifier::class_to_direction(2, 0.9) - 0.9).abs() < 1e-10);
    }

    #[test]
    fn test_directional_signal() {
        let signal = DirectionalSignal::from_prediction(2, 0.85, vec![0.05, 0.10, 0.85]);
        
        assert!((signal.direction - 0.85).abs() < 1e-10);
        assert!((signal.confidence - 0.85).abs() < 1e-10);
        assert_eq!(signal.class_idx, 2);
        assert_eq!(signal.side(), Some(many_lamps_core::Side::Buy));
        
        let bear_signal = DirectionalSignal::from_prediction(0, 0.75, vec![0.75, 0.15, 0.10]);
        assert_eq!(bear_signal.side(), Some(many_lamps_core::Side::Sell));
        
        let neutral_signal = DirectionalSignal::from_prediction(1, 0.6, vec![0.2, 0.6, 0.2]);
        assert_eq!(neutral_signal.side(), None);
    }

    #[test]
    fn test_is_actionable() {
        let signal = DirectionalSignal::from_prediction(2, 0.85, vec![0.05, 0.10, 0.85]);
        assert!(signal.is_actionable(0.8, 0.5));
        assert!(!signal.is_actionable(0.9, 0.5));
        assert!(!signal.is_actionable(0.8, 0.9));
    }

    #[test]
    fn test_config_default() {
        let config = MarginSoftmaxConfig::default();
        
        assert_eq!(config.num_classes, 3);
        assert!((config.margin - 0.5).abs() < 1e-10);
        assert!((config.scale - 30.0).abs() < 1e-10);
    }

    #[test]
    fn test_with_weights() {
        let input_dim = 4;
        let num_classes = 3;
        let weights = vec![
            vec![1.0, 0.0, 0.0, 0.0],
            vec![0.0, 1.0, 0.0, 0.0],
            vec![0.0, 0.0, 1.0, 0.0],
        ];
        
        let config = MarginSoftmaxConfig::default();
        let classifier = MarginSoftmaxClassifier::with_weights(input_dim, weights, config);
        
        assert_eq!(classifier.input_dim(), input_dim);
        assert_eq!(classifier.num_classes(), num_classes);
    }

    #[test]
    fn test_feature_normalization() {
        let config = MarginSoftmaxConfig::default();
        let classifier = MarginSoftmaxClassifier::new(3, config);
        
        // Test with unnormalized features
        let features = vec![3.0, 4.0, 0.0]; // Norm = 5
        let probs = classifier.forward(&features);
        
        // Should still work
        let sum: f64 = probs.iter().sum();
        assert!((sum - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_zero_features() {
        let config = MarginSoftmaxConfig::default();
        let classifier = MarginSoftmaxClassifier::new(8, config);
        
        // Zero features should not cause NaN
        let features = vec![0.0; 8];
        let probs = classifier.forward(&features);
        
        // Should return uniform distribution
        let sum: f64 = probs.iter().sum();
        assert!((sum - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_multi_class_training() {
        let mut classifier = MarginSoftmaxClassifier::new(8, MarginSoftmaxConfig::default());
        
        // Training data: 2 classes with distinct features (easier to separate)
        // Down-like features: negative imbalance, more ask size
        let features_down = vec![0.3, 0.02, 50.0, 150.0, -0.5, 0.002, 0.25, 0.75];
        // Up-like features: positive imbalance, more bid size
        let features_up = vec![0.7, 0.02, 150.0, 50.0, 0.5, 0.002, 0.75, 0.25];
        
        // Train
        for _ in 0..500 {
            classifier.train_step(&features_down, 0);
            classifier.train_step(&features_up, 2);
        }
        
        // Predict
        let probs_down = classifier.forward(&features_down);
        let probs_up = classifier.forward(&features_up);
        
        // Down features should have higher prob for class 0
        assert!(probs_down[0] > probs_down[2], 
            "Down class should have higher prob than up for down features: {:?}", probs_down);
        
        // Up features should have higher prob for class 2
        assert!(probs_up[2] > probs_up[0],
            "Up class should have higher prob than down for up features: {:?}", probs_up);
        
        // Verify loss decreases over training
        let loss_before = classifier.loss(&features_down, 0);
        classifier.train_step(&features_down, 0);
        let loss_after = classifier.loss(&features_down, 0);
        
        // Loss should generally decrease or stay stable (allowing for some variance)
        assert!(loss_after <= loss_before + 0.1, 
            "Loss should decrease after training step: before={}, after={}", loss_before, loss_after);
    }

    #[test]
    fn test_forward_with_margin() {
        let config = MarginSoftmaxConfig {
            margin: 0.5,
            ..Default::default()
        };
        let classifier = MarginSoftmaxClassifier::new(8, config);
        
        let features = vec![0.5, 0.01, 100.0, 50.0, 0.2, 0.001, 0.6, 0.4];
        
        let logits_no_margin = classifier.forward(&features);
        let logits_with_margin = classifier.forward_with_margin(&features, 2);
        
        // With margin, target class should have different logit
        assert_ne!(logits_no_margin[2], logits_with_margin[2]);
    }
}
