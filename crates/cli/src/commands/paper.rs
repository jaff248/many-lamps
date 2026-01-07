//! Paper trading command.

use crate::config::Config;
use anyhow::Result;
use mtrader_book::ArrayBook;
use mtrader_core::clock::MonotonicClock;
use mtrader_core::events::CoreEvent;
use mtrader_core::health::{SafeModeReason, SystemHealth};
use mtrader_core::Side;
use mtrader_gateway::{WsClient, WsMessage};
use mtrader_recorder::EventRecorder;
use mtrader_risk::{CircuitBreaker, PnLTracker, Position, PositionLimits};
use mtrader_sim::{FillSimConfig, FillSimulator, PaperBook, PaperOrder};
use mtrader_strategy::{MakerMMConfig, MakerMMStrategy, Strategy, StrategyAction, StrategyContext};
use std::collections::VecDeque;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

/// Run paper trading mode.
pub async fn run(config: &Config, market: &str, strategy_name: &str, record: bool) -> Result<()> {
    info!(
        market = market,
        strategy = strategy_name,
        "Starting paper trading mode"
    );

    // Ensure safe mode is enabled for paper trading
    if !config.safe_mode.enabled {
        warn!("Safe mode is disabled - enabling for paper trading");
    }

    // Initialize components
    let clock = MonotonicClock::new();
    let mut health = SystemHealth::new();
    let mut book = ArrayBook::new(100); // 1% tick size
    let mut paper_book = PaperBook::new(100);
    let mut position = Position::new();
    let mut pnl_tracker = PnLTracker::new();
    let mut circuit_breaker = CircuitBreaker::new();

    let limits = PositionLimits {
        max_position: config.risk.max_position,
        max_notional: config.risk.max_position as u64 * 2,
        max_open_orders: config.risk.max_open_orders,
        max_order_size: config.strategy.order_size as i64,
    };

    // Initialize fill simulator
    let fill_config = FillSimConfig {
        fee_rate_bps: config.risk.fee_rate_bps,
        ..Default::default()
    };
    let mut fill_sim = FillSimulator::new(fill_config);

    // Initialize strategy
    let strategy_config = MakerMMConfig {
        spread_ticks: config.strategy.spread_ticks,
        order_size: config.strategy.order_size as i64,
        num_levels: config.strategy.num_levels,
        skew_factor: config.strategy.skew_factor,
        requote_threshold_ticks: config.strategy.requote_threshold,
        min_edge_ticks: config.strategy.min_edge_ticks,
    };
    let mut strategy = MakerMMStrategy::new(strategy_config);

    // Initialize recorder if requested
    let mut recorder = if record {
        let rec_config = mtrader_recorder::event_recorder::EventRecorderConfig {
            output_dir: config.recording.output_dir.clone().into(),
            file_prefix: format!("paper_{}", market),
            ..Default::default()
        };
        Some(EventRecorder::new(rec_config))
    } else {
        None
    };

    // Create channels for message passing
    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<CoreEvent>();

    // Connect to market
    info!("Connecting to market: {}", market);
    let mut ws_client = WsClient::new(&config.gateway.ws_url)?;

    // Subscribe to market
    ws_client.subscribe_book(market).await?;
    ws_client.subscribe_trades(market).await?;

    // Main event loop
    let mut pending_actions: VecDeque<StrategyAction> = VecDeque::new();
    let mut order_counter = 0u64;

    info!("Paper trading active - press Ctrl+C to stop");

    loop {
        tokio::select! {
            // Handle WebSocket messages
            msg = ws_client.next_message() => {
                let msg = match msg {
                    Ok(Some(m)) => m,
                    Ok(None) => {
                        warn!("WebSocket connection closed");
                        break;
                    }
                    Err(e) => {
                        error!("WebSocket error: {}", e);
                        health.enter_safe_mode(SafeModeReason::ConnectionError);
                        continue;
                    }
                };

                let ts_recv = clock.now_ns();

                // Process message
                match &msg {
                    WsMessage::BookUpdate { side, price, size, .. } => {
                        // Validate tick
                        let tick = (*price * 100.0) as u16;
                        if let Err(e) = book.validate_inbound_tick(tick) {
                            error!("Invalid tick {}: {:?}", tick, e);
                            health.enter_safe_mode(SafeModeReason::InvalidData);
                            continue;
                        }

                        // Update book
                        let side = if *side == "buy" { Side::Buy } else { Side::Sell };
                        book.set_size(side, tick, *size as i64);

                        // Also update paper book's market view
                        paper_book.market_book_mut().set_size(side, tick, *size as i64);

                        let ts_process = clock.now_ns();

                        // Record event
                        let event = CoreEvent::BookUpdate(mtrader_core::events::BookUpdateEvent {
                            side,
                            price_tick: tick,
                            new_size: *size as i64,
                            ts_exchange_ms: 0, // Would come from message
                            ts_recv_mono_ns: ts_recv,
                            ts_process_mono_ns: ts_process,
                        });

                        if let Some(ref mut rec) = recorder {
                            let _ = rec.record(event.clone());
                        }
                        let _ = event_tx.send(event);
                    }

                    WsMessage::Trade { side, price, size, id, .. } => {
                        let side = if *side == "buy" { Side::Buy } else { Side::Sell };
                        let tick = (*price * 100.0) as u16;
                        let ts_process = clock.now_ns();

                        // Check for fills on our orders
                        let fills = fill_sim.on_trade(side, tick, *size as i64, ts_process);
                        for fill in fills {
                            info!(
                                order_id = fill.order_id,
                                price = fill.price_tick,
                                size = fill.size,
                                fee = fill.fee_micro_usdc,
                                "PAPER FILL"
                            );

                            // Update position
                            let fill_side = fill_sim.order_side(&fill.order_id).unwrap_or(Side::Buy);
                            position.apply_fill(fill_side, fill.price_tick, fill.size);
                            pnl_tracker.record_fill(
                                fill_side,
                                fill.price_tick,
                                fill.size,
                                fill.fee_micro_usdc,
                            );

                            // Remove from paper book
                            paper_book.remove_order(&fill.order_id);

                            // Check circuit breaker
                            if pnl_tracker.unrealized_pnl() < -config.risk.max_daily_loss {
                                circuit_breaker.trip(mtrader_risk::TripReason::LossLimit);
                                health.enter_safe_mode(SafeModeReason::CircuitBreakerTripped);
                            }
                        }

                        // Record trade event
                        let event = CoreEvent::Trade(mtrader_core::events::TradeEvent {
                            side,
                            price_tick: tick,
                            size: *size as i64,
                            trade_id: id.clone(),
                            ts_exchange_ms: 0,
                            ts_recv_mono_ns: ts_recv,
                            ts_process_mono_ns: ts_process,
                        });

                        if let Some(ref mut rec) = recorder {
                            let _ = rec.record(event.clone());
                        }
                        let _ = event_tx.send(event);
                    }

                    _ => {}
                }

                // Advance fill simulator
                let sim_events = fill_sim.advance(clock.now_ns());
                for sim_event in sim_events {
                    match sim_event {
                        mtrader_sim::SimEvent::OrderAcked(order_id) => {
                            info!(order_id = order_id, "PAPER ORDER ACKED");
                        }
                        mtrader_sim::SimEvent::OrderCancelled(order_id) => {
                            info!(order_id = order_id, "PAPER ORDER CANCELLED");
                            paper_book.remove_order(&order_id);
                        }
                        mtrader_sim::SimEvent::OrderRejected(order_id, reason) => {
                            warn!(order_id = order_id, reason = reason, "PAPER ORDER REJECTED");
                            paper_book.remove_order(&order_id);
                        }
                    }
                }

                // Run strategy if not in safe mode
                if health.can_trade() && !circuit_breaker.is_tripped() {
                    let context = StrategyContext {
                        book: &book,
                        position: &position,
                        limits: &limits,
                        timestamp_ns: clock.now_ns(),
                    };

                    let actions = strategy.on_book_update(&context);
                    pending_actions.extend(actions);
                }

                // Process pending actions (paper orders)
                while let Some(action) = pending_actions.pop_front() {
                    match action {
                        StrategyAction::PlaceOrder { side, price_tick, size, .. } => {
                            // Check limits
                            if limits.check_order(side, size, &position).is_ok() {
                                order_counter += 1;
                                let order_id = format!("paper-{}", order_counter);

                                info!(
                                    order_id = order_id,
                                    side = ?side,
                                    price = price_tick,
                                    size = size,
                                    "PAPER ORDER PLACED"
                                );

                                // Submit to simulator
                                fill_sim.submit_order(
                                    order_id.clone(),
                                    side,
                                    price_tick,
                                    size,
                                    clock.now_ns(),
                                );

                                // Add to paper book
                                paper_book.add_order(PaperOrder {
                                    order_id,
                                    side,
                                    price_tick,
                                    size,
                                    timestamp_ns: clock.now_ns(),
                                });
                            }
                        }

                        StrategyAction::CancelOrder { order_id, .. } => {
                            info!(order_id = order_id, "PAPER CANCEL");
                            fill_sim.cancel_order(&order_id, clock.now_ns());
                        }

                        StrategyAction::CancelAll { .. } => {
                            info!("PAPER CANCEL ALL");
                            // Cancel all in simulator
                            for tick in paper_book.our_bid_ticks() {
                                for order in paper_book.our_orders_at(Side::Buy, tick) {
                                    fill_sim.cancel_order(&order.order_id, clock.now_ns());
                                }
                            }
                            for tick in paper_book.our_ask_ticks() {
                                for order in paper_book.our_orders_at(Side::Sell, tick) {
                                    fill_sim.cancel_order(&order.order_id, clock.now_ns());
                                }
                            }
                        }
                    }
                }
            }

            // Handle events from other sources
            Some(_event) = event_rx.recv() => {
                // Event already processed
            }

            // Periodic status update
            _ = tokio::time::sleep(tokio::time::Duration::from_secs(10)) => {
                info!(
                    position = position.net_position(),
                    pnl = pnl_tracker.unrealized_pnl(),
                    trades = pnl_tracker.trade_count(),
                    open_orders = paper_book.our_order_count(),
                    "Paper trading status"
                );
            }
        }
    }

    // Cleanup
    info!("Shutting down paper trading");

    if let Some(ref mut rec) = recorder {
        rec.close()?;
        info!(
            events = rec.total_events(),
            files = rec.files_written(),
            "Recording saved"
        );
    }

    info!(
        final_pnl = pnl_tracker.unrealized_pnl(),
        total_trades = pnl_tracker.trade_count(),
        "Paper trading complete"
    );

    Ok(())
}
