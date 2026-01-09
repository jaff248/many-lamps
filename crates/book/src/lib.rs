//! Order book management for Many Lamps trading system.
//!
//! This crate provides:
//! - ArrayBook: High-performance L2 order book with tick-aligned indexing
//! - BookSync: Book synchronization state machine with integrity verification
//! - Tick validation and error handling

pub mod array_book;
pub mod error;
pub mod sync;

pub use array_book::ArrayBook;
pub use error::BookError;
pub use sync::{BookSync, BookSyncState, SyncResult};
