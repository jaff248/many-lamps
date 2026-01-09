//! Event types for the deterministic core loop.
//!
//! All events flow through the core loop in strict order.
//! Events include timing information for latency analysis and replay.

use crate::health::SafeModeReason;
use crate::types::*;
use serde::{Deserialize, Serialize};

/// Timestamps attached to every event
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct EventTimestamps {
    /// Exchange timestamp from WS message (milliseconds since epoch)
    pub ts_exchange_ms: i64,
    /// Local monotonic timestamp when bytes received (nanoseconds since process start)
    pub ts_recv_mono_ns: i64,
    /// Local monotonic timestamp when event processed (nanoseconds since process start)
    pub ts_process_mono_ns: i64,
}

impl EventTimestamps {
    pub fn new(ts_exchange_ms: i64, ts_recv_mono_ns: i64) -> Self {
        Self {
            ts_exchange_ms,
            ts_recv_mono_ns,
            ts_process_mono_ns: 0, // Set when processed
        }
    }

    pub fn with_process_time(mut self, ts_process_mono_ns: i64) -> Self {
        self.ts_process_mono_ns = ts_process_mono_ns;
        self
    }

    /// Calculate WS lag (time from exchange to receive)
    pub fn ws_lag_ms(&self, local_wall_clock_ms: i64) -> i64 {
        local_wall_clock_ms - self.ts_exchange_ms
    }

    /// Calculate internal processing latency
    pub fn processing_latency_ns(&self) -> i64 {
        self.ts_process_mono_ns - self.ts_recv_mono_ns
    }
}

/// Core event enum - all events flow through the deterministic loop
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CoreEvent {
    /// Market data events from gateway
    MarketData(MarketDataEvent),
    /// Strategy signals
    Signal(StrategySignal),
    /// Order intents (requests to place/cancel)
    OrderIntent(OrderIntent),
    /// Order acknowledgements from execution
    OrderAck(OrderAck),
    /// Fill events
    Fill(FillEvent),
    /// Cancel acknowledgements
    CancelAck(CancelAck),
    /// Risk events
    Risk(RiskEvent),
    /// System events
    System(SystemEvent),
}

/// Market data events from WebSocket
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MarketDataEvent {
    /// Full book snapshot
    BookSnapshot {
        market_id: MarketId,
        token_id: TokenId,
        bids: Vec<(Tick, Size)>,
        asks: Vec<(Tick, Size)>,
        tick_size: Tick,
        snapshot_hash: String,
        timestamps: EventTimestamps,
    },
    /// Book delta (price level change)
    BookDelta {
        market_id: MarketId,
        token_id: TokenId,
        side: Side,
        tick: Tick,
        new_size: Size,
        best_bid: Option<Tick>,
        best_ask: Option<Tick>,
        order_hash: String, // Hash of the order, NOT book state
        timestamps: EventTimestamps,
    },
    /// Trade occurred
    Trade {
        market_id: MarketId,
        token_id: TokenId,
        side: Side,
        price_tick: Tick,
        size: Size,
        fee_rate_bps: u16,
        timestamps: EventTimestamps,
    },
    /// Tick size changed
    TickSizeChange {
        market_id: MarketId,
        token_id: TokenId,
        old_tick_size: Tick,
        new_tick_size: Tick,
        timestamps: EventTimestamps,
    },
    /// Best bid/ask update (auxiliary)
    BestBidAsk {
        market_id: MarketId,
        token_id: TokenId,
        best_bid: Option<Tick>,
        best_ask: Option<Tick>,
        spread: Option<Tick>,
        timestamps: EventTimestamps,
    },
    /// Connection status change
    ConnectionStatus {
        connected: bool,
        timestamp_mono_ns: i64,
    },
    /// Parse error (for recording)
    ParseError {
        raw_bytes: Vec<u8>,
        error: String,
        timestamp_mono_ns: i64,
    },
}

/// Strategy signals
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategySignal {
    pub strategy_id: StrategyId,
    pub market_id: MarketId,
    pub token_id: TokenId,
    pub signal_type: SignalType,
    pub timestamp_mono_ns: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SignalType {
    /// Quote on one or both sides
    Quote {
        bid: Option<QuoteLevel>,
        ask: Option<QuoteLevel>,
    },
    /// Cancel existing orders
    CancelAll { reason: OrderReason },
    /// Cross spread (taker)
    Cross {
        side: Side,
        price_tick: Tick,
        size: Size,
    },
    /// Bundle arb opportunity
    BundleArb {
        ask_yes: Tick,
        ask_no: Tick,
        size: Size,
        net_edge_bps: i16,
    },
    /// Pause strategy
    Pause { reason: OrderReason },
    /// Resume strategy  
    Resume,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuoteLevel {
    pub price_tick: Tick,
    pub size: Size,
}

/// Order intent - request to place an order
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderIntent {
    pub client_order_id: ClientOrderId,
    pub strategy_id: StrategyId,
    pub market_id: MarketId,
    pub token_id: TokenId,
    pub side: Side,
    pub price_tick: Tick,
    pub size: Size,
    pub order_type: OrderType,
    pub reason: OrderReason,
    pub timestamp_mono_ns: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderType {
    /// Good-til-cancelled, rests on book
    Gtc,
    /// Good-til-date with expiration
    Gtd { expiration_ts: u64 },
    /// Fill-or-kill
    Fok,
    /// Fill-and-kill (immediate-or-cancel)
    Fak,
    /// Post-only (reject if would cross)
    PostOnly,
}

/// Order acknowledgement
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderAck {
    pub client_order_id: ClientOrderId,
    pub exchange_order_id: Option<String>,
    pub status: OrderAckStatus,
    pub timestamp_mono_ns: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OrderAckStatus {
    /// Order accepted, resting on book
    Accepted,
    /// Order rejected
    Rejected { reason: String },
    /// Order timed out (no response)
    Timeout,
    /// Order matched immediately (for taker orders)
    Matched { fill_size: Size, fill_price: Tick },
}

/// Cancel intent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CancelIntent {
    pub client_order_id: ClientOrderId,
    pub reason: OrderReason,
    pub timestamp_mono_ns: i64,
}

/// Cancel acknowledgement
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CancelAck {
    pub client_order_id: ClientOrderId,
    pub status: CancelAckStatus,
    pub timestamp_mono_ns: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CancelAckStatus {
    /// Cancel successful
    Cancelled,
    /// Order not found (already filled or cancelled)
    NotFound,
    /// Cancel rejected
    Rejected { reason: String },
    /// Cancel timed out
    Timeout,
}

/// Fill event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FillEvent {
    pub client_order_id: ClientOrderId,
    pub exchange_order_id: String,
    pub exchange_trade_id: String,
    pub strategy_id: StrategyId,
    pub market_id: MarketId,
    pub token_id: TokenId,
    pub side: Side,
    pub price_tick: Tick,
    pub fill_size: Size,
    pub remaining_size: Size,
    pub is_maker: bool,
    pub fee_amount: u64, // In micro-units
    pub timestamps: EventTimestamps,
}

/// Risk events
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RiskEvent {
    /// Position limit reached
    PositionLimit {
        market_id: MarketId,
        token_id: TokenId,
        current: i64,
        limit: i64,
    },
    /// Notional limit reached
    NotionalLimit { current: u64, limit: u64 },
    /// Daily loss limit reached
    DailyLossLimit { current_pnl: i64, limit: i64 },
    /// Order rate limit
    OrderRateLimit { rate: u32, limit: u32 },
}

/// System events
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SystemEvent {
    /// System entering SAFE_MODE
    SafeMode {
        reasons: Vec<SafeModeReason>,
        timestamp_mono_ns: i64,
    },
    /// System recovered from SAFE_MODE
    SafeModeCleared { timestamp_mono_ns: i64 },
    /// Resnapshot requested
    ResnaphotRequested {
        token_id: TokenId,
        reason: String,
        timestamp_mono_ns: i64,
    },
    /// Market lifecycle event
    MarketLifecycle {
        market_id: MarketId,
        event: MarketLifecycleEvent,
        timestamp_mono_ns: i64,
    },
    /// Heartbeat (for liveness tracking)
    Heartbeat { timestamp_mono_ns: i64 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MarketLifecycleEvent {
    /// Market discovered and subscribed
    Subscribed,
    /// Market approaching expiry, stop quoting
    WindingDown,
    /// Market expired, cancelling orders
    Flattening,
    /// Switching to next market
    Switching { next_market_id: MarketId },
    /// Market unsubscribed
    Unsubscribed,
}

/// Wrapper for raw WS frame storage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawFrame {
    /// Raw bytes received
    pub bytes: Vec<u8>,
    /// Parse result
    pub parse_result: ParseResult,
    /// Receive timestamp
    pub timestamp_recv_mono_ns: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ParseResult {
    Success,
    Error { message: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_timestamps_lag() {
        let ts = EventTimestamps {
            ts_exchange_ms: 1000,
            ts_recv_mono_ns: 1_000_000_000,
            ts_process_mono_ns: 1_001_000_000,
        };

        assert_eq!(ts.ws_lag_ms(1050), 50);
        assert_eq!(ts.processing_latency_ns(), 1_000_000);
    }

    #[test]
    fn test_order_reason_serialization() {
        let reason = OrderReason::QuoteRefresh;
        let json = serde_json::to_string(&reason).unwrap();
        assert_eq!(json, "\"QuoteRefresh\"");
    }
}
