//! Paper trading command.

use crate::config::Config;
use crate::fee_profile::classify_fee_profile;
use anyhow::Result;
use mtrader_book::ArrayBook;
use mtrader_core::clock::MonotonicClock;
use mtrader_core::fees::{FeeSchedule, MarketFeeProfile};
use mtrader_core::Side;
use mtrader_dashboard::DashboardController;
use mtrader_execution::{Order, OrderKind, OrderStateManager, OrderType};
use mtrader_execution::state_manager::OrderManagerConfig;
use mtrader_gateway::{ParsedEvent, RestClient, RestConfig, WsClient, WsConfig};
use mtrader_risk::{PnLSnapshot, Position};
use mtrader_sim::{FillSimConfig, FillSimulator, PaperBook};
use mtrader_sim::fill_sim::SimEvent;
use mtrader_sim::paper_book::PaperOrder;
use mtrader_strategy::{
    BundleMakerStrategy, MakerMMStrategy, Strategy, StrategyAction, StrategyContext,
    UnaffectedArbStrategy, WorkingOrder,
};
use mtrader_strategy::bundle_maker::BundleMakerConfig;
use mtrader_strategy::maker_mm::MakerMMConfig;
use mtrader_strategy::unaffected_arb::UnaffectedArbConfig;
use std::collections::VecDeque;
use tracing::{error, info, warn};

/// Run paper trading mode.
pub async fn run(config: &Config, market: &str, strategy_name: &str, record: bool) -> Result<()> {
    info!(
        market = market,
        strategy = strategy_name,
        "Starting paper trading mode"
    );

    if !config.safe_mode.enabled {
        warn!("Safe mode is disabled - enabling for paper trading");
    }

    let clock = MonotonicClock::new();
    let mut book = ArrayBook::new(100);
    let mut paper_book = PaperBook::new(100);
    let mut position = Position::new();

    let mut order_manager = OrderStateManager::new(OrderManagerConfig::default());

    let fill_config = FillSimConfig {
        fee_rate_bps: config.risk.fee_rate_bps,
        ..Default::default()
    };
    let mut fill_sim = FillSimulator::new(fill_config);

    let rest_client = RestClient::new(RestConfig {
        base_url: config.gateway.rest_url.clone(),
        ..Default::default()
    })?;

    let market_info = rest_client.get_market(market).await.ok();
    let fee_profile = market_info
        .as_ref()
        .map(|info| classify_fee_profile(info, config.risk.fee_rate_bps as u16))
        .unwrap_or_else(|| MarketFeeProfile {
            label: "default_fee_profile".to_string(),
            schedule: FeeSchedule::Parabolic {
                fee_rate_bps: config.risk.fee_rate_bps as u16,
            },
        });

    let mut strategy: Box<dyn Strategy> = match strategy_name {
        "bundle_maker" => Box::new(BundleMakerStrategy::new(
            "bundle_maker".to_string(),
            BundleMakerConfig {
                fee_profile: fee_profile.clone(),
                ..Default::default()
            },
            market.to_string(),
            market.to_string(),
        )),
        "unaffected_arb" => Box::new(UnaffectedArbStrategy::new(
            "unaffected_arb".to_string(),
            UnaffectedArbConfig {
                fee_profile: fee_profile.clone(),
                ..Default::default()
            },
            market.to_string(),
            market.to_string(),
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

    let mut recorder = if record {
        let rec_config = mtrader_recorder::event_recorder::EventRecorderConfig {
            output_dir: config.recording.output_dir.clone().into(),
            file_prefix: format!("paper_{}", market),
            ..Default::default()
        };
        Some(mtrader_recorder::EventRecorder::new(rec_config))
    } else {
        None
    };

    let ws_config = WsConfig {
        url: config.gateway.ws_url.clone(),
        ..Default::default()
    };
    let ws_client = WsClient::new(ws_config);
    let (mut frame_rx, _cmd_tx) = ws_client.run(vec![market.to_string()]);

    let mut pending_actions: VecDeque<StrategyAction> = VecDeque::new();

    // Initialize dashboard
    let mut dashboard = DashboardController::new(market.to_string(), strategy_name.to_string());
    dashboard.set_connected(true);

    info!("Paper trading active - dashboard starting... press Ctrl+C to stop");

    loop {
        tokio::select! {
            Some(frame_result) = frame_rx.recv() => {
                let frame = match frame_result {
                    Ok(frame) => frame,
                    Err(e) => {
                        error!(error = ?e, "WebSocket error");
                        continue;
                    }
                };

                let now_ns = clock.now_ns().max(0) as u64;
                handle_sim_events(&mut fill_sim, &mut paper_book, now_ns);

                for event in frame.events {
                    match event {
                        ParsedEvent::Book(book_snapshot) => {
                            book.clear();
                            paper_book.market_book_mut().clear();
                            for (tick, size) in book_snapshot.bids {
                                book.set_level_unchecked(Side::Buy, tick, size);
                                paper_book.market_book_mut().set_level_unchecked(Side::Buy, tick, size);
                            }
                            for (tick, size) in book_snapshot.asks {
                                book.set_level_unchecked(Side::Sell, tick, size);
                                paper_book.market_book_mut().set_level_unchecked(Side::Sell, tick, size);
                            }
                        }
                        ParsedEvent::PriceChange(change) => {
                            for update in change.price_changes {
                                book.set_level_unchecked(update.side, update.price_tick, update.size);
                                paper_book
                                    .market_book_mut()
                                    .set_level_unchecked(update.side, update.price_tick, update.size);
                            }
                        }
                        ParsedEvent::LastTradePrice(trade) => {
                            if let Some(trade_side) = infer_trade_side(&book, trade.price_tick) {
                                let fills = fill_sim.on_trade(
                                    trade.price_tick,
                                    trade.size_shares,
                                    trade_side,
                                    now_ns,
                                );
                                handle_fills(
                                    fills,
                                    trade_side,
                                    &mut fill_sim,
                                    &mut paper_book,
                                    &mut position,
                                    &mut *strategy,
                                    &book,
                                    market,
                                    now_ns,
                                );
                            }
                        }
                        _ => {}
                    }
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

                // Update dashboard
                dashboard.update_book(&book);
                dashboard.update_pnl(&pnl);
                dashboard.update_position(&position);
                dashboard.update_active_orders(our_bids.len(), our_asks.len());
                dashboard.update_timestamp();

                let context = StrategyContext::from_book(
                    &book,
                    market.to_string(),
                    position.clone(),
                    pnl,
                    our_bids.clone(),
                    our_asks.clone(),
                    now_ns,
                );

                let actions = strategy.on_update(&context);
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
                                market.to_string(),
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

                if let Some(rec) = recorder.as_mut() {
                    rec.flush()?;
                }
            }

            _ = tokio::signal::ctrl_c() => {
                break;
            }
        }
    }

    Ok(())
}

fn infer_trade_side(book: &ArrayBook, price_tick: u16) -> Option<Side> {
    match (book.best_bid(), book.best_ask()) {
        (Some(best_bid), Some(best_ask)) => {
            if price_tick <= best_bid {
                Some(Side::Sell)
            } else if price_tick >= best_ask {
                Some(Side::Buy)
            } else {
                None
            }
        }
        (Some(best_bid), None) => (price_tick <= best_bid).then_some(Side::Sell),
        (None, Some(best_ask)) => (price_tick >= best_ask).then_some(Side::Buy),
        (None, None) => None,
    }
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
    market: &str,
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
            market.to_string(),
            position.clone(),
            pnl,
            our_bids,
            our_asks,
            now_ns,
        );

        strategy.on_fill(&ctx, fill_side, fill.price_tick, fill.size);
    }
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
