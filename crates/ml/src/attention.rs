//! Multi-Head Attention for Cross-Asset Correlation
//!
//! This module implements multi-head attention mechanism for capturing
//! cross-asset correlations and dependencies in multi-market trading systems.

use rand::Rng;
use serde::{Deserialize, Serialize};

/// Configuration for multi-head attention
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttentionConfig {
    /// Number of attention heads (default: 4)
    pub num_heads: usize,
    /// Model dimension (default: 64)
    pub d_model: usize,
    /// Dropout rate for regularization (default: 0.1)
    pub dropout: f64,
}

impl Default for AttentionConfig {
    fn default() -> Self {
        Self {
            num_heads: 4,
            d_model: 64,
            dropout: 0.1,
        }
    }
}

impl AttentionConfig {
    /// Validate configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.num_heads == 0 {
            return Err("num_heads must be greater than 0".to_string());
        }
        if self.d_model == 0 {
            return Err("d_model must be greater than 0".to_string());
        }
        if self.d_model % self.num_heads != 0 {
            return Err(format!(
                "d_model ({}) must be divisible by num_heads ({})",
                self.d_model, self.num_heads
            ));
        }
        if self.dropout < 0.0 || self.dropout >= 1.0 {
            return Err("dropout must be in [0, 1)".to_string());
        }
        Ok(())
    }
}

/// Multi-Head Attention implementation
#[derive(Debug, Clone)]
pub struct MultiHeadAttention {
    num_heads: usize,
    d_model: usize,
    d_k: usize,
    d_v: usize,
    dropout: f64,
    w_q: Vec<Vec<Vec<f64>>>,
    w_k: Vec<Vec<Vec<f64>>>,
    w_v: Vec<Vec<Vec<f64>>>,
    w_o: Vec<Vec<f64>>,
    layer_norm_gamma: Vec<f64>,
    layer_norm_beta: Vec<f64>,
    layer_norm_eps: f64,
    training: bool,
}

impl MultiHeadAttention {
    pub fn new(config: &AttentionConfig) -> Self {
        config.validate().expect("Invalid attention config");

        let num_heads = config.num_heads;
        let d_model = config.d_model;
        let d_k = d_model / num_heads;
        let d_v = d_k;

        let mut rng = rand::thread_rng();

        let scale_qk = (2.0 / d_model as f64).sqrt();
        let w_q: Vec<Vec<Vec<f64>>> = (0..num_heads)
            .map(|_| {
                (0..d_model)
                    .map(|_| {
                        (0..d_k)
                            .map(|_| rng.gen_range(-1.0..1.0) * scale_qk)
                            .collect()
                    })
                    .collect()
            })
            .collect();

        let w_k: Vec<Vec<Vec<f64>>> = (0..num_heads)
            .map(|_| {
                (0..d_model)
                    .map(|_| {
                        (0..d_k)
                            .map(|_| rng.gen_range(-1.0..1.0) * scale_qk)
                            .collect()
                    })
                    .collect()
            })
            .collect();

        let w_v: Vec<Vec<Vec<f64>>> = (0..num_heads)
            .map(|_| {
                (0..d_model)
                    .map(|_| {
                        (0..d_v)
                            .map(|_| rng.gen_range(-1.0..1.0) * scale_qk)
                            .collect()
                    })
                    .collect()
            })
            .collect();

        let scale_o = (2.0 / (num_heads * d_v) as f64).sqrt();
        let w_o: Vec<Vec<f64>> = (0..num_heads * d_v)
            .map(|_| (0..d_model).map(|_| rng.gen_range(-1.0..1.0) * scale_o).collect())
            .collect();

        let layer_norm_gamma = vec![1.0; d_model];
        let layer_norm_beta = vec![0.0; d_model];

        Self {
            num_heads,
            d_model,
            d_k,
            d_v,
            dropout: config.dropout,
            w_q,
            w_k,
            w_v,
            w_o,
            layer_norm_gamma,
            layer_norm_beta,
            layer_norm_eps: 1e-5,
            training: false,
        }
    }

    #[inline]
    pub fn set_training(&mut self, training: bool) {
        self.training = training;
    }

    pub fn forward(
        &self,
        queries: &[Vec<f64>],
        keys: &[Vec<f64>],
        values: &[Vec<f64>],
    ) -> Vec<Vec<f64>> {
        let seq_len_q = queries.len();
        let seq_len_k = keys.len();
        let seq_len_v = values.len();

        // Handle empty sequences gracefully
        if seq_len_q == 0 || seq_len_k == 0 || seq_len_k != seq_len_v {
            return Vec::new();
        }

        let num_heads = self.num_heads;
        let d_k = self.d_k;

        let mut head_outputs: Vec<Vec<Vec<f64>>> = Vec::with_capacity(num_heads);

        for h in 0..num_heads {
            let q_projected = Self::project(&queries, &self.w_q[h]);
            let k_projected = Self::project(&keys, &self.w_k[h]);
            let v_projected = Self::project(&values, &self.w_v[h]);

            let head_output = scaled_dot_product_attention(
                &q_projected,
                &k_projected,
                &v_projected,
                d_k,
                self.dropout,
                self.training,
            );

            head_outputs.push(head_output);
        }

        let concatenated = Self::concatenate_heads(&head_outputs, self.d_v);
        let output = Self::project(&concatenated, &self.w_o);

        let residual = if queries.len() == output.len() {
            queries.to_vec()
        } else {
            let min_len = queries.len().min(output.len());
            queries[..min_len].to_vec()
        };

        Self::layer_norm_add(&output, &residual, &self.layer_norm_gamma, &self.layer_norm_beta, self.layer_norm_eps)
    }

    pub fn self_attention(&self, input: &[Vec<f64>]) -> Vec<Vec<f64>> {
        self.forward(input, input, input)
    }

    pub fn attention_weights(&self, queries: &[Vec<f64>], keys: &[Vec<f64>]) -> Vec<Vec<f64>> {
        assert!(!queries.is_empty(), "Queries must not be empty");
        assert!(!keys.is_empty(), "Keys must not be empty");

        let d_k = self.d_k;
        let scale = (d_k as f64).sqrt();

        let mut attention_scores: Vec<Vec<f64>> = Vec::with_capacity(queries.len());

        for q in queries {
            let mut scores: Vec<f64> = Vec::with_capacity(keys.len());
            for k in keys {
                let score = Self::dot_product_single(q, k) / scale;
                scores.push(score);
            }
            attention_scores.push(scores);
        }

        let mut attention_weights: Vec<Vec<f64>> = Vec::with_capacity(queries.len());
        for scores in &attention_scores {
            let max_score = scores.iter().fold(f64::MIN, |m, s| m.max(*s));
            let exp_scores: Vec<f64> = scores.iter().map(|s| (s - max_score).exp()).collect();
            let sum_exp: f64 = exp_scores.iter().sum();
            let softmax: Vec<f64> = exp_scores.iter().map(|e| e / sum_exp).collect();
            attention_weights.push(softmax);
        }

        attention_weights
    }

    #[inline]
    fn project(input: &[Vec<f64>], weights: &[Vec<f64>]) -> Vec<Vec<f64>> {
        if input.is_empty() {
            return Vec::new();
        }

        let output_dim = weights.len();
        let input_dim = weights[0].len();

        input
            .iter()
            .map(|vec| {
                if vec.len() != input_dim {
                    let mut padded = vec![0.0; input_dim];
                    let copy_len = vec.len().min(input_dim);
                    padded[..copy_len].copy_from_slice(&vec[..copy_len]);
                    padded
                } else {
                    vec.clone()
                }
            })
            .map(|vec| {
                let mut output = vec![0.0; output_dim];
                for i in 0..output_dim {
                    for j in 0..input_dim {
                        output[i] += vec[j] * weights[i][j];
                    }
                }
                output
            })
            .collect()
    }

    #[inline]
    fn dot_product(a: &[f64], b: &[Vec<f64>]) -> Vec<f64> {
        let min_len = a.len().min(b.len());
        (0..b.len())
            .map(|i| {
                let mut sum = 0.0;
                for j in 0..min_len {
                    sum += a[j] * b[i][j];
                }
                sum
            })
            .collect()
    }

    #[inline]
    fn dot_product_single(a: &[f64], b: &[f64]) -> f64 {
        let min_len = a.len().min(b.len());
        let mut sum = 0.0;
        for j in 0..min_len {
            sum += a[j] * b[j];
        }
        sum
    }

    #[inline]
    fn concatenate_heads(head_outputs: &[Vec<Vec<f64>>], _d_v: usize) -> Vec<Vec<f64>> {
        if head_outputs.is_empty() {
            return Vec::new();
        }

        let seq_len = head_outputs[0].len();

        (0..seq_len)
            .map(|i| {
                head_outputs
                    .iter()
                    .flat_map(|head| head[i].clone())
                    .collect()
            })
            .collect()
    }

    #[inline]
    fn layer_norm_add(
        output: &[Vec<f64>],
        residual: &[Vec<f64>],
        gamma: &[f64],
        beta: &[f64],
        eps: f64,
    ) -> Vec<Vec<f64>> {
        assert_eq!(
            output.len(),
            residual.len(),
            "Output and residual must have same sequence length"
        );

        output
            .iter()
            .zip(residual.iter())
            .map(|(out, res)| {
                let added: Vec<f64> = out.iter().zip(res.iter()).map(|(o, r)| o + r).collect();
                Self::layer_norm(&added, gamma, beta, eps)
            })
            .collect()
    }

    #[inline]
    fn layer_norm(input: &[f64], gamma: &[f64], beta: &[f64], eps: f64) -> Vec<f64> {
        let n = input.len() as f64;
        let mean: f64 = input.iter().sum::<f64>() / n;
        let variance: f64 = input
            .iter()
            .map(|x| (x - mean).powi(2))
            .sum::<f64>()
            / n;

        let std = (variance + eps).sqrt();

        input
            .iter()
            .zip(gamma.iter())
            .zip(beta.iter())
            .map(|((x, g), b)| g * ((x - mean) / std) + b)
            .collect()
    }
}

/// Scaled dot-product attention
#[inline]
pub fn scaled_dot_product_attention(
    q: &[Vec<f64>],
    k: &[Vec<f64>],
    v: &[Vec<f64>],
    d_k: usize,
    dropout: f64,
    training: bool,
) -> Vec<Vec<f64>> {
    let seq_len_q = q.len();
    let seq_len_k = k.len();
    let seq_len_v = v.len();

    if seq_len_q == 0 || seq_len_k == 0 || seq_len_k != seq_len_v {
        return Vec::new();
    }

    let scale = 1.0 / (d_k as f64).sqrt();

    let mut attention_scores: Vec<Vec<f64>> = Vec::with_capacity(seq_len_q);

    for qi in q {
        let mut scores: Vec<f64> = Vec::with_capacity(seq_len_k);
        for kj in k {
            let mut score = 0.0;
            for d in 0..d_k {
                score += qi[d] * kj[d];
            }
            scores.push(score * scale);
        }
        attention_scores.push(scores);
    }

    let mut attention_weights: Vec<Vec<f64>> = Vec::with_capacity(seq_len_q);

    for scores in &attention_scores {
        let max_score = scores.iter().fold(f64::MIN, |m, s| m.max(*s));
        let exp_scores: Vec<f64> = scores.iter().map(|s| (s - max_score).exp()).collect();
        let sum_exp: f64 = exp_scores.iter().sum();
        let softmax: Vec<f64> = exp_scores.iter().map(|e| e / sum_exp).collect();

        if training && dropout > 0.0 {
            let mut rng = rand::thread_rng();
            attention_weights.push(
                softmax
                    .into_iter()
                    .map(|w| if rng.gen_bool(dropout) { 0.0 } else { w / (1.0 - dropout) })
                    .collect(),
            );
        } else {
            attention_weights.push(softmax);
        }
    }

    let d_v = if v.is_empty() { 0 } else { v[0].len() };
    let mut output: Vec<Vec<f64>> = Vec::with_capacity(seq_len_q);

    for (i, weights) in attention_weights.iter().enumerate() {
        let mut weighted_sum = vec![0.0; d_v];
        for (j, &weight) in weights.iter().enumerate() {
            if j < v.len() {
                for d in 0..d_v {
                    weighted_sum[d] += weight * v[j][d];
                }
            }
        }
        output.push(weighted_sum);
    }

    output
}

/// Cross-Asset Attention Wrapper
#[derive(Debug, Clone)]
pub struct CrossAssetAttention {
    attention: MultiHeadAttention,
    num_assets: usize,
    feature_dim: usize,
}

impl CrossAssetAttention {
    pub fn new(num_assets: usize, feature_dim: usize, num_heads: usize) -> Self {
        let d_model = feature_dim * num_heads;
        let config = AttentionConfig {
            num_heads,
            d_model,
            dropout: 0.1,
        };

        Self {
            attention: MultiHeadAttention::new(&config),
            num_assets,
            feature_dim,
        }
    }

    pub fn process(&self, asset_features: &[Vec<f64>]) -> Vec<Vec<f64>> {
        assert_eq!(
            asset_features.len(),
            self.num_assets,
            "Expected {} asset features, got {}",
            self.num_assets,
            asset_features.len()
        );

        let attended = self.attention.self_attention(asset_features);

        let mut enriched: Vec<Vec<f64>> = Vec::with_capacity(self.num_assets);
        for (original, attended) in asset_features.iter().zip(attended.iter()) {
            let mixed: Vec<f64> = original
                .iter()
                .zip(attended.iter())
                .map(|(&orig, &att)| orig * 0.7 + att * 0.3)
                .collect();
            enriched.push(mixed);
        }

        enriched
    }

    pub fn attention_scores(&self, asset_features: &[Vec<f64>]) -> Vec<Vec<f64>> {
        self.attention.attention_weights(asset_features, asset_features)
    }
}

/// Compute mean pooling of a sequence
#[inline]
pub fn mean_pooling(sequences: &[Vec<f64>]) -> Vec<f64> {
    if sequences.is_empty() {
        return Vec::new();
    }

    let seq_len = sequences.len();
    let feat_dim = sequences[0].len();

    (0..feat_dim)
        .map(|i| sequences.iter().map(|s| s[i]).sum::<f64>() / seq_len as f64)
        .collect()
}

/// Compute max pooling of a sequence
#[inline]
pub fn max_pooling(sequences: &[Vec<f64>]) -> Vec<f64> {
    if sequences.is_empty() {
        return Vec::new();
    }

    let feat_dim = sequences[0].len();

    (0..feat_dim)
        .map(|i| sequences.iter().map(|s| s[i]).fold(f64::MIN, f64::max))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_attention_config_default() {
        let config = AttentionConfig::default();
        assert_eq!(config.num_heads, 4);
        assert_eq!(config.d_model, 64);
        assert_eq!(config.dropout, 0.1);
    }

    #[test]
    fn test_attention_config_validation() {
        let mut config = AttentionConfig::default();
        assert!(config.validate().is_ok());

        config.num_heads = 0;
        assert!(config.validate().is_err());

        config = AttentionConfig::default();
        config.d_model = 65;
        assert!(config.validate().is_err());

        config = AttentionConfig::default();
        config.dropout = 1.5;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_single_head_attention() {
        let config = AttentionConfig {
            num_heads: 1,
            d_model: 8,
            dropout: 0.0,
        };
        let attention = MultiHeadAttention::new(&config);

        let queries = vec![vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]];
        let keys = vec![vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]];
        let values = vec![vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]];

        let output = attention.forward(&queries, &keys, &values);
        assert_eq!(output.len(), 1);
        assert_eq!(output[0].len(), 8);
    }

    #[test]
    fn test_multi_head_attention() {
        let config = AttentionConfig {
            num_heads: 4,
            d_model: 64,
            dropout: 0.0,
        };
        let attention = MultiHeadAttention::new(&config);

        let seq_len = 3;
        let queries: Vec<Vec<f64>> = (0..seq_len).map(|_| vec![0.1; 64]).collect();
        let keys: Vec<Vec<f64>> = (0..seq_len).map(|_| vec![0.1; 64]).collect();
        let values: Vec<Vec<f64>> = (0..seq_len).map(|_| vec![0.2; 64]).collect();

        let output = attention.forward(&queries, &keys, &values);
        assert_eq!(output.len(), seq_len);
        assert_eq!(output[0].len(), 64);

        for vec in &output {
            for &val in vec {
                assert!(val.is_finite(), "Output contains NaN or Inf");
            }
        }
    }

    #[test]
    fn test_self_attention() {
        let config = AttentionConfig {
            num_heads: 4,
            d_model: 32,
            dropout: 0.0,
        };
        let attention = MultiHeadAttention::new(&config);

        let input: Vec<Vec<f64>> = (0..5).map(|i| vec![i as f64; 32]).collect();
        let output = attention.self_attention(&input);
        assert_eq!(output.len(), 5);
        assert_eq!(output[0].len(), 32);
    }

    #[test]
    fn test_attention_weights() {
        let config = AttentionConfig {
            num_heads: 2,
            d_model: 16,
            dropout: 0.0,
        };
        let attention = MultiHeadAttention::new(&config);

        let queries = vec![vec![1.0; 16], vec![0.5; 16]];
        let keys = vec![vec![1.0; 16], vec![0.5; 16], vec![0.0; 16]];

        let weights = attention.attention_weights(&queries, &keys);
        assert_eq!(weights.len(), 2);
        assert_eq!(weights[0].len(), 3);

        for row in &weights {
            let sum: f64 = row.iter().sum();
            assert!((sum - 1.0).abs() < 1e-5, "Attention weights should sum to 1");
        }
    }

    #[test]
    fn test_cross_asset_attention() {
        let cross_attention = CrossAssetAttention::new(3, 24, 4);

        let asset1 = vec![0.5; 24];
        let asset2 = vec![0.3; 24];
        let asset3 = vec![0.7; 24];
        let asset_features = vec![asset1, asset2, asset3];

        let enriched = cross_attention.process(&asset_features);
        assert_eq!(enriched.len(), 3);
        assert_eq!(enriched[0].len(), 24);
    }

    #[test]
    fn test_cross_asset_attention_scores() {
        let cross_attention = CrossAssetAttention::new(3, 16, 2);

        let features = vec![
            vec![1.0; 16],
            vec![0.5; 16],
            vec![0.0; 16],
        ];

        let scores = cross_attention.attention_scores(&features);
        assert_eq!(scores.len(), 3);
        assert_eq!(scores[0].len(), 3);

        for i in 0..3 {
            let sum: f64 = scores[i].iter().sum();
            assert!((sum - 1.0).abs() < 1e-5);
        }
    }

    #[test]
    fn test_layer_normalization() {
        let input = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let gamma = vec![1.0; 5];
        let beta = vec![0.0; 5];
        let eps = 1e-5;

        let output = MultiHeadAttention::layer_norm(&input, &gamma, &beta, eps);

        let mean: f64 = output.iter().sum::<f64>() / output.len() as f64;
        assert!(mean.abs() < 0.01, "Layer norm output should have mean ~0");

        let variance: f64 = output.iter().map(|x| x.powi(2)).sum::<f64>() / output.len() as f64;
        assert!((variance - 1.0).abs() < 0.1, "Layer norm output should have variance ~1");
    }

    #[test]
    fn test_residual_connection() {
        let config = AttentionConfig {
            num_heads: 2,
            d_model: 16,
            dropout: 0.0,
        };
        let attention = MultiHeadAttention::new(&config);

        let input: Vec<Vec<f64>> = (0..2).map(|i| vec![i as f64; 16]).collect();
        let output = attention.self_attention(&input);

        for (inp, out) in input.iter().zip(output.iter()) {
            for (i, o) in out.iter().enumerate() {
                assert!((o - inp[i]).abs() > 1e-10, "Residual should be applied");
            }
        }
    }

    #[test]
    fn test_scaled_dot_product_attention() {
        // Test with identical queries and keys - should produce valid attention output
        let q = vec![vec![1.0, 0.0], vec![0.0, 1.0]];
        let k = vec![vec![1.0, 0.0], vec![0.0, 1.0]];
        let v = vec![vec![1.0, 2.0], vec![3.0, 4.0]];

        let output = scaled_dot_product_attention(&q, &k, &v, 2, 0.0, false);

        assert_eq!(output.len(), 2);
        assert_eq!(output[0].len(), 2);

        // Verify output values are valid (not NaN or Inf)
        for row in &output {
            for &val in row {
                assert!(val.is_finite(), "Output contains NaN or Inf");
            }
        }

        // Verify output is a convex combination of values
        // (attention weights sum to 1)
        assert!(output[0][0] >= 1.0 && output[0][0] <= 3.0);
        assert!(output[0][1] >= 2.0 && output[0][1] <= 4.0);
    }

    #[test]
    fn test_mean_pooling() {
        let sequences = vec![
            vec![1.0, 2.0],
            vec![3.0, 4.0],
            vec![5.0, 6.0],
        ];

        let pooled = mean_pooling(&sequences);
        assert_eq!(pooled.len(), 2);
        assert!((pooled[0] - 3.0).abs() < 1e-10);
        assert!((pooled[1] - 4.0).abs() < 1e-10);
    }

    #[test]
    fn test_max_pooling() {
        let sequences = vec![
            vec![1.0, 2.0],
            vec![3.0, 4.0],
            vec![5.0, 6.0],
        ];

        let pooled = max_pooling(&sequences);
        assert_eq!(pooled.len(), 2);
        assert!((pooled[0] - 5.0).abs() < 1e-10);
        assert!((pooled[1] - 6.0).abs() < 1e-10);
    }

    #[test]
    fn test_different_sequence_lengths() {
        let config = AttentionConfig {
            num_heads: 2,
            d_model: 16,
            dropout: 0.0,
        };
        let attention = MultiHeadAttention::new(&config);

        let queries = vec![vec![1.0; 16]];
        let keys = vec![vec![1.0; 16], vec![0.5; 16], vec![0.0; 16]];
        let values = vec![vec![2.0; 16], vec![1.0; 16], vec![0.0; 16]];

        let output = attention.forward(&queries, &keys, &values);
        assert_eq!(output.len(), 1);
        assert_eq!(output[0].len(), 16);
    }

    #[test]
    fn test_empty_sequences() {
        let config = AttentionConfig {
            num_heads: 2,
            d_model: 16,
            dropout: 0.0,
        };
        let attention = MultiHeadAttention::new(&config);

        let empty: Vec<Vec<f64>> = vec![];
        let non_empty = vec![vec![1.0; 16]];

        let output = attention.forward(&empty, &non_empty, &non_empty);
        assert!(output.is_empty());
    }
}
