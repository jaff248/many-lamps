//! Recording command.

use crate::config::Config;
use anyhow::Result;
use mtrader_core::clock::MonotonicClock;
use mtrader_core::events::{CoreEvent, EventTimestamps, MarketDataEvent};
use mtrader_core::{MarketId, Side, TokenId};
use mtrader_gateway::{ParsedEvent, WsClient, WsConfig};
use mtrader_recorder::{event_recorder::EventRecorderConfig, EventRecorder, FrameRecorder};
use std::path::PathBuf;
use std::time::Duration;
use tracing::{error, info};

/// Parse duration string (e.g., "1h", "30m", "24h").
fn parse_duration(s: &str) -> Option<Duration> {
    let s = s.trim().to_lowercase();

    if s.ends_with('h') {
        let hours: u64 = s.trim_end_matches('h').parse().ok()?;
        Some(Duration::from_secs(hours * 3600))
    } else if s.ends_with('m') {
        let mins: u64 = s.trim_end_matches('m').parse().ok()?;
        Some(Duration::from_secs(mins * 60))
    } else if s.ends_with('s') {
        let secs: u64 = s.trim_end_matches('s').parse().ok()?;
        Some(Duration::from_secs(secs))
    } else {
        // Assume seconds
        let secs: u64 = s.parse().ok()?;
        Some(Duration::from_secs(secs))
    }
}

/// Run recording mode.
pub async fn run(
    config: &Config,
    market: &str,
    output: &str,
    duration: Option<&str>,
) -> Result<()> {
    info!(market = market, output = output, "Starting recording mode");

    // Parse duration
    let record_duration = duration.and_then(parse_duration);
    if let Some(d) = &record_duration {
        info!("Recording for {:?}", d);
    } else {
        info!("Recording indefinitely - press Ctrl+C to stop");
    }

    // Create output directory
    let output_dir = PathBuf::from(output);
    std::fs::create_dir_all(&output_dir)?;

    // Initialize clock
    let clock = MonotonicClock::new();
    let start_time = clock.now_ns();

    // Initialize frame recorder
    let mut frame_recorder = if config.recording.record_raw_frames {
        let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
        let frame_path = output_dir.join(format!("frames_{}_{}.bin.gz", market, timestamp));
        let mut rec = FrameRecorder::new();
        rec.open(&frame_path)?;
        info!("Raw frame recording: {}", frame_path.display());
        Some(rec)
    } else {
        None
    };

    // Initialize event recorder
    let mut event_recorder = if config.recording.record_events {
        let rec_config = EventRecorderConfig {
            output_dir: output_dir.clone(),
            file_prefix: format!("events_{}", market),
            max_events_per_file: config.recording.max_events_per_file,
            ..Default::default()
        };
        Some(EventRecorder::new(rec_config))
    } else {
        None
    };

    // Connect to market
    info!("Connecting to market: {}", market);
    let ws_config = WsConfig {
        url: config.gateway.ws_url.clone(),
        ..Default::default()
    };
    let ws_client = WsClient::new(ws_config);
    let (mut frame_rx, _cmd_tx) = ws_client.run(vec![market.to_string()]);

    let mut message_count = 0u64;
    let mut book_updates = 0u64;
    let mut trades = 0u64;

    // Recording loop
    loop {
        // Check duration
        if let Some(max_duration) = record_duration {
            let elapsed_ns = (clock.now_ns() - start_time).max(0) as u64;
            if elapsed_ns > max_duration.as_nanos() as u64 {
                info!("Recording duration reached");
                break;
            }
        }

        tokio::select! {
            Some(frame_result) = frame_rx.recv() => {
                let frame = match frame_result {
                    Ok(frame) => frame,
                    Err(e) => {
                        error!("WebSocket error: {}", e);
                        tokio::time::sleep(Duration::from_millis(config.gateway.reconnect_delay_ms)).await;
                        continue;
                    }
                };

                let ts_recv = frame.ts_recv_mono_ns as i64;
                let wall_time = chrono::Utc::now().timestamp_millis() as u64;
                let ts_process = clock.now_ns();
                message_count += 1;

                if let Some(ref mut rec) = frame_recorder {
                    let _ = rec.record_raw(frame.ts_recv_mono_ns, wall_time, &frame.raw_json);
                }

                for event in frame.events {
                    match event {
                        ParsedEvent::Book(book_snapshot) => {
                            book_updates += 1;
                            let timestamps = EventTimestamps::new(
                                book_snapshot.timestamp_ms as i64,
                                ts_recv,
                            )
                            .with_process_time(ts_process);

                            let event = CoreEvent::MarketData(MarketDataEvent::BookSnapshot {
                                market_id: MarketId(market.to_string()),
                                token_id: TokenId(book_snapshot.asset_id.clone()),
                                bids: book_snapshot.bids,
                                asks: book_snapshot.asks,
                                tick_size: 0,
                                snapshot_hash: book_snapshot.hash,
                                timestamps,
                            });

                            if let Some(ref mut rec) = event_recorder {
                                let _ = rec.record(event);
                            }
                        }
                        ParsedEvent::PriceChange(change) => {
                            for update in change.price_changes {
                                book_updates += 1;
                                let timestamps = EventTimestamps::new(
                                    change.timestamp_ms as i64,
                                    ts_recv,
                                )
                                .with_process_time(ts_process);

                                let event = CoreEvent::MarketData(MarketDataEvent::BookDelta {
                                    market_id: MarketId(market.to_string()),
                                    token_id: TokenId(change.asset_id.clone()),
                                    side: update.side,
                                    tick: update.price_tick,
                                    new_size: update.size,
                                    best_bid: None,
                                    best_ask: None,
                                    order_hash: String::new(),
                                    timestamps,
                                });

                                if let Some(ref mut rec) = event_recorder {
                                    let _ = rec.record(event);
                                }
                            }
                        }
                        ParsedEvent::LastTradePrice(trade) => {
                            trades += 1;
                            let timestamps = EventTimestamps::new(
                                trade.timestamp_ms as i64,
                                ts_recv,
                            )
                            .with_process_time(ts_process);

                            let event = CoreEvent::MarketData(MarketDataEvent::Trade {
                                market_id: MarketId(market.to_string()),
                                token_id: TokenId(trade.asset_id.clone()),
                                side: Side::Buy,
                                price_tick: trade.price_tick,
                                size: trade.size_shares,
                                fee_rate_bps: 0,
                                timestamps,
                            });

                            if let Some(ref mut rec) = event_recorder {
                                let _ = rec.record(event);
                            }
                        }
                        ParsedEvent::TickSizeChange(change) => {
                            let timestamps = EventTimestamps::new(
                                change.timestamp_ms as i64,
                                ts_recv,
                            )
                            .with_process_time(ts_process);

                            let event = CoreEvent::MarketData(MarketDataEvent::TickSizeChange {
                                market_id: MarketId(market.to_string()),
                                token_id: TokenId(change.asset_id.clone()),
                                old_tick_size: change.old_tick_bps,
                                new_tick_size: change.new_tick_bps,
                                timestamps,
                            });

                            if let Some(ref mut rec) = event_recorder {
                                let _ = rec.record(event);
                            }
                        }
                        _ => {}
                    }
                }
            }

            // Periodic status update
            _ = tokio::time::sleep(Duration::from_secs(30)) => {
                info!(
                    messages = message_count,
                    book_updates = book_updates,
                    trades = trades,
                    "Recording status"
                );

                // Flush recorders
                if let Some(ref mut rec) = frame_recorder {
                    let _ = rec.flush();
                }
                if let Some(ref mut rec) = event_recorder {
                    let _ = rec.flush();
                }
            }
        }
    }

    // Cleanup
    info!("Shutting down recording");

    if let Some(ref mut rec) = frame_recorder {
        rec.close()?;
        info!(
            frames = rec.frames_written(),
            bytes = rec.bytes_written(),
            "Raw frames saved"
        );
    }

    if let Some(ref mut rec) = event_recorder {
        rec.close()?;
        info!(
            events = rec.total_events(),
            files = rec.files_written(),
            "Events saved"
        );
    }

    info!(
        total_messages = message_count,
        book_updates = book_updates,
        trades = trades,
        "Recording complete"
    );

    Ok(())
}
