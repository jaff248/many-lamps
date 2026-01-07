//! Error types for book operations.

use many_lamps_core::types::Tick;
use thiserror::Error;

/// Errors that can occur during book operations
#[derive(Debug, Error, Clone)]
pub enum BookError {
    /// Invalid tick observed from venue (not divisible by tick_size)
    #[error("Invalid tick observed: {raw_tick} not divisible by tick_size {tick_size} (price: {price_str})")]
    InvalidTickObserved {
        raw_tick: Tick,
        tick_size: Tick,
        price_str: String,
    },

    /// Tick out of valid range [0, 10000]
    #[error("Tick out of range: {tick} (must be 0-10000)")]
    TickOutOfRange { tick: Tick },

    /// Book hash mismatch on snapshot
    #[error("Book hash mismatch: computed={computed}, received={received}")]
    HashMismatch { computed: String, received: String },

    /// Book not synced, cannot apply delta
    #[error("Book not synced, cannot apply delta")]
    NotSynced,

    /// Invalid price string format
    #[error("Invalid price format: {0}")]
    InvalidPriceFormat(String),

    /// Invalid size string format
    #[error("Invalid size format: {0}")]
    InvalidSizeFormat(String),

    /// Tick size mismatch between local and remote
    #[error("Tick size mismatch: local={local}, remote={remote}")]
    TickSizeMismatch { local: Tick, remote: Tick },

    /// Best bid/ask mismatch during reconciliation
    #[error("Best {side} mismatch: local={local:?}, remote={remote:?}")]
    BestPriceMismatch {
        side: String,
        local: Option<Tick>,
        remote: Option<Tick>,
    },

    /// Level size mismatch during reconciliation
    #[error("Level size mismatch at tick {tick}: local={local}, remote={remote}")]
    LevelSizeMismatch { tick: Tick, local: u64, remote: u64 },
}

/// Result type for book operations
pub type BookResult<T> = Result<T, BookError>;
