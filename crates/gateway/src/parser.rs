//! Message parser with raw frame recording.
//!
//! Parses Polymarket WebSocket messages while preserving the raw bytes
//! for recording to Parquet. Never modifies inbound data.

use crate::error::GatewayError;
use crate::messages::{
    BookSnapshot, LastTradePrice, MarketEvent, PriceChange, PriceChangeUpdate, PriceLevel,
    TickSizeChange, WsMessage,
};
use mtrader_core::{parse_price_to_tick_strict, parse_size, Side, Size, Tick};

/// Parsed event with raw frame preserved.
#[derive(Debug, Clone)]
pub struct ParsedFrame {
    /// Monotonic timestamp when frame was received (nanoseconds)
    pub ts_recv_mono_ns: u64,
    /// Raw JSON bytes (for recording)
    pub raw_json: Vec<u8>,
    /// Parsed events
    pub events: Vec<ParsedEvent>,
}

/// A single parsed event from a WebSocket frame.
#[derive(Debug, Clone)]
pub enum ParsedEvent {
    Book(BookSnapshot),
    PriceChange(PriceChange),
    LastTradePrice(LastTradePrice),
    TickSizeChange(TickSizeChange),
    Heartbeat,
    Unknown(String),
}

/// Parser for Polymarket WebSocket messages.
pub struct Parser {
    /// Current tick size in basis points (e.g., 100 = 0.01)
    tick_size_bps: u16,
}

impl Parser {
    pub fn new() -> Self {
        Self {
            // Default to 1 cent ticks
            tick_size_bps: 100,
        }
    }

    /// Set the tick size for price parsing.
    pub fn set_tick_size_bps(&mut self, tick_size_bps: u16) {
        self.tick_size_bps = tick_size_bps;
    }

    /// Parse a raw WebSocket frame.
    ///
    /// Records the raw bytes and returns parsed events.
    pub fn parse_frame(
        &self,
        raw_bytes: &[u8],
        ts_recv_mono_ns: u64,
    ) -> Result<ParsedFrame, GatewayError> {
        let events = self.parse_json(raw_bytes)?;

        Ok(ParsedFrame {
            ts_recv_mono_ns,
            raw_json: raw_bytes.to_vec(),
            events,
        })
    }

    fn parse_json(&self, raw_bytes: &[u8]) -> Result<Vec<ParsedEvent>, GatewayError> {
        let msg: WsMessage = serde_json::from_slice(raw_bytes)?;

        let market_events = match msg {
            WsMessage::Events(events) => events,
            WsMessage::SingleEvent(event) => vec![event],
            WsMessage::Heartbeat(_) => return Ok(vec![ParsedEvent::Heartbeat]),
            WsMessage::Error(e) => {
                return Err(GatewayError::InvalidMessage(format!(
                    "Server error: {}",
                    e.error
                )));
            }
        };

        let mut parsed = Vec::with_capacity(market_events.len());
        for event in market_events {
            parsed.push(self.parse_market_event(event)?);
        }

        Ok(parsed)
    }

    fn parse_market_event(&self, event: MarketEvent) -> Result<ParsedEvent, GatewayError> {
        match event.event_type.as_str() {
            "book" => self.parse_book_event(event),
            "price_change" => self.parse_price_change(event),
            "last_trade_price" => self.parse_last_trade(event),
            "tick_size_change" => self.parse_tick_size_change(event),
            other => Ok(ParsedEvent::Unknown(other.to_string())),
        }
    }

    fn parse_book_event(&self, event: MarketEvent) -> Result<ParsedEvent, GatewayError> {
        let hash = event
            .hash
            .ok_or_else(|| GatewayError::InvalidMessage("Book event missing hash".into()))?;

        let bids = event.bids.unwrap_or_default();
        let asks = event.asks.unwrap_or_default();

        let parsed_bids = self.parse_price_levels(&bids)?;
        let parsed_asks = self.parse_price_levels(&asks)?;

        Ok(ParsedEvent::Book(BookSnapshot {
            asset_id: event.asset_id,
            timestamp_ms: event.timestamp.unwrap_or(0),
            hash,
            bids: parsed_bids,
            asks: parsed_asks,
        }))
    }

    fn parse_price_levels(&self, levels: &[PriceLevel]) -> Result<Vec<(u16, u64)>, GatewayError> {
        let mut result = Vec::with_capacity(levels.len());

        for level in levels {
            let tick = self.parse_price_to_tick(&level.price)?;
            let size = self.parse_size(&level.size)?;
            result.push((tick, size));
        }

        Ok(result)
    }

    fn parse_price_change(&self, event: MarketEvent) -> Result<ParsedEvent, GatewayError> {
        let price_changes = event.price_changes.ok_or_else(|| {
            GatewayError::InvalidMessage("Price change missing price_changes".into())
        })?;
        let mut changes = Vec::with_capacity(price_changes.len());
        for change in price_changes {
            let price_tick = self.parse_price_to_tick(&change.price)?;
            let side = parse_side(&change.side)?;
            let new_size = self.parse_size(&change.size)?;
            changes.push(PriceChangeUpdate {
                price_tick,
                side,
                new_size,
            });
        }

        Ok(ParsedEvent::PriceChange(PriceChange {
            asset_id: event.asset_id,
            timestamp_ms: event.timestamp.unwrap_or(0),
            changes,
        }))
    }

    fn parse_last_trade(&self, event: MarketEvent) -> Result<ParsedEvent, GatewayError> {
        let price_str = event
            .price
            .ok_or_else(|| GatewayError::InvalidMessage("Trade missing price".into()))?;
        let size_str = event
            .size
            .ok_or_else(|| GatewayError::InvalidMessage("Trade missing size".into()))?;

        let price_tick = self.parse_price_to_tick(&price_str)?;
        let size_shares = self.parse_size(&size_str)?;

        Ok(ParsedEvent::LastTradePrice(LastTradePrice {
            asset_id: event.asset_id,
            timestamp_ms: event.timestamp.unwrap_or(0),
            price_tick,
            size_shares,
        }))
    }

    fn parse_tick_size_change(&self, event: MarketEvent) -> Result<ParsedEvent, GatewayError> {
        let old_tick_str = event.old_tick_size.ok_or_else(|| {
            GatewayError::InvalidMessage("Tick size change missing old_tick_size".into())
        })?;
        let new_tick_str = event.new_tick_size.ok_or_else(|| {
            GatewayError::InvalidMessage("Tick size change missing new_tick_size".into())
        })?;

        let old_tick_bps = parse_tick_size_string(&old_tick_str)?;
        let new_tick_bps = parse_tick_size_string(&new_tick_str)?;

        Ok(ParsedEvent::TickSizeChange(TickSizeChange {
            asset_id: event.asset_id,
            timestamp_ms: event.timestamp.unwrap_or(0),
            old_tick_bps,
            new_tick_bps,
        }))
    }

    /// Parse a price string (e.g., "0.55") to tick.
    fn parse_price_to_tick(&self, price_str: &str) -> Result<Tick, GatewayError> {
        parse_price_to_tick_strict(price_str, self.tick_size_bps)
            .map_err(|e| GatewayError::InvalidMessage(format!("Invalid price: {}", e)))
    }

    /// Parse a size string (e.g., "1000.5") to shares.
    fn parse_size(&self, size_str: &str) -> Result<Size, GatewayError> {
        parse_size(size_str)
            .map_err(|e| GatewayError::InvalidMessage(format!("Invalid size: {}", e)))
    }
}

impl Default for Parser {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse side string to enum.
fn parse_side(side_str: &str) -> Result<Side, GatewayError> {
    match side_str.to_lowercase().as_str() {
        "buy" | "bid" => Ok(Side::Buy),
        "sell" | "ask" => Ok(Side::Sell),
        _ => Err(GatewayError::InvalidMessage(format!(
            "Invalid side: {}",
            side_str
        ))),
    }
}

/// Parse tick size string (e.g., "0.01") to basis points (100).
fn parse_tick_size_string(tick_str: &str) -> Result<u16, GatewayError> {
    let tick: f64 = tick_str
        .parse()
        .map_err(|_| GatewayError::InvalidMessage(format!("Invalid tick size: {}", tick_str)))?;

    let bps = (tick * 10000.0).round() as u16;

    if bps == 0 || bps > 1000 {
        return Err(GatewayError::InvalidMessage(format!(
            "Tick size {} out of valid range",
            tick_str
        )));
    }

    Ok(bps)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_price_to_tick() {
        let parser = Parser::new(); // 1 cent ticks (100 bps)

        assert_eq!(parser.parse_price_to_tick("0.50").unwrap(), 5000);
        assert_eq!(parser.parse_price_to_tick("0.01").unwrap(), 100);
        assert_eq!(parser.parse_price_to_tick("0.99").unwrap(), 9900);
        assert_eq!(parser.parse_price_to_tick("1.00").unwrap(), 10000);
    }

    #[test]
    fn test_parse_price_alignment_error() {
        let parser = Parser::new(); // 1 cent ticks

        // 0.551 is not aligned to 1 cent
        assert!(parser.parse_price_to_tick("0.551").is_err());
    }

    #[test]
    fn test_parse_size() {
        let parser = Parser::new();

        assert_eq!(parser.parse_size("1000").unwrap(), 1_000_000_000);
        assert_eq!(parser.parse_size("1000.5").unwrap(), 1_000_500_000);
        assert_eq!(parser.parse_size("0.01").unwrap(), 10_000);
    }

    #[test]
    fn test_parse_book_event() {
        let json = r#"[{
            "event_type": "book",
            "asset_id": "12345",
            "timestamp": 1700000000000,
            "hash": "abc123",
            "bids": [{"price": "0.50", "size": "1000"}],
            "asks": [{"price": "0.51", "size": "2000"}]
        }]"#;

        let parser = Parser::new();
        let frame = parser.parse_frame(json.as_bytes(), 0).unwrap();

        assert_eq!(frame.events.len(), 1);
        match &frame.events[0] {
            ParsedEvent::Book(book) => {
                assert_eq!(book.asset_id, "12345");
                assert_eq!(book.hash, "abc123");
                assert_eq!(book.bids.len(), 1);
                assert_eq!(book.bids[0], (5000, 1_000_000_000));
                assert_eq!(book.asks[0], (5100, 2_000_000_000));
            }
            _ => panic!("Expected book event"),
        }
    }

    #[test]
    fn test_parse_price_change_event() {
        let json = r#"[{
            "event_type": "price_change",
            "asset_id": "12345",
            "timestamp": 1700000000000,
            "price_changes": [
                {"price": "0.50", "side": "buy", "size": "100"},
                {"price": "0.51", "side": "sell", "size": "200"}
            ]
        }]"#;

        let parser = Parser::new();
        let frame = parser.parse_frame(json.as_bytes(), 0).unwrap();

        assert_eq!(frame.events.len(), 1);
        match &frame.events[0] {
            ParsedEvent::PriceChange(change) => {
                assert_eq!(change.changes.len(), 2);
                assert_eq!(change.changes[0].price_tick, 5000);
                assert_eq!(change.changes[0].new_size, 100_000_000);
                assert_eq!(change.changes[1].price_tick, 5100);
                assert_eq!(change.changes[1].new_size, 200_000_000);
            }
            _ => panic!("Expected price_change event"),
        }
    }

    #[test]
    fn test_parse_side() {
        assert!(matches!(parse_side("buy"), Ok(Side::Buy)));
        assert!(matches!(parse_side("BUY"), Ok(Side::Buy)));
        assert!(matches!(parse_side("bid"), Ok(Side::Buy)));
        assert!(matches!(parse_side("sell"), Ok(Side::Sell)));
        assert!(matches!(parse_side("SELL"), Ok(Side::Sell)));
        assert!(matches!(parse_side("ask"), Ok(Side::Sell)));
        assert!(parse_side("invalid").is_err());
    }
}
