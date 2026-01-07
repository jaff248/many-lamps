//! Event recording and Parquet storage.
//!
//! Records:
//! - Raw WebSocket frames (for perfect replay)
//! - Parsed events with three timestamps
//! - Strategy decisions and fills

pub mod error;
pub mod frame_recorder;
pub mod event_recorder;
pub mod parquet_writer;

pub use error::RecorderError;
pub use frame_recorder::{FrameRecorder, RawFrame};
pub use event_recorder::EventRecorder;
pub use parquet_writer::ParquetEventWriter;
