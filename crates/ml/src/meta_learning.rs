//! Model-Agnostic Meta-Learning (MAML) for Rapid Regime Adaptation
//!
//! This module implements MAML to enable rapid adaptation to new market regimes
//! with few samples. The key idea is to train the model to be easily fine-tunable
//! with few gradient steps.
//!
//! # MAML Algorithm
//!
//! The meta-objective is: θ* = argmin_θ Σ_tasks L(θ - α∇L(θ, D_train), D_val)
//!
//! Where:
//! - θ: meta-parameters (base model weights)
//! - α: inner loop learning rate (adaptation rate)
//! - D_train: training samples (support set)
//! - D_val: validation samples (query set)
//!
//! # Usage
//!
//! ```
//! use mtrader_ml::{MAMLConfig, MetaLearner, Task, RegimeConverter};
//!
//! // Create MAML configuration
//! let config = MAMLConfig::default();
//!
//! // Create meta-learner with model dimension
//! let mut meta_learner = MetaLearner::new(64, config.clone());
//!
//! // Create tasks (market regimes)
//! let task = Task::new(
//!     "bullish_trending".to_string(),
//!     vec![
//!         (vec![0.1, 0.2, 0.3], 1.0),
//!         (vec![0.15, 0.25, 0.35], 0.9),
//!     ],
//!     vec![
//!         (vec![0.12, 0.22, 0.32], 0.95),
//!     ],
//! );
//!
//! // Meta-train on tasks
//! let meta_loss = meta_learner.meta_train(&[task]);
//! println!("Meta-loss: {}", meta_loss);
//!
//! // Fast adaptation to new regime
//! let samples = vec![
//!     (vec![0.1, 0.2, 0.3], 1.0),
//!     (vec![0.15, 0.25, 0.35], 0.9),
//! ];
//! let adapted_params = meta_learner.fast_adapt(&samples, 5);
//! ```

use rand::Rng;
use std::collections::VecDeque;
use thiserror::Error;

/// MAML configuration parameters
#[derive(Debug, Clone)]
pub struct MAMLConfig {
    /// Inner loop learning rate (adaptation rate)
    pub inner_lr: f64,
    /// Outer loop learning rate (meta-update rate)
    pub outer_lr: f64,
    /// Number of adaptation steps in inner loop
    pub inner_steps: usize,
    /// Number of tasks per meta-update
    pub meta_batch_size: usize,
    /// Model parameter dimension
    pub model_dim: usize,
}

impl Default for MAMLConfig {
    fn default() -> Self {
        Self {
            inner_lr: 0.01,
            outer_lr: 0.001,
            inner_steps: 5,
            meta_batch_size: 4,
            model_dim: 64,
        }
    }
}

/// Represents a market regime task for meta-learning
///
/// Each task contains:
/// - training_samples: Support set for inner loop adaptation
/// - validation_samples: Query set for meta-objective evaluation
/// - regime_id: Identifier for the market regime
#[derive(Debug, Clone)]
pub struct Task {
    /// Training samples (support set) for inner loop adaptation
    pub training_samples: Vec<(Vec<f64>, f64)>,
    /// Validation samples (query set) for meta-update
    pub validation_samples: Vec<(Vec<f64>, f64)>,
    /// Unique identifier for this market regime
    pub regime_id: String,
}

impl Task {
    /// Create a new task with samples and regime ID
    pub fn new(
        regime_id: String,
        training_samples: Vec<(Vec<f64>, f64)>,
        validation_samples: Vec<(Vec<f64>, f64)>,
    ) -> Self {
        Self {
            training_samples,
            validation_samples,
            regime_id,
        }
    }

    /// Create a synthetic task for testing
    #[cfg(test)]
    pub fn synthetic(
        regime_id: String,
        num_train: usize,
        num_val: usize,
        feature_dim: usize,
        rng: &mut impl Rng,
    ) -> Self {
        let training_samples: Vec<_> = (0..num_train)
            .map(|_| {
                let features: Vec<f64> = (0..feature_dim).map(|_| rng.gen_range(-1.0..1.0)).collect();
                let label = if features[0] > 0.0 { 1.0 } else { -1.0 };
                (features, label)
            })
            .collect();

        let validation_samples: Vec<_> = (0..num_val)
            .map(|_| {
                let features: Vec<f64> = (0..feature_dim).map(|_| rng.gen_range(-1.0..1.0)).collect();
                let label = if features[0] > 0.0 { 1.0 } else { -1.0 };
                (features, label)
            })
            .collect();

        Self::new(regime_id, training_samples, validation_samples)
    }
}

/// Meta-learner implementing MAML algorithm
///
/// Maintains base model parameters and provides methods for:
/// - meta_train: Train on multiple tasks
/// - adapt: Get adapted parameters for a task
/// - fast_adapt: Quick adaptation with few samples
#[derive(Debug, Clone)]
pub struct MetaLearner {
    /// Base model parameters (meta-parameters θ)
    pub parameters: Vec<f64>,
    /// MAML configuration
    pub config: MAMLConfig,
    /// Buffer of recent tasks for experience replay
    task_buffer: VecDeque<Task>,
    /// Maximum buffer size
    max_buffer_size: usize,
}

impl MetaLearner {
    /// Create a new meta-learner
    ///
    /// # Arguments
    /// * `model_dim` - Dimension of model parameters
    /// * `config` - MAML configuration
    ///
    /// # Returns
    /// New MetaLearner with random initial parameters
    pub fn new(model_dim: usize, config: MAMLConfig) -> Self {
        let mut rng = rand::thread_rng();
        let parameters: Vec<f64> = (0..model_dim).map(|_| rng.gen_range(-0.1..0.1)).collect();

        Self {
            parameters,
            config,
            task_buffer: VecDeque::new(),
            max_buffer_size: 100,
        }
    }

    /// Create meta-learner from existing parameters
    pub fn from_parameters(parameters: Vec<f64>, config: MAMLConfig) -> Self {
        Self {
            parameters,
            config,
            task_buffer: VecDeque::new(),
            max_buffer_size: 100,
        }
    }

    /// Add a task to the buffer
    pub fn add_task(&mut self, task: Task) {
        if self.task_buffer.len() >= self.max_buffer_size {
            self.task_buffer.pop_front();
        }
        self.task_buffer.push_back(task);
    }

    /// Get recent tasks from buffer
    pub fn recent_tasks(&self) -> Vec<&Task> {
        self.task_buffer.iter().collect()
    }

    /// Meta-train on a batch of tasks
    ///
    /// Implements the MAML meta-objective:
    /// θ* = argmin_θ Σ_tasks L(θ - α∇L(θ, D_train), D_val)
    ///
    /// # Arguments
    /// * `tasks` - Array of tasks for meta-training
    ///
    /// # Returns
    /// Meta-loss averaged across tasks
    pub fn meta_train(&mut self, tasks: &[Task]) -> f64 {
        if tasks.is_empty() {
            return 0.0;
        }

        // Collect tasks for meta-update
        let meta_batch: Vec<Task> = if tasks.len() >= self.config.meta_batch_size {
            // Sample meta_batch_size tasks
            let sampled: Vec<Task> = tasks.iter().take(self.config.meta_batch_size).cloned().collect();
            
            // Add to buffer
            for task in sampled.iter() {
                self.add_task(task.clone());
            }
            
            sampled
        } else {
            tasks.iter().cloned().collect()
        };

        // Compute meta-gradient and update
        let meta_gradient = self.compute_meta_gradient(&meta_batch);
        
        // Apply outer loop update: θ = θ - β * meta_gradient
        let lr = self.config.outer_lr;
        for (param, grad) in self.parameters.iter_mut().zip(meta_gradient.iter()) {
            *param -= lr * grad;
        }

        // Compute and return average meta-loss
        self.compute_average_meta_loss(&meta_batch)
    }

    /// Compute meta-gradient across tasks
    ///
    /// For each task:
    /// 1. Inner loop: adapt parameters with gradient steps on training samples
    /// 2. Compute loss on validation samples with adapted parameters
    /// 3. Accumulate gradients for meta-update
    fn compute_meta_gradient(&self, tasks: &[Task]) -> Vec<f64> {
        let mut meta_gradient: Vec<f64> = vec![0.0; self.parameters.len()];
        
        for task in tasks {
            // Inner loop adaptation
            let adapted_params = self.inner_loop_adapt(&task);
            
            // Compute validation loss gradient w.r.t. adapted parameters
            let val_grad = self.compute_loss_gradient(
                &task.validation_samples,
                &adapted_params,
            );
            
            // Accumulate meta-gradient
            for (mg, g) in meta_gradient.iter_mut().zip(val_grad.iter()) {
                *mg += g;
            }
        }

        // Average gradients
        let n = tasks.len() as f64;
        for grad in meta_gradient.iter_mut() {
            *grad /= n;
        }

        meta_gradient
    }

    /// Inner loop: adapt parameters on training samples
    ///
    /// Performs multiple gradient steps on support set:
    /// θ' = θ - α * ∇L(θ, D_train)
    fn inner_loop_adapt(&self, task: &Task) -> Vec<f64> {
        let mut adapted_params = self.parameters.clone();
        
        for _ in 0..self.config.inner_steps {
            // Compute gradient on training samples
            let grad = self.compute_loss_gradient(&task.training_samples, &adapted_params);
            
            // Gradient descent step
            let lr = self.config.inner_lr;
            for (param, g) in adapted_params.iter_mut().zip(grad.iter()) {
                *param -= lr * g;
            }
        }

        adapted_params
    }

    /// Compute loss gradient with respect to parameters
    fn compute_loss_gradient(
        &self,
        samples: &[(Vec<f64>, f64)],
        params: &[f64],
    ) -> Vec<f64> {
        if samples.is_empty() {
            return vec![0.0; params.len()];
        }

        // Simplified gradient: dL/dW for linear model
        // L = MSE = (y_pred - y_true)²
        // For linear model: y_pred = W * x + b
        // dL/dW = 2 * (y_pred - y_true) * x
        let mut gradients: Vec<f64> = vec![0.0; params.len()];
        
        for (features, label) in samples {
            let prediction = self.forward_flat(features, params);
            let error = prediction - label;

            // Gradient for weights: dL/dW = 2 * error * x
            for (i, &feat) in features.iter().enumerate() {
                if i < params.len() {
                    gradients[i] += 2.0 * error * feat / samples.len() as f64;
                }
            }
        }

        gradients
    }

    /// Forward pass with flat parameters
    fn forward_flat(&self, features: &[f64], params: &[f64]) -> f64 {
        let mut sum = 0.0;
        for (i, &param) in params.iter().enumerate().take(features.len()) {
            sum += param * features[i];
        }
        sum
    }

    /// Compute average meta-loss across tasks
    fn compute_average_meta_loss(&self, tasks: &[Task]) -> f64 {
        let mut total_loss = 0.0;
        
        for task in tasks {
            // Compute loss on validation samples with adapted parameters
            let adapted_params = self.inner_loop_adapt(task);
            let val_loss = self.compute_mse_loss(&task.validation_samples, &adapted_params);
            total_loss += val_loss;
        }

        total_loss / tasks.len() as f64
    }

    /// Compute MSE loss on samples
    fn compute_mse_loss(&self, samples: &[(Vec<f64>, f64)], params: &[f64]) -> f64 {
        let mut total_loss = 0.0;
        
        for (features, label) in samples {
            let prediction = self.forward_flat(features, params);
            let error = prediction - label;
            total_loss += error * error;
        }
        
        total_loss / samples.len() as f64
    }

    /// Adapt to a specific task
    ///
    /// Returns the adapted parameters after inner loop adaptation
    pub fn adapt(&self, task: &Task) -> Vec<f64> {
        self.inner_loop_adapt(task)
    }

    /// Fast adaptation with few samples
    ///
    /// Quick adaptation to new regime with specified number of gradient steps.
    /// This is the primary method for regime switching scenarios.
    ///
    /// # Arguments
    /// * `samples` - Training samples for adaptation
    /// * `steps` - Number of gradient steps (typically 5-10)
    ///
    /// # Returns
    /// Adapted model parameters
    pub fn fast_adapt(&self, samples: &[(Vec<f64>, f64)], steps: usize) -> Vec<f64> {
        let mut adapted_params = self.parameters.clone();
        
        for _ in 0..steps {
            // Compute gradient
            let grad = self.compute_loss_gradient(samples, &adapted_params);
            
            // Gradient descent step
            for (param, g) in adapted_params.iter_mut().zip(grad.iter()) {
                *param -= self.config.inner_lr * g;
            }
        }

        adapted_params
    }

    /// Get current parameters
    pub fn get_parameters(&self) -> &Vec<f64> {
        &self.parameters
    }

    /// Get adapted parameters for inference
    pub fn get_adapted_parameters(&self, task: &Task) -> Vec<f64> {
        self.adapt(task)
    }
}

/// Helper functions for gradient computation and parameter updates
impl MetaLearner {
    /// Compute numerical gradient for verification
    pub fn numerical_gradient(&self, samples: &[(Vec<f64>, f64)]) -> Vec<f64> {
        let eps = 1e-5;
        let mut grad = vec![0.0; self.parameters.len()];

        for i in 0..self.parameters.len() {
            let mut params_plus = self.parameters.clone();
            let mut params_minus = self.parameters.clone();
            
            params_plus[i] += eps;
            params_minus[i] -= eps;
            
            let loss_plus = self.compute_mse_loss(samples, &params_plus);
            let loss_minus = self.compute_mse_loss(samples, &params_minus);
            
            grad[i] = (loss_plus - loss_minus) / (2.0 * eps);
        }

        grad
    }

    /// Update parameters with gradient (in-place)
    pub fn update_parameters(&mut self, gradient: &[f64], lr: f64) {
        for (param, grad) in self.parameters.iter_mut().zip(gradient.iter()) {
            *param -= lr * grad;
        }
    }

    /// Clip gradients to max norm
    pub fn clip_gradients(&mut self, max_norm: f64) {
        let norm: f64 = self.parameters
            .iter()
            .map(|p| p * p)
            .sum::<f64>()
            .sqrt();
        
        if norm > max_norm {
            let scale = max_norm / norm;
            for param in self.parameters.iter_mut() {
                *param *= scale;
            }
        }
    }
}

/// Errors for MAML operations
#[derive(Debug, Error)]
pub enum MAMLError {
    #[error("Empty task batch for meta-training")]
    EmptyTaskBatch,
    #[error("Invalid parameter dimension")]
    InvalidParameterDimension,
    #[error("No samples for adaptation")]
    NoSamples,
}

/// Converter for market data to MAML tasks
///
/// Wraps market data into tasks that can be used for meta-learning
/// and regime detection.
pub struct RegimeConverter {
    feature_dim: usize,
    samples_per_task: usize,
}

impl RegimeConverter {
    /// Create a new regime converter
    pub fn new(feature_dim: usize, samples_per_task: usize) -> Self {
        Self {
            feature_dim,
            samples_per_task,
        }
    }

    /// Convert price history to task samples
    ///
    /// # Arguments
    /// * `prices` - Price history
    /// * `spreads` - Spread history
    /// * `direction` - Market direction (-1, 0, or 1)
    ///
    /// # Returns
    /// Vector of (features, label) samples
    pub fn to_samples(
        &self,
        prices: &[f64],
        spreads: &[f64],
        direction: f64,
    ) -> Vec<(Vec<f64>, f64)> {
        let mut samples = Vec::new();
        
        for i in self.samples_per_task.min(prices.len())..prices.len() {
            let features = self.extract_features(prices, spreads, i);
            samples.push((features, direction));
        }
        
        samples
    }

    /// Extract features at a given index
    fn extract_features(&self, prices: &[f64], spreads: &[f64], idx: usize) -> Vec<f64> {
        let mut features = Vec::with_capacity(self.feature_dim + 1);
        
        // Price returns
        let start = if idx > 0 { idx - 1 } else { 0 };
        for j in start..idx {
            if j > 0 && prices[j - 1] > 0.0 {
                let ret = (prices[j] - prices[j - 1]) / prices[j - 1];
                features.push(ret.clamp(-1.0, 1.0));
            } else {
                features.push(0.0);
            }
        }
        
        // Fill remaining
        while features.len() < self.feature_dim {
            features.push(0.0);
        }
        
        // Add spread if available
        if idx < spreads.len() {
            features.push(spreads[idx].clamp(0.0, 1.0));
        }
        
        features
    }

    /// Convert regime data to MAML task
    pub fn to_task(
        &self,
        regime_id: String,
        train_prices: &[f64],
        train_spreads: &[f64],
        train_direction: f64,
        val_prices: &[f64],
        val_spreads: &[f64],
        val_direction: f64,
    ) -> Task {
        let training_samples = self.to_samples(train_prices, train_spreads, train_direction);
        let validation_samples = self.to_samples(val_prices, val_spreads, val_direction);
        
        Task::new(regime_id, training_samples, validation_samples)
    }

    /// Detect regime from recent data and create task
    pub fn detect_and_create_task(
        &self,
        recent_prices: &[f64],
        recent_spreads: &[f64],
        regime_id: String,
    ) -> Task {
        // Split into train/val (80/20)
        let split_idx = (recent_prices.len() as f64 * 0.8) as usize;
        
        let train_prices = &recent_prices[..split_idx.min(recent_prices.len())];
        let train_spreads = &recent_spreads[..split_idx.min(recent_spreads.len())];
        let val_prices = &recent_prices[split_idx..];
        let val_spreads = &recent_spreads[split_idx..];
        
        // Detect direction from price movement
        let direction = if recent_prices.len() >= 2 {
            let ret = (recent_prices[recent_prices.len() - 1] - recent_prices[0]) / recent_prices[0].max(0.001);
            ret.clamp(-1.0, 1.0)
        } else {
            0.0
        };
        
        self.to_task(
            regime_id,
            train_prices,
            train_spreads,
            direction,
            val_prices,
            val_spreads,
            direction,
        )
    }
}

/// Adapter for hot-swapping weights into inference engine
pub struct MAMLInferenceAdapter {
    meta_learner: MetaLearner,
}

impl MAMLInferenceAdapter {
    /// Create a new adapter
    pub fn new(meta_learner: MetaLearner) -> Self {
        Self { meta_learner }
    }

    /// Adapt to detected regime and get weights for inference
    pub fn adapt_to_regime(&mut self, task: &Task) -> Vec<f64> {
        self.meta_learner.adapt(task)
    }

    /// Quick adaptation with samples
    pub fn quick_adapt(&self, samples: &[(Vec<f64>, f64)]) -> Vec<f64> {
        self.meta_learner.fast_adapt(samples, 5)
    }

    /// Get current meta-parameters
    pub fn get_meta_parameters(&self) -> &Vec<f64> {
        self.meta_learner.get_parameters()
    }

    /// Update meta-learner with new task
    pub fn update(&mut self, task: &Task) {
        self.meta_learner.add_task(task.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;

    fn create_test_config() -> MAMLConfig {
        MAMLConfig {
            inner_lr: 0.01,
            outer_lr: 0.001,
            inner_steps: 5,
            meta_batch_size: 4,
            model_dim: 64,
        }
    }

    #[test]
    fn test_maml_config_defaults() {
        let config = MAMLConfig::default();
        assert_eq!(config.inner_lr, 0.01);
        assert_eq!(config.outer_lr, 0.001);
        assert_eq!(config.inner_steps, 5);
        assert_eq!(config.meta_batch_size, 4);
    }

    #[test]
    fn test_task_creation() {
        let task = Task::new(
            "test_regime".to_string(),
            vec![(vec![0.1, 0.2], 1.0), (vec![0.3, 0.4], -1.0)],
            vec![(vec![0.5, 0.6], 0.5)],
        );
        
        assert_eq!(task.regime_id, "test_regime");
        assert_eq!(task.training_samples.len(), 2);
        assert_eq!(task.validation_samples.len(), 1);
    }

    #[test]
    fn test_meta_learner_creation() {
        let config = create_test_config();
        let learner = MetaLearner::new(64, config);
        
        assert_eq!(learner.parameters.len(), 64);
        assert!(learner.task_buffer.is_empty());
    }

    #[test]
    fn test_inner_loop_adaptation() {
        let config = create_test_config();
        let learner = MetaLearner::new(10, config);
        
        let task = Task::new(
            "test".to_string(),
            vec![(vec![1.0, 0.5], 1.0)],
            vec![(vec![0.8, 0.6], 0.9)],
        );
        
        let adapted = learner.adapt(&task);
        assert_eq!(adapted.len(), learner.parameters.len());
    }

    #[test]
    fn test_fast_adaptation() {
        let config = create_test_config();
        let learner = MetaLearner::new(10, config);
        
        let samples = vec![
            (vec![1.0, 0.5, 0.3], 1.0),
            (vec![0.8, 0.6, 0.4], 0.9),
            (vec![0.9, 0.7, 0.5], 0.95),
        ];
        
        let adapted = learner.fast_adapt(&samples, 5);
        assert_eq!(adapted.len(), 10);
    }

    #[test]
    fn test_meta_train() {
        let config = create_test_config();
        let mut learner = MetaLearner::new(10, config);
        
        let mut rng = rand::rngs::StdRng::seed_from_u64(42);
        let tasks: Vec<_> = (0..4)
            .map(|i| Task::synthetic(format!("regime_{}", i), 10, 5, 10, &mut rng))
            .collect();
        
        let meta_loss = learner.meta_train(&tasks);
        assert!(meta_loss >= 0.0);
        assert!(meta_loss.is_finite());
    }

    #[test]
    fn test_multi_task_meta_training() {
        let config = create_test_config();
        let mut learner = MetaLearner::new(20, config);
        
        let mut rng = rand::rngs::StdRng::seed_from_u64(123);
        let tasks: Vec<_> = (0..8)
            .map(|i| Task::synthetic(format!("regime_{}", i), 15, 10, 20, &mut rng))
            .collect();
        
        // Multiple meta-training iterations
        for _ in 0..3 {
            let loss = learner.meta_train(&tasks);
            assert!(loss.is_finite());
        }
    }

    #[test]
    fn test_fast_adaptation_with_few_samples() {
        let config = create_test_config();
        let learner = MetaLearner::new(10, config);
        
        // Test with 5 samples (typical regime switch scenario)
        let samples_5 = vec![
            (vec![1.0, 0.5], 1.0),
            (vec![0.9, 0.6], 0.95),
            (vec![0.85, 0.55], 0.9),
            (vec![0.95, 0.65], 0.92),
            (vec![0.88, 0.58], 0.88),
        ];
        
        let adapted = learner.fast_adapt(&samples_5, 5);
        assert_eq!(adapted.len(), 10);
        
        // Test with 10 samples
        let samples_10: Vec<(Vec<f64>, f64)> = samples_5.iter()
            .cloned()
            .chain(samples_5.iter().cloned())
            .collect();
        
        let adapted_10 = learner.fast_adapt(&samples_10, 5);
        assert_eq!(adapted_10.len(), 10);
    }

    #[test]
    fn test_regime_switching_scenario() {
        let config = create_test_config();
        let mut learner = MetaLearner::new(20, config);
        
        // Pre-train on diverse regimes
        let mut rng = rand::rngs::StdRng::seed_from_u64(456);
        let regimes = ["bull", "bear", "sideways", "volatile", "calm"];
        let pretrain_tasks: Vec<_> = regimes.iter()
            .map(|r| Task::synthetic(r.to_string(), 20, 10, 20, &mut rng))
            .collect();
        
        learner.meta_train(&pretrain_tasks);
        
        // Simulate regime switch - new unseen regime
        let new_regime_samples = vec![
            (vec![0.1, 0.2, 0.3, 0.4, 0.5], 0.8),
            (vec![0.12, 0.22, 0.32, 0.42, 0.52], 0.75),
            (vec![0.11, 0.21, 0.31, 0.41, 0.51], 0.78),
        ];
        
        // Fast adaptation should work despite regime switch
        let adapted = learner.fast_adapt(&new_regime_samples, 10);
        assert_eq!(adapted.len(), 20);
        assert!(adapted.iter().all(|p| p.is_finite()));
    }

    #[test]
    fn test_gradient_computation() {
        let config = create_test_config();
        let learner = MetaLearner::new(3, config);
        
        let samples = vec![
            (vec![1.0, 0.5, 0.3], 1.0),
            (vec![0.8, 0.6, 0.4], 0.9),
        ];
        
        let grad = learner.compute_loss_gradient(&samples, &learner.parameters);
        assert_eq!(grad.len(), 3);
        assert!(grad.iter().all(|g| g.is_finite()));
    }

    #[test]
    fn test_parameter_update() {
        let config = create_test_config();
        let mut learner = MetaLearner::new(10, config);
        
        let original_params: Vec<f64> = learner.parameters.iter().copied().collect();
        let gradient: Vec<f64> = vec![0.01; 10];
        
        learner.update_parameters(&gradient, 0.1);
        
        // Parameters should have changed
        for (orig, new) in original_params.iter().zip(learner.parameters.iter()) {
            assert!((orig - new).abs() > 0.0);
        }
    }

    #[test]
    fn test_task_buffer() {
        let config = create_test_config();
        let mut learner = MetaLearner::new(10, config);
        
        for i in 0..10 {
            let task = Task::new(
                format!("regime_{}", i),
                vec![],
                vec![],
            );
            learner.add_task(task);
        }
        
        assert_eq!(learner.task_buffer.len(), 10);
        
        // Add more than buffer size
        for i in 10..120 {
            let task = Task::new(
                format!("regime_{}", i),
                vec![],
                vec![],
            );
            learner.add_task(task);
        }
        
        // Buffer should be capped at max_buffer_size (100)
        assert_eq!(learner.task_buffer.len(), 100);
    }

    #[test]
    fn test_regime_converter() {
        let converter = RegimeConverter::new(10, 5);
        
        let prices = vec![100.0, 101.0, 102.0, 103.0, 104.0, 105.0, 106.0, 107.0, 108.0, 109.0];
        let spreads = vec![0.01, 0.02, 0.015, 0.02, 0.018, 0.02, 0.022, 0.02, 0.019, 0.02];
        
        let samples = converter.to_samples(&prices, &spreads, 1.0);
        assert!(!samples.is_empty());
        
        // Test task creation
        let task = converter.to_task(
            "test_regime".to_string(),
            &prices[..5],
            &spreads[..5],
            1.0,
            &prices[5..],
            &spreads[5..],
            0.5,
        );
        
        assert_eq!(task.regime_id, "test_regime");
    }

    #[test]
    fn test_maml_inference_adapter() {
        let config = create_test_config();
        let meta_learner = MetaLearner::new(10, config);
        let mut adapter = MAMLInferenceAdapter::new(meta_learner);
        
        let meta_params = adapter.get_meta_parameters();
        assert_eq!(meta_params.len(), 10);
        
        let task = Task::new(
            "test".to_string(),
            vec![(vec![1.0, 0.5], 1.0)],
            vec![(vec![0.8, 0.6], 0.9)],
        );
        
        let adapted = adapter.adapt_to_regime(&task);
        assert_eq!(adapted.len(), 10);
    }

    #[test]
    fn test_empty_meta_train() {
        let config = create_test_config();
        let mut learner = MetaLearner::new(10, config);
        
        let loss = learner.meta_train(&[]);
        assert_eq!(loss, 0.0);
    }

    #[test]
    fn test_gradient_clipping() {
        let config = create_test_config();
        let mut learner = MetaLearner::new(10, config);
        
        // Set large parameters
        for param in learner.parameters.iter_mut() {
            *param = 1000.0;
        }
        
        learner.clip_gradients(1.0);
        
        let norm: f64 = learner.parameters.iter().map(|p| p * p).sum::<f64>().sqrt();
        assert!(norm <= 1.0 + 1e-6);
    }
}
