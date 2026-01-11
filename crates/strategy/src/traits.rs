//! Strategy trait and common types.

use mtrader_book::ArrayBook;
use mtrader_core::{ClientOrderId, OrderReason, Side, Size, Tick};
use mtrader_execution::{OrderKind, OrderType};
use mtrader_risk::{PnLSnapshot, Position};

/// Context provided to strategies for decision making.
#[derive(Debug, Clone)]
pub struct StrategyContext {
    /// Current timestamp (mono ns)
    pub now_ns: u64,
    /// Asset ID
    pub asset_id: String,
    /// Current position
    pub position: Position,
    /// PnL snapshot
    pub pnl: PnLSnapshot,
    /// Best bid tick (if any)
    pub best_bid: Option<Tick>,
    /// Best ask tick (if any)
    pub best_ask: Option<Tick>,
    /// Best bid size
    pub best_bid_size: Size,
    /// Best ask size
    pub best_ask_size: Size,
    /// Mid price tick (if spread exists)
    pub mid_tick: Option<Tick>,
    /// Spread in ticks
    pub spread_ticks: Option<u16>,
    /// Our active bid orders
    pub our_bids: Vec<WorkingOrder>,
    /// Our active ask orders
    pub our_asks: Vec<WorkingOrder>,
}

#[derive(Debug, Clone)]
pub struct WorkingOrder {
    pub tick: Tick,
    pub client_order_id: ClientOrderId,
}

impl StrategyContext {
    /// Create context from order book.
    pub fn from_book(
        book: &ArrayBook,
        asset_id: String,
        position: Position,
        pnl: PnLSnapshot,
        our_bids: Vec<WorkingOrder>,
        our_asks: Vec<WorkingOrder>,
        now_ns: u64,
    ) -> Self {
        let best_bid = book.best_bid();
        let best_ask = book.best_ask();
        let (best_bid_size, best_ask_size) = match (best_bid, best_ask) {
            (Some(_), Some(_)) => (
                book.best_bid_size().unwrap_or(0),
                book.best_ask_size().unwrap_or(0),
            ),
            (Some(_), None) => (book.best_bid_size().unwrap_or(0), 0),
            (None, Some(_)) => (0, book.best_ask_size().unwrap_or(0)),
            (None, None) => (0, 0),
        };

        let (mid_tick, spread_ticks) = match (best_bid, best_ask) {
            (Some(bid), Some(ask)) if ask > bid => {
                let mid = (bid + ask) / 2;
                let spread = ask - bid;
                (Some(mid), Some(spread))
            }
            _ => (None, None),
        };

        Self {
            now_ns,
            asset_id,
            position,
            pnl,
            best_bid,
            best_ask,
            best_bid_size,
            best_ask_size,
            mid_tick,
            spread_ticks,
            our_bids,
            our_asks,
        }
    }

    /// Check if market has a valid two-sided quote.
    pub fn has_two_sided_market(&self) -> bool {
        self.best_bid.is_some() && self.best_ask.is_some()
    }

    /// Get the edge available for a new order.
    pub fn edge_for_order(&self, side: Side, tick: Tick) -> Option<i16> {
        match side {
            Side::Buy => self.best_ask.map(|ask| ask as i16 - tick as i16),
            Side::Sell => self.best_bid.map(|bid| tick as i16 - bid as i16),
        }
    }
}

/// Action requested by a strategy.
#[derive(Debug, Clone)]
pub enum StrategyAction {
    /// Place a new order
    PlaceOrder {
        asset_id: String,
        side: Side,
        kind: OrderKind,
        order_type: OrderType,
        reason: OrderReason,
    },
    /// Cancel an existing order
    CancelOrder {
        client_order_id: ClientOrderId,
        reason: String,
    },
    /// Amend an order (cancel + replace)
    AmendOrder {
        client_order_id: ClientOrderId,
        new_kind: OrderKind,
        reason: String,
    },
    /// No action needed
    NoOp,
}

/// Strategy trait for implementing trading strategies.
pub trait Strategy {
    /// Get strategy name.
    fn name(&self) -> &str;

    /// Called on each market data update.
    /// Returns actions to take.
    fn on_update(&mut self, ctx: &StrategyContext) -> Vec<StrategyAction>;

    /// Called when an order is filled.
    fn on_fill(&mut self, ctx: &StrategyContext, side: Side, tick: Tick, size: Size);

    /// Called when circuit breaker trips.
    fn on_halt(&mut self);

    /// Called when circuit breaker resets.
    fn on_resume(&mut self);

    /// Check if strategy is active.
    fn is_active(&self) -> bool;

    /// Activate the strategy.
    fn activate(&mut self);

    /// Deactivate the strategy.
    fn deactivate(&mut self);
}
