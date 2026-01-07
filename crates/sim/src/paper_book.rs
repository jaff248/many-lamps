//! Paper trading order book.
//!
//! Mirrors the live book and tracks our simulated orders.

use mtrader_book::ArrayBook;
use mtrader_core::{Side, Size, Tick};
use std::collections::HashMap;

/// Our order in the paper book.
#[derive(Debug, Clone)]
pub struct PaperOrder {
    pub order_id: String,
    pub client_order_id: mtrader_core::ClientOrderId,
    pub side: Side,
    pub price_tick: Tick,
    pub size: Size,
    pub timestamp_ns: u64,
}

/// Paper order book that tracks both market state and our orders.
pub struct PaperBook {
    /// The underlying market book (from live feed)
    market_book: ArrayBook,
    /// Our buy orders by tick
    our_bids: HashMap<Tick, Vec<PaperOrder>>,
    /// Our sell orders by tick
    our_asks: HashMap<Tick, Vec<PaperOrder>>,
}

impl PaperBook {
    pub fn new(tick_size_bps: u16) -> Self {
        Self {
            market_book: ArrayBook::new(tick_size_bps),
            our_bids: HashMap::new(),
            our_asks: HashMap::new(),
        }
    }

    /// Get reference to the underlying market book.
    pub fn market_book(&self) -> &ArrayBook {
        &self.market_book
    }

    /// Get mutable reference to the underlying market book.
    pub fn market_book_mut(&mut self) -> &mut ArrayBook {
        &mut self.market_book
    }

    /// Add our order to the paper book.
    pub fn add_order(&mut self, order: PaperOrder) {
        let orders = match order.side {
            Side::Buy => self.our_bids.entry(order.price_tick).or_default(),
            Side::Sell => self.our_asks.entry(order.price_tick).or_default(),
        };
        orders.push(order);
    }

    /// Remove our order from the paper book.
    pub fn remove_order(&mut self, order_id: &str) -> Option<PaperOrder> {
        // Try bids first
        for orders in self.our_bids.values_mut() {
            if let Some(idx) = orders.iter().position(|o| o.order_id == order_id) {
                return Some(orders.remove(idx));
            }
        }

        // Try asks
        for orders in self.our_asks.values_mut() {
            if let Some(idx) = orders.iter().position(|o| o.order_id == order_id) {
                return Some(orders.remove(idx));
            }
        }

        None
    }

    /// Update order size (after partial fill).
    pub fn update_order_size(&mut self, order_id: &str, new_size: Size) {
        for orders in self.our_bids.values_mut().chain(self.our_asks.values_mut()) {
            if let Some(order) = orders.iter_mut().find(|o| o.order_id == order_id) {
                order.size = new_size;
                return;
            }
        }
    }

    /// Get our orders at a specific tick.
    pub fn our_orders_at(&self, side: Side, tick: Tick) -> &[PaperOrder] {
        let map = match side {
            Side::Buy => &self.our_bids,
            Side::Sell => &self.our_asks,
        };
        map.get(&tick).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Get all our bid ticks.
    pub fn our_bid_ticks(&self) -> Vec<Tick> {
        self.our_bids.keys().copied().collect()
    }

    /// Get all our ask ticks.
    pub fn our_ask_ticks(&self) -> Vec<Tick> {
        self.our_asks.keys().copied().collect()
    }

    /// Get total size of our orders on a side.
    pub fn our_total_size(&self, side: Side) -> Size {
        let map = match side {
            Side::Buy => &self.our_bids,
            Side::Sell => &self.our_asks,
        };
        map.values().flat_map(|v| v.iter()).map(|o| o.size).sum()
    }

    /// Estimate queue ahead size at a price level.
    pub fn estimate_queue_ahead(&self, side: Side, tick: Tick) -> Size {
        let market_size = match side {
            Side::Buy => self.market_book.bid_size_at(tick).unwrap_or(0),
            Side::Sell => self.market_book.ask_size_at(tick).unwrap_or(0),
        };
        let our_size: Size = self.our_orders_at(side, tick).iter().map(|o| o.size).sum();
        market_size.saturating_sub(our_size)
    }

    /// Get our best bid tick.
    pub fn our_best_bid(&self) -> Option<Tick> {
        self.our_bids
            .iter()
            .filter(|(_, orders)| !orders.is_empty())
            .map(|(&tick, _)| tick)
            .max()
    }

    /// Get our best ask tick.
    pub fn our_best_ask(&self) -> Option<Tick> {
        self.our_asks
            .iter()
            .filter(|(_, orders)| !orders.is_empty())
            .map(|(&tick, _)| tick)
            .min()
    }

    /// Check if our orders would cross (self-trade potential).
    pub fn would_self_trade(&self) -> bool {
        match (self.our_best_bid(), self.our_best_ask()) {
            (Some(bid), Some(ask)) => bid >= ask,
            _ => false,
        }
    }

    /// Clear all our orders.
    pub fn clear_our_orders(&mut self) {
        self.our_bids.clear();
        self.our_asks.clear();
    }

    /// Count total number of our orders.
    pub fn our_order_count(&self) -> usize {
        self.our_bids.values().map(|v| v.len()).sum::<usize>()
            + self.our_asks.values().map(|v| v.len()).sum::<usize>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_and_remove_order() {
        let mut book = PaperBook::new(100);

        let order = PaperOrder {
            order_id: "order-1".into(),
            client_order_id: mtrader_core::ClientOrderId("order-1".into()),
            side: Side::Buy,
            price_tick: 5000,
            size: 100_000,
            timestamp_ns: 1000,
        };

        book.add_order(order.clone());
        assert_eq!(book.our_order_count(), 1);
        assert_eq!(book.our_best_bid(), Some(5000));

        let removed = book.remove_order("order-1");
        assert!(removed.is_some());
        assert_eq!(book.our_order_count(), 0);
    }

    #[test]
    fn test_self_trade_detection() {
        let mut book = PaperBook::new(100);

        // Add bid at 50c
        book.add_order(PaperOrder {
            order_id: "bid-1".into(),
            client_order_id: mtrader_core::ClientOrderId("bid-1".into()),
            side: Side::Buy,
            price_tick: 5000,
            size: 100_000,
            timestamp_ns: 1000,
        });

        // Add ask at 52c - no self trade
        book.add_order(PaperOrder {
            order_id: "ask-1".into(),
            client_order_id: mtrader_core::ClientOrderId("ask-1".into()),
            side: Side::Sell,
            price_tick: 5200,
            size: 100_000,
            timestamp_ns: 2000,
        });

        assert!(!book.would_self_trade());

        // Add crossing ask at 50c - would self trade
        book.add_order(PaperOrder {
            order_id: "ask-2".into(),
            client_order_id: mtrader_core::ClientOrderId("ask-2".into()),
            side: Side::Sell,
            price_tick: 5000,
            size: 100_000,
            timestamp_ns: 3000,
        });

        assert!(book.would_self_trade());
    }

    #[test]
    fn test_update_order_size() {
        let mut book = PaperBook::new(100);

        book.add_order(PaperOrder {
            order_id: "order-1".into(),
            client_order_id: mtrader_core::ClientOrderId("order-1".into()),
            side: Side::Buy,
            price_tick: 5000,
            size: 100_000,
            timestamp_ns: 1000,
        });

        book.update_order_size("order-1", 50_000);

        let orders = book.our_orders_at(Side::Buy, 5000);
        assert_eq!(orders[0].size, 50_000);
    }
}
