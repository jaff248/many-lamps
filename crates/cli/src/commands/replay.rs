//! Replay command for backtesting.

use crate::config::Config;
use anyhow::Result;
use arrow::array::{Array, Int64Array, StringArray, UInt64Array, UInt8Array};
use arrow::record_batch::RecordBatch;
use mtrader_book::ArrayBook;
use mtrader_core::events::{
    CancelAck, CancelAckStatus, CoreEvent, EventTimestamps, FillEvent, MarketDataEvent, OrderAck,
    OrderAckStatus, OrderIntent, OrderType as CoreOrderType, RiskEvent, SignalType, StrategySignal,
    SystemEvent,
};
use mtrader_core::fees::{FeeSchedule, MarketFeeProfile};
use mtrader_core::{ClientOrderId, MarketId, OrderReason, Side, StrategyId, TokenId};
use mtrader_execution::state_manager::OrderManagerConfig;
use mtrader_execution::{Order, OrderKind, OrderStateManager, OrderType};
use mtrader_risk::{PnLSnapshot, Position};
use mtrader_sim::fill_sim::SimEvent;
use mtrader_sim::paper_book::PaperOrder;
use mtrader_sim::replay::{ReplayMode, ReplayStats};
use mtrader_sim::{FillSimConfig, FillSimulator, PaperBook, ReplayEngine};
use mtrader_strategy::bundle_maker::BundleMakerConfig;
use mtrader_strategy::maker_mm::MakerMMConfig;
use mtrader_strategy::unaffected_arb::UnaffectedArbConfig;
use mtrader_strategy::{
    BundleMakerStrategy, MakerMMStrategy, Strategy, StrategyAction, StrategyContext,
    UnaffectedArbStrategy, WorkingOrder,
};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::fs::File;
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
    let file = File::open(_path)?;
    let builder = ParquetRecordBatchReaderBuilder::try_new(file)?;
    let reader = builder.build()?;
    let mut events = Vec::new();

    for batch_result in reader {
        let batch = batch_result?;
        let columns = ParquetColumns::from_batch(&batch)?;
        for row in 0..batch.num_rows() {
            if let Some(event) = parse_event_row(&columns, row)? {
                events.push(event);
            }
        }
    }

    Ok(events)
}

struct ParquetColumns {
    event_type: StringArray,
    ts_exchange_ms: UInt64Array,
    ts_recv_mono_ns: UInt64Array,
    ts_process_mono_ns: UInt64Array,
    side: UInt8Array,
    price_tick: UInt64Array,
    size: Int64Array,
    order_id: StringArray,
    trade_id: StringArray,
    reason: StringArray,
    payload_json: StringArray,
}

impl ParquetColumns {
    fn from_batch(batch: &RecordBatch) -> Result<Self> {
        Ok(Self {
            event_type: column_as(batch, "event_type")?,
            ts_exchange_ms: column_as(batch, "ts_exchange_ms")?,
            ts_recv_mono_ns: column_as(batch, "ts_recv_mono_ns")?,
            ts_process_mono_ns: column_as(batch, "ts_process_mono_ns")?,
            side: column_as(batch, "side")?,
            price_tick: column_as(batch, "price_tick")?,
            size: column_as(batch, "size")?,
            order_id: column_as(batch, "order_id")?,
            trade_id: column_as(batch, "trade_id")?,
            reason: column_as(batch, "reason")?,
            payload_json: column_as(batch, "payload_json")?,
        })
    }
}

fn column_as<T: 'static>(batch: &RecordBatch, name: &str) -> Result<T>
where
    T: arrow::array::Array + Clone,
{
    let array = batch
        .column_by_name(name)
        .ok_or_else(|| anyhow::anyhow!("Missing Parquet column: {}", name))?
        .as_any()
        .downcast_ref::<T>()
        .ok_or_else(|| anyhow::anyhow!("Unexpected type for Parquet column: {}", name))?;
    Ok(array.clone())
}

fn parse_event_row(columns: &ParquetColumns, row: usize) -> Result<Option<CoreEvent>> {
    let event_type = columns.event_type.value(row);
    let timestamps = EventTimestamps {
        ts_exchange_ms: columns.ts_exchange_ms.value(row) as i64,
        ts_recv_mono_ns: columns.ts_recv_mono_ns.value(row) as i64,
        ts_process_mono_ns: columns.ts_process_mono_ns.value(row) as i64,
    };
    let payload = parse_payload(&columns.payload_json, row)?;

    let event = match event_type {
        "BookSnapshot" => {
            let market_id = MarketId(payload_string(&payload, "market_id").unwrap_or_default());
            let token_id = TokenId(payload_string(&payload, "token_id").unwrap_or_default());
            let tick_size = payload_u64(&payload, "tick_size").unwrap_or_default() as u16;
            let snapshot_hash = payload_string(&payload, "snapshot_hash").unwrap_or_default();
            Some(CoreEvent::MarketData(MarketDataEvent::BookSnapshot {
                market_id,
                token_id,
                bids: Vec::new(),
                asks: Vec::new(),
                tick_size,
                snapshot_hash,
                timestamps,
            }))
        }
        "BookDelta" => {
            let market_id = MarketId(payload_string(&payload, "market_id").unwrap_or_default());
            let token_id = TokenId(payload_string(&payload, "token_id").unwrap_or_default());
            let side = parse_side(&columns.side, row).unwrap_or(Side::Buy);
            let tick = columns.price_tick.value(row) as u16;
            let new_size = columns.size.value(row) as u64;
            let best_bid = payload_u64(&payload, "best_bid").map(|v| v as u16);
            let best_ask = payload_u64(&payload, "best_ask").map(|v| v as u16);
            let order_hash = payload_string(&payload, "order_hash").unwrap_or_default();
            Some(CoreEvent::MarketData(MarketDataEvent::BookDelta {
                market_id,
                token_id,
                side,
                tick,
                new_size,
                best_bid,
                best_ask,
                order_hash,
                timestamps,
            }))
        }
        "Trade" => {
            let market_id = MarketId(payload_string(&payload, "market_id").unwrap_or_default());
            let token_id = TokenId(payload_string(&payload, "token_id").unwrap_or_default());
            let side = parse_side(&columns.side, row).unwrap_or(Side::Buy);
            let price_tick = columns.price_tick.value(row) as u16;
            let size = columns.size.value(row) as u64;
            let fee_rate_bps = payload_u64(&payload, "fee_rate_bps").unwrap_or(0) as u16;
            Some(CoreEvent::MarketData(MarketDataEvent::Trade {
                market_id,
                token_id,
                side,
                price_tick,
                size,
                fee_rate_bps,
                timestamps,
            }))
        }
        "TickSizeChange" => {
            let market_id = MarketId(payload_string(&payload, "market_id").unwrap_or_default());
            let token_id = TokenId(payload_string(&payload, "token_id").unwrap_or_default());
            let old_tick_size = payload_u64(&payload, "old_tick_size").unwrap_or(0) as u16;
            let new_tick_size = payload_u64(&payload, "new_tick_size").unwrap_or(0) as u16;
            Some(CoreEvent::MarketData(MarketDataEvent::TickSizeChange {
                market_id,
                token_id,
                old_tick_size,
                new_tick_size,
                timestamps,
            }))
        }
        "BestBidAsk" => {
            let market_id = MarketId(payload_string(&payload, "market_id").unwrap_or_default());
            let token_id = TokenId(payload_string(&payload, "token_id").unwrap_or_default());
            let best_bid = payload_u64(&payload, "best_bid").map(|v| v as u16);
            let best_ask = payload_u64(&payload, "best_ask").map(|v| v as u16);
            let spread = payload_u64(&payload, "spread").map(|v| v as u16);
            Some(CoreEvent::MarketData(MarketDataEvent::BestBidAsk {
                market_id,
                token_id,
                best_bid,
                best_ask,
                spread,
                timestamps,
            }))
        }
        "ConnectionStatus" => Some(CoreEvent::MarketData(MarketDataEvent::ConnectionStatus {
            connected: payload_bool(&payload, "connected").unwrap_or(false),
            timestamp_mono_ns: timestamps.ts_recv_mono_ns,
        })),
        "ParseError" => Some(CoreEvent::MarketData(MarketDataEvent::ParseError {
            raw_bytes: Vec::new(),
            error: payload_string(&payload, "error").unwrap_or_default(),
            timestamp_mono_ns: timestamps.ts_recv_mono_ns,
        })),
        "Signal" => {
            let strategy_id =
                StrategyId(payload_string(&payload, "strategy_id").unwrap_or_default());
            let market_id = MarketId(payload_string(&payload, "market_id").unwrap_or_default());
            let token_id = TokenId(payload_string(&payload, "token_id").unwrap_or_default());
            let signal_type = payload
                .as_ref()
                .and_then(|value| value.get("signal_type"))
                .and_then(|v| serde_json::from_value::<SignalType>(v.clone()).ok())
                .unwrap_or(SignalType::Resume);
            Some(CoreEvent::Signal(StrategySignal {
                strategy_id,
                market_id,
                token_id,
                signal_type,
                timestamp_mono_ns: timestamps.ts_process_mono_ns,
            }))
        }
        "OrderIntent" => {
            let strategy_id =
                StrategyId(payload_string(&payload, "strategy_id").unwrap_or_default());
            let market_id = MarketId(payload_string(&payload, "market_id").unwrap_or_default());
            let token_id = TokenId(payload_string(&payload, "token_id").unwrap_or_default());
            let order_type = payload
                .as_ref()
                .and_then(|value| value.get("order_type"))
                .and_then(|v| serde_json::from_value::<CoreOrderType>(v.clone()).ok())
                .unwrap_or(CoreOrderType::Gtc);
            let reason = reason_value(&columns.reason, row)
                .as_deref()
                .map(parse_order_reason)
                .unwrap_or(OrderReason::Manual);
            let client_order_id =
                ClientOrderId(reason_value(&columns.order_id, row).unwrap_or_default());
            Some(CoreEvent::OrderIntent(OrderIntent {
                client_order_id,
                strategy_id,
                market_id,
                token_id,
                side: parse_side(&columns.side, row).unwrap_or(Side::Buy),
                price_tick: columns.price_tick.value(row) as u16,
                size: columns.size.value(row) as u64,
                order_type,
                reason,
                timestamp_mono_ns: timestamps.ts_process_mono_ns,
            }))
        }
        "OrderAck" => {
            let client_order_id =
                ClientOrderId(reason_value(&columns.order_id, row).unwrap_or_default());
            let exchange_order_id = payload_string(&payload, "exchange_order_id");
            let status = reason_value(&columns.reason, row)
                .as_deref()
                .map(parse_order_ack_status)
                .unwrap_or(OrderAckStatus::Accepted);
            Some(CoreEvent::OrderAck(OrderAck {
                client_order_id,
                exchange_order_id,
                status,
                timestamp_mono_ns: timestamps.ts_process_mono_ns,
            }))
        }
        "Fill" => {
            let client_order_id =
                ClientOrderId(payload_string(&payload, "client_order_id").unwrap_or_default());
            let exchange_order_id = reason_value(&columns.order_id, row).unwrap_or_default();
            let exchange_trade_id = reason_value(&columns.trade_id, row).unwrap_or_default();
            let strategy_id =
                StrategyId(payload_string(&payload, "strategy_id").unwrap_or_default());
            let market_id = MarketId(payload_string(&payload, "market_id").unwrap_or_default());
            let token_id = TokenId(payload_string(&payload, "token_id").unwrap_or_default());
            let remaining_size = payload_u64(&payload, "remaining_size").unwrap_or(0);
            let is_maker = payload_bool(&payload, "is_maker").unwrap_or(false);
            let fee_amount = payload_u64(&payload, "fee_amount").unwrap_or(0);
            Some(CoreEvent::Fill(FillEvent {
                client_order_id,
                exchange_order_id,
                exchange_trade_id,
                strategy_id,
                market_id,
                token_id,
                side: parse_side(&columns.side, row).unwrap_or(Side::Buy),
                price_tick: columns.price_tick.value(row) as u16,
                fill_size: columns.size.value(row) as u64,
                remaining_size,
                is_maker,
                fee_amount,
                timestamps,
            }))
        }
        "CancelAck" => {
            let client_order_id =
                ClientOrderId(reason_value(&columns.order_id, row).unwrap_or_default());
            let status = reason_value(&columns.reason, row)
                .as_deref()
                .map(parse_cancel_status)
                .unwrap_or(CancelAckStatus::Cancelled);
            Some(CoreEvent::CancelAck(CancelAck {
                client_order_id,
                status,
                timestamp_mono_ns: timestamps.ts_process_mono_ns,
            }))
        }
        "Risk" => payload
            .and_then(|value| serde_json::from_value::<RiskEvent>(value).ok())
            .map(CoreEvent::Risk),
        "System" => payload
            .and_then(|value| serde_json::from_value::<SystemEvent>(value).ok())
            .map(CoreEvent::System),
        _ => None,
    };

    Ok(event)
}

fn parse_payload(payloads: &StringArray, row: usize) -> Result<Option<Value>> {
    if payloads.is_null(row) {
        return Ok(None);
    }
    let raw = payloads.value(row);
    if raw.trim().is_empty() {
        return Ok(None);
    }
    Ok(Some(serde_json::from_str(raw)?))
}

fn payload_string(payload: &Option<Value>, key: &str) -> Option<String> {
    payload
        .as_ref()
        .and_then(|value| value.get(key))
        .and_then(|value| value.as_str())
        .map(|value| value.to_string())
}

fn payload_u64(payload: &Option<Value>, key: &str) -> Option<u64> {
    payload
        .as_ref()
        .and_then(|value| value.get(key))
        .and_then(|value| value.as_u64().or_else(|| value.as_i64().map(|v| v as u64)))
}

fn payload_bool(payload: &Option<Value>, key: &str) -> Option<bool> {
    payload
        .as_ref()
        .and_then(|value| value.get(key))
        .and_then(|value| value.as_bool())
}

fn reason_value(array: &StringArray, row: usize) -> Option<String> {
    if array.is_null(row) {
        None
    } else {
        Some(array.value(row).to_string())
    }
}

fn parse_side(array: &UInt8Array, row: usize) -> Option<Side> {
    if array.is_null(row) {
        None
    } else {
        match array.value(row) {
            0 => Some(Side::Buy),
            1 => Some(Side::Sell),
            _ => None,
        }
    }
}

fn parse_order_reason(raw: &str) -> OrderReason {
    match raw {
        "QuoteRefresh" => OrderReason::QuoteRefresh,
        "MakerQuote" => OrderReason::MakerQuote,
        "TickMove" => OrderReason::TickMove,
        "InventorySkew" => OrderReason::InventorySkew,
        "VolGate" => OrderReason::VolGate,
        "WindDown" => OrderReason::WindDown,
        "Resync" => OrderReason::Resync,
        "Reconcile" => OrderReason::Reconcile,
        "SelfTradeAvoid" => OrderReason::SelfTradeAvoid,
        "RiskKill" => OrderReason::RiskKill,
        "Manual" => OrderReason::Manual,
        "PostFill" => OrderReason::PostFill,
        "Signal" => OrderReason::Signal,
        "BundleArb" => OrderReason::BundleArb,
        "SafeMode" => OrderReason::SafeMode,
        "Timeout" => OrderReason::Timeout,
        _ => OrderReason::Manual,
    }
}

fn parse_order_ack_status(raw: &str) -> OrderAckStatus {
    if raw.starts_with("Rejected") {
        return OrderAckStatus::Rejected {
            reason: raw.to_string(),
        };
    }
    if raw.starts_with("Matched") {
        let fill_size = parse_number_field(raw, "fill_size").unwrap_or(0);
        let fill_price = parse_number_field(raw, "fill_price").unwrap_or(0);
        return OrderAckStatus::Matched {
            fill_size,
            fill_price: fill_price as u16,
        };
    }
    match raw {
        "Accepted" => OrderAckStatus::Accepted,
        "Timeout" => OrderAckStatus::Timeout,
        _ => OrderAckStatus::Accepted,
    }
}

fn parse_cancel_status(raw: &str) -> CancelAckStatus {
    if raw.starts_with("Rejected") {
        return CancelAckStatus::Rejected {
            reason: raw.to_string(),
        };
    }
    match raw {
        "Cancelled" => CancelAckStatus::Cancelled,
        "NotFound" => CancelAckStatus::NotFound,
        "Timeout" => CancelAckStatus::Timeout,
        _ => CancelAckStatus::Cancelled,
    }
}

fn parse_number_field(raw: &str, field: &str) -> Option<u64> {
    let start = raw.find(field)?;
    let value_start = raw[start..].find(':')? + start + 1;
    let remainder = raw[value_start..].trim();
    let end = remainder
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(remainder.len());
    remainder[..end].trim().parse::<u64>().ok()
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
    let mut paper_book = PaperBook::new(100.0, 100);
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
                        paper_book
                            .market_book_mut()
                            .set_level_unchecked(Side::Buy, tick, size);
                    }
                    for (tick, size) in asks {
                        book.set_level_unchecked(Side::Sell, tick, size);
                        paper_book
                            .market_book_mut()
                            .set_level_unchecked(Side::Sell, tick, size);
                    }
                }
                MarketDataEvent::BookDelta {
                    side,
                    tick,
                    new_size,
                    ..
                } => {
                    if book.validate_inbound_tick(tick, "book").is_ok() {
                        book.set_level_unchecked(side, tick, new_size);
                        paper_book
                            .market_book_mut()
                            .set_level_unchecked(side, tick, new_size);
                    }
                }
                MarketDataEvent::Trade {
                    side,
                    price_tick,
                    size,
                    timestamps,
                    ..
                } => {
                    let fills = fill_sim.on_trade(
                        price_tick,
                        size,
                        side,
                        timestamps.ts_process_mono_ns as u64,
                    );
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
            HashMap::new(),
            now_ns,
        );

        let actions = strategy.on_update(&ctx);
        pending_actions.extend(actions);

        while let Some(action) = pending_actions.pop_front() {
            match action {
                StrategyAction::PlaceOrder {
                    asset_id,
                    side,
                    kind,
                    order_type,
                    reason,
                } => {
                    let client_order_id = order_manager.generate_client_id();
                    let queue_ahead = match kind {
                        OrderKind::Limit { price_tick, .. } => {
                            paper_book.estimate_queue_ahead(side, price_tick)
                        }
                        _ => 0,
                    };

                    let order = Order::new(
                        client_order_id.clone(),
                        asset_id,
                        side,
                        kind,
                        order_type,
                        reason,
                        now_ns,
                    );

                    let order_id = fill_sim.submit_order_with_queue(order, queue_ahead, now_ns);

                    if let OrderKind::Limit {
                        price_tick,
                        size_shares,
                    } = kind
                    {
                        paper_book.add_order(PaperOrder {
                            order_id,
                            client_order_id,
                            side,
                            price_tick,
                            size: size_shares,
                            timestamp_ns: now_ns,
                            queue_position: queue_ahead,
                        });
                    }
                }
                StrategyAction::CancelOrder {
                    client_order_id, ..
                } => {
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
            SimEvent::OrderRejected {
                client_order_id, ..
            } => {
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
            HashMap::new(),
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
            | MarketDataEvent::BestBidAsk { timestamps, .. } => {
                timestamps.ts_process_mono_ns as u64
            }
            MarketDataEvent::ConnectionStatus {
                timestamp_mono_ns, ..
            }
            | MarketDataEvent::ParseError {
                timestamp_mono_ns, ..
            } => *timestamp_mono_ns as u64,
        },
        CoreEvent::Signal(signal) => signal.timestamp_mono_ns as u64,
        CoreEvent::OrderIntent(intent) => intent.timestamp_mono_ns as u64,
        CoreEvent::OrderAck(ack) => ack.timestamp_mono_ns as u64,
        CoreEvent::Fill(fill) => fill.timestamps.ts_process_mono_ns as u64,
        CoreEvent::CancelAck(cancel) => cancel.timestamp_mono_ns as u64,
        CoreEvent::Risk(_) => 0,
        CoreEvent::System(system) => match system {
            mtrader_core::events::SystemEvent::SafeMode {
                timestamp_mono_ns, ..
            }
            | mtrader_core::events::SystemEvent::SafeModeCleared { timestamp_mono_ns }
            | mtrader_core::events::SystemEvent::ResnaphotRequested {
                timestamp_mono_ns, ..
            }
            | mtrader_core::events::SystemEvent::MarketLifecycle {
                timestamp_mono_ns, ..
            }
            | mtrader_core::events::SystemEvent::Heartbeat { timestamp_mono_ns } => {
                *timestamp_mono_ns as u64
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mtrader_core::events::EventTimestamps;
    use mtrader_recorder::ParquetEventWriter;
    use tempfile::tempdir;

    fn make_timestamps(ts_ms: u64) -> EventTimestamps {
        EventTimestamps {
            ts_exchange_ms: ts_ms as i64,
            ts_recv_mono_ns: (ts_ms * 1_000_000) as i64,
            ts_process_mono_ns: (ts_ms * 1_000_000) as i64,
        }
    }

    #[test]
    fn parquet_roundtrip_market_data() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("events.parquet");

        let mut writer = ParquetEventWriter::new();
        writer.open(&path).unwrap();

        let event = CoreEvent::MarketData(MarketDataEvent::BookDelta {
            market_id: MarketId("market".to_string()),
            token_id: TokenId("token".to_string()),
            side: Side::Buy,
            tick: 5000,
            new_size: 100,
            best_bid: Some(4990),
            best_ask: Some(5010),
            order_hash: "hash".to_string(),
            timestamps: make_timestamps(1_000),
        });
        writer.write_event(&event).unwrap();

        let trade = CoreEvent::MarketData(MarketDataEvent::Trade {
            market_id: MarketId("market".to_string()),
            token_id: TokenId("token".to_string()),
            side: Side::Sell,
            price_tick: 5050,
            size: 50,
            fee_rate_bps: 50,
            timestamps: make_timestamps(2_000),
        });
        writer.write_event(&trade).unwrap();
        writer.close().unwrap();

        let loaded = load_events_from_parquet(&path).unwrap();
        assert_eq!(loaded.len(), 2);
        assert!(matches!(
            loaded[0],
            CoreEvent::MarketData(MarketDataEvent::BookDelta { .. })
        ));
        assert!(matches!(
            loaded[1],
            CoreEvent::MarketData(MarketDataEvent::Trade { .. })
        ));
    }
}
