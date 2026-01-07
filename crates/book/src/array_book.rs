//! High-performance L2 order book with tick-aligned indexing.
//!
//! Uses fixed-size arrays for O(1) updates and precomputed tick tables
//! for O(1) amortized best price lookups.

use crate::error::{BookError, BookResult};
use many_lamps_core::types::{Side, Size, Tick, MAX_TICK};

/// Maximum array size (0 to 10000 inclusive)
pub const BOOK_ARRAY_SIZE: usize = (MAX_TICK as usize) + 1;

/// High-performance L2 order book
#[derive(Debug, Clone)]
pub struct ArrayBook {
    /// Bid sizes at each tick level
    bids: Box<[Size; BOOK_ARRAY_SIZE]>,
    /// Ask sizes at each tick level
    asks: Box<[Size; BOOK_ARRAY_SIZE]>,
    /// Current best bid tick (highest bid)
    best_bid: Option<Tick>,
    /// Current best ask tick (lowest ask)
    best_ask: Option<Tick>,
    /// Current tick size (e.g., 100 for 0.01)
    tick_size: Tick,
    /// Precomputed previous valid tick for each position
    prev_valid: Box<[Option<Tick>; BOOK_ARRAY_SIZE]>,
    /// Precomputed next valid tick for each position
    next_valid: Box<[Option<Tick>; BOOK_ARRAY_SIZE]>,
}

impl ArrayBook {
    /// Create a new empty order book with given tick size
    pub fn new(tick_size: Tick) -> Self {
        let mut book = Self {
            bids: Box::new([0; BOOK_ARRAY_SIZE]),
            asks: Box::new([0; BOOK_ARRAY_SIZE]),
            best_bid: None,
            best_ask: None,
            tick_size,
            prev_valid: Box::new([None; BOOK_ARRAY_SIZE]),
            next_valid: Box::new([None; BOOK_ARRAY_SIZE]),
        };
        book.precompute_tick_tables();
        book
    }

    /// Get current tick size
    pub fn tick_size(&self) -> Tick {
        self.tick_size
    }

    /// Check if a tick is valid given current tick_size
    #[inline]
    pub fn is_valid_tick(&self, tick: Tick) -> bool {
        if self.tick_size == 0 {
            return true;
        }
        (tick as usize) % (self.tick_size as usize) == 0
    }

    /// Validate an inbound tick from venue - NEVER snap, only validate
    pub fn validate_inbound_tick(&self, tick: Tick, price_str: &str) -> BookResult<()> {
        if tick > MAX_TICK {
            return Err(BookError::TickOutOfRange { tick });
        }
        if !self.is_valid_tick(tick) {
            return Err(BookError::InvalidTickObserved {
                raw_tick: tick,
                tick_size: self.tick_size,
                price_str: price_str.to_string(),
            });
        }
        Ok(())
    }

    /// Set a price level (after validation)
    /// Returns error if tick is invalid
    pub fn set_level(&mut self, side: Side, tick: Tick, size: Size) -> BookResult<()> {
        if tick > MAX_TICK {
            return Err(BookError::TickOutOfRange { tick });
        }

        // For internal use, we trust the tick is valid
        // Validation should happen at the boundary (parsing)
        match side {
            Side::Buy => {
                self.bids[tick as usize] = size;
                self.update_best_bid(tick, size);
            }
            Side::Sell => {
                self.asks[tick as usize] = size;
                self.update_best_ask(tick, size);
            }
        }
        Ok(())
    }

    /// Set a price level without validation (internal use after batch validation)
    #[inline]
    pub fn set_level_unchecked(&mut self, side: Side, tick: Tick, size: Size) {
        match side {
            Side::Buy => {
                self.bids[tick as usize] = size;
                self.update_best_bid(tick, size);
            }
            Side::Sell => {
                self.asks[tick as usize] = size;
                self.update_best_ask(tick, size);
            }
        }
    }

    /// Get size at a price level
    #[inline]
    pub fn get_level(&self, side: Side, tick: Tick) -> Size {
        if tick > MAX_TICK {
            return 0;
        }
        match side {
            Side::Buy => self.bids[tick as usize],
            Side::Sell => self.asks[tick as usize],
        }
    }

    /// Get best bid tick
    #[inline]
    pub fn best_bid(&self) -> Option<Tick> {
        self.best_bid
    }

    /// Get best ask tick
    #[inline]
    pub fn best_ask(&self) -> Option<Tick> {
        self.best_ask
    }

    /// Get best bid size
    pub fn best_bid_size(&self) -> Option<Size> {
        self.best_bid.map(|t| self.bids[t as usize])
    }

    /// Get best ask size
    pub fn best_ask_size(&self) -> Option<Size> {
        self.best_ask.map(|t| self.asks[t as usize])
    }

    /// Calculate spread in ticks
    pub fn spread(&self) -> Option<Tick> {
        match (self.best_bid, self.best_ask) {
            (Some(bb), Some(ba)) if ba > bb => Some(ba - bb),
            _ => None,
        }
    }

    /// Calculate mid price as tick (rounded down)
    pub fn mid_tick(&self) -> Option<Tick> {
        match (self.best_bid, self.best_ask) {
            (Some(bb), Some(ba)) => Some((bb + ba) / 2),
            _ => None,
        }
    }

    /// Calculate microprice (size-weighted mid)
    pub fn microprice(&self) -> Option<f64> {
        let (bb, ba) = (self.best_bid?, self.best_ask?);
        let bid_size = self.bids[bb as usize] as f64;
        let ask_size = self.asks[ba as usize] as f64;
        if bid_size + ask_size == 0.0 {
            return Some((bb + ba) as f64 / 2.0 / 10000.0);
        }
        Some((bb as f64 * ask_size + ba as f64 * bid_size) / (bid_size + ask_size) / 10000.0)
    }

    /// Calculate depth at N levels from best (in size units)
    pub fn depth_at_levels(&self, side: Side, levels: usize) -> Size {
        let mut depth = 0;
        let mut count = 0;
        
        match side {
            Side::Buy => {
                let mut tick_opt = self.best_bid;
                while let Some(tick) = tick_opt {
                    if count >= levels {
                        break;
                    }
                    depth += self.bids[tick as usize];
                    count += 1;
                    tick_opt = self.prev_valid[tick as usize];
                }
            }
            Side::Sell => {
                let mut tick_opt = self.best_ask;
                while let Some(tick) = tick_opt {
                    if count >= levels {
                        break;
                    }
                    depth += self.asks[tick as usize];
                    count += 1;
                    tick_opt = self.next_valid[tick as usize];
                }
            }
        }
        
        depth
    }

    /// Clear the book
    pub fn clear(&mut self) {
        self.bids.fill(0);
        self.asks.fill(0);
        self.best_bid = None;
        self.best_ask = None;
    }

    /// Update tick size and recompute tables
    /// This clears the book as all existing levels may be invalid
    pub fn set_tick_size(&mut self, new_tick_size: Tick) {
        self.tick_size = new_tick_size;
        self.precompute_tick_tables();
        self.clear();
    }

    /// Get bids as vector of (tick, size) for non-zero levels
    /// Sorted by tick descending (best first)
    pub fn bids_vec(&self) -> Vec<(Tick, Size)> {
        let mut result = vec![];
        let mut tick_opt = self.best_bid;
        while let Some(tick) = tick_opt {
            let size = self.bids[tick as usize];
            if size > 0 {
                result.push((tick, size));
            }
            tick_opt = self.prev_valid[tick as usize];
        }
        result
    }

    /// Get asks as vector of (tick, size) for non-zero levels
    /// Sorted by tick ascending (best first)
    pub fn asks_vec(&self) -> Vec<(Tick, Size)> {
        let mut result = vec![];
        let mut tick_opt = self.best_ask;
        while let Some(tick) = tick_opt {
            let size = self.asks[tick as usize];
            if size > 0 {
                result.push((tick, size));
            }
            tick_opt = self.next_valid[tick as usize];
        }
        result
    }

    /// Generate JSON-compatible bid levels for hash computation
    pub fn bids_as_json(&self) -> Vec<serde_json::Value> {
        self.bids_vec()
            .into_iter()
            .map(|(tick, size)| {
                serde_json::json!({
                    "price": tick_to_price_string(tick),
                    "size": size_to_string(size)
                })
            })
            .collect()
    }

    /// Generate JSON-compatible ask levels for hash computation
    pub fn asks_as_json(&self) -> Vec<serde_json::Value> {
        self.asks_vec()
            .into_iter()
            .map(|(tick, size)| {
                serde_json::json!({
                    "price": tick_to_price_string(tick),
                    "size": size_to_string(size)
                })
            })
            .collect()
    }

    // Internal helpers

    fn precompute_tick_tables(&mut self) {
        let ts = self.tick_size as usize;
        if ts == 0 {
            // No tick size restriction - every tick is valid
            for i in 0..BOOK_ARRAY_SIZE {
                self.prev_valid[i] = if i > 0 { Some((i - 1) as Tick) } else { None };
                self.next_valid[i] = if i < MAX_TICK as usize { Some((i + 1) as Tick) } else { None };
            }
            return;
        }

        for i in 0..BOOK_ARRAY_SIZE {
            // prev_valid[i] = largest valid tick < i
            if i >= ts {
                // Find the valid tick at or below i-1
                let prev_candidate = ((i - 1) / ts) * ts;
                self.prev_valid[i] = if prev_candidate < i {
                    Some(prev_candidate as Tick)
                } else if prev_candidate >= ts {
                    Some((prev_candidate - ts) as Tick)
                } else {
                    None
                };
            } else {
                self.prev_valid[i] = None;
            }

            // next_valid[i] = smallest valid tick > i
            let next_candidate = ((i / ts) + 1) * ts;
            self.next_valid[i] = if next_candidate <= MAX_TICK as usize {
                Some(next_candidate as Tick)
            } else {
                None
            };
        }
    }

    fn update_best_bid(&mut self, tick: Tick, new_size: Size) {
        if new_size > 0 {
            // New or updated bid
            if self.best_bid.map_or(true, |bb| tick > bb) {
                self.best_bid = Some(tick);
            }
        } else if self.best_bid == Some(tick) {
            // Best bid removed, scan for new best
            self.best_bid = self.scan_best_bid_from(tick);
        }
    }

    fn update_best_ask(&mut self, tick: Tick, new_size: Size) {
        if new_size > 0 {
            // New or updated ask
            if self.best_ask.map_or(true, |ba| tick < ba) {
                self.best_ask = Some(tick);
            }
        } else if self.best_ask == Some(tick) {
            // Best ask removed, scan for new best
            self.best_ask = self.scan_best_ask_from(tick);
        }
    }

    fn scan_best_bid_from(&self, start: Tick) -> Option<Tick> {
        let mut tick_opt = self.prev_valid[start as usize];
        while let Some(tick) = tick_opt {
            if self.bids[tick as usize] > 0 {
                return Some(tick);
            }
            tick_opt = self.prev_valid[tick as usize];
        }
        None
    }

    fn scan_best_ask_from(&self, start: Tick) -> Option<Tick> {
        let mut tick_opt = self.next_valid[start as usize];
        while let Some(tick) = tick_opt {
            if self.asks[tick as usize] > 0 {
                return Some(tick);
            }
            tick_opt = self.next_valid[tick as usize];
        }
        None
    }
}

/// Convert tick to price string for API/hashing
fn tick_to_price_string(tick: Tick) -> String {
    let price = tick as f64 / 10000.0;
    // Format to remove trailing zeros
    let s = format!("{:.4}", price);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// Convert size (micro-units) to string
fn size_to_string(size: Size) -> String {
    let f = size as f64 / 1_000_000.0;
    format!("{}", f)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_book_empty() {
        let book = ArrayBook::new(100);
        assert_eq!(book.best_bid(), None);
        assert_eq!(book.best_ask(), None);
        assert_eq!(book.spread(), None);
    }

    #[test]
    fn test_set_level_updates_best() {
        let mut book = ArrayBook::new(100);
        
        // Add bid
        book.set_level(Side::Buy, 5000, 100).unwrap();
        assert_eq!(book.best_bid(), Some(5000));
        
        // Add higher bid
        book.set_level(Side::Buy, 5100, 200).unwrap();
        assert_eq!(book.best_bid(), Some(5100));
        
        // Add ask
        book.set_level(Side::Sell, 5200, 300).unwrap();
        assert_eq!(book.best_ask(), Some(5200));
        
        // Add lower ask
        book.set_level(Side::Sell, 5100, 400).unwrap();
        assert_eq!(book.best_ask(), Some(5100));
    }

    #[test]
    fn test_remove_best_updates() {
        let mut book = ArrayBook::new(100);
        
        book.set_level(Side::Buy, 5000, 100).unwrap();
        book.set_level(Side::Buy, 5100, 200).unwrap();
        assert_eq!(book.best_bid(), Some(5100));
        
        // Remove best bid
        book.set_level(Side::Buy, 5100, 0).unwrap();
        assert_eq!(book.best_bid(), Some(5000));
        
        // Remove remaining bid
        book.set_level(Side::Buy, 5000, 0).unwrap();
        assert_eq!(book.best_bid(), None);
    }

    #[test]
    fn test_tick_validation() {
        let book = ArrayBook::new(100); // tick_size = 0.01
        
        // Valid ticks (divisible by 100)
        assert!(book.is_valid_tick(0));
        assert!(book.is_valid_tick(100));
        assert!(book.is_valid_tick(5000));
        assert!(book.is_valid_tick(10000));
        
        // Invalid ticks
        assert!(!book.is_valid_tick(50));
        assert!(!book.is_valid_tick(5050));
        assert!(!book.is_valid_tick(101));
    }

    #[test]
    fn test_validate_inbound_tick_error() {
        let book = ArrayBook::new(100);
        
        // Valid tick should succeed
        assert!(book.validate_inbound_tick(5000, "0.50").is_ok());
        
        // Invalid tick should fail
        let result = book.validate_inbound_tick(5050, "0.505");
        assert!(matches!(result, Err(BookError::InvalidTickObserved { .. })));
    }

    #[test]
    fn test_spread() {
        let mut book = ArrayBook::new(100);
        
        book.set_level(Side::Buy, 4900, 100).unwrap();
        book.set_level(Side::Sell, 5100, 100).unwrap();
        
        assert_eq!(book.spread(), Some(200)); // 0.02 spread
    }

    #[test]
    fn test_microprice() {
        let mut book = ArrayBook::new(100);
        
        // Equal sizes -> mid price
        book.set_level(Side::Buy, 4900, 1_000_000).unwrap();
        book.set_level(Side::Sell, 5100, 1_000_000).unwrap();
        
        let mp = book.microprice().unwrap();
        assert!((mp - 0.50).abs() < 0.0001);
        
        // Weighted toward bid (larger ask size)
        book.set_level(Side::Sell, 5100, 3_000_000).unwrap();
        let mp2 = book.microprice().unwrap();
        assert!(mp2 < 0.50); // Should be closer to bid
    }

    #[test]
    fn test_depth_at_levels() {
        let mut book = ArrayBook::new(100);
        
        book.set_level(Side::Buy, 5000, 100).unwrap();
        book.set_level(Side::Buy, 4900, 200).unwrap();
        book.set_level(Side::Buy, 4800, 300).unwrap();
        
        assert_eq!(book.depth_at_levels(Side::Buy, 1), 100);
        assert_eq!(book.depth_at_levels(Side::Buy, 2), 300);
        assert_eq!(book.depth_at_levels(Side::Buy, 3), 600);
        assert_eq!(book.depth_at_levels(Side::Buy, 10), 600); // Only 3 levels
    }

    #[test]
    fn test_clear() {
        let mut book = ArrayBook::new(100);
        
        book.set_level(Side::Buy, 5000, 100).unwrap();
        book.set_level(Side::Sell, 5100, 200).unwrap();
        
        book.clear();
        
        assert_eq!(book.best_bid(), None);
        assert_eq!(book.best_ask(), None);
        assert_eq!(book.get_level(Side::Buy, 5000), 0);
    }

    #[test]
    fn test_tick_size_change() {
        let mut book = ArrayBook::new(100);
        
        book.set_level(Side::Buy, 5000, 100).unwrap();
        assert_eq!(book.best_bid(), Some(5000));
        
        // Change tick size - should clear book
        book.set_tick_size(10);
        assert_eq!(book.best_bid(), None);
        assert!(book.is_valid_tick(5005)); // Now valid with tick_size=10
    }

    #[test]
    fn test_bids_vec_sorted() {
        let mut book = ArrayBook::new(100);
        
        book.set_level(Side::Buy, 4800, 100).unwrap();
        book.set_level(Side::Buy, 5000, 200).unwrap();
        book.set_level(Side::Buy, 4900, 300).unwrap();
        
        let bids = book.bids_vec();
        assert_eq!(bids.len(), 3);
        assert_eq!(bids[0], (5000, 200)); // Best first
        assert_eq!(bids[1], (4900, 300));
        assert_eq!(bids[2], (4800, 100));
    }

    #[test]
    fn test_asks_vec_sorted() {
        let mut book = ArrayBook::new(100);
        
        book.set_level(Side::Sell, 5200, 100).unwrap();
        book.set_level(Side::Sell, 5000, 200).unwrap();
        book.set_level(Side::Sell, 5100, 300).unwrap();
        
        let asks = book.asks_vec();
        assert_eq!(asks.len(), 3);
        assert_eq!(asks[0], (5000, 200)); // Best first
        assert_eq!(asks[1], (5100, 300));
        assert_eq!(asks[2], (5200, 100));
    }

    #[test]
    fn test_tick_to_price_string() {
        assert_eq!(tick_to_price_string(5500), "0.55");
        assert_eq!(tick_to_price_string(100), "0.01");
        assert_eq!(tick_to_price_string(10000), "1");
        assert_eq!(tick_to_price_string(0), "0");
        assert_eq!(tick_to_price_string(5050), "0.505");
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn test_valid_ticks_after_updates(
            updates in prop::collection::vec(
                (any::<bool>(), 0u16..=100u16, 0u64..1000000u64),
                0..100
            )
        ) {
            let mut book = ArrayBook::new(100);
            
            for (is_bid, tick_factor, size) in updates {
                let tick = tick_factor * 100; // Ensure valid tick
                let side = if is_bid { Side::Buy } else { Side::Sell };
                book.set_level(side, tick, size).unwrap();
            }
            
            // Verify all non-zero levels are on valid ticks
            for tick in 0..=MAX_TICK {
                if book.bids[tick as usize] > 0 {
                    prop_assert!(book.is_valid_tick(tick), "Invalid bid tick: {}", tick);
                }
                if book.asks[tick as usize] > 0 {
                    prop_assert!(book.is_valid_tick(tick), "Invalid ask tick: {}", tick);
                }
            }
            
            // Verify best bid/ask are valid ticks
            if let Some(bb) = book.best_bid() {
                prop_assert!(book.is_valid_tick(bb), "Invalid best bid tick: {}", bb);
            }
            if let Some(ba) = book.best_ask() {
                prop_assert!(book.is_valid_tick(ba), "Invalid best ask tick: {}", ba);
            }
        }

        #[test]
        fn test_best_bid_is_highest(
            bids in prop::collection::vec((0u16..=100u16, 1u64..1000000u64), 1..50)
        ) {
            let mut book = ArrayBook::new(100);
            let mut max_tick = 0;
            
            for (tick_factor, size) in bids {
                let tick = tick_factor * 100;
                book.set_level(Side::Buy, tick, size).unwrap();
                if tick > max_tick {
                    max_tick = tick;
                }
            }
            
            prop_assert_eq!(book.best_bid(), Some(max_tick));
        }

        #[test]
        fn test_best_ask_is_lowest(
            asks in prop::collection::vec((0u16..=100u16, 1u64..1000000u64), 1..50)
        ) {
            let mut book = ArrayBook::new(100);
            let mut min_tick = MAX_TICK;
            
            for (tick_factor, size) in asks {
                let tick = tick_factor * 100;
                book.set_level(Side::Sell, tick, size).unwrap();
                if tick < min_tick {
                    min_tick = tick;
                }
            }
            
            prop_assert_eq!(book.best_ask(), Some(min_tick));
        }
    }
}
