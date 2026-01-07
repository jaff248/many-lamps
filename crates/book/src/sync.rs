//! Book synchronization state machine with integrity verification.
//!
//! Manages the lifecycle of book state:
//! - Unsynced: No valid book data
//! - Syncing: Waiting for snapshot
//! - Synced: Book is valid, can apply deltas
//!
//! Uses REST verification to detect drift rather than trying to
//! reproduce server-side hash computation.

use crate::array_book::ArrayBook;
use crate::error::{BookError, BookResult};
use many_lamps_core::types::{MarketId, Side, Size, Tick, TokenId};
use serde::{Deserialize, Serialize};

/// Book synchronization state
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BookSyncState {
    /// No valid book data, need snapshot
    Unsynced,
    /// Waiting for REST snapshot after inconsistency detected
    Syncing,
    /// Book is synced, can apply deltas
    Synced {
        /// Hash from last snapshot (canonical, not computed)
        snapshot_hash: String,
        /// Timestamp of last snapshot
        last_snapshot_ts: u64,
        /// Timestamp of last applied delta
        last_delta_ts: u64,
    },
}

impl BookSyncState {
    pub fn is_synced(&self) -> bool {
        matches!(self, BookSyncState::Synced { .. })
    }
}

/// Result of sync operations
#[derive(Debug, Clone)]
pub enum SyncResult {
    /// Book is now synced
    Synced,
    /// Delta applied successfully
    Applied,
    /// Event ignored (wrong state or wrong token)
    Ignored,
    /// Need to request resnapshot
    NeedResnapshot { token_id: TokenId, reason: String },
    /// Invalid tick detected from venue
    InvalidTick { error: BookError },
}

/// Book synchronization manager
#[derive(Debug)]
pub struct BookSync {
    /// Current sync state
    state: BookSyncState,
    /// The order book
    book: ArrayBook,
    /// Market ID
    market_id: MarketId,
    /// Token ID (asset_id)
    token_id: TokenId,
    /// Metadata for potential hash computation
    min_order_size: String,
    neg_risk: bool,
    last_trade_price: Option<String>,
}

impl BookSync {
    pub fn new(market_id: MarketId, token_id: TokenId, tick_size: Tick) -> Self {
        Self {
            state: BookSyncState::Unsynced,
            book: ArrayBook::new(tick_size),
            market_id,
            token_id,
            min_order_size: "0.01".to_string(),
            neg_risk: false,
            last_trade_price: None,
        }
    }

    /// Get current sync state
    pub fn state(&self) -> &BookSyncState {
        &self.state
    }

    /// Get reference to the order book (only if synced)
    pub fn book(&self) -> Option<&ArrayBook> {
        if self.state.is_synced() {
            Some(&self.book)
        } else {
            None
        }
    }

    /// Get reference to the order book (regardless of state, for inspection)
    pub fn book_unchecked(&self) -> &ArrayBook {
        &self.book
    }

    /// Get token ID
    pub fn token_id(&self) -> &TokenId {
        &self.token_id
    }

    /// Get market ID
    pub fn market_id(&self) -> &MarketId {
        &self.market_id
    }

    /// Handle a book snapshot from WS or REST
    pub fn on_snapshot(&mut self, snapshot: BookSnapshot) -> SyncResult {
        // Validate that this snapshot is for our token
        if snapshot.token_id != self.token_id {
            return SyncResult::Ignored;
        }

        // Check tick size consistency
        if snapshot.tick_size != self.book.tick_size() {
            // Tick size changed - clear and update
            self.book.set_tick_size(snapshot.tick_size);
        }

        // Clear and rebuild book
        self.book.clear();

        // Validate and apply all levels
        for (tick, size) in &snapshot.bids {
            if let Err(e) = self.book.validate_inbound_tick(*tick, &format!("{}", tick)) {
                self.state = BookSyncState::Syncing;
                return SyncResult::InvalidTick { error: e };
            }
            self.book.set_level_unchecked(Side::Buy, *tick, *size);
        }

        for (tick, size) in &snapshot.asks {
            if let Err(e) = self.book.validate_inbound_tick(*tick, &format!("{}", tick)) {
                self.state = BookSyncState::Syncing;
                return SyncResult::InvalidTick { error: e };
            }
            self.book.set_level_unchecked(Side::Sell, *tick, *size);
        }

        // Store metadata
        self.min_order_size = snapshot.min_order_size.clone();
        self.neg_risk = snapshot.neg_risk;
        self.last_trade_price = snapshot.last_trade_price.clone();

        // Trust the server's hash as canonical
        self.state = BookSyncState::Synced {
            snapshot_hash: snapshot.hash.clone(),
            last_snapshot_ts: snapshot.timestamp,
            last_delta_ts: snapshot.timestamp,
        };

        tracing::debug!(
            token_id = %self.token_id,
            hash = %snapshot.hash,
            bids = snapshot.bids.len(),
            asks = snapshot.asks.len(),
            "Book snapshot applied"
        );

        SyncResult::Synced
    }

    /// Handle a price change delta from WS
    pub fn on_delta(&mut self, delta: BookDelta) -> SyncResult {
        // Must be synced to apply deltas
        let BookSyncState::Synced { last_delta_ts, .. } = &mut self.state else {
            return SyncResult::Ignored;
        };

        // Validate token
        if delta.token_id != self.token_id {
            return SyncResult::Ignored;
        }

        // Validate tick
        if let Err(e) = self.book.validate_inbound_tick(delta.tick, &delta.price_str) {
            self.state = BookSyncState::Syncing;
            return SyncResult::InvalidTick { error: e };
        }

        // Apply the delta
        self.book.set_level_unchecked(delta.side, delta.tick, delta.new_size);

        // Update timestamp
        *last_delta_ts = delta.timestamp;

        SyncResult::Applied
    }

    /// Handle tick size change event
    pub fn on_tick_size_change(&mut self, new_tick_size: Tick) -> SyncResult {
        // Tick size change requires full resnapshot
        self.book.set_tick_size(new_tick_size);
        self.state = BookSyncState::Syncing;
        
        SyncResult::NeedResnapshot {
            token_id: self.token_id.clone(),
            reason: "Tick size changed".to_string(),
        }
    }

    /// Handle WS disconnect - go to Syncing until resnapshot
    pub fn on_disconnect(&mut self) -> SyncResult {
        if self.state.is_synced() {
            self.state = BookSyncState::Syncing;
            SyncResult::NeedResnapshot {
                token_id: self.token_id.clone(),
                reason: "WS disconnected".to_string(),
            }
        } else {
            SyncResult::Ignored
        }
    }

    /// Verify book state against REST snapshot
    /// Returns mismatches found, or Ok if consistent
    pub fn verify_against_rest(&self, rest_snapshot: &RestBookSnapshot) -> BookResult<()> {
        // Check tick size
        if rest_snapshot.tick_size != self.book.tick_size() {
            return Err(BookError::TickSizeMismatch {
                local: self.book.tick_size(),
                remote: rest_snapshot.tick_size,
            });
        }

        // Check best bid
        if self.book.best_bid() != rest_snapshot.best_bid {
            return Err(BookError::BestPriceMismatch {
                side: "bid".to_string(),
                local: self.book.best_bid(),
                remote: rest_snapshot.best_bid,
            });
        }

        // Check best ask
        if self.book.best_ask() != rest_snapshot.best_ask {
            return Err(BookError::BestPriceMismatch {
                side: "ask".to_string(),
                local: self.book.best_ask(),
                remote: rest_snapshot.best_ask,
            });
        }

        // Check top N levels
        let local_bids = self.book.bids_vec();
        for (i, (tick, size)) in rest_snapshot.top_bids.iter().enumerate() {
            let local_size = local_bids.get(i).map(|(_, s)| *s).unwrap_or(0);
            if local_size != *size {
                return Err(BookError::LevelSizeMismatch {
                    tick: *tick,
                    local: local_size,
                    remote: *size,
                });
            }
        }

        let local_asks = self.book.asks_vec();
        for (i, (tick, size)) in rest_snapshot.top_asks.iter().enumerate() {
            let local_size = local_asks.get(i).map(|(_, s)| *s).unwrap_or(0);
            if local_size != *size {
                return Err(BookError::LevelSizeMismatch {
                    tick: *tick,
                    local: local_size,
                    remote: *size,
                });
            }
        }

        Ok(())
    }

    /// Mark as needing resnapshot (e.g., after reconciliation failure)
    pub fn request_resnapshot(&mut self, reason: &str) -> SyncResult {
        self.state = BookSyncState::Syncing;
        SyncResult::NeedResnapshot {
            token_id: self.token_id.clone(),
            reason: reason.to_string(),
        }
    }

    /// Check if we should request REST verification
    pub fn should_verify(&self, now_ms: u64, interval_ms: u64) -> bool {
        match &self.state {
            BookSyncState::Synced { last_snapshot_ts, .. } => {
                now_ms.saturating_sub(*last_snapshot_ts) >= interval_ms
            }
            _ => false,
        }
    }
}

/// Book snapshot data (from WS or REST)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BookSnapshot {
    pub market_id: MarketId,
    pub token_id: TokenId,
    pub bids: Vec<(Tick, Size)>,
    pub asks: Vec<(Tick, Size)>,
    pub tick_size: Tick,
    pub hash: String,
    pub timestamp: u64,
    pub min_order_size: String,
    pub neg_risk: bool,
    pub last_trade_price: Option<String>,
}

/// Book delta data (price_change event)
#[derive(Debug, Clone)]
pub struct BookDelta {
    pub market_id: MarketId,
    pub token_id: TokenId,
    pub side: Side,
    pub tick: Tick,
    pub new_size: Size,
    pub price_str: String,
    pub timestamp: u64,
}

/// REST book snapshot for verification (subset of full snapshot)
#[derive(Debug, Clone)]
pub struct RestBookSnapshot {
    pub tick_size: Tick,
    pub best_bid: Option<Tick>,
    pub best_ask: Option<Tick>,
    pub top_bids: Vec<(Tick, Size)>,
    pub top_asks: Vec<(Tick, Size)>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_sync() -> BookSync {
        BookSync::new(
            MarketId("0xtest".to_string()),
            TokenId("123456".to_string()),
            100, // tick_size = 0.01
        )
    }

    fn make_snapshot() -> BookSnapshot {
        BookSnapshot {
            market_id: MarketId("0xtest".to_string()),
            token_id: TokenId("123456".to_string()),
            bids: vec![(4900, 100_000_000), (4800, 200_000_000)],
            asks: vec![(5100, 150_000_000), (5200, 250_000_000)],
            tick_size: 100,
            hash: "abc123".to_string(),
            timestamp: 1000,
            min_order_size: "0.01".to_string(),
            neg_risk: false,
            last_trade_price: Some("0.50".to_string()),
        }
    }

    #[test]
    fn test_initial_state_unsynced() {
        let sync = make_sync();
        assert!(matches!(sync.state(), BookSyncState::Unsynced));
        assert!(sync.book().is_none());
    }

    #[test]
    fn test_snapshot_syncs_book() {
        let mut sync = make_sync();
        let snapshot = make_snapshot();
        
        let result = sync.on_snapshot(snapshot);
        assert!(matches!(result, SyncResult::Synced));
        assert!(sync.state().is_synced());
        
        let book = sync.book().unwrap();
        assert_eq!(book.best_bid(), Some(4900));
        assert_eq!(book.best_ask(), Some(5100));
    }

    #[test]
    fn test_delta_requires_synced() {
        let mut sync = make_sync();
        
        let delta = BookDelta {
            market_id: MarketId("0xtest".to_string()),
            token_id: TokenId("123456".to_string()),
            side: Side::Buy,
            tick: 5000,
            new_size: 500_000_000,
            price_str: "0.50".to_string(),
            timestamp: 2000,
        };
        
        // Should be ignored when not synced
        let result = sync.on_delta(delta.clone());
        assert!(matches!(result, SyncResult::Ignored));
        
        // After snapshot, delta should apply
        sync.on_snapshot(make_snapshot());
        let result = sync.on_delta(delta);
        assert!(matches!(result, SyncResult::Applied));
        
        let book = sync.book().unwrap();
        assert_eq!(book.best_bid(), Some(5000)); // New best bid
    }

    #[test]
    fn test_invalid_tick_triggers_resync() {
        let mut sync = make_sync();
        sync.on_snapshot(make_snapshot());
        
        // Invalid tick (not divisible by 100)
        let delta = BookDelta {
            market_id: MarketId("0xtest".to_string()),
            token_id: TokenId("123456".to_string()),
            side: Side::Buy,
            tick: 5050, // Invalid!
            new_size: 500_000_000,
            price_str: "0.505".to_string(),
            timestamp: 2000,
        };
        
        let result = sync.on_delta(delta);
        assert!(matches!(result, SyncResult::InvalidTick { .. }));
        assert!(matches!(sync.state(), BookSyncState::Syncing));
    }

    #[test]
    fn test_tick_size_change_triggers_resync() {
        let mut sync = make_sync();
        sync.on_snapshot(make_snapshot());
        assert!(sync.state().is_synced());
        
        let result = sync.on_tick_size_change(10);
        assert!(matches!(result, SyncResult::NeedResnapshot { .. }));
        assert!(matches!(sync.state(), BookSyncState::Syncing));
    }

    #[test]
    fn test_disconnect_triggers_resync() {
        let mut sync = make_sync();
        sync.on_snapshot(make_snapshot());
        
        let result = sync.on_disconnect();
        assert!(matches!(result, SyncResult::NeedResnapshot { .. }));
        assert!(matches!(sync.state(), BookSyncState::Syncing));
    }

    #[test]
    fn test_verify_against_rest_success() {
        let mut sync = make_sync();
        sync.on_snapshot(make_snapshot());
        
        let rest = RestBookSnapshot {
            tick_size: 100,
            best_bid: Some(4900),
            best_ask: Some(5100),
            top_bids: vec![(4900, 100_000_000), (4800, 200_000_000)],
            top_asks: vec![(5100, 150_000_000), (5200, 250_000_000)],
        };
        
        assert!(sync.verify_against_rest(&rest).is_ok());
    }

    #[test]
    fn test_verify_against_rest_mismatch() {
        let mut sync = make_sync();
        sync.on_snapshot(make_snapshot());
        
        // Best bid mismatch
        let rest = RestBookSnapshot {
            tick_size: 100,
            best_bid: Some(5000), // Different!
            best_ask: Some(5100),
            top_bids: vec![],
            top_asks: vec![],
        };
        
        let result = sync.verify_against_rest(&rest);
        assert!(matches!(result, Err(BookError::BestPriceMismatch { .. })));
    }

    #[test]
    fn test_wrong_token_ignored() {
        let mut sync = make_sync();
        
        let mut snapshot = make_snapshot();
        snapshot.token_id = TokenId("999999".to_string()); // Different token
        
        let result = sync.on_snapshot(snapshot);
        assert!(matches!(result, SyncResult::Ignored));
        assert!(matches!(sync.state(), BookSyncState::Unsynced));
    }
}
