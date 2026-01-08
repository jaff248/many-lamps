//! Replay command for backtesting.

use crate::config::Config;
use anyhow::Result;
use mtrader_book::ArrayBook;
use mtrader_core::events::{CoreEvent, MarketDataEvent};
use mtrader_core::fees::{FeeSchedule, MarketFeeProfile};
use mtrader_core::Side;
use mtrader_execution::{Order, OrderKind, OrderStateManager, OrderType};
use mtrader_execution::state_manager::OrderManagerConfig;
use mtrader_risk::{PnLSnapshot, Position};
use mtrader_sim::{FillSimConfig, FillSimulator, PaperBook, ReplayEngine};
use mtrader_sim::fill_sim::SimEvent;
use mtrader_sim::paper_book::PaperOrder;
use mtrader_sim::replay::{ReplayMode, ReplayStats};
use mtrader_strategy::{
    BundleMakerStrategy, MakerMMStrategy, Strategy, StrategyAction, StrategyContext,
    UnaffectedArbStrategy, WorkingOrder,
};
use mtrader_strategy::bundle_maker::BundleMakerConfig;
use mtrader_strategy::maker_mm::MakerMMConfig;
use mtrader_strategy::unaffected_arb::UnaffectedArbConfig;
use std::collections::VecDeque;
use std::path::Path;
use tracing::{error, info, warn};

/// Backtest report.
#[derive(Debug, Default)]
pub struct BacktestReport {
    pub total_events: u64,
    pub duration_ns: u64,
    pub total_trades: u64,
}

impl BacktestReport {
    pub fn to_json(&self) -> String {
        serde_json::json!({
            "total_events": self.total_events,
            "duration_ns": self.duration_ns,
            "duration_hours": self.duration_ns as f64 / 3_600_000_000_000.0,
            "total_trades": self.total_trades,
        })
        .to_string()
    }
}

/// Load events from Parquet file.
fn load_events_from_parquet(_path: &Path) -> Result<Vec<CoreEvent>> {
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

    let events = if input_path.is_file() {
        load_events_from_parquet(input_path)?
    } else {
        let mut all_events = Vec::new();
        for entry in std::fs::read_dir(input_path)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().map(|e| e == "parquet").unwrap_or(false) {
                let file_events = load_events_from_parquet(&path)?;
                all_events.extend(file_events);
            }
        }
        all_events.sort_by_key(event_timestamp);
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

    let mode = if speed > 0.0 {
        ReplayMode::RealTime
    } else {
        ReplayMode::FastForward
    };

    let mut engine = ReplayEngine::new(mode);
    engine.load_events(events);

    let mut book = ArrayBook::new(100);
    let mut paper_book = PaperBook::new(100);
    let mut position = Position::new();
    let mut order_manager = OrderStateManager::new(OrderManagerConfig::default());

    let fill_config = FillSimConfig {
        fee_rate_bps: config.risk.fee_rate_bps,
        ..Default::default()
    };
    let mut fill_sim = FillSimulator::new(fill_config);

    let fee_profile = MarketFeeProfile {
        label: "replay_default".to_string(),
        schedule: FeeSchedule::Parabolic {
            fee_rate_bps: config.risk.fee_rate_bps as u16,
        },
    };

    let mut strategy: Box<dyn Strategy> = match strategy_name {
        "bundle_maker" => Box::new(BundleMakerStrategy::new(
            "bundle_maker".to_string(),
            BundleMakerConfig {
                fee_profile: fee_profile.clone(),
                ..Default::default()
            },
            "yes".to_string(),
            "no".to_string(),
        )),
        "unaffected_arb" => Box::new(UnaffectedArbStrategy::new(
            "unaffected_arb".to_string(),
            UnaffectedArbConfig {
                fee_profile: fee_profile.clone(),
                ..Default::default()
            },
            "yes".to_string(),
            "no".to_string(),
        )),
        _ => Box::new(MakerMMStrategy::new(
            "maker_mm".to_string(),
            MakerMMConfig {
                half_spread_ticks: config.strategy.spread_ticks,
                order_size: config.strategy.order_size,
                max_position: config.risk.max_position,
                skew_factor: config.strategy.skew_factor,
                min_edge_ticks: config.strategy.min_edge_ticks,
                requote_threshold_ticks: config.strategy.requote_threshold,
                quote_both_sides: true,
            },
        )),
    };

    strategy.activate();

    let mut pending_actions: VecDeque<StrategyAction> = VecDeque::new();

    while let Some(event) = engine.next_event() {
        let now_ns = event_timestamp(&event);
        handle_sim_events(&mut fill_sim, &mut paper_book, now_ns);

        match event {
            CoreEvent::MarketData(market) => match market {
                MarketDataEvent::BookSnapshot { bids, asks, .. } => {
                    book.clear();
                    paper_book.market_book_mut().clear();
                    for (tick, size) in bids {
                        book.set_level_unchecked(Side::Buy, tick, size);
                        paper_book.market_book_mut().set_level_unchecked(Side::Buy, tick, size);
                    }
                    for (tick, size) in asks {
                        book.set_level_unchecked(Side::Sell, tick, size);
                        paper_book.market_book_mut().set_level_unchecked(Side::Sell, tick, size);
                    }
                }
                MarketDataEvent::BookDelta { side, tick, new_size, .. } => {
                    if book.validate_inbound_tick(tick, "book").is_ok() {
                        book.set_level_unchecked(side, tick, new_size);
                        paper_book.market_book_mut().set_level_unchecked(side, tick, new_size);
                    }
                }
                MarketDataEvent::Trade { side, price_tick, size, timestamps, .. } => {
                    let fills = fill_sim.on_trade(price_tick, size, side, timestamps.ts_process_mono_ns as u64);
                    handle_fills(
                        fills,
                        side,
                        &mut fill_sim,
                        &mut paper_book,
                        &mut position,
                        &mut *strategy,
                        &book,
                        now_ns,
                    );
                }
                MarketDataEvent::TickSizeChange { new_tick_size, .. } => {
                    book.set_tick_size(new_tick_size);
                    paper_book.market_book_mut().set_tick_size(new_tick_size);
                }
                _ => {}
            },
            _ => {}
        }

        let pnl = PnLSnapshot {
            timestamp_ns: now_ns,
            realized_pnl: position.realized_pnl_micro_usdc,
            unrealized_pnl: 0,
            total_pnl: position.realized_pnl_micro_usdc,
            total_fees: 0,
            net_pnl: position.realized_pnl_micro_usdc,
            high_water_mark: 0,
            drawdown: 0,
            drawdown_bps: 0,
        };

        let our_bids = collect_working_orders(&paper_book, Side::Buy);
        let our_asks = collect_working_orders(&paper_book, Side::Sell);

        let ctx = StrategyContext::from_book(
            &book,
            "asset".to_string(),
            position.clone(),
            pnl,
            our_bids,
            our_asks,
            now_ns,
        );

        let actions = strategy.on_update(&ctx);
        pending_actions.extend(actions);

        while let Some(action) = pending_actions.pop_front() {
            match action {
                StrategyAction::PlaceOrder { side, kind, order_type, reason } => {
                    let client_order_id = order_manager.generate_client_id();
                    let queue_ahead = match kind {
                        OrderKind::Limit { price_tick, .. } => {
                            paper_book.estimate_queue_ahead(side, price_tick)
                        }
                        _ => 0,
                    };

                    let order = Order::new(
                        client_order_id.clone(),
                        "asset".to_string(),
                        side,
                        kind,
                        order_type,
                        reason,
                        now_ns,
                    );

                    let order_id = fill_sim.submit_order_with_queue(order, queue_ahead, now_ns);

                    if let OrderKind::Limit { price_tick, size_shares } = kind {
                        paper_book.add_order(PaperOrder {
                            order_id,
                            client_order_id,
                            side,
                            price_tick,
                            size: size_shares,
                            timestamp_ns: now_ns,
                        });
                    }
                }
                StrategyAction::CancelOrder { client_order_id, .. } => {
                    let order_id = client_order_id.0.clone();
                    fill_sim.cancel_order(&order_id, now_ns);
                }
                StrategyAction::AmendOrder { .. } | StrategyAction::NoOp => {}
            }
        }
    }

    let report = BacktestReport {
        total_events: stats.total_events,
        duration_ns: stats.duration_ns(),
        total_trades: stats.trades,
    };

    if let Some(path) = report_path {
        std::fs::write(path, report.to_json())?;
        info!(path = path, "Report written");
    } else {
        println!("{}", report.to_json());
    }

    Ok(())
}

fn collect_working_orders(paper_book: &PaperBook, side: Side) -> Vec<WorkingOrder> {
    let ticks = match side {
        Side::Buy => paper_book.our_bid_ticks(),
        Side::Sell => paper_book.our_ask_ticks(),
    };

    let mut orders = Vec::new();
    for tick in ticks {
        for order in paper_book.our_orders_at(side, tick) {
            orders.push(WorkingOrder {
                tick,
                client_order_id: order.client_order_id.clone(),
            });
        }
    }
    orders
}

fn handle_sim_events(fill_sim: &mut FillSimulator, paper_book: &mut PaperBook, now_ns: u64) {
    let sim_events = fill_sim.advance(now_ns);
    for event in sim_events {
        match event {
            SimEvent::OrderCancelled { order_id, .. } => {
                paper_book.remove_order(&order_id);
            }
            SimEvent::OrderRejected { client_order_id, .. } => {
                let _ = paper_book.remove_order_by_client_id(&client_order_id);
            }
            SimEvent::OrderAcked { .. } => {}
        }
    }
}

fn handle_fills(
    fills: Vec<mtrader_sim::SimulatedFill>,
    trade_side: Side,
    fill_sim: &mut FillSimulator,
    paper_book: &mut PaperBook,
    position: &mut Position,
    strategy: &mut dyn Strategy,
    book: &ArrayBook,
    now_ns: u64,
) {
    for fill in fills {
        let fill_side = trade_side.opposite();
        position.on_fill(fill_side, fill.price_tick, fill.size);

        if let Some(order) = fill_sim.get_order(&fill.order_id) {
            paper_book.update_order_size(&fill.order_id, order.remaining_size);
        } else {
            paper_book.remove_order(&fill.order_id);
        }

        let pnl = PnLSnapshot {
            timestamp_ns: now_ns,
            realized_pnl: position.realized_pnl_micro_usdc,
            unrealized_pnl: 0,
            total_pnl: position.realized_pnl_micro_usdc,
            total_fees: 0,
            net_pnl: position.realized_pnl_micro_usdc,
            high_water_mark: 0,
            drawdown: 0,
            drawdown_bps: 0,
        };

        let our_bids = collect_working_orders(paper_book, Side::Buy);
        let our_asks = collect_working_orders(paper_book, Side::Sell);

        let ctx = StrategyContext::from_book(
            book,
            "asset".to_string(),
            position.clone(),
            pnl,
            our_bids,
            our_asks,
            now_ns,
        );

        strategy.on_fill(&ctx, fill_side, fill.price_tick, fill.size);
    }
}

fn event_timestamp(event: &CoreEvent) -> u64 {
    match event {
        CoreEvent::MarketData(data) => match data {
            MarketDataEvent::BookSnapshot { timestamps, .. }
            | MarketDataEvent::BookDelta { timestamps, .. }
            | MarketDataEvent::Trade { timestamps, .. }
            | MarketDataEvent::TickSizeChange { timestamps, .. }
            | MarketDataEvent::BestBidAsk { timestamps, .. } => timestamps.ts_process_mono_ns as u64,
            MarketDataEvent::ConnectionStatus { timestamp_mono_ns, .. }
            | MarketDataEvent::ParseError { timestamp_mono_ns, .. } => *timestamp_mono_ns as u64,
        },
        CoreEvent::Signal(signal) => signal.timestamp_mono_ns as u64,
        CoreEvent::OrderIntent(intent) => intent.timestamp_mono_ns as u64,
        CoreEvent::OrderAck(ack) => ack.timestamp_mono_ns as u64,
        CoreEvent::Fill(fill) => fill.timestamps.ts_process_mono_ns as u64,
        CoreEvent::CancelAck(cancel) => cancel.timestamp_mono_ns as u64,
        CoreEvent::Risk(_) => 0,
        CoreEvent::System(system) => match system {
            mtrader_core::events::SystemEvent::SafeMode { timestamp_mono_ns, .. }
            | mtrader_core::events::SystemEvent::SafeModeCleared { timestamp_mono_ns }
            | mtrader_core::events::SystemEvent::ResnaphotRequested { timestamp_mono_ns, .. }
            | mtrader_core::events::SystemEvent::MarketLifecycle { timestamp_mono_ns, .. }
            | mtrader_core::events::SystemEvent::Heartbeat { timestamp_mono_ns } => {
                *timestamp_mono_ns as u64
            }
        },
    }
}
