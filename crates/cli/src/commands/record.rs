//! Recording command.

use crate::config::Config;
use anyhow::Result;
use mtrader_core::clock::MonotonicClock;
use mtrader_core::events::CoreEvent;
use mtrader_core::Side;
use mtrader_gateway::{WsClient, WsMessage};
use mtrader_recorder::{
    event_recorder::EventRecorderConfig, EventRecorder, FrameRecorder,
};
use std::path::PathBuf;
use std::time::Duration;
use tracing::{error, info, warn};

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
    let mut ws_client = WsClient::new(&config.gateway.ws_url)?;

    // Subscribe to market
    ws_client.subscribe_book(market).await?;
    ws_client.subscribe_trades(market).await?;

    let mut message_count = 0u64;
    let mut book_updates = 0u64;
    let mut trades = 0u64;

    // Recording loop
    loop {
        // Check duration
        if let Some(max_duration) = record_duration {
            let elapsed_ns = clock.now_ns() - start_time;
            if elapsed_ns > max_duration.as_nanos() as u64 {
                info!("Recording duration reached");
                break;
            }
        }

        tokio::select! {
            msg = ws_client.next_message() => {
                let msg = match msg {
                    Ok(Some(m)) => m,
                    Ok(None) => {
                        warn!("WebSocket connection closed");
                        break;
                    }
                    Err(e) => {
                        error!("WebSocket error: {}", e);
                        // Try to reconnect
                        tokio::time::sleep(Duration::from_millis(config.gateway.reconnect_delay_ms)).await;
                        continue;
                    }
                };

                let ts_recv = clock.now_ns();
                let wall_time = chrono::Utc::now().timestamp_millis() as u64;
                message_count += 1;

                // Record raw frame
                if let Some(ref mut rec) = frame_recorder {
                    // Serialize message to bytes
                    if let Ok(json) = serde_json::to_vec(&msg) {
                        let _ = rec.record_raw(ts_recv, wall_time, &json);
                    }
                }

                // Process and record event
                match &msg {
                    WsMessage::BookUpdate { side, price, size, .. } => {
                        book_updates += 1;
                        let side = if *side == "buy" { Side::Buy } else { Side::Sell };
                        let tick = (*price * 100.0) as u16;
                        let ts_process = clock.now_ns();

                        let event = CoreEvent::BookUpdate(mtrader_core::events::BookUpdateEvent {
                            side,
                            price_tick: tick,
                            new_size: *size as i64,
                            ts_exchange_ms: wall_time,
                            ts_recv_mono_ns: ts_recv,
                            ts_process_mono_ns: ts_process,
                        });

                        if let Some(ref mut rec) = event_recorder {
                            let _ = rec.record(event);
                        }
                    }

                    WsMessage::Trade { side, price, size, id, .. } => {
                        trades += 1;
                        let side = if *side == "buy" { Side::Buy } else { Side::Sell };
                        let tick = (*price * 100.0) as u16;
                        let ts_process = clock.now_ns();

                        let event = CoreEvent::Trade(mtrader_core::events::TradeEvent {
                            side,
                            price_tick: tick,
                            size: *size as i64,
                            trade_id: id.clone(),
                            ts_exchange_ms: wall_time,
                            ts_recv_mono_ns: ts_recv,
                            ts_process_mono_ns: ts_process,
                        });

                        if let Some(ref mut rec) = event_recorder {
                            let _ = rec.record(event);
                        }
                    }

                    _ => {}
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
