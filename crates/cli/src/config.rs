//! Configuration loading and management.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Top-level configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Gateway configuration.
    #[serde(default)]
    pub gateway: GatewayConfig,

    /// Strategy configuration.
    #[serde(default)]
    pub strategy: StrategyConfig,

    /// Risk configuration.
    #[serde(default)]
    pub risk: RiskConfig,

    /// Recording configuration.
    #[serde(default)]
    pub recording: RecordingConfig,

    /// Safe mode configuration.
    #[serde(default)]
    pub safe_mode: SafeModeConfig,
}

/// Gateway connection configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayConfig {
    /// WebSocket URL.
    #[serde(default = "default_ws_url")]
    pub ws_url: String,

    /// REST API URL.
    #[serde(default = "default_rest_url")]
    pub rest_url: String,

    /// Reconnection attempts.
    #[serde(default = "default_reconnect_attempts")]
    pub reconnect_attempts: u32,

    /// Reconnection delay (milliseconds).
    #[serde(default = "default_reconnect_delay_ms")]
    pub reconnect_delay_ms: u64,

    /// Request timeout (milliseconds).
    #[serde(default = "default_request_timeout_ms")]
    pub request_timeout_ms: u64,
}

fn default_ws_url() -> String {
    "wss://ws-subscriptions-clob.polymarket.com/ws/market".into()
}

fn default_rest_url() -> String {
    "https://clob.polymarket.com".into()
}

fn default_reconnect_attempts() -> u32 {
    5
}

fn default_reconnect_delay_ms() -> u64 {
    1000
}

fn default_request_timeout_ms() -> u64 {
    5000
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            ws_url: default_ws_url(),
            rest_url: default_rest_url(),
            reconnect_attempts: default_reconnect_attempts(),
            reconnect_delay_ms: default_reconnect_delay_ms(),
            request_timeout_ms: default_request_timeout_ms(),
        }
    }
}

/// Strategy configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyConfig {
    /// Tick size in basis points (100 = $0.01).
    #[serde(default = "default_tick_size_bps")]
    pub tick_size_bps: u16,

    /// Half-spread in basis points (tick = 0.0001).
    #[serde(default = "default_half_spread_bps")]
    pub half_spread_bps: u16,

    /// Quote size in micro-shares.
    #[serde(default = "default_quote_size_shares")]
    pub quote_size_shares: u64,

    /// Number of levels to quote.
    #[serde(default = "default_num_levels")]
    pub num_levels: u8,

    /// Skew factor.
    #[serde(default = "default_skew_factor")]
    pub skew_factor: f64,

    /// Requote threshold (bps ticks).
    #[serde(default = "default_requote_threshold_bps")]
    pub requote_threshold_bps: u16,

    /// Minimum edge (bps ticks).
    #[serde(default = "default_min_edge_bps")]
    pub min_edge_bps: u16,
}

fn default_tick_size_bps() -> u16 {
    100
}

fn default_half_spread_bps() -> u16 {
    2
}

fn default_quote_size_shares() -> u64 {
    1_000_000 // 1 share
}

fn default_num_levels() -> u8 {
    3
}

fn default_skew_factor() -> f64 {
    0.5
}

fn default_requote_threshold_bps() -> u16 {
    1
}

fn default_min_edge_bps() -> u16 {
    1
}

impl Default for StrategyConfig {
    fn default() -> Self {
        Self {
            tick_size_bps: default_tick_size_bps(),
            half_spread_bps: default_half_spread_bps(),
            quote_size_shares: default_quote_size_shares(),
            num_levels: default_num_levels(),
            skew_factor: default_skew_factor(),
            requote_threshold_bps: default_requote_threshold_bps(),
            min_edge_bps: default_min_edge_bps(),
        }
    }
}

/// Risk configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskConfig {
    /// Maximum position size (micro-shares).
    #[serde(default = "default_max_position")]
    pub max_position: i64,

    /// Maximum daily loss (micro USDC).
    #[serde(default = "default_max_daily_loss")]
    pub max_daily_loss: i64,

    /// Maximum drawdown percentage.
    #[serde(default = "default_max_drawdown_pct")]
    pub max_drawdown_pct: f64,

    /// Maximum open orders.
    #[serde(default = "default_max_open_orders")]
    pub max_open_orders: usize,

    /// Fee rate BPS for 15-min markets.
    #[serde(default = "default_fee_rate_bps")]
    pub fee_rate_bps: u32,
}

fn default_max_position() -> i64 {
    1_000_000_000 // 1000 USDC
}

fn default_max_daily_loss() -> i64 {
    100_000_000 // 100 USDC
}

fn default_max_drawdown_pct() -> f64 {
    0.10
}

fn default_max_open_orders() -> usize {
    20
}

fn default_fee_rate_bps() -> u32 {
    1000 // 10% for 15-min markets
}

impl Default for RiskConfig {
    fn default() -> Self {
        Self {
            max_position: default_max_position(),
            max_daily_loss: default_max_daily_loss(),
            max_drawdown_pct: default_max_drawdown_pct(),
            max_open_orders: default_max_open_orders(),
            fee_rate_bps: default_fee_rate_bps(),
        }
    }
}

/// Recording configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordingConfig {
    /// Output directory.
    #[serde(default = "default_recording_dir")]
    pub output_dir: String,

    /// Enable raw frame recording.
    #[serde(default)]
    pub record_raw_frames: bool,

    /// Enable event recording.
    #[serde(default = "default_true")]
    pub record_events: bool,

    /// Maximum events per file.
    #[serde(default = "default_max_events_per_file")]
    pub max_events_per_file: usize,
}

fn default_recording_dir() -> String {
    "data/recordings".into()
}

fn default_true() -> bool {
    true
}

fn default_max_events_per_file() -> usize {
    1_000_000
}

impl Default for RecordingConfig {
    fn default() -> Self {
        Self {
            output_dir: default_recording_dir(),
            record_raw_frames: false,
            record_events: true,
            max_events_per_file: default_max_events_per_file(),
        }
    }
}

/// Safe mode configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SafeModeConfig {
    /// Enable safe mode (no orders sent).
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Maximum consecutive errors before safe mode.
    #[serde(default = "default_max_consecutive_errors")]
    pub max_consecutive_errors: u32,

    /// Book drift threshold (ticks) before safe mode.
    #[serde(default = "default_book_drift_threshold")]
    pub book_drift_threshold: u16,

    /// Maximum message latency (ms) before safe mode.
    #[serde(default = "default_max_message_latency_ms")]
    pub max_message_latency_ms: u64,
}

fn default_max_consecutive_errors() -> u32 {
    3
}

fn default_book_drift_threshold() -> u16 {
    5
}

fn default_max_message_latency_ms() -> u64 {
    5000
}

impl Default for SafeModeConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_consecutive_errors: default_max_consecutive_errors(),
            book_drift_threshold: default_book_drift_threshold(),
            max_message_latency_ms: default_max_message_latency_ms(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            gateway: GatewayConfig::default(),
            strategy: StrategyConfig::default(),
            risk: RiskConfig::default(),
            recording: RecordingConfig::default(),
            safe_mode: SafeModeConfig::default(),
        }
    }
}

/// Load configuration from file.
pub fn load_config(path: &str) -> Result<Config> {
    let path = Path::new(path);

    if !path.exists() {
        tracing::warn!("Config file not found at {}, using defaults", path.display());
        return Ok(Config::default());
    }

    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read config file: {}", path.display()))?;

    let config: Config = toml::from_str(&contents)
        .with_context(|| format!("Failed to parse config file: {}", path.display()))?;

    Ok(config)
}

/// Save configuration to file.
pub fn save_config(config: &Config, path: &str) -> Result<()> {
    let contents = toml::to_string_pretty(config)?;
    std::fs::write(path, contents)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert!(config.safe_mode.enabled);
        assert_eq!(config.risk.fee_rate_bps, 1000);
    }

    #[test]
    fn test_config_roundtrip() {
        let config = Config::default();
        let serialized = toml::to_string_pretty(&config).unwrap();
        let deserialized: Config = toml::from_str(&serialized).unwrap();

        assert_eq!(config.gateway.ws_url, deserialized.gateway.ws_url);
        assert_eq!(config.risk.max_position, deserialized.risk.max_position);
    }
}
