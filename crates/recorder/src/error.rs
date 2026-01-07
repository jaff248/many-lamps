//! Recorder error types.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum RecorderError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Parquet error: {0}")]
    Parquet(#[from] parquet::errors::ParquetError),

    #[error("Arrow error: {0}")]
    Arrow(#[from] arrow::error::ArrowError),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Recorder not open")]
    NotOpen,

    #[error("Buffer full")]
    BufferFull,
}
