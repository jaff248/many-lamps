//! High-level event recording with buffering.
//!
//! Buffers events and flushes to Parquet periodically.

use crate::{ParquetEventWriter, RecorderError};
use mtrader_core::events::CoreEvent;
use std::path::PathBuf;

/// Configuration for event recording.
#[derive(Debug, Clone)]
pub struct EventRecorderConfig {
    /// Directory for output files.
    pub output_dir: PathBuf,
    /// File prefix.
    pub file_prefix: String,
    /// Max events per file.
    pub max_events_per_file: usize,
    /// Max file age before rotation (seconds).
    pub max_file_age_secs: u64,
    /// Buffer size before flush.
    pub buffer_size: usize,
}

impl Default for EventRecorderConfig {
    fn default() -> Self {
        Self {
            output_dir: PathBuf::from("data/events"),
            file_prefix: "events".into(),
            max_events_per_file: 1_000_000,
            max_file_age_secs: 3600, // 1 hour
            buffer_size: 10_000,
        }
    }
}

/// High-level event recorder with automatic file rotation.
pub struct EventRecorder {
    config: EventRecorderConfig,
    buffer: Vec<CoreEvent>,
    current_writer: Option<ParquetEventWriter>,
    current_file_events: usize,
    current_file_start_ns: Option<u64>,
    total_events: u64,
    files_written: u64,
}

impl EventRecorder {
    pub fn new(config: EventRecorderConfig) -> Self {
        Self {
            config,
            buffer: Vec::new(),
            current_writer: None,
            current_file_events: 0,
            current_file_start_ns: None,
            total_events: 0,
            files_written: 0,
        }
    }

    /// Record an event.
    pub fn record(&mut self, event: CoreEvent) -> Result<(), RecorderError> {
        self.buffer.push(event);

        if self.buffer.len() >= self.config.buffer_size {
            self.flush()?;
        }

        Ok(())
    }

    /// Record multiple events.
    pub fn record_batch(&mut self, events: impl IntoIterator<Item = CoreEvent>) -> Result<(), RecorderError> {
        for event in events {
            self.record(event)?;
        }
        Ok(())
    }

    /// Flush buffered events to disk.
    pub fn flush(&mut self) -> Result<(), RecorderError> {
        if self.buffer.is_empty() {
            return Ok(());
        }

        // Ensure writer is open
        if self.current_writer.is_none() {
            self.rotate_file()?;
        }

        let writer = self.current_writer.as_mut().unwrap();

        // Write buffered events
        let event_count = self.buffer.len();
        writer.write_events(&self.buffer)?;
        self.buffer.clear();

        self.current_file_events += event_count;
        self.total_events += event_count as u64;

        // Check if rotation needed
        if self.current_file_events >= self.config.max_events_per_file {
            self.rotate_file()?;
        }

        Ok(())
    }

    /// Force file rotation.
    pub fn rotate_file(&mut self) -> Result<(), RecorderError> {
        // Close current writer
        if let Some(mut writer) = self.current_writer.take() {
            writer.close()?;
            self.files_written += 1;
        }

        // Create new file
        std::fs::create_dir_all(&self.config.output_dir)?;
        let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
        let filename = format!("{}_{}.parquet", self.config.file_prefix, timestamp);
        let path = self.config.output_dir.join(filename);

        let mut writer = ParquetEventWriter::new();
        writer.open(path)?;
        self.current_writer = Some(writer);
        self.current_file_events = 0;
        self.current_file_start_ns = None;

        Ok(())
    }

    /// Close the recorder.
    pub fn close(&mut self) -> Result<(), RecorderError> {
        self.flush()?;
        if let Some(mut writer) = self.current_writer.take() {
            writer.close()?;
            self.files_written += 1;
        }
        Ok(())
    }

    /// Get total events recorded.
    pub fn total_events(&self) -> u64 {
        self.total_events
    }

    /// Get total files written.
    pub fn files_written(&self) -> u64 {
        self.files_written
    }

    /// Get buffered event count.
    pub fn buffered_count(&self) -> usize {
        self.buffer.len()
    }
}

impl Drop for EventRecorder {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mtrader_core::events::{EventTimestamps, MarketDataEvent};
    use mtrader_core::{MarketId, Side, TokenId};
    use tempfile::tempdir;

    fn make_event(ts: u64) -> CoreEvent {
        let timestamps = EventTimestamps {
            ts_exchange_ms: (ts / 1_000_000) as i64,
            ts_recv_mono_ns: ts as i64,
            ts_process_mono_ns: ts as i64,
        };

        CoreEvent::MarketData(MarketDataEvent::BookDelta {
            market_id: MarketId("market".to_string()),
            token_id: TokenId("token".to_string()),
            side: Side::Buy,
            tick: 5000,
            new_size: 1000,
            best_bid: None,
            best_ask: None,
            order_hash: "hash".to_string(),
            timestamps,
        })
    }

    #[test]
    fn test_event_recorder_basic() {
        let dir = tempdir().unwrap();

        let config = EventRecorderConfig {
            output_dir: dir.path().to_path_buf(),
            file_prefix: "test".into(),
            max_events_per_file: 1000,
            max_file_age_secs: 3600,
            buffer_size: 10,
        };

        let mut recorder = EventRecorder::new(config);

        // Record some events
        for i in 0..25 {
            recorder.record(make_event(i * 1000)).unwrap();
        }

        recorder.close().unwrap();

        assert_eq!(recorder.total_events(), 25);
        assert!(recorder.files_written() >= 1);
    }
}
