//! Event-Driven Backtest Architecture
//!
//! This module provides an event-driven backtesting framework that processes
//! events in timestamp order, simulating real market conditions including
//! latency modeling and order book dynamics.
//!
//! # Architecture
//!
//! - `EventDrivenBacktestConfig`: Configuration for the backtest
//! - `SimulationEvent`: Events that drive the simulation
//! - `EventQueue`: Priority queue ordered by timestamp
//! - `EventDrivenBacktest`: Main backtest engine
//!
//! # Event Processing Flow
//!
//! 1. Load market data from parquet files
//! 2. Initialize event queue with market data events
//! 3. Process events in timestamp order:
//!    - Apply latency model to event timestamps
//!    - Process event based on type
//!    - Generate new events (e.g., fills generate PnL updates)
//!    - Continue until queue empty or end time reached
//!
//! # Latency Modeling
//!
//! Supports three latency models:
//! - `Zero`: No latency (instant execution)
//! - `Fixed`: Fixed latency in microseconds
//! - `Variable`: Normally distributed latency with configurable mean/std

use mtrader_book::ArrayBook;
use mtrader_core::{ClientOrderId, OrderReason, Side, Size, Tick};
use mtrader_execution::{Order, OrderKind, OrderState, OrderType};
use mtrader_risk::{PnLTracker, PositionTracker};
use mtrader_strategy::{Strategy, StrategyAction, StrategyContext, WorkingOrder};
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::collections::{BinaryHeap, HashMap};
use std::cmp::Reverse;
use std::path::Path;
use thiserror::Error;

/// Configuration for event-driven backtesting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventDrivenBacktestConfig {
    /// Start time of simulation (timestamp in ns)
    pub start_time: i64,
    /// End time of simulation (timestamp in ns)
    pub end_time: i64,
    /// Initial balance (micro-USDC)
    pub initial_balance: i64,
    /// Latency model for order processing
    pub latency_model: LatencyModel,
    /// Enable slippage modeling
    pub enable_slippage: bool,
    /// Enable queue position modeling
    pub enable_queue_position: bool,
    /// Position limits
    pub position_limits: PositionLimits,
    /// Fee rate in basis points
    pub fee_rate_bps: u32,
}

impl Default for EventDrivenBacktestConfig {
    fn default() -> Self {
        Self {
            start_time: 0,
            end_time: i64::MAX,
            initial_balance: 1_000_000_000, // 1000 USDC
            latency_model: LatencyModel::Fixed(50_000), // 50ms default latency
            enable_slippage: true,
            enable_queue_position: true,
            position_limits: PositionLimits::default(),
            fee_rate_bps: 50,
        }
    }
}

/// Re-export PositionLimits for convenience
pub use mtrader_risk::PositionLimits;

/// Latency model for simulating order processing delays.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LatencyModel {
    /// No latency (instant execution)
    Zero,
    /// Fixed latency in microseconds
    Fixed(u64),
    /// Variable latency with normal distribution
    Variable {
        /// Mean latency in microseconds
        mean_us: u64,
        /// Standard deviation in microseconds
        std_us: u64,
    },
}

impl LatencyModel {
    /// Generate latency in nanoseconds for the current time.
    pub fn sample<R: Rng>(&self, rng: &mut R, _now_ns: u64) -> u64 {
        match self {
            LatencyModel::Zero => 0,
            LatencyModel::Fixed(us) => *us * 1000,
            LatencyModel::Variable { mean_us, std_us } => {
                let normal = rand_distr::Normal::new(*mean_us as f64, *std_us as f64).unwrap();
                let sample = rng.sample(normal);
                (sample.max(0.0) as u64) * 1000
            }
        }
    }
}

/// Events that drive the simulation.
#[derive(Debug, Clone)]
pub enum SimulationEvent {
    /// Market data update (order book snapshot or delta)
    MarketDataEvent {
        /// Timestamp of the event
        timestamp_ns: u64,
        /// Asset/market identifier
        asset_id: String,
        /// Order book data
        book_data: BookData,
    },
    /// Strategy signal (orders to place)
    StrategySignalEvent {
        /// Timestamp of the event
        timestamp_ns: u64,
        /// Orders to submit
        orders: Vec<Order>,
    },
    /// Order fill confirmation
    FillEvent {
        /// Timestamp of the fill
        timestamp_ns: u64,
        /// Order that was filled
        order_id: String,
        /// Client order ID
        client_order_id: ClientOrderId,
        /// Asset ID
        asset_id: String,
        /// Side (buy/sell)
        side: Side,
        /// Fill price
        price_tick: Tick,
        /// Fill size
        size: Size,
        /// Fee charged
        fee_micro_usdc: i64,
    },
    /// Timer/callback event
    TimerEvent {
        /// Timestamp when timer fires
        timestamp_ns: u64,
        /// Callback identifier
        timer_id: String,
        /// User data
        data: serde_json::Value,
    },
    /// Circuit breaker event
    CircuitBreakerEvent {
        /// Timestamp of the event
        timestamp_ns: u64,
        /// Reason for trip
        reason: String,
        /// Asset ID (if specific)
        asset_id: Option<String>,
    },
}

impl SimulationEvent {
    /// Get the timestamp of this event.
    pub fn timestamp_ns(&self) -> u64 {
        match self {
            SimulationEvent::MarketDataEvent { timestamp_ns, .. } => *timestamp_ns,
            SimulationEvent::StrategySignalEvent { timestamp_ns, .. } => *timestamp_ns,
            SimulationEvent::FillEvent { timestamp_ns, .. } => *timestamp_ns,
            SimulationEvent::TimerEvent { timestamp_ns, .. } => *timestamp_ns,
            SimulationEvent::CircuitBreakerEvent { timestamp_ns, .. } => *timestamp_ns,
        }
    }
}

/// Order book data for market events.
#[derive(Debug, Clone)]
pub enum BookData {
    /// Full order book snapshot
    Snapshot {
        best_bid: Option<Tick>,
        best_bid_size: Size,
        best_ask: Option<Tick>,
        best_ask_size: Size,
        mid_tick: Option<Tick>,
    },
    /// Order book delta (updates)
    Delta {
        best_bid: Option<Tick>,
        best_bid_size: Size,
        best_ask: Option<Tick>,
        best_ask_size: Size,
    },
}

/// Order manager for tracking active orders.
#[derive(Debug, Clone)]
struct ActiveOrder {
    order: Order,
    /// When the order was submitted
    submitted_at_ns: u64,
    /// Queue position (if enabled)
    queue_ahead: Size,
}

/// Event queue with priority ordering by timestamp.
struct EventQueue {
    heap: BinaryHeap<Reverse<QueuedEvent>>,
}

#[derive(Debug, Clone)]
struct QueuedEvent {
    timestamp_ns: u64,
    event: SimulationEvent,
    /// Sequence number for tie-breaking
    sequence: u64,
}

impl PartialEq for QueuedEvent {
    fn eq(&self, other: &Self) -> bool {
        self.timestamp_ns == other.timestamp_ns && self.sequence == other.sequence
    }
}

impl Eq for QueuedEvent {}

impl PartialOrd for QueuedEvent {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for QueuedEvent {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Reverse order for min-heap behavior (earliest first)
        self.timestamp_ns
            .cmp(&other.timestamp_ns)
            .then(other.sequence.cmp(&self.sequence))
    }
}

impl EventQueue {
    fn new() -> Self {
        Self {
            heap: BinaryHeap::new(),
        }
    }

    fn push(&mut self, event: SimulationEvent) {
        let timestamp_ns = event.timestamp_ns();
        self.heap.push(Reverse(QueuedEvent {
            timestamp_ns,
            event,
            sequence: 0, // Will be set by backtest
        }));
    }

    fn push_with_sequence(&mut self, event: SimulationEvent, sequence: u64) {
        let timestamp_ns = event.timestamp_ns();
        self.heap.push(Reverse(QueuedEvent {
            timestamp_ns,
            event,
            sequence,
        }));
    }

    fn pop(&mut self) -> Option<SimulationEvent> {
        self.heap.pop().map(|qe| qe.0.event)
    }

    fn peek(&self) -> Option<&SimulationEvent> {
        self.heap.peek().map(|qe| &qe.0.event)
    }

    fn len(&self) -> usize {
        self.heap.len()
    }

    fn is_empty(&self) -> bool {
        self.heap.is_empty()
    }
}

/// Performance metrics collected during backtest.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct BacktestMetrics {
    /// Total number of events processed
    pub events_processed: u64,
    /// Number of market data events
    pub market_data_events: u64,
    /// Number of orders submitted
    pub orders_submitted: u64,
    /// Number of fills
    pub fills: u64,
    /// Total fees paid
    pub total_fees_micro_usdc: i64,
    /// Total volume traded (micro-shares)
    pub total_volume: u64,
    /// Gross PnL (micro-USDC)
    pub gross_pnl: i64,
    /// Net PnL after fees (micro-USDC)
    pub net_pnl: i64,
    /// Maximum drawdown (micro-USDC)
    pub max_drawdown: i64,
    /// Final balance (micro-USDC)
    pub final_balance: i64,
    /// Return percentage
    pub return_pct: f64,
    /// Sharpe ratio (placeholder)
    pub sharpe_ratio: f64,
    /// Number of round trips
    pub round_trips: u64,
    /// Average trade duration (ns)
    pub avg_trade_duration_ns: u64,
    /// Events by type for debugging
    pub event_counts: HashMap<String, u64>,
    /// Fill price deviation from mid (for slippage analysis)
    pub fill_deviations: Vec<f64>,
    /// Timestamp of first event
    pub first_event_ns: Option<u64>,
    /// Timestamp of last event
    pub last_event_ns: Option<u64>,
}

impl BacktestMetrics {
    fn record_fill(&mut self, price_tick: Tick, mid_tick: Option<Tick>, size: Size) {
        self.fills += 1;
        self.total_volume += size as u64;
        
        if let Some(mid) = mid_tick {
            let deviation = ((price_tick as f64 - mid as f64) / mid as f64) * 10000.0;
            self.fill_deviations.push(deviation);
        }
    }

    fn compute_return_pct(&mut self, initial_balance: i64) {
        if initial_balance != 0 {
            self.return_pct = (self.final_balance as f64 / initial_balance as f64 - 1.0) * 100.0;
        }
    }
}

/// Results of a backtest run.
#[derive(Debug, Clone)]
pub struct BacktestResults {
    /// Performance metrics
    pub metrics: BacktestMetrics,
    /// Trade history
    pub trades: Vec<TradeRecord>,
    /// Order history
    pub orders: Vec<OrderRecord>,
    /// Final position (net size only for comparison)
    pub final_position_net_size: i64,
    /// Final PnL snapshot
    pub final_pnl: mtrader_risk::PnLSnapshot,
}

/// A recorded trade.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradeRecord {
    pub timestamp_ns: u64,
    pub order_id: String,
    pub asset_id: String,
    pub side: Side,
    pub price_tick: Tick,
    pub size: Size,
    pub fee_micro_usdc: i64,
    pub pnl_micro_usdc: i64,
}

/// A recorded order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderRecord {
    pub timestamp_ns: u64,
    pub order_id: String,
    pub client_order_id: ClientOrderId,
    pub asset_id: String,
    pub side: Side,
    pub price_tick: Tick,
    pub size: Size,
    pub state: OrderState,
    pub filled_size: Size,
    pub filled_at_ns: Option<u64>,
}

/// Backtest errors.
#[derive(Error, Debug)]
pub enum BacktestError {
    #[error("Market data load error: {0}")]
    MarketDataLoadError(String),
    
    #[error("Order processing error: {0}")]
    OrderProcessingError(String),
    
    #[error("Fill simulation error: {0}")]
    FillSimulationError(String),
    
    #[error("Strategy error: {0}")]
    StrategyError(String),
    
    #[error("Limit check failed: {0}")]
    LimitCheckFailed(String),
    
    #[error("End of simulation")]
    EndOfSimulation,
}

/// Main event-driven backtest engine.
pub struct EventDrivenBacktest {
    /// Configuration
    config: EventDrivenBacktestConfig,
    /// Event queue
    event_queue: EventQueue,
    /// Strategy instance (owned for mutation during backtest)
    strategy: Box<dyn Strategy>,
    /// Active orders by order ID
    active_orders: HashMap<String, ActiveOrder>,
    /// Order book per asset
    order_books: HashMap<String, ArrayBook>,
    /// PnL tracker
    pnl_tracker: PnLTracker,
    /// Position tracker
    position_tracker: PositionTracker,
    /// Performance metrics
    metrics: BacktestMetrics,
    /// Trade history
    trades: Vec<TradeRecord>,
    /// Order history
    orders: Vec<OrderRecord>,
    /// Sequence counter for event ordering
    sequence_counter: u64,
    /// Next order ID counter
    next_order_id: u64,
    /// Random number generator for latency/slippage
    rng: rand::rngs::StdRng,
}

impl EventDrivenBacktest {
    /// Create a new event-driven backtest.
    pub fn new(strategy: Box<dyn Strategy>, config: EventDrivenBacktestConfig) -> Self {
        let position_tracker = PositionTracker::new();
        let pnl_tracker = PnLTracker::new(config.initial_balance);
        
        Self {
            config,
            event_queue: EventQueue::new(),
            strategy,
            active_orders: HashMap::new(),
            order_books: HashMap::new(),
            pnl_tracker,
            position_tracker,
            metrics: BacktestMetrics::default(),
            trades: Vec::new(),
            orders: Vec::new(),
            sequence_counter: 0,
            next_order_id: 1,
            rng: rand::SeedableRng::from_entropy(),
        }
    }

    /// Load market data from a parquet file.
    pub fn load_market_data(&mut self, _parquet_path: &Path) -> Result<(), BacktestError> {
        // Placeholder: In a full implementation, this would:
        // 1. Read parquet file with order book data
        // 2. Convert to MarketDataEvent(s)
        // 3. Add to event queue
        //
        // For now, this is a placeholder that allows the backtest
        // to run with synthetic data added later.
        Ok(())
    }

    /// Load market data from a directory of parquet files.
    pub fn load_market_data_dir(&mut self, dir: &Path) -> Result<(), BacktestError> {
        // Similar to load_market_data but reads all parquet files in directory
        self.load_market_data(dir)
    }

    /// Add a market data event to the queue.
    pub fn add_market_data(&mut self, timestamp_ns: u64, asset_id: String, book_data: BookData) {
        let event = SimulationEvent::MarketDataEvent {
            timestamp_ns,
            asset_id,
            book_data,
        };
        self.event_queue.push_with_sequence(event, self.sequence_counter);
        self.sequence_counter += 1;
    }

    /// Add a timer event.
    pub fn add_timer(&mut self, timestamp_ns: u64, timer_id: String, data: serde_json::Value) {
        let event = SimulationEvent::TimerEvent {
            timestamp_ns,
            timer_id,
            data,
        };
        self.event_queue.push_with_sequence(event, self.sequence_counter);
        self.sequence_counter += 1;
    }

    /// Generate a new order ID.
    fn next_order_id(&mut self) -> String {
        let id = format!("bt-{}", self.next_order_id);
        self.next_order_id += 1;
        id
    }

    /// Run the backtest.
    pub fn run(&mut self) -> Result<BacktestResults, BacktestError> {
        let start_time_ns = self.config.start_time as u64;
        let _end_time_ns = self.config.end_time as u64;

        self.metrics.first_event_ns = Some(start_time_ns);

        loop {
            // Check if we've reached end time
            if let Some(event) = self.event_queue.peek() {
                let timestamp = event.timestamp_ns();
                if timestamp as i64 > self.config.end_time {
                    break;
                }
            }

            // Get next event
            let event = match self.event_queue.pop() {
                Some(e) => e,
                None => break,
            };

            self.metrics.last_event_ns = Some(event.timestamp_ns());

            // Process the event
            let new_events = self.process_event(event)?;

            // Add new events to queue
            for new_event in new_events {
                self.event_queue.push_with_sequence(new_event, self.sequence_counter);
                self.sequence_counter += 1;
            }
        }

        // Finalize metrics
        let prices: HashMap<String, Tick> = HashMap::new();
        self.metrics.final_balance = self.pnl_tracker.latest()
            .map(|s| s.net_pnl + self.config.initial_balance)
            .unwrap_or(self.config.initial_balance);
        self.metrics.compute_return_pct(self.config.initial_balance);

        // Calculate Sharpe ratio (placeholder - would need return series)
        self.metrics.sharpe_ratio = 0.0;

        // Get final position (just the net size)
        let final_position_net_size = self.position_tracker.get("default")
            .map(|p| p.net_size)
            .unwrap_or(0);

        // Get final PnL snapshot
        let final_pnl = self.pnl_tracker.snapshot(&self.position_tracker, &prices, 
            self.metrics.last_event_ns.unwrap_or(0));

        let results = BacktestResults {
            metrics: self.metrics.clone(),
            trades: self.trades.clone(),
            orders: self.orders.clone(),
            final_position_net_size,
            final_pnl,
        };

        Ok(results)
    }

    /// Process a single event and return new events to add.
    fn process_event(
        &mut self,
        event: SimulationEvent,
    ) -> Result<Vec<SimulationEvent>, BacktestError> {
        self.metrics.events_processed += 1;
        self.record_event_count(&event);

        match event {
            SimulationEvent::MarketDataEvent {
                timestamp_ns,
                asset_id,
                book_data,
            } => self.process_market_data(timestamp_ns, &asset_id, book_data),
            
            SimulationEvent::StrategySignalEvent {
                timestamp_ns,
                orders,
            } => self.process_strategy_signal(timestamp_ns, orders),
            
            SimulationEvent::FillEvent {
                timestamp_ns,
                order_id,
                client_order_id,
                asset_id,
                side,
                price_tick,
                size,
                fee_micro_usdc,
            } => self.process_fill(
                timestamp_ns,
                &order_id,
                &client_order_id,
                &asset_id,
                side,
                price_tick,
                size,
                fee_micro_usdc,
            ),
            
            SimulationEvent::TimerEvent {
                timestamp_ns: _,
                timer_id: _,
                data: _,
            } => {
                // Timer events are for future extensions (scheduled actions, etc.)
                Ok(Vec::new())
            }
            
            SimulationEvent::CircuitBreakerEvent {
                timestamp_ns: _,
                reason: _,
                asset_id: _,
            } => {
                // Circuit breaker handling for future extensions
                Ok(Vec::new())
            }
        }
    }

    /// Record event count for metrics.
    fn record_event_count(&mut self, event: &SimulationEvent) {
        let name = match event {
            SimulationEvent::MarketDataEvent { .. } => "market_data",
            SimulationEvent::StrategySignalEvent { .. } => "strategy_signal",
            SimulationEvent::FillEvent { .. } => "fill",
            SimulationEvent::TimerEvent { .. } => "timer",
            SimulationEvent::CircuitBreakerEvent { .. } => "circuit_breaker",
        };
        *self.metrics.event_counts.entry(name.to_string()).or_insert(0) += 1;
    }

    /// Process market data event.
    fn process_market_data(
        &mut self,
        timestamp_ns: u64,
        asset_id: &str,
        book_data: BookData,
    ) -> Result<Vec<SimulationEvent>, BacktestError> {
        self.metrics.market_data_events += 1;

        // Update order book
        let book = self.order_books.entry(asset_id.to_string())
            .or_insert_with(|| ArrayBook::new(100));
        
        match &book_data {
            BookData::Snapshot {
                best_bid,
                best_bid_size,
                best_ask,
                best_ask_size,
                mid_tick: _,
            } => {
                if let Some(bid) = best_bid {
                    book.set_level_unchecked(mtrader_core::Side::Buy, *bid, *best_bid_size);
                }
                if let Some(ask) = best_ask {
                    book.set_level_unchecked(mtrader_core::Side::Sell, *ask, *best_ask_size);
                }
            }
            BookData::Delta {
                best_bid,
                best_bid_size,
                best_ask,
                best_ask_size,
            } => {
                if let Some(bid) = best_bid {
                    book.set_level_unchecked(mtrader_core::Side::Buy, *bid, *best_bid_size);
                }
                if let Some(ask) = best_ask {
                    book.set_level_unchecked(mtrader_core::Side::Sell, *ask, *best_ask_size);
                }
            }
        }

        // Get position and PnL
        let position = self.position_tracker.get(asset_id)
            .cloned()
            .unwrap_or_default();
        
        let position_copy = position.clone(); // Clone for use in limit check
        let prices: HashMap<String, Tick> = HashMap::new();
        let pnl = self.pnl_tracker.snapshot(&self.position_tracker, &prices, timestamp_ns);

        // Get our active orders
        let our_bids: Vec<WorkingOrder> = self
            .active_orders
            .values()
            .filter(|o| o.order.side == Side::Buy)
            .map(|o| WorkingOrder {
                tick: o.order.price_tick(),
                client_order_id: o.order.client_order_id.clone(),
            })
            .collect();

        let our_asks: Vec<WorkingOrder> = self
            .active_orders
            .values()
            .filter(|o| o.order.side == Side::Sell)
            .map(|o| WorkingOrder {
                tick: o.order.price_tick(),
                client_order_id: o.order.client_order_id.clone(),
            })
            .collect();

        // Build strategy context
        let ctx = StrategyContext::from_book(
            book,
            asset_id.to_string(),
            position,
            pnl,
            our_bids,
            our_asks,
            HashMap::new(),
            timestamp_ns,
        );

        // Call strategy
        let actions = self.strategy.on_update(&ctx);

        // Convert actions to events
        let mut new_events = Vec::new();

        for action in actions {
            match action {
                StrategyAction::PlaceOrder {
                    asset_id,
                    side,
                    kind,
                    order_type: _,
                    reason,
                } => {
                    if let OrderKind::Limit { price_tick, size_shares } = kind {
                        // Check limits
                        let gross_position = self.position_tracker.gross_position();
                        let open_orders = self.active_orders.len();
                        let daily_volume = 0; // Simplified

                        let is_buy = matches!(side, Side::Buy);
                        let check = self.config.position_limits.check_order(
                            size_shares,
                            position_copy.net_size,
                            is_buy,
                            gross_position,
                            daily_volume,
                            open_orders,
                        );

                        if !check.is_ok() {
                            continue; // Skip order that fails limits
                        }

                        // Apply latency
                        let latency_ns = self.config.latency_model.sample(&mut self.rng, timestamp_ns);
                        let order_timestamp = timestamp_ns + latency_ns;

                        let order = Order::new(
                            ClientOrderId(format!("bt-cli-{}", self.next_order_id)),
                            asset_id.clone(),
                            side,
                            kind,
                            OrderType::Limit,
                            reason,
                            order_timestamp,
                        );

                        self.metrics.orders_submitted += 1;

                        // Estimate queue position
                        let queue_ahead = if self.config.enable_queue_position {
                            // Get book data before we enter the loop
                            let best_ask_size = book.best_ask_size().unwrap_or(0);
                            let best_bid_size = book.best_bid_size().unwrap_or(0);
                            match side {
                                Side::Buy => best_ask_size,
                                Side::Sell => best_bid_size,
                            }
                        } else {
                            0
                        };

                        let order_id = order.order_id.clone();
                        self.active_orders.insert(order_id.clone(), ActiveOrder {
                            order: order.clone(),
                            submitted_at_ns: order_timestamp,
                            queue_ahead,
                        });

                        // Record order
                        self.orders.push(OrderRecord {
                            timestamp_ns: order_timestamp,
                            order_id: order_id.clone(),
                            client_order_id: order.client_order_id.clone(),
                            asset_id: asset_id.clone(),
                            side,
                            price_tick,
                            size: size_shares,
                            state: OrderState::PendingNew,
                            filled_size: 0,
                            filled_at_ns: None,
                        });

                        // Schedule order acknowledgment after latency
                        let ack_event = SimulationEvent::StrategySignalEvent {
                            timestamp_ns: order_timestamp,
                            orders: vec![order],
                        };
                        new_events.push(ack_event);
                    }
                }
                StrategyAction::CancelOrder { client_order_id, reason: _ } => {
                    // Handle cancel - remove order from active orders
                    if let Some(_active_order) = self.active_orders.remove(&client_order_id.0) {
                        // Update order history
                        for order_rec in self.orders.iter_mut() {
                            if order_rec.client_order_id == client_order_id {
                                order_rec.state = OrderState::Cancelled;
                                break;
                            }
                        }
                    }
                }
                StrategyAction::AmendOrder { client_order_id, new_kind, reason: _ } => {
                    // Handle amend - cancel old, place new
                    if let Some(active_order) = self.active_orders.remove(&client_order_id.0) {
                        // Update old order as cancelled
                        for order_rec in self.orders.iter_mut() {
                            if order_rec.client_order_id == client_order_id {
                                order_rec.state = OrderState::Cancelled;
                                break;
                            }
                        }

                        // Place new order
                        if let OrderKind::Limit { price_tick: _, size_shares: _ } = new_kind {
                            let order = Order::new(
                                ClientOrderId(format!("bt-cli-{}", self.next_order_id)),
                                active_order.order.asset_id.clone(),
                                active_order.order.side,
                                new_kind,
                                OrderType::Limit,
                                OrderReason::MakerQuote,
                                timestamp_ns,
                            );

                            let latency_ns = self.config.latency_model.sample(&mut self.rng, timestamp_ns);
                            let order_timestamp = timestamp_ns + latency_ns;

                            let new_order_id = order.order_id.clone();
                            self.active_orders.insert(new_order_id.clone(), ActiveOrder {
                                order: order.clone(),
                                submitted_at_ns: order_timestamp,
                                queue_ahead: active_order.queue_ahead,
                            });

                            let ack_event = SimulationEvent::StrategySignalEvent {
                                timestamp_ns: order_timestamp,
                                orders: vec![order],
                            };
                            new_events.push(ack_event);
                        }
                    }
                }
                StrategyAction::NoOp => {}
            }
        }

        Ok(new_events)
    }

    /// Process strategy signal (order acknowledgment).
    fn process_strategy_signal(
        &mut self,
        timestamp_ns: u64,
        orders: Vec<Order>,
    ) -> Result<Vec<SimulationEvent>, BacktestError> {
        let mut new_events = Vec::new();

        for order in orders {
            // Order is now live - simulate fill if price is hit
            let book = self.order_books.get(&order.asset_id).cloned();
            
            // Simulate fill
            if let Some((price_tick, size)) = self.simulate_fill(book.as_ref(), &order, timestamp_ns) {
                // Calculate fee
                let fee = self.calculate_fee(price_tick, size);
                
                // Record fill
                let fill_event = SimulationEvent::FillEvent {
                    timestamp_ns,
                    order_id: order.order_id.clone(),
                    client_order_id: order.client_order_id.clone(),
                    asset_id: order.asset_id.clone(),
                    side: order.side,
                    price_tick,
                    size,
                    fee_micro_usdc: fee,
                };
                
                new_events.push(fill_event);
            } else {
                // Order remains active - update state to Open
                for order_rec in self.orders.iter_mut() {
                    if order_rec.order_id == order.order_id {
                        order_rec.state = OrderState::Open;
                        break;
                    }
                }
            }
        }

        Ok(new_events)
    }

    /// Process a fill event.
    fn process_fill(
        &mut self,
        timestamp_ns: u64,
        order_id: &str,
        _client_order_id: &ClientOrderId,
        asset_id: &str,
        side: Side,
        price_tick: Tick,
        size: Size,
        fee_micro_usdc: i64,
    ) -> Result<Vec<SimulationEvent>, BacktestError> {
        // Remove from active orders
        self.active_orders.remove(order_id);

        // Calculate PnL (simplified)
        let cost = (price_tick as i64 * size as i64) / 10000;
        let pnl_delta = if side == Side::Buy {
            -cost - fee_micro_usdc
        } else {
            cost - fee_micro_usdc
        };

        // Update position tracker
        self.position_tracker.on_fill(asset_id, side, price_tick, size);
        
        // Add fees to PnL tracker
        self.pnl_tracker.add_fees(fee_micro_usdc);

        // Update metrics
        self.metrics.fills += 1;
        self.metrics.total_fees_micro_usdc += fee_micro_usdc;
        self.metrics.gross_pnl += pnl_delta;
        self.metrics.net_pnl += pnl_delta;

        let book = self.order_books.get(asset_id).cloned();
        let mid = book.as_ref().and_then(|b| b.mid_tick());
        self.metrics.record_fill(price_tick, mid, size);

        // Record trade
        self.trades.push(TradeRecord {
            timestamp_ns,
            order_id: order_id.to_string(),
            asset_id: asset_id.to_string(),
            side,
            price_tick,
            size,
            fee_micro_usdc,
            pnl_micro_usdc: pnl_delta,
        });

        // Update order record
        for order_rec in self.orders.iter_mut() {
            if order_rec.order_id == order_id {
                order_rec.state = OrderState::Filled;
                order_rec.filled_size = size;
                order_rec.filled_at_ns = Some(timestamp_ns);
                break;
            }
        }

        // Notify strategy
        let position = self.position_tracker.get(asset_id)
            .cloned()
            .unwrap_or_default();
        let prices: HashMap<String, Tick> = HashMap::new();
        let pnl = self.pnl_tracker.snapshot(&self.position_tracker, &prices, timestamp_ns);
        
        self.strategy.on_fill(
            &StrategyContext::from_book(
                book.as_ref().unwrap_or(&ArrayBook::new(100)),
                asset_id.to_string(),
                position,
                pnl,
                Vec::new(),
                Vec::new(),
                HashMap::new(),
                timestamp_ns,
            ),
            side,
            price_tick,
            size,
        );

        // Update max drawdown
        let current_balance = self.pnl_tracker.latest()
            .map(|s| s.net_pnl + self.config.initial_balance)
            .unwrap_or(self.config.initial_balance);
        let peak_balance = self.metrics.final_balance.max(current_balance);
        let drawdown = peak_balance - current_balance;
        self.metrics.max_drawdown = self.metrics.max_drawdown.max(drawdown);

        Ok(Vec::new())
    }

    /// Estimate queue position at a price level.
    fn estimate_queue_ahead(&self, book: &ArrayBook, side: Side, _price_tick: Tick) -> Size {
        if !self.config.enable_queue_position {
            return 0;
        }

        // Simple queue estimation based on order book depth
        match side {
            Side::Buy => {
                // Estimate sell orders at or below our price
                book.best_ask_size().unwrap_or(0)
            }
            Side::Sell => {
                // Estimate buy orders at or above our price
                book.best_bid_size().unwrap_or(0)
            }
        }
    }

    /// Simulate fill for an order.
    fn simulate_fill(
        &self,
        book: Option<&ArrayBook>,
        order: &Order,
        _timestamp_ns: u64,
    ) -> Option<(Tick, Size)> {
        let price_tick = order.price_tick();

        match order.side {
            Side::Buy => {
                // Check if market ask is at or below our bid
                if let Some(b) = book {
                    if let Some(best_ask) = b.best_ask() {
                        if best_ask <= price_tick {
                            // Calculate fill size with slippage
                            let slippage = if self.config.enable_slippage {
                                let mut rng = rand::thread_rng();
                                rng.gen_range(0..2) // 0-1 tick slippage
                            } else {
                                0
                            };
                            let fill_price = best_ask + slippage;
                            let fill_size = order.size_shares()
                                .and_then(|s| b.best_ask_size().map(|bs| s.min(bs.max(1))))
                                .unwrap_or(1);
                            return Some((fill_price, fill_size));
                        }
                    }
                }
            }
            Side::Sell => {
                if let Some(b) = book {
                    if let Some(best_bid) = b.best_bid() {
                        if best_bid >= price_tick {
                            // Calculate fill size with slippage
                            let slippage = if self.config.enable_slippage {
                                let mut rng = rand::thread_rng();
                                rng.gen_range(0..2)
                            } else {
                                0
                            };
                            let fill_price = best_bid - slippage;
                            let fill_size = order.size_shares()
                                .and_then(|s| b.best_bid_size().map(|bs| s.min(bs.max(1))))
                                .unwrap_or(1);
                            return Some((fill_price, fill_size));
                        }
                    }
                }
            }
        }

        None
    }

    /// Calculate fee for a trade.
    fn calculate_fee(&self, price_tick: Tick, size: Size) -> i64 {
        // Fee model: min(price, 1-price) * size * fee_rate / 10000
        let min_tick = price_tick.min(10000 - price_tick) as u128;
        let fee = (self.config.fee_rate_bps as u128 * min_tick * size as u128)
            / (10_000u128 * 10_000u128);
        fee as i64
    }

    /// Get the event queue for testing/debugging.
    pub fn event_queue_len(&self) -> usize {
        self.event_queue.len()
    }

    /// Get active orders count.
    pub fn active_orders_count(&self) -> usize {
        self.active_orders.len()
    }

    /// Get current metrics (for monitoring during run).
    pub fn current_metrics(&self) -> BacktestMetrics {
        self.metrics.clone()
    }
}

/// Create a simple test strategy for unit tests.
#[cfg(test)]
pub mod test_strategy {
    use super::*;
    use mtrader_strategy::Strategy;

    /// A simple moving average crossover strategy for testing.
    pub struct TestStrategy {
        active: bool,
    }

    impl TestStrategy {
        pub fn new() -> Self {
            Self { active: true }
        }
    }

    impl Strategy for TestStrategy {
        fn name(&self) -> &str {
            "test-ma-crossover"
        }

        fn on_update(&mut self, ctx: &StrategyContext) -> Vec<StrategyAction> {
            if !self.active {
                return vec![];
            }

            // Simple logic: buy if we have no position and market looks cheap
            if ctx.position.net_size == 0 && ctx.best_bid.is_some() {
                return vec![StrategyAction::PlaceOrder {
                    asset_id: ctx.asset_id.clone(),
                    side: Side::Buy,
                    kind: OrderKind::Limit {
                        price_tick: ctx.best_ask.unwrap_or(50),
                        size_shares: 1000,
                    },
                    order_type: OrderType::Limit,
                    reason: OrderReason::MakerQuote,
                }];
            }

            // Sell if we have position and market looks expensive
            if ctx.position.net_size > 0 && ctx.best_ask.is_some() {
                return vec![StrategyAction::PlaceOrder {
                    asset_id: ctx.asset_id.clone(),
                    side: Side::Sell,
                    kind: OrderKind::Limit {
                        price_tick: ctx.best_bid.unwrap_or(50),
                        size_shares: 1000,
                    },
                    order_type: OrderType::Limit,
                    reason: OrderReason::MakerQuote,
                }];
            }

            vec![]
        }

        fn on_fill(&mut self, _ctx: &StrategyContext, _side: Side, _tick: Tick, _size: Size) {}

        fn on_halt(&mut self) {
            self.active = false;
        }

        fn on_resume(&mut self) {
            self.active = true;
        }

        fn is_active(&self) -> bool {
            self.active
        }

        fn activate(&mut self) {
            self.active = true;
        }

        fn deactivate(&mut self) {
            self.active = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mtrader_core::Side;

    #[test]
    fn test_event_queue_ordering() {
        let mut queue = EventQueue::new();
        
        // Add events with different timestamps
        queue.push(SimulationEvent::MarketDataEvent {
            timestamp_ns: 100,
            asset_id: "asset-1".to_string(),
            book_data: BookData::Snapshot {
                best_bid: Some(50),
                best_bid_size: 1000,
                best_ask: Some(51),
                best_ask_size: 1000,
                mid_tick: Some(50),
            },
        });
        
        queue.push(SimulationEvent::MarketDataEvent {
            timestamp_ns: 50,
            asset_id: "asset-1".to_string(),
            book_data: BookData::Snapshot {
                best_bid: Some(50),
                best_bid_size: 1000,
                best_ask: Some(51),
                best_ask_size: 1000,
                mid_tick: Some(50),
            },
        });
        
        queue.push(SimulationEvent::MarketDataEvent {
            timestamp_ns: 75,
            asset_id: "asset-1".to_string(),
            book_data: BookData::Snapshot {
                best_bid: Some(50),
                best_bid_size: 1000,
                best_ask: Some(51),
                best_ask_size: 1000,
                mid_tick: Some(50),
            },
        });

        // Events should come out in timestamp order
        assert_eq!(queue.pop().unwrap().timestamp_ns(), 50);
        assert_eq!(queue.pop().unwrap().timestamp_ns(), 75);
        assert_eq!(queue.pop().unwrap().timestamp_ns(), 100);
        assert!(queue.is_empty());
    }

    #[test]
    fn test_event_timestamp_extraction() {
        let event = SimulationEvent::FillEvent {
            timestamp_ns: 12345,
            order_id: "order-1".to_string(),
            client_order_id: ClientOrderId("client-1".into()),
            asset_id: "asset-1".to_string(),
            side: Side::Buy,
            price_tick: 50,
            size: 1000,
            fee_micro_usdc: 10,
        };
        
        assert_eq!(event.timestamp_ns(), 12345);
    }

    #[test]
    fn test_latency_model_zero() {
        let model = LatencyModel::Zero;
        let mut rng = rand::thread_rng();
        let latency = model.sample(&mut rng, 0);
        assert_eq!(latency, 0);
    }

    #[test]
    fn test_latency_model_fixed() {
        let model = LatencyModel::Fixed(1000); // 1000us = 1ms
        let mut rng = rand::thread_rng();
        let latency = model.sample(&mut rng, 0);
        assert_eq!(latency, 1_000_000); // 1ms in ns
    }

    #[test]
    fn test_event_driven_backtest_basic() {
        let config = EventDrivenBacktestConfig::default();
        let strategy = Box::new(test_strategy::TestStrategy::new());
        let mut backtest = EventDrivenBacktest::new(strategy, config);

        // Add some market data
        backtest.add_market_data(
            0,
            "asset-1".to_string(),
            BookData::Snapshot {
                best_bid: Some(49),
                best_bid_size: 1000,
                best_ask: Some(51),
                best_ask_size: 1000,
                mid_tick: Some(50),
            },
        );

        backtest.add_market_data(
            100_000_000, // 100ms later
            "asset-1".to_string(),
            BookData::Snapshot {
                best_bid: Some(48),
                best_bid_size: 1000,
                best_ask: Some(52),
                best_ask_size: 1000,
                mid_tick: Some(50),
            },
        );

        // Run backtest
        let results = backtest.run().unwrap();

        assert!(results.metrics.events_processed >= 2);
    }

    #[test]
    fn test_backtest_metrics_recording() {
        let config = EventDrivenBacktestConfig::default();
        let strategy = Box::new(test_strategy::TestStrategy::new());
        let mut backtest = EventDrivenBacktest::new(strategy, config);

        // Add market data
        backtest.add_market_data(
            0,
            "asset-1".to_string(),
            BookData::Snapshot {
                best_bid: Some(50),
                best_bid_size: 1000,
                best_ask: Some(51),
                best_ask_size: 1000,
                mid_tick: Some(50),
            },
        );

        let results = backtest.run().unwrap();

        // Check metrics were recorded
        assert!(results.metrics.events_processed >= 1);
        assert!(results.metrics.first_event_ns.is_some());
        assert!(results.metrics.final_balance > 0);
    }

    #[test]
    fn test_fee_calculation() {
        let config = EventDrivenBacktestConfig::default();
        let strategy = Box::new(test_strategy::TestStrategy::new());
        let backtest = EventDrivenBacktest::new(strategy, config);

        // Test fee at different price levels
        let fee_buy = backtest.calculate_fee(50, 100000); // 0.5% price
        let fee_mid = backtest.calculate_fee(5000, 100000); // 50% price
        let fee_high = backtest.calculate_fee(9500, 100000); // 95% price

        // Fee should be lower at extremes (min(price, 1-price) model)
        assert!(fee_buy < fee_mid);
        assert!(fee_high < fee_mid);
    }

    #[test]
    fn test_position_limits_check() {
        let limits = PositionLimits::default();
        
        // Should pass
        let check = limits.check_order(10_000_000, 0, true, 0, 0, 0);
        assert!(check.is_ok());

        // Order too large
        let check = limits.check_order(30_000_000, 0, true, 0, 0, 0);
        assert!(!check.is_ok());

        // Order too small
        let check = limits.check_order(100_000, 0, true, 0, 0, 0);
        assert!(!check.is_ok());
    }

    #[test]
    fn test_event_cascade() {
        let config = EventDrivenBacktestConfig {
            latency_model: LatencyModel::Zero,
            ..Default::default()
        };
        let strategy = Box::new(test_strategy::TestStrategy::new());
        let mut backtest = EventDrivenBacktest::new(strategy, config);

        // Add market data with price that will trigger buy
        backtest.add_market_data(
            0,
            "asset-1".to_string(),
            BookData::Snapshot {
                best_bid: Some(49),
                best_bid_size: 1000,
                best_ask: Some(51),
                best_ask_size: 1000,
                mid_tick: Some(50),
            },
        );

        // Add second market data that triggers sell (round trip)
        backtest.add_market_data(
            1_000_000,
            "asset-1".to_string(),
            BookData::Snapshot {
                best_bid: Some(55), // Price moved up, we should have been filled
                best_bid_size: 2000,
                best_ask: Some(56),
                best_ask_size: 1000,
                mid_tick: Some(55),
            },
        );

        let results = backtest.run().unwrap();

        // Events should cascade: market data -> strategy signal -> fill
        assert!(results.metrics.market_data_events >= 2);
        assert!(results.metrics.events_processed >= 2);
    }
}
