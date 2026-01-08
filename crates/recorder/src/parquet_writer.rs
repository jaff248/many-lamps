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
            CoreEvent::MarketData(data) => match data {
                mtrader_core::events::MarketDataEvent::BookSnapshot {
                    market_id,
                    token_id,
                    tick_size,
                    snapshot_hash,
                    timestamps,
                    bids,
                    asks,
                } => {
                    self.event_types.push("BookSnapshot".into());
                    self.ts_exchange_ms.push(timestamps.ts_exchange_ms.max(0) as u64);
                    self.ts_recv_mono_ns.push(timestamps.ts_recv_mono_ns.max(0) as u64);
                    self.ts_process_mono_ns.push(timestamps.ts_process_mono_ns.max(0) as u64);
                    self.sides.push(None);
                    self.price_ticks.push(None);
                    self.sizes.push(None);
                    self.order_ids.push(None);
                    self.trade_ids.push(None);
                    self.reasons.push(None);
                    self.payload_jsons.push(Some(serde_json::json!({
                        "market_id": market_id,
                        "token_id": token_id,
                        "tick_size": tick_size,
                        "snapshot_hash": snapshot_hash,
                        "bid_levels": bids.len(),
                        "ask_levels": asks.len(),
                    }).to_string()));
                }
                mtrader_core::events::MarketDataEvent::BookDelta {
                    market_id,
                    token_id,
                    side,
                    tick,
                    new_size,
                    best_bid,
                    best_ask,
                    order_hash,
                    timestamps,
                } => {
                    self.event_types.push("BookDelta".into());
                    self.ts_exchange_ms.push(timestamps.ts_exchange_ms.max(0) as u64);
                    self.ts_recv_mono_ns.push(timestamps.ts_recv_mono_ns.max(0) as u64);
                    self.ts_process_mono_ns.push(timestamps.ts_process_mono_ns.max(0) as u64);
                    self.sides.push(Some(*side as u8));
                    self.price_ticks.push(Some(*tick as u64));
                    self.sizes.push(Some(*new_size as i64));
                    self.order_ids.push(None);
                    self.trade_ids.push(None);
                    self.reasons.push(None);
                    self.payload_jsons.push(Some(serde_json::json!({
                        "market_id": market_id,
                        "token_id": token_id,
                        "best_bid": best_bid,
                        "best_ask": best_ask,
                        "order_hash": order_hash,
                    }).to_string()));
                }
                mtrader_core::events::MarketDataEvent::Trade {
                    market_id,
                    token_id,
                    side,
                    price_tick,
                    size,
                    fee_rate_bps,
                    timestamps,
                } => {
                    self.event_types.push("Trade".into());
                    self.ts_exchange_ms.push(timestamps.ts_exchange_ms.max(0) as u64);
                    self.ts_recv_mono_ns.push(timestamps.ts_recv_mono_ns.max(0) as u64);
                    self.ts_process_mono_ns.push(timestamps.ts_process_mono_ns.max(0) as u64);
                    self.sides.push(Some(*side as u8));
                    self.price_ticks.push(Some(*price_tick as u64));
                    self.sizes.push(Some(*size as i64));
                    self.order_ids.push(None);
                    self.trade_ids.push(None);
                    self.reasons.push(None);
                    self.payload_jsons.push(Some(serde_json::json!({
                        "market_id": market_id,
                        "token_id": token_id,
                        "fee_rate_bps": fee_rate_bps,
                    }).to_string()));
                }
                mtrader_core::events::MarketDataEvent::TickSizeChange {
                    market_id,
                    token_id,
                    old_tick_size,
                    new_tick_size,
                    timestamps,
                } => {
                    self.event_types.push("TickSizeChange".into());
                    self.ts_exchange_ms.push(timestamps.ts_exchange_ms.max(0) as u64);
                    self.ts_recv_mono_ns.push(timestamps.ts_recv_mono_ns.max(0) as u64);
                    self.ts_process_mono_ns.push(timestamps.ts_process_mono_ns.max(0) as u64);
                    self.sides.push(None);
                    self.price_ticks.push(None);
                    self.sizes.push(None);
                    self.order_ids.push(None);
                    self.trade_ids.push(None);
                    self.reasons.push(None);
                    self.payload_jsons.push(Some(serde_json::json!({
                        "market_id": market_id,
                        "token_id": token_id,
                        "old_tick_size": old_tick_size,
                        "new_tick_size": new_tick_size,
                    }).to_string()));
                }
                mtrader_core::events::MarketDataEvent::BestBidAsk {
                    market_id,
                    token_id,
                    best_bid,
                    best_ask,
                    spread,
                    timestamps,
                } => {
                    self.event_types.push("BestBidAsk".into());
                    self.ts_exchange_ms.push(timestamps.ts_exchange_ms.max(0) as u64);
                    self.ts_recv_mono_ns.push(timestamps.ts_recv_mono_ns.max(0) as u64);
                    self.ts_process_mono_ns.push(timestamps.ts_process_mono_ns.max(0) as u64);
                    self.sides.push(None);
                    self.price_ticks.push(None);
                    self.sizes.push(None);
                    self.order_ids.push(None);
                    self.trade_ids.push(None);
                    self.reasons.push(None);
                    self.payload_jsons.push(Some(serde_json::json!({
                        "market_id": market_id,
                        "token_id": token_id,
                        "best_bid": best_bid,
                        "best_ask": best_ask,
                        "spread": spread,
                    }).to_string()));
                }
                mtrader_core::events::MarketDataEvent::ConnectionStatus { connected, timestamp_mono_ns } => {
                    self.event_types.push("ConnectionStatus".into());
                    self.ts_exchange_ms.push(0);
                    self.ts_recv_mono_ns.push(*timestamp_mono_ns as u64);
                    self.ts_process_mono_ns.push(*timestamp_mono_ns as u64);
                    self.sides.push(None);
                    self.price_ticks.push(None);
                    self.sizes.push(None);
                    self.order_ids.push(None);
                    self.trade_ids.push(None);
                    self.reasons.push(None);
                    self.payload_jsons.push(Some(serde_json::json!({
                        "connected": connected,
                    }).to_string()));
                }
                mtrader_core::events::MarketDataEvent::ParseError { error, timestamp_mono_ns, raw_bytes } => {
                    self.event_types.push("ParseError".into());
                    self.ts_exchange_ms.push(0);
                    self.ts_recv_mono_ns.push(*timestamp_mono_ns as u64);
                    self.ts_process_mono_ns.push(*timestamp_mono_ns as u64);
                    self.sides.push(None);
                    self.price_ticks.push(None);
                    self.sizes.push(None);
                    self.order_ids.push(None);
                    self.trade_ids.push(None);
                    self.reasons.push(None);
                    self.payload_jsons.push(Some(serde_json::json!({
                        "error": error,
                        "raw_len": raw_bytes.len(),
                    }).to_string()));
                }
            },
            CoreEvent::Signal(signal) => {
                self.event_types.push("Signal".into());
                self.ts_exchange_ms.push((signal.timestamp_mono_ns / 1_000_000) as u64);
                self.ts_recv_mono_ns.push(signal.timestamp_mono_ns as u64);
                self.ts_process_mono_ns.push(signal.timestamp_mono_ns as u64);
                self.sides.push(None);
                self.price_ticks.push(None);
                self.sizes.push(None);
                self.order_ids.push(None);
                self.trade_ids.push(None);
                self.reasons.push(None);
                self.payload_jsons.push(Some(serde_json::json!({
                    "strategy_id": signal.strategy_id,
                    "market_id": signal.market_id,
                    "token_id": signal.token_id,
                    "signal_type": signal.signal_type,
                }).to_string()));
            }
            CoreEvent::OrderIntent(intent) => {
                self.event_types.push("OrderIntent".into());
                self.ts_exchange_ms.push((intent.timestamp_mono_ns / 1_000_000) as u64);
                self.ts_recv_mono_ns.push(intent.timestamp_mono_ns as u64);
                self.ts_process_mono_ns.push(intent.timestamp_mono_ns as u64);
                self.sides.push(Some(intent.side as u8));
                self.price_ticks.push(Some(intent.price_tick as u64));
                self.sizes.push(Some(intent.size as i64));
                self.order_ids.push(Some(intent.client_order_id.0.clone()));
                self.trade_ids.push(None);
                self.reasons.push(Some(format!("{:?}", intent.reason)));
                self.payload_jsons.push(Some(serde_json::json!({
                    "strategy_id": intent.strategy_id,
                    "market_id": intent.market_id,
                    "token_id": intent.token_id,
                    "order_type": intent.order_type,
                }).to_string()));
            }
            CoreEvent::OrderAck(ack) => {
                self.event_types.push("OrderAck".into());
                self.ts_exchange_ms.push((ack.timestamp_mono_ns / 1_000_000) as u64);
                self.ts_recv_mono_ns.push(ack.timestamp_mono_ns as u64);
                self.ts_process_mono_ns.push(ack.timestamp_mono_ns as u64);
                self.sides.push(None);
                self.price_ticks.push(None);
                self.sizes.push(None);
                self.order_ids.push(Some(ack.client_order_id.0.clone()));
                self.trade_ids.push(None);
                self.reasons.push(Some(format!("{:?}", ack.status)));
                self.payload_jsons.push(Some(serde_json::json!({
                    "exchange_order_id": ack.exchange_order_id,
                }).to_string()));
            }
            CoreEvent::Fill(fill) => {
                self.event_types.push("Fill".into());
                self.ts_exchange_ms.push(fill.timestamps.ts_exchange_ms.max(0) as u64);
                self.ts_recv_mono_ns.push(fill.timestamps.ts_recv_mono_ns.max(0) as u64);
                self.ts_process_mono_ns.push(fill.timestamps.ts_process_mono_ns.max(0) as u64);
                self.sides.push(Some(fill.side as u8));
                self.price_ticks.push(Some(fill.price_tick as u64));
                self.sizes.push(Some(fill.fill_size as i64));
                self.order_ids.push(Some(fill.exchange_order_id.clone()));
                self.trade_ids.push(Some(fill.exchange_trade_id.clone()));
                self.reasons.push(None);
                self.payload_jsons.push(Some(serde_json::json!({
                    "client_order_id": fill.client_order_id,
                    "strategy_id": fill.strategy_id,
                    "market_id": fill.market_id,
                    "token_id": fill.token_id,
                    "remaining_size": fill.remaining_size,
                    "is_maker": fill.is_maker,
                    "fee_amount": fill.fee_amount,
                }).to_string()));
            }
            CoreEvent::CancelAck(cancel) => {
                self.event_types.push("CancelAck".into());
                self.ts_exchange_ms.push((cancel.timestamp_mono_ns / 1_000_000) as u64);
                self.ts_recv_mono_ns.push(cancel.timestamp_mono_ns as u64);
                self.ts_process_mono_ns.push(cancel.timestamp_mono_ns as u64);
                self.sides.push(None);
                self.price_ticks.push(None);
                self.sizes.push(None);
                self.order_ids.push(Some(cancel.client_order_id.0.clone()));
                self.trade_ids.push(None);
                self.reasons.push(Some(format!("{:?}", cancel.status)));
                self.payload_jsons.push(None);
            }
            CoreEvent::Risk(risk) => {
                self.event_types.push("Risk".into());
                self.ts_exchange_ms.push(0);
                self.ts_recv_mono_ns.push(0);
                self.ts_process_mono_ns.push(0);
                self.sides.push(None);
                self.price_ticks.push(None);
                self.sizes.push(None);
                self.order_ids.push(None);
                self.trade_ids.push(None);
                self.reasons.push(None);
                self.payload_jsons.push(Some(serde_json::to_string(risk).unwrap_or_default()));
            }
            CoreEvent::System(system) => {
                self.event_types.push("System".into());
                let timestamp_ns = match system {
                    mtrader_core::events::SystemEvent::SafeMode { timestamp_mono_ns, .. }
                    | mtrader_core::events::SystemEvent::SafeModeCleared { timestamp_mono_ns }
                    | mtrader_core::events::SystemEvent::ResnaphotRequested { timestamp_mono_ns, .. }
                    | mtrader_core::events::SystemEvent::MarketLifecycle { timestamp_mono_ns, .. }
                    | mtrader_core::events::SystemEvent::Heartbeat { timestamp_mono_ns } => *timestamp_mono_ns,
                };
                self.ts_exchange_ms.push((timestamp_ns / 1_000_000) as u64);
                self.ts_recv_mono_ns.push(timestamp_ns as u64);
                self.ts_process_mono_ns.push(timestamp_ns as u64);
                self.sides.push(None);
                self.price_ticks.push(None);
                self.sizes.push(None);
                self.order_ids.push(None);
                self.trade_ids.push(None);
                self.reasons.push(None);
                self.payload_jsons.push(Some(serde_json::to_string(system).unwrap_or_default()));
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
    use mtrader_core::events::{EventTimestamps, MarketDataEvent};
    use mtrader_core::{MarketId, Side, TokenId};
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
                CoreEvent::MarketData(MarketDataEvent::BookDelta {
                    market_id: MarketId("market".to_string()),
                    token_id: TokenId("token".to_string()),
                    side: Side::Buy,
                    tick: 5000,
                    new_size: 10000,
                    best_bid: None,
                    best_ask: None,
                    order_hash: "hash".to_string(),
                    timestamps: EventTimestamps {
                        ts_exchange_ms: 1000,
                        ts_recv_mono_ns: 1_000_000,
                        ts_process_mono_ns: 1_001_000,
                    },
                }),
                CoreEvent::MarketData(MarketDataEvent::Trade {
                    market_id: MarketId("market".to_string()),
                    token_id: TokenId("token".to_string()),
                    side: Side::Sell,
                    price_tick: 5100,
                    size: 500,
                    fee_rate_bps: 1000,
                    timestamps: EventTimestamps {
                        ts_exchange_ms: 2000,
                        ts_recv_mono_ns: 2_000_000,
                        ts_process_mono_ns: 2_001_000,
                    },
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
