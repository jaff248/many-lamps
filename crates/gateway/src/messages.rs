//! Polymarket WebSocket message types.
//!
//! These types match the exact JSON schema from Polymarket's CLOB WebSocket API.
//! Reference: https://docs.polymarket.com/#websocket-api

use serde::{Deserialize, Serialize};

/// Top-level WebSocket message wrapper.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum WsMessage {
    /// Array of market events (typical format)
    Events(Vec<MarketEvent>),
    /// Single event
    SingleEvent(MarketEvent),
    /// Heartbeat/ping
    Heartbeat(HeartbeatMsg),
    /// Error message
    Error(ErrorMsg),
}

/// Heartbeat message from server.
#[derive(Debug, Clone, Deserialize)]
pub struct HeartbeatMsg {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub timestamp: Option<u64>,
}

/// Error message from server.
#[derive(Debug, Clone, Deserialize)]
pub struct ErrorMsg {
    pub error: String,
    pub code: Option<i32>,
}

/// Market event from WebSocket feed.
#[derive(Debug, Clone, Deserialize)]
pub struct MarketEvent {
    /// Event type: "book", "price_change", "last_trade_price", "tick_size_change"
    pub event_type: String,
    /// Asset ID (token ID)
    pub asset_id: String,
    /// Market identifier (condition ID)
    #[serde(default)]
    pub market: Option<String>,
    /// Event timestamp from exchange (milliseconds)
    #[serde(default)]
    pub timestamp: Option<u64>,
    /// Hash of the order book state (for book events)
    #[serde(default)]
    pub hash: Option<String>,
    /// Book data (for book events)
    #[serde(default)]
    pub bids: Option<Vec<PriceLevel>>,
    #[serde(default)]
    pub asks: Option<Vec<PriceLevel>>,
    /// Price change data (book deltas)
    #[serde(default)]
    pub price_changes: Option<Vec<PriceChangeLevel>>,
    /// Legacy fields (some event types may still use price/side)
    #[serde(default)]
    pub price: Option<String>,
    #[serde(default)]
    pub side: Option<String>,
    /// Tick size change data
    #[serde(default)]
    pub old_tick_size: Option<String>,
    #[serde(default)]
    pub new_tick_size: Option<String>,
    /// Trade data
    #[serde(default)]
    pub size: Option<String>,
    #[serde(default)]
    pub last_trade_price: Option<String>,
}

/// Price level in order book.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PriceLevel {
    /// Price as string (e.g., "0.55")
    pub price: String,
    /// Size as string (e.g., "1000.5")
    pub size: String,
}

/// Price change entry (aggregate size at a price level).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PriceChangeLevel {
    pub price: String,
    pub side: String,
    pub size: String,
}

/// Parsed book snapshot event.
#[derive(Debug, Clone)]
pub struct BookSnapshot {
    pub asset_id: String,
    pub timestamp_ms: u64,
    pub hash: String,
    pub bids: Vec<(u16, u64)>, // (tick, size in micro-shares)
    pub asks: Vec<(u16, u64)>,
}

/// Parsed price change event.
#[derive(Debug, Clone)]
pub struct PriceChange {
    pub asset_id: String,
    pub timestamp_ms: u64,
    pub price_changes: Vec<ParsedPriceChange>,
}

/// Parsed price change entry (new size at level).
#[derive(Debug, Clone)]
pub struct ParsedPriceChange {
    pub price_tick: u16,
    pub side: crate::Side,
    pub size: u64,
}

/// Parsed last trade price event.
#[derive(Debug, Clone)]
pub struct LastTradePrice {
    pub asset_id: String,
    pub timestamp_ms: u64,
    pub price_tick: u16,
    pub size_shares: u64,
}

/// Parsed tick size change event.
#[derive(Debug, Clone)]
pub struct TickSizeChange {
    pub asset_id: String,
    pub timestamp_ms: u64,
    pub old_tick_bps: u16, // e.g., 10 for 0.001, 100 for 0.01
    pub new_tick_bps: u16,
}

/// Subscription request message.
#[derive(Debug, Clone, Serialize)]
pub struct SubscribeRequest {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub channel: String,
    pub assets_ids: Vec<String>,
}

impl SubscribeRequest {
    /// Create a market subscription for book events.
    pub fn market(asset_ids: Vec<String>) -> Self {
        Self {
            msg_type: "subscribe".to_string(),
            channel: "market".to_string(),
            assets_ids: asset_ids,
        }
    }
}

/// Unsubscription request message.
#[derive(Debug, Clone, Serialize)]
pub struct UnsubscribeRequest {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub channel: String,
    pub assets_ids: Vec<String>,
}

impl UnsubscribeRequest {
    pub fn market(asset_ids: Vec<String>) -> Self {
        Self {
            msg_type: "unsubscribe".to_string(),
            channel: "market".to_string(),
            assets_ids: asset_ids,
        }
    }
}

/// REST API book snapshot response.
#[derive(Debug, Clone, Deserialize)]
pub struct RestBookSnapshot {
    pub market: Option<String>,
    pub asset_id: String,
    pub hash: String,
    pub timestamp: Option<u64>,
    pub bids: Vec<PriceLevel>,
    pub asks: Vec<PriceLevel>,
}

/// REST API market info.
#[derive(Debug, Clone, Deserialize)]
pub struct RestMarketInfo {
    pub condition_id: String,
    pub tokens: Vec<RestTokenInfo>,
    pub minimum_order_size: Option<String>,
    pub minimum_tick_size: Option<String>,
    pub active: bool,
    pub closed: bool,
    pub rewards: Option<RestRewardsInfo>,
}

/// Token info from REST API.
#[derive(Debug, Clone, Deserialize)]
pub struct RestTokenInfo {
    pub token_id: String,
    pub outcome: String,
    pub winner: bool,
}

/// Rewards info from REST API.
#[derive(Debug, Clone, Deserialize)]
pub struct RestRewardsInfo {
    pub rates: Option<Vec<RestRateInfo>>,
    pub min_size: Option<String>,
    pub max_spread: Option<String>,
}

/// Rate info for maker rewards.
#[derive(Debug, Clone, Deserialize)]
pub struct RestRateInfo {
    pub asset_id: String,
    pub rewards_daily_rate: Option<String>,
}

// Re-export Side for convenience
pub use mtrader_core::Side;
