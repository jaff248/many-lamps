//! Core type definitions used throughout the trading system.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Price tick: price × 10000 (0.0001 precision)
/// e.g., 0.55 = 5500, 0.01 = 100, 1.00 = 10000
pub type Tick = u16;

/// Size in micro-shares (10^-6 outcome tokens)
/// 1.0 share = 1_000_000 micro-shares
pub type Size = u64;

/// USDC amount in micro-units (10^-6)
pub type UsdcAmount = u64;

/// Maximum valid tick (price = 1.0000)
pub const MAX_TICK: Tick = 10000;

/// Minimum valid tick (price = 0.0000)  
pub const MIN_TICK: Tick = 0;

/// Conversion factor from decimal to micro-units
pub const SIZE_DECIMALS: u64 = 1_000_000;

/// Order side
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Side {
    Buy,
    Sell,
}

impl Side {
    pub fn opposite(&self) -> Self {
        match self {
            Side::Buy => Side::Sell,
            Side::Sell => Side::Buy,
        }
    }
}

impl fmt::Display for Side {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Side::Buy => write!(f, "BUY"),
            Side::Sell => write!(f, "SELL"),
        }
    }
}

/// Token type for binary markets
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Token {
    Yes,
    No,
}

impl Token {
    pub fn opposite(&self) -> Self {
        match self {
            Token::Yes => Token::No,
            Token::No => Token::Yes,
        }
    }
}

/// Unique identifier for client orders (deterministic for idempotency)
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ClientOrderId(pub String);

impl fmt::Display for ClientOrderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Market identifier (condition_id from Polymarket)
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MarketId(pub String);

impl fmt::Display for MarketId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Token identifier (asset_id from Polymarket - long integer as string)
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TokenId(pub String);

impl fmt::Display for TokenId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Strategy identifier
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StrategyId(pub String);

/// Reason codes for order actions (debuggability)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderReason {
    /// Initial quote placement
    QuoteRefresh,
    /// Price moved, need to requote
    TickMove,
    /// Inventory changed, adjusting skew
    InventorySkew,
    /// Volatility gate triggered
    VolGate,
    /// Market expiry approaching
    WindDown,
    /// Book resync required
    Resync,
    /// Periodic reconciliation
    Reconcile,
    /// Avoiding self-trade
    SelfTradeAvoid,
    /// Risk limit breached
    RiskKill,
    /// Manual intervention
    Manual,
    /// Fill occurred, reposting
    PostFill,
    /// Strategy signal
    Signal,
    /// System entering SAFE_MODE
    SafeMode,
    /// Order timeout
    Timeout,
}

/// Parse price string to tick (strict - returns error on invalid)
/// "0.55" → Ok(5500), ".48" → Ok(4800)
pub fn parse_price_to_tick_strict(s: &str, tick_size: Tick) -> Result<Tick, TickParseError> {
    let f: f64 = s.parse().map_err(|_| TickParseError::InvalidFormat(s.to_string()))?;
    
    if f < 0.0 || f > 1.0 {
        return Err(TickParseError::OutOfRange(f));
    }
    
    let raw_tick = (f * 10000.0).round() as Tick;
    
    // STRICT: Never snap - if tick is invalid, return error
    if tick_size > 0 && raw_tick % tick_size != 0 {
        return Err(TickParseError::InvalidTick { 
            raw_tick, 
            tick_size,
            price_str: s.to_string(),
        });
    }
    
    Ok(raw_tick)
}

/// Parse price string to tick for OUR quote generation (snapping allowed)
pub fn parse_price_to_tick_snap(price: f64, tick_size: Tick) -> Tick {
    let raw_tick = (price * 10000.0).round() as Tick;
    if tick_size == 0 {
        return raw_tick;
    }
    // Snap to nearest valid tick
    let tick_size_u32 = tick_size as u32;
    let half = tick_size_u32 / 2;
    (((raw_tick as u32 + half) / tick_size_u32) * tick_size_u32) as Tick
}

/// Convert tick to price string for API/hashing
pub fn tick_to_price_string(tick: Tick) -> String {
    let price = tick as f64 / 10000.0;
    format!("{:.4}", price).trim_end_matches('0').trim_end_matches('.').to_string()
}

/// Parse size string to micro-shares
/// "30" → 30_000_000, "1.5" → 1_500_000
pub fn parse_size(s: &str) -> Result<Size, SizeParseError> {
    let f: f64 = s.parse().map_err(|_| SizeParseError::InvalidFormat(s.to_string()))?;
    if f < 0.0 {
        return Err(SizeParseError::Negative(f));
    }
    Ok((f * SIZE_DECIMALS as f64).round() as Size)
}

/// Convert micro-shares to size string
pub fn size_to_string(size: Size) -> String {
    let f = size as f64 / SIZE_DECIMALS as f64;
    format!("{}", f)
}

#[derive(Debug, thiserror::Error)]
pub enum TickParseError {
    #[error("Invalid price format: {0}")]
    InvalidFormat(String),
    #[error("Price out of range [0,1]: {0}")]
    OutOfRange(f64),
    #[error("Invalid tick {raw_tick} for tick_size {tick_size} (price: {price_str})")]
    InvalidTick {
        raw_tick: Tick,
        tick_size: Tick,
        price_str: String,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum SizeParseError {
    #[error("Invalid size format: {0}")]
    InvalidFormat(String),
    #[error("Negative size: {0}")]
    Negative(f64),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_price_strict_valid() {
        assert_eq!(parse_price_to_tick_strict("0.55", 100).unwrap(), 5500);
        assert_eq!(parse_price_to_tick_strict(".48", 100).unwrap(), 4800);
        assert_eq!(parse_price_to_tick_strict("0.01", 100).unwrap(), 100);
        assert_eq!(parse_price_to_tick_strict("1", 100).unwrap(), 10000);
        assert_eq!(parse_price_to_tick_strict("0", 100).unwrap(), 0);
    }

    #[test]
    fn test_parse_price_strict_invalid_tick() {
        // 0.551 = tick 5510, which is not divisible by 100
        let result = parse_price_to_tick_strict("0.551", 100);
        assert!(matches!(result, Err(TickParseError::InvalidTick { .. })));
    }

    #[test]
    fn test_parse_price_snap() {
        // Snapping rounds to nearest valid tick
        assert_eq!(parse_price_to_tick_snap(0.555, 100), 5600); // 5550 → 5600
        assert_eq!(parse_price_to_tick_snap(0.554, 100), 5500);
        assert_eq!(parse_price_to_tick_snap(0.556, 100), 5600);
    }

    #[test]
    fn test_tick_to_price_string() {
        assert_eq!(tick_to_price_string(5500), "0.55");
        assert_eq!(tick_to_price_string(100), "0.01");
        assert_eq!(tick_to_price_string(10000), "1");
        assert_eq!(tick_to_price_string(0), "0");
    }

    #[test]
    fn test_parse_size() {
        assert_eq!(parse_size("30").unwrap(), 30_000_000);
        assert_eq!(parse_size("1.5").unwrap(), 1_500_000);
        assert_eq!(parse_size("0.000001").unwrap(), 1);
    }
}
