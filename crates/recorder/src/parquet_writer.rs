//! Parquet file writer for events.
//!
//! Uses Arrow/Parquet for efficient columnar storage.

use crate::RecorderError;
use arrow::array::{Int64Array, StringArray, UInt64Array, UInt8Array};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use mtrader_core::events::CoreEvent;
use parquet::arrow::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;
use std::fs::File;
use std::path::Path;
use std::sync::Arc;

/// Creates the schema for event records.
fn event_schema() -> Schema {
    Schema::new(vec![
        Field::new("event_type", DataType::Utf8, false),
        Field::new("ts_exchange_ms", DataType::UInt64, false),
        Field::new("ts_recv_mono_ns", DataType::UInt64, false),
        Field::new("ts_process_mono_ns", DataType::UInt64, false),
        Field::new("side", DataType::UInt8, true),         // 0=Buy, 1=Sell
        Field::new("price_tick", DataType::UInt64, true),
        Field::new("size", DataType::Int64, true),
        Field::new("order_id", DataType::Utf8, true),
        Field::new("trade_id", DataType::Utf8, true),
        Field::new("reason", DataType::Utf8, true),
        Field::new("payload_json", DataType::Utf8, true), // For complex events
    ])
}

/// Writes events to Parquet files.
pub struct ParquetEventWriter {
    writer: Option<ArrowWriter<File>>,
    schema: Arc<Schema>,
    batch_size: usize,

    // Column builders
    event_types: Vec<String>,
    ts_exchange_ms: Vec<u64>,
    ts_recv_mono_ns: Vec<u64>,
    ts_process_mono_ns: Vec<u64>,
    sides: Vec<Option<u8>>,
    price_ticks: Vec<Option<u64>>,
    sizes: Vec<Option<i64>>,
    order_ids: Vec<Option<String>>,
    trade_ids: Vec<Option<String>>,
    reasons: Vec<Option<String>>,
    payload_jsons: Vec<Option<String>>,
}

impl ParquetEventWriter {
    pub fn new() -> Self {
        Self::with_batch_size(10_000)
    }

    pub fn with_batch_size(batch_size: usize) -> Self {
        Self {
            writer: None,
            schema: Arc::new(event_schema()),
            batch_size,
            event_types: Vec::new(),
            ts_exchange_ms: Vec::new(),
            ts_recv_mono_ns: Vec::new(),
            ts_process_mono_ns: Vec::new(),
            sides: Vec::new(),
            price_ticks: Vec::new(),
            sizes: Vec::new(),
            order_ids: Vec::new(),
            trade_ids: Vec::new(),
            reasons: Vec::new(),
            payload_jsons: Vec::new(),
        }
    }

    /// Open a new Parquet file for writing.
    pub fn open(&mut self, path: impl AsRef<Path>) -> Result<(), RecorderError> {
        let file = File::create(path)?;

        let props = WriterProperties::builder()
            .set_compression(Compression::SNAPPY)
            .build();

        let writer = ArrowWriter::try_new(file, self.schema.clone(), Some(props))?;
        self.writer = Some(writer);
        self.clear_buffers();

        Ok(())
    }

    /// Write events to the file.
    pub fn write_events(&mut self, events: &[CoreEvent]) -> Result<(), RecorderError> {
        for event in events {
            self.buffer_event(event);

            if self.event_types.len() >= self.batch_size {
                self.flush_batch()?;
            }
        }

        Ok(())
    }

    /// Write a single event.
    pub fn write_event(&mut self, event: &CoreEvent) -> Result<(), RecorderError> {
        self.buffer_event(event);

        if self.event_types.len() >= self.batch_size {
            self.flush_batch()?;
        }

        Ok(())
    }

    fn buffer_event(&mut self, event: &CoreEvent) {
        match event {
            CoreEvent::BookUpdate(e) => {
                self.event_types.push("BookUpdate".into());
                self.ts_exchange_ms.push(e.ts_exchange_ms);
                self.ts_recv_mono_ns.push(e.ts_recv_mono_ns);
                self.ts_process_mono_ns.push(e.ts_process_mono_ns);
                self.sides.push(Some(e.side as u8));
                self.price_ticks.push(Some(e.price_tick as u64));
                self.sizes.push(Some(e.new_size as i64));
                self.order_ids.push(None);
                self.trade_ids.push(None);
                self.reasons.push(None);
                self.payload_jsons.push(None);
            }
            CoreEvent::Trade(e) => {
                self.event_types.push("Trade".into());
                self.ts_exchange_ms.push(e.ts_exchange_ms);
                self.ts_recv_mono_ns.push(e.ts_recv_mono_ns);
                self.ts_process_mono_ns.push(e.ts_process_mono_ns);
                self.sides.push(Some(e.side as u8));
                self.price_ticks.push(Some(e.price_tick as u64));
                self.sizes.push(Some(e.size as i64));
                self.order_ids.push(None);
                self.trade_ids.push(Some(e.trade_id.clone()));
                self.reasons.push(None);
                self.payload_jsons.push(None);
            }
            CoreEvent::OrderAck(e) => {
                self.event_types.push("OrderAck".into());
                self.ts_exchange_ms.push(e.ts_exchange_ms);
                self.ts_recv_mono_ns.push(e.ts_recv_mono_ns);
                self.ts_process_mono_ns.push(e.ts_process_mono_ns);
                self.sides.push(Some(e.side as u8));
                self.price_ticks.push(Some(e.price_tick as u64));
                self.sizes.push(Some(e.size as i64));
                self.order_ids.push(Some(e.order_id.clone()));
                self.trade_ids.push(None);
                self.reasons.push(None);
                self.payload_jsons.push(None);
            }
            CoreEvent::OrderFill(e) => {
                self.event_types.push("OrderFill".into());
                self.ts_exchange_ms.push(e.ts_exchange_ms);
                self.ts_recv_mono_ns.push(e.ts_recv_mono_ns);
                self.ts_process_mono_ns.push(e.ts_process_mono_ns);
                self.sides.push(Some(e.side as u8));
                self.price_ticks.push(Some(e.fill_price_tick as u64));
                self.sizes.push(Some(e.fill_size as i64));
                self.order_ids.push(Some(e.order_id.clone()));
                self.trade_ids.push(None);
                self.reasons.push(None);
                self.payload_jsons.push(Some(serde_json::json!({
                    "is_maker": e.is_maker,
                    "fee_micro_usdc": e.fee_micro_usdc,
                    "remaining_size": e.remaining_size,
                }).to_string()));
            }
            CoreEvent::OrderCancel(e) => {
                self.event_types.push("OrderCancel".into());
                self.ts_exchange_ms.push(e.ts_exchange_ms);
                self.ts_recv_mono_ns.push(e.ts_recv_mono_ns);
                self.ts_process_mono_ns.push(e.ts_process_mono_ns);
                self.sides.push(None);
                self.price_ticks.push(None);
                self.sizes.push(None);
                self.order_ids.push(Some(e.order_id.clone()));
                self.trade_ids.push(None);
                self.reasons.push(Some(e.reason.clone()));
                self.payload_jsons.push(None);
            }
            CoreEvent::OrderReject(e) => {
                self.event_types.push("OrderReject".into());
                self.ts_exchange_ms.push(e.ts_exchange_ms);
                self.ts_recv_mono_ns.push(e.ts_recv_mono_ns);
                self.ts_process_mono_ns.push(e.ts_process_mono_ns);
                self.sides.push(None);
                self.price_ticks.push(None);
                self.sizes.push(None);
                self.order_ids.push(Some(e.order_id.clone()));
                self.trade_ids.push(None);
                self.reasons.push(Some(e.reason.clone()));
                self.payload_jsons.push(None);
            }
            CoreEvent::StrategySignal(e) => {
                self.event_types.push("StrategySignal".into());
                self.ts_exchange_ms.push(e.timestamp_ns / 1_000_000);
                self.ts_recv_mono_ns.push(e.timestamp_ns);
                self.ts_process_mono_ns.push(e.timestamp_ns);
                self.sides.push(None);
                self.price_ticks.push(None);
                self.sizes.push(None);
                self.order_ids.push(None);
                self.trade_ids.push(None);
                self.reasons.push(None);
                self.payload_jsons.push(Some(serde_json::json!({
                    "signal_name": e.signal_name,
                    "signal_value": e.signal_value,
                }).to_string()));
            }
            CoreEvent::RiskEvent(e) => {
                self.event_types.push("RiskEvent".into());
                self.ts_exchange_ms.push(e.timestamp_ns / 1_000_000);
                self.ts_recv_mono_ns.push(e.timestamp_ns);
                self.ts_process_mono_ns.push(e.timestamp_ns);
                self.sides.push(None);
                self.price_ticks.push(None);
                self.sizes.push(None);
                self.order_ids.push(None);
                self.trade_ids.push(None);
                self.reasons.push(Some(e.event_type.clone()));
                self.payload_jsons.push(Some(e.details.clone()));
            }
            CoreEvent::SystemHealth(e) => {
                self.event_types.push("SystemHealth".into());
                self.ts_exchange_ms.push(e.timestamp_ns / 1_000_000);
                self.ts_recv_mono_ns.push(e.timestamp_ns);
                self.ts_process_mono_ns.push(e.timestamp_ns);
                self.sides.push(None);
                self.price_ticks.push(None);
                self.sizes.push(None);
                self.order_ids.push(None);
                self.trade_ids.push(None);
                self.reasons.push(Some(e.state.clone()));
                self.payload_jsons.push(e.reason.clone());
            }
        }
    }

    fn flush_batch(&mut self) -> Result<(), RecorderError> {
        if self.event_types.is_empty() {
            return Ok(());
        }

        let writer = self.writer.as_mut().ok_or(RecorderError::NotOpen)?;

        // Build arrays
        let event_type_arr = StringArray::from(self.event_types.clone());
        let ts_exchange_arr = UInt64Array::from(self.ts_exchange_ms.clone());
        let ts_recv_arr = UInt64Array::from(self.ts_recv_mono_ns.clone());
        let ts_process_arr = UInt64Array::from(self.ts_process_mono_ns.clone());
        let side_arr = UInt8Array::from(self.sides.clone());
        let price_arr = UInt64Array::from(self.price_ticks.clone());
        let size_arr = Int64Array::from(self.sizes.clone());
        let order_id_arr = StringArray::from(self.order_ids.clone());
        let trade_id_arr = StringArray::from(self.trade_ids.clone());
        let reason_arr = StringArray::from(self.reasons.clone());
        let payload_arr = StringArray::from(self.payload_jsons.clone());

        let batch = RecordBatch::try_new(
            self.schema.clone(),
            vec![
                Arc::new(event_type_arr),
                Arc::new(ts_exchange_arr),
                Arc::new(ts_recv_arr),
                Arc::new(ts_process_arr),
                Arc::new(side_arr),
                Arc::new(price_arr),
                Arc::new(size_arr),
                Arc::new(order_id_arr),
                Arc::new(trade_id_arr),
                Arc::new(reason_arr),
                Arc::new(payload_arr),
            ],
        )?;

        writer.write(&batch)?;
        self.clear_buffers();

        Ok(())
    }

    fn clear_buffers(&mut self) {
        self.event_types.clear();
        self.ts_exchange_ms.clear();
        self.ts_recv_mono_ns.clear();
        self.ts_process_mono_ns.clear();
        self.sides.clear();
        self.price_ticks.clear();
        self.sizes.clear();
        self.order_ids.clear();
        self.trade_ids.clear();
        self.reasons.clear();
        self.payload_jsons.clear();
    }

    /// Close the writer and finalize the file.
    pub fn close(&mut self) -> Result<(), RecorderError> {
        self.flush_batch()?;
        if let Some(writer) = self.writer.take() {
            writer.close()?;
        }
        Ok(())
    }

    /// Check if writer is open.
    pub fn is_open(&self) -> bool {
        self.writer.is_some()
    }
}

impl Default for ParquetEventWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for ParquetEventWriter {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mtrader_core::events::{BookUpdateEvent, TradeEvent};
    use mtrader_core::Side;
    use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
    use tempfile::tempdir;

    #[test]
    fn test_parquet_write_read() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("events.parquet");

        // Write events
        {
            let mut writer = ParquetEventWriter::new();
            writer.open(&path).unwrap();

            let events = vec![
                CoreEvent::BookUpdate(BookUpdateEvent {
                    side: Side::Buy,
                    price_tick: 50,
                    new_size: 10000,
                    ts_exchange_ms: 1000,
                    ts_recv_mono_ns: 1000000,
                    ts_process_mono_ns: 1001000,
                }),
                CoreEvent::Trade(TradeEvent {
                    side: Side::Sell,
                    price_tick: 51,
                    size: 500,
                    trade_id: "trade-1".into(),
                    ts_exchange_ms: 2000,
                    ts_recv_mono_ns: 2000000,
                    ts_process_mono_ns: 2001000,
                }),
            ];

            writer.write_events(&events).unwrap();
            writer.close().unwrap();
        }

        // Read back and verify
        let file = File::open(&path).unwrap();
        let reader = ParquetRecordBatchReaderBuilder::try_new(file)
            .unwrap()
            .build()
            .unwrap();

        let mut total_rows = 0;
        for batch_result in reader {
            let batch = batch_result.unwrap();
            total_rows += batch.num_rows();
        }

        assert_eq!(total_rows, 2);
    }
}
