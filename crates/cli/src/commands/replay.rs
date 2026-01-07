//! Replay command for backtesting.

use crate::config::Config;
use anyhow::Result;
use mtrader_book::ArrayBook;
use mtrader_core::events::CoreEvent;
use mtrader_core::ClientOrderId;
use mtrader_risk::{PnLTracker, Position, PositionLimits};
use mtrader_sim::{FillSimConfig, FillSimulator, PaperBook, PaperOrder, ReplayEngine, ReplayMode, ReplayStats};
use mtrader_strategy::{MakerMMConfig, MakerMMStrategy, Strategy, StrategyAction, StrategyContext};
use mtrader_execution::{Order, OrderType};
use mtrader_core::OrderReason;
use std::collections::VecDeque;
use std::path::Path;
use tracing::{error, info, warn};

/// Backtest report.
#[derive(Debug, Default)]
pub struct BacktestReport {
    pub total_events: u64,
    pub duration_ns: u64,
    pub final_pnl: i64,
    pub total_trades: u64,
    pub total_volume: u64,
    pub total_fees: i64,
    pub max_drawdown: i64,
    pub win_rate: f64,
    pub sharpe_ratio: f64,
}

impl BacktestReport {
    pub fn to_json(&self) -> String {
        serde_json::json!({
            "total_events": self.total_events,
            "duration_ns": self.duration_ns,
            "duration_hours": self.duration_ns as f64 / 3_600_000_000_000.0,
            "final_pnl_usdc": self.final_pnl as f64 / 1_000_000.0,
            "total_trades": self.total_trades,
            "total_volume_usdc": self.total_volume as f64 / 1_000_000.0,
            "total_fees_usdc": self.total_fees as f64 / 1_000_000.0,
            "max_drawdown_usdc": self.max_drawdown as f64 / 1_000_000.0,
            "win_rate": self.win_rate,
            "sharpe_ratio": self.sharpe_ratio,
        })
        .to_string()
    }
}

/// Load events from Parquet file.
fn load_events_from_parquet(_path: &Path) -> Result<Vec<CoreEvent>> {
    // TODO: Implement proper Parquet reading
    // For now, return empty - this would use arrow/parquet to read
    warn!("Parquet reading not yet fully implemented");
    Ok(Vec::new())
}

/// Run replay/backtest.
pub async fn run(
    config: &Config,
    input: &str,
    strategy_name: &str,
    speed: f64,
    report_path: Option<&str>,
) -> Result<()> {
    info!(
        input = input,
        strategy = strategy_name,
        speed = speed,
        "Starting replay/backtest"
    );

    let input_path = Path::new(input);
    if !input_path.exists() {
        error!("Input path does not exist: {}", input);
        return Err(anyhow::anyhow!("Input path not found"));
    }

    // Load events
    let events = if input_path.is_file() {
        load_events_from_parquet(input_path)?
    } else {
        // Directory - load all parquet files
        let mut all_events = Vec::new();
        for entry in std::fs::read_dir(input_path)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().map(|e| e == "parquet").unwrap_or(false) {
                let file_events = load_events_from_parquet(&path)?;
                all_events.extend(file_events);
            }
        }
        // Sort by timestamp
        all_events.sort_by_key(|e| match e {
            CoreEvent::BookUpdate(e) => e.ts_process_mono_ns,
            CoreEvent::Trade(e) => e.ts_process_mono_ns,
            CoreEvent::OrderAck(e) => e.ts_process_mono_ns,
            CoreEvent::OrderFill(e) => e.ts_process_mono_ns,
            CoreEvent::OrderCancel(e) => e.ts_process_mono_ns,
            CoreEvent::OrderReject(e) => e.ts_process_mono_ns,
            CoreEvent::StrategySignal(e) => e.timestamp_ns,
            CoreEvent::RiskEvent(e) => e.timestamp_ns,
            CoreEvent::SystemHealth(e) => e.timestamp_ns,
        });
        all_events
    };

    if events.is_empty() {
        warn!("No events loaded - nothing to replay");
        return Ok(());
    }

    let stats = ReplayStats::from_events(&events);
    info!(
        total_events = stats.total_events,
        book_updates = stats.book_updates,
        trades = stats.trades,
        duration_ns = stats.duration_ns(),
        "Loaded events for replay"
    );

    // Initialize replay engine
    let mode = if speed > 0.0 {
        ReplayMode::RealTime
    } else {
        ReplayMode::FastForward
    };
    let mut replay = ReplayEngine::new(mode);
    replay.set_speed(speed);
    replay.load_events(events);

    // Initialize trading components
    let mut book = ArrayBook::new(100);
    let mut paper_book = PaperBook::new(100);
    let mut position = Position::new();
    let mut pnl_tracker = PnLTracker::new();

    let limits = PositionLimits {
        max_position: config.risk.max_position,
        max_notional: config.risk.max_position as u64 * 2,
        max_open_orders: config.risk.max_open_orders,
        max_order_size: config.strategy.quote_size_shares as i64,
    };

    // Initialize fill simulator
    let fill_config = FillSimConfig {
        fee_rate_bps: config.risk.fee_rate_bps,
        ..Default::default()
    };
    let mut fill_sim = FillSimulator::new(fill_config);

    // Initialize strategy
    let strategy_config = MakerMMConfig {
        spread_ticks: config.strategy.half_spread_bps,
        order_size: config.strategy.quote_size_shares as i64,
        num_levels: config.strategy.num_levels,
        skew_factor: config.strategy.skew_factor,
        requote_threshold_ticks: config.strategy.requote_threshold_bps,
        min_edge_ticks: config.strategy.min_edge_bps,
    };
    let mut strategy = MakerMMStrategy::new(strategy_config);

    let mut pending_actions: VecDeque<StrategyAction> = VecDeque::new();
    let mut order_counter = 0u64;
    let mut events_processed = 0u64;

    // Replay loop
    while let Some(event) = replay.next_event() {
        events_processed += 1;
        let current_time = replay.current_time_ns();

        match &event {
            CoreEvent::BookUpdate(e) => {
                // Validate and update book
                if book.validate_inbound_tick(e.price_tick).is_ok() {
                    book.set_size(e.side, e.price_tick, e.new_size);
                    paper_book.market_book_mut().set_size(e.side, e.price_tick, e.new_size);
                }
            }

            CoreEvent::Trade(e) => {
                // Check for fills
                let fills = fill_sim.on_trade(e.side, e.price_tick, e.size, current_time);
                for fill in fills {
                    let fill_side = fill_sim.order_side(&fill.order_id).unwrap_or(e.side);
                    position.apply_fill(fill_side, fill.price_tick, fill.size);
                    pnl_tracker.record_fill(fill_side, fill.price_tick, fill.size, fill.fee_micro_usdc);
                    paper_book.remove_order(&fill.order_id);
                }
            }

            _ => {}
        }

        // Advance fill simulator
        let sim_events = fill_sim.advance(current_time);
        for sim_event in sim_events {
            if let mtrader_sim::SimEvent::OrderCancelled { order_id, .. } = sim_event {
                paper_book.remove_order(&order_id);
            }
        }

        // Run strategy
        let context = StrategyContext {
            book: &book,
            position: &position,
            limits: &limits,
            timestamp_ns: current_time,
        };

        let actions = strategy.on_book_update(&context);
        pending_actions.extend(actions);

        // Process actions
        while let Some(action) = pending_actions.pop_front() {
            match action {
                StrategyAction::PlaceOrder { side, price_tick, size, .. } => {
                    if limits.check_order(side, size, &position).is_ok() {
                        order_counter += 1;
                        let client_order_id = ClientOrderId(format!("replay-{}", order_counter));
                        let order = Order::new(
                            client_order_id,
                            "replay-asset".to_string(),
                            side,
                            price_tick,
                            size,
                            OrderType::Limit,
                            OrderReason::MakerQuote,
                            current_time,
                        );
                        let queue_ahead = paper_book.estimate_queue_ahead(side, price_tick);
                        let order_id = fill_sim.submit_order(order, queue_ahead, current_time);

                        paper_book.add_order(PaperOrder {
                            order_id,
                            side,
                            price_tick,
                            size,
                            timestamp_ns: current_time,
                            queue_ahead,
                        });
                    }
                }

                StrategyAction::CancelOrder { order_id, .. } => {
                    fill_sim.cancel_order(&order_id.0, current_time);
                }

                StrategyAction::CancelAll { .. } => {
                    for tick in paper_book.our_bid_ticks() {
                        for order in paper_book.our_orders_at(mtrader_core::Side::Buy, tick) {
                            fill_sim.cancel_order(&order.order_id, current_time);
                        }
                    }
                    for tick in paper_book.our_ask_ticks() {
                        for order in paper_book.our_orders_at(mtrader_core::Side::Sell, tick) {
                            fill_sim.cancel_order(&order.order_id, current_time);
                        }
                    }
                }
            }
        }

        // Progress update
        if events_processed % 100_000 == 0 {
            info!(
                events = events_processed,
                remaining = replay.events_remaining(),
                pnl = pnl_tracker.unrealized_pnl(),
                "Replay progress"
            );
        }
    }

    // Generate report
    let report = BacktestReport {
        total_events: events_processed,
        duration_ns: stats.duration_ns(),
        final_pnl: pnl_tracker.unrealized_pnl(),
        total_trades: pnl_tracker.trade_count(),
        total_volume: pnl_tracker.total_volume() as u64,
        total_fees: pnl_tracker.total_fees(),
        max_drawdown: pnl_tracker.max_drawdown(),
        win_rate: 0.0, // TODO: Calculate
        sharpe_ratio: 0.0, // TODO: Calculate
    };

    info!("Backtest complete");
    println!("\n=== BACKTEST REPORT ===");
    println!("Total Events: {}", report.total_events);
    println!("Duration: {:.2} hours", report.duration_ns as f64 / 3_600_000_000_000.0);
    println!("Final PnL: ${:.2}", report.final_pnl as f64 / 1_000_000.0);
    println!("Total Trades: {}", report.total_trades);
    println!("Total Volume: ${:.2}", report.total_volume as f64 / 1_000_000.0);
    println!("Total Fees: ${:.2}", report.total_fees as f64 / 1_000_000.0);
    println!("Max Drawdown: ${:.2}", report.max_drawdown as f64 / 1_000_000.0);
    println!("========================\n");

    // Save report if requested
    if let Some(path) = report_path {
        let json = report.to_json();
        std::fs::write(path, &json)?;
        info!("Report saved to: {}", path);
    }

    Ok(())
}
