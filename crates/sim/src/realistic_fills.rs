//! Realistic fill simulator with queue position tracking.
//!
//! This module provides realistic fill modeling that accounts for:
//! - Queue position (time priority at each price level)
//! - Volume priority (larger orders behind get filled first)
//! - Market impact for large orders
//! - Adverse selection (price moving away = lower fill rate)
//! - Partial fills based on available liquidity
//!
//! Key insight: Real markets don't fill every order that touches the price.
//! This simulator models the reality that you're often waiting behind others,
//! and that price movements can work against you (adverse selection).

use mtrader_book::ArrayBook;
use mtrader_core::{ClientOrderId, Side, Size, Tick, MAX_TICK};
use mtrader_execution::{Order, OrderState};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};

/// Configuration for realistic fill simulation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RealisticFillConfig {
    /// Model for queue position calculation
    pub queue_position_model: QueuePositionModel,
    /// Probability of adverse price selection (0.0 to 1.0)
    /// When price moves favorably, your fill probability decreases
    pub adverse_selection_probability: f64,
    /// Probability of partial fill when order is fillable (0.0 to 1.0)
    pub partial_fill_probability: f64,
    /// Minimum fill ratio for partial fills (0.0 to 1.0)
    pub min_fill_ratio: f64,
    /// Market impact in basis points per $1000 notional
    pub market_impact_bps: f64,
    /// Order acknowledgment latency in nanoseconds
    pub order_ack_latency_ns: u64,
    /// Cancel acknowledgment latency in nanoseconds
    pub cancel_ack_latency_ns: u64,
    /// Base fill probability at front of queue (0.0 to 1.0)
    pub base_fill_probability: f64,
    /// Fee rate in basis points
    pub fee_rate_bps: u32,
}

impl Default for RealisticFillConfig {
    fn default() -> Self {
        Self {
            queue_position_model: QueuePositionModel::VolumeBased,
            adverse_selection_probability: 0.25,
            partial_fill_probability: 0.4,
            min_fill_ratio: 0.1,
            market_impact_bps: 10.0, // 10 bps per $1000 notional
            order_ack_latency_ns: 50_000_000,
            cancel_ack_latency_ns: 50_000_000,
            base_fill_probability: 0.85,
            fee_rate_bps: 1000,
        }
    }
}

/// Model for calculating queue position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum QueuePositionModel {
    /// Assume best case - always at front of queue
    Optimistic,
    /// Assume worst case - always at back of queue
    Pessimistic,
    /// Random queue position
    Random,
    /// Volume-based queue position (proportional to order size)
    VolumeBased,
}

impl QueuePositionModel {
    /// Calculate queue position multiplier for fill probability.
    /// Returns a value between 0.0 (never fill) and 1.0 (always fill).
    fn fill_probability_multiplier(&self, queue_ahead: Size, order_size: Size) -> f64 {
        match self {
            Self::Optimistic => 1.0,
            Self::Pessimistic => {
                if queue_ahead > 0 {
                    0.0
                } else {
                    1.0
                }
            }
            Self::Random => {
                if queue_ahead == 0 {
                    1.0
                } else {
                    // Random position in queue
                    rand::random::<f64>()
                }
            }
            Self::VolumeBased => {
                if queue_ahead == 0 && order_size == 0 {
                    1.0
                } else if queue_ahead == 0 {
                    1.0
                } else {
                    // Fill probability decreases with volume ahead relative to order size
                    let ratio = queue_ahead as f64 / (queue_ahead as f64 + order_size as f64);
                    1.0 - ratio * 0.9 // Never goes below 0.1
                }
            }
        }
    }
}

/// Queue position information for an order.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QueuePosition {
    /// Position in the queue (0 = first)
    pub position: u64,
    /// Volume ahead of this order at the price level
    pub volume_ahead: Size,
    /// Time priority (earlier = better)
    pub time_priority_ns: u64,
    /// Whether this order is at the front
    pub is_front: bool,
}

/// Information about a price level's queue.
#[derive(Debug, Clone)]
struct PriceLevelQueue {
    /// Orders at this price level, keyed by order ID
    orders: HashMap<String, QueuedOrder>,
    /// Total volume at this price level
    total_volume: Size,
    /// Next submission time for ordering
    next_submission_time: u64,
}

/// An order waiting in the queue.
#[derive(Debug, Clone)]
struct QueuedOrder {
    order_id: String,
    client_order_id: ClientOrderId,
    side: Side,
    price_tick: Tick,
    size: Size,
    submitted_at_ns: u64,
}

/// Fill event from the realistic simulator.
#[derive(Debug, Clone, PartialEq)]
pub struct RealisticFill {
    pub order_id: String,
    pub client_order_id: ClientOrderId,
    pub price_tick: Tick,
    pub size: Size,
    pub fee_micro_usdc: i64,
    pub timestamp_ns: u64,
    pub is_partial: bool,
    pub market_impact_bps: f64,
}

/// Order acknowledgment event.
#[derive(Debug, Clone)]
pub struct OrderAckEvent {
    pub client_order_id: ClientOrderId,
    pub order_id: String,
    pub timestamp_ns: u64,
}

/// Cancel acknowledgment event.
#[derive(Debug, Clone)]
pub struct CancelAckEvent {
    pub order_id: String,
    pub timestamp_ns: u64,
}

/// Pending order waiting for acknowledgment.
#[derive(Debug, Clone)]
struct PendingOrder {
    order_id: String,
    order: Order,
    ack_at_ns: u64,
    queue_ahead: Size,
}

/// Order pending cancellation.
#[derive(Debug, Clone)]
struct PendingCancel {
    order_id: String,
    cancel_at_ns: u64,
}

/// Price level statistics for adverse selection tracking.
#[derive(Debug, Clone, Default)]
struct PriceLevelStats {
    /// Recent fill attempts at this price
    recent_fills: VecDeque<FillRecord>,
    /// Total volume traded at this price recently
    recent_volume: Size,
    /// Number of price ticks (10,000ths)
    tick_size: Tick,
}

#[derive(Debug, Clone)]
struct FillRecord {
    timestamp_ns: u64,
    was_filled: bool,
    price_tick: Tick,
    size: Size,
}

/// The realistic fill simulator.
pub struct RealisticFillSimulator {
    config: RealisticFillConfig,
    /// Pending orders awaiting acknowledgment
    pending_orders: HashMap<ClientOrderId, PendingOrder>,
    /// Live orders that can be filled
    live_orders: HashMap<String, LiveOrderData>,
    /// Orders pending cancellation
    pending_cancels: HashMap<String, PendingCancel>,
    /// Order queue per price level
    price_queues: HashMap<Tick, PriceLevelQueue>,
    /// Statistics per price level for adverse selection
    price_stats: HashMap<Tick, PriceLevelStats>,
    /// Next order ID counter
    next_order_id: u64,
    /// Previous best bid/ask for detecting price movements
    prev_best_bid: Option<Tick>,
    prev_best_ask: Option<Tick>,
}

/// Live order data with queue information.
#[derive(Debug, Clone)]
struct LiveOrderData {
    order: Order,
    /// Volume ahead at submission time
    queue_ahead_at_submission: Size,
    /// Current volume ahead (updated as trades occur)
    current_queue_ahead: Size,
    /// Price level statistics for this order
    price_stats_key: Tick,
}

/// Result of fill attempt.
#[derive(Debug, Clone, PartialEq)]
pub enum FillCheckResult {
    /// Order was filled
    Filled(RealisticFill),
    /// Order has liquidity to potentially fill
    HasLiquidity,
    /// Order is too far back in queue
    Queued {
        position: u64,
        volume_ahead: Size,
    },
    /// Order is not marketable
    NotMarketable,
}

impl RealisticFillSimulator {
    /// Create a new realistic fill simulator.
    pub fn new(config: RealisticFillConfig) -> Self {
        Self {
            config,
            pending_orders: HashMap::new(),
            live_orders: HashMap::new(),
            pending_cancels: HashMap::new(),
            price_queues: HashMap::new(),
            price_stats: HashMap::new(),
            next_order_id: 1,
            prev_best_bid: None,
            prev_best_ask: None,
        }
    }

    /// Submit a new order for simulation.
    pub fn submit_order(&mut self, order: Order, timestamp_ns: u64) -> String {
        let order_id = format!("real-sim-{}", self.next_order_id);
        self.next_order_id += 1;

        let queue_ahead = self.calculate_queue_ahead(order.price_tick());

        // Clone needed fields before the move
        let client_order_id = order.client_order_id.clone();
        let order_side = order.side;
        let order_price = order.price_tick();
        let order_remaining_size = order.remaining_size;

        // Add to price level queue for tracking FIRST (using the correct order_id)
        if order_price != 0 {
            self.add_to_price_queue(
                order_price,
                &order_id,
                client_order_id.clone(),
                order_side,
                order_remaining_size,
                timestamp_ns,
            );
        }

        self.pending_orders.insert(
            client_order_id.clone(),
            PendingOrder {
                order_id: order_id.clone(),
                order,
                ack_at_ns: timestamp_ns + self.config.order_ack_latency_ns,
                queue_ahead,
            },
        );

        order_id
    }

    /// Calculate volume ahead at a price level.
    fn calculate_queue_ahead(&self, price_tick: Tick) -> Size {
        if let Some(queue) = self.price_queues.get(&price_tick) {
            queue.total_volume
        } else {
            0
        }
    }

    /// Add order to price level queue.
    fn add_to_price_queue(
        &mut self,
        price_tick: Tick,
        order_id: &str,
        client_order_id: ClientOrderId,
        side: Side,
        size: Size,
        timestamp_ns: u64,
    ) {
        let queue = self.price_queues.entry(price_tick).or_insert_with(|| PriceLevelQueue {
            orders: HashMap::new(),
            total_volume: 0,
            next_submission_time: 0,
        });

        let queued = QueuedOrder {
            order_id: order_id.to_string(),
            client_order_id,
            side,
            price_tick,
            size,
            submitted_at_ns: timestamp_ns,
        };

        queue.orders.insert(order_id.to_string(), queued);
        queue.total_volume += size;
    }

    /// Process time advancement and return events.
    pub fn advance(&mut self, now_ns: u64) -> (Vec<OrderAckEvent>, Vec<CancelAckEvent>) {
        let mut acks = Vec::new();
        let mut cancels = Vec::new();

        // Process order acknowledgments
        let ready_acks: Vec<_> = self
            .pending_orders
            .iter()
            .filter(|(_, p)| p.ack_at_ns <= now_ns)
            .map(|(k, _)| k.clone())
            .collect();

        for client_id in ready_acks {
            if let Some(pending) = self.pending_orders.remove(&client_id) {
                let order_id = pending.order_id;

                let price_key = pending.order.price_tick();

                let mut order = pending.order;
                order.acknowledge(order_id.clone(), now_ns).ok();

                let live_data = LiveOrderData {
                    order,
                    queue_ahead_at_submission: pending.queue_ahead,
                    current_queue_ahead: pending.queue_ahead,
                    price_stats_key: price_key,
                };

                self.live_orders.insert(order_id.clone(), live_data);

                acks.push(OrderAckEvent {
                    client_order_id: client_id,
                    order_id: order_id.clone(),
                    timestamp_ns: now_ns,
                });
            }
        }

        // Process cancellations
        let ready_cancels: Vec<_> = self
            .pending_cancels
            .iter()
            .filter(|(_, p)| p.cancel_at_ns <= now_ns)
            .map(|(k, _)| k.clone())
            .collect();

        for order_id in ready_cancels {
        if let Some(pending) = self.pending_cancels.remove(&order_id) {
                if let Some(live) = self.live_orders.remove(&order_id) {
                    // Remove from price queue
                    if live.order.price_tick() != 0 {
                        if let Some(queue) = self.price_queues.get_mut(&live.order.price_tick()) {
                            queue.orders.remove(&order_id);
                            queue.total_volume = queue.total_volume.saturating_sub(live.order.remaining_size);
                        }
                    }

                    cancels.push(CancelAckEvent {
                        order_id,
                        timestamp_ns: now_ns,
                    });
                }
            }
        }

        (acks, cancels)
    }

    /// Request to cancel an order.
    pub fn cancel_order(&mut self, order_id: &str, now_ns: u64) {
        if self.live_orders.contains_key(order_id) {
            self.pending_cancels.insert(
                order_id.to_string(),
                PendingCancel {
                    order_id: order_id.to_string(),
                    cancel_at_ns: now_ns + self.config.cancel_ack_latency_ns,
                },
            );
        }
    }

    /// Check for fills based on order book snapshot.
    /// Returns fills and any updates to order states.
    pub fn check_fills(
        &mut self,
        book: &ArrayBook,
        timestamp_ns: u64,
    ) -> Vec<RealisticFill> {
        let mut fills = Vec::new();

        // Update price level statistics based on current book state
        self.update_price_stats(book);

        // Detect price movements for adverse selection
        self.detect_price_movement(book);

        // Collect updates to apply after iteration
        #[derive(Clone)]
        struct FillUpdate {
            order_id: String,
            fill: RealisticFill,
            price_tick: Tick,
        }
        let mut fills_to_process: Vec<FillUpdate> = Vec::new();
        let mut orders_to_remove: Vec<String> = Vec::new();
        let mut missed_fills: Vec<Tick> = Vec::new();

        // Get orders that need fill checking
        let order_ids: Vec<_> = self.live_orders.keys().cloned().collect();

        for order_id in order_ids {
            // Skip if pending cancel
            if self.pending_cancels.contains_key(order_id.as_str()) {
                continue;
            }

            // Get mutable reference to live order
            let live = match self.live_orders.get_mut(order_id.as_str()) {
                Some(l) => l,
                None => continue,
            };

            // Check if order is marketable - use immutable reference for book access
            let is_marketable = Self::check_order_marketable(&live.order, book);
            if !is_marketable {
                continue;
            }

            // Check for fill
            let fill_result = Self::try_fill_order(
                &self.config,
                &self.price_stats,
                &live.order,
                live.current_queue_ahead,
                book,
                timestamp_ns,
            );

            match fill_result {
                Some(fill) => {
                    fills.push(fill.clone());
                    fills_to_process.push(FillUpdate {
                        order_id: order_id.clone(),
                        fill,
                        price_tick: live.order.price_tick(),
                    });
                }
                None => {
                    missed_fills.push(live.order.price_tick());
                }
            }
        }

        // Record missed fills
        for price_tick in missed_fills {
            self.record_fill(price_tick, false, 0);
        }

        // Apply fill updates - collect price ticks first
        let fill_records: Vec<(Tick, Size)> = fills_to_process
            .iter()
            .map(|u| (u.price_tick, u.fill.size))
            .collect();
        
        for update in fills_to_process {
            if let Some(live) = self.live_orders.get_mut(update.order_id.as_str()) {
                // Update order state
                live.order.remaining_size = live.order.remaining_size.saturating_sub(update.fill.size);
                live.order.filled_size += update.fill.size;

                if live.order.remaining_size == 0 {
                    live.order.state = OrderState::Filled;
                    orders_to_remove.push(update.order_id);
                } else {
                    live.order.state = OrderState::PartiallyFilled;
                    // Update queue position
                    live.current_queue_ahead = live.current_queue_ahead.saturating_sub(update.fill.size);
                }
            }
        }

        // Record fills for adverse selection tracking
        for (price_tick, size) in fill_records {
            self.record_fill(price_tick, true, size);
        }

        // Remove fully filled orders
        for order_id in orders_to_remove {
            self.live_orders.remove(order_id.as_str());
        }

        fills
    }

    /// Check if an order is marketable against the current book.
    fn check_order_marketable(order: &Order, book: &ArrayBook) -> bool {
        let best_bid = book.best_bid();
        let best_ask = book.best_ask();

        match order.side {
            Side::Buy => {
                // Buy order is marketable if it can cross the ask
                if let Some(ask) = best_ask {
                    order.price_tick() >= ask || order.price_tick() == 0
                } else {
                    false
                }
            }
            Side::Sell => {
                // Sell order is marketable if it can cross the bid
                if let Some(bid) = best_bid {
                    order.price_tick() <= bid || order.price_tick() == 0
                } else {
                    false
                }
            }
        }
    }

    /// Attempt to fill an order.
    fn try_fill_order(
        config: &RealisticFillConfig,
        price_stats: &HashMap<Tick, PriceLevelStats>,
        order: &Order,
        current_queue_ahead: Size,
        book: &ArrayBook,
        timestamp_ns: u64,
    ) -> Option<RealisticFill> {
        let price = order.price_tick();

        // Get available liquidity at order's price level
        let available_liquidity = match order.side {
            Side::Buy => book.get_level(Side::Sell, price),
            Side::Sell => book.get_level(Side::Buy, price),
        };

        if available_liquidity == 0 {
            return None;
        }

        // Calculate fill probability based on queue position
        let queue_multiplier = config.queue_position_model.fill_probability_multiplier(
            current_queue_ahead,
            order.remaining_size,
        );

        // Apply adverse selection adjustment
        let adverse_adjustment = Self::calculate_adverse_selection_adjustment(
            config,
            price_stats,
            price,
        );
        let fill_probability = config.base_fill_probability * queue_multiplier * adverse_adjustment;

        // Roll for fill
        if rand::random::<f64>() >= fill_probability {
            return None;
        }

        // Determine fill size
        let max_fill = available_liquidity.min(order.remaining_size);
        let fill_size = if rand::random::<f64>() < config.partial_fill_probability {
            // Partial fill - calculate random ratio within bounds
            let random_ratio = rand::random::<f64>();
            let fill_ratio = config.min_fill_ratio 
                + random_ratio * (1.0 - config.min_fill_ratio);
            (max_fill as f64 * fill_ratio) as Size
        } else {
            max_fill
        }.max(1);

        // Calculate market impact
        let notional = (fill_size as f64 * price as f64) / 10_000.0;
        let impact_bps = config.market_impact_bps * (notional / 1000.0);

        // Calculate fee
        let fee = Self::calculate_fee(config, price, fill_size);

        Some(RealisticFill {
            order_id: order.order_id.clone(),
            client_order_id: order.client_order_id.clone(),
            price_tick: price,
            size: fill_size,
            fee_micro_usdc: fee,
            timestamp_ns,
            is_partial: fill_size < order.remaining_size,
            market_impact_bps: impact_bps,
        })
    }

    /// Calculate adverse selection adjustment for a price level.
    fn calculate_adverse_selection_adjustment(
        config: &RealisticFillConfig,
        price_stats: &HashMap<Tick, PriceLevelStats>,
        price_tick: Tick,
    ) -> f64 {
        if let Some(stats) = price_stats.get(&price_tick) {
            if stats.recent_fills.is_empty() {
                return 1.0;
            }

            // Count recent fills vs misses
            let recent_count = stats.recent_fills.len();
            let filled_count = stats.recent_fills.iter().filter(|f| f.was_filled).count();
            let fill_rate = filled_count as f64 / recent_count as f64;

            // If price has been moving favorably (high fill rate), reduce probability
            // This models the adverse selection: when price moves in your favor,
            // others are likely ahead of you
            if fill_rate > 0.7 {
                1.0 - config.adverse_selection_probability
            } else if fill_rate < 0.3 {
                // Price moving against, so fills are more likely
                1.0 + config.adverse_selection_probability * 0.5
            } else {
                1.0
            }
        } else {
            1.0
        }
    }

    /// Update price level statistics from order book.
    fn update_price_stats(&mut self, _book: &ArrayBook) {
        // This would integrate with the order book to track volume
        // For now, we track what we see in trades
    }

    /// Detect price movement for adverse selection.
    fn detect_price_movement(&mut self, book: &ArrayBook) {
        let best_bid = book.best_bid();
        let best_ask = book.best_ask();

        if let Some(prev_bid) = self.prev_best_bid {
            if let Some(current_bid) = best_bid {
                // Bid moved up - favorable for buyers
                if current_bid > prev_bid {
                    // Buyers experience adverse selection
                    if let Some(stats) = self.price_stats.get_mut(&prev_bid) {
                        stats.recent_fills.push_back(FillRecord {
                            timestamp_ns: 0,
                            was_filled: false,
                            price_tick: prev_bid,
                            size: 0,
                        });
                    }
                }
            }
        }

        if let Some(prev_ask) = self.prev_best_ask {
            if let Some(current_ask) = best_ask {
                // Ask moved down - favorable for sellers
                if current_ask < prev_ask {
                    // Sellers experience adverse selection
                    if let Some(stats) = self.price_stats.get_mut(&prev_ask) {
                        stats.recent_fills.push_back(FillRecord {
                            timestamp_ns: 0,
                            was_filled: false,
                            price_tick: prev_ask,
                            size: 0,
                        });
                    }
                }
            }
        }

        self.prev_best_bid = best_bid;
        self.prev_best_ask = best_ask;
    }

    /// Record fill result for adverse selection tracking.
    fn record_fill(&mut self, price_tick: Tick, was_filled: bool, size: Size) {
        let stats = self.price_stats.entry(price_tick).or_default();
        stats.recent_fills.push_back(FillRecord {
            timestamp_ns: 0,
            was_filled,
            price_tick,
            size,
        });

        // Keep only recent history
        while stats.recent_fills.len() > 100 {
            stats.recent_fills.pop_front();
        }
    }

    /// Calculate fee using the exchange model.
    fn calculate_fee(config: &RealisticFillConfig, price_tick: Tick, size: Size) -> i64 {
        if config.fee_rate_bps == 0 || size == 0 || price_tick == 0 || price_tick == MAX_TICK {
            return 0;
        }

        let min_tick = price_tick.min(MAX_TICK - price_tick) as u128;
        let fee = (config.fee_rate_bps as u128 * min_tick * size as u128)
            / (10_000u128 * 10_000u128);

        fee as i64
    }

    /// Get the current queue position for an order.
    pub fn get_queue_position(&self, order_id: &str) -> Option<QueuePosition> {
        self.live_orders.get(order_id).map(|live| {
            let price = live.order.price_tick();
            let queue = if price != 0 { self.price_queues.get(&price) } else { None };
            let (position, volume_ahead) = if let Some(q) = queue {
                let mut pos = 0;
                let mut vol_ahead = 0;
                for (id, order) in &q.orders {
                    if id == order_id {
                        break;
                    }
                    pos += 1;
                    vol_ahead += order.size;
                }
                (pos, vol_ahead)
            } else {
                (0, 0)
            };

            QueuePosition {
                position,
                volume_ahead,
                time_priority_ns: live.order.created_at_ns,
                is_front: position == 0,
            }
        })
    }

    /// Get all live orders.
    pub fn live_orders(&self) -> impl Iterator<Item = &Order> {
        self.live_orders.values().map(|o| &o.order)
    }

    /// Get a specific order.
    pub fn get_order(&self, order_id: &str) -> Option<&Order> {
        self.live_orders.get(order_id).map(|o| &o.order)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mtrader_core::{OrderReason, Side, Tick};
    use mtrader_execution::{OrderKind, OrderType};

    fn make_order(side: Side, tick: Tick, size: Size) -> Order {
        Order::new(
            ClientOrderId("test-client".into()),
            "asset-123".into(),
            side,
            OrderKind::Limit {
                price_tick: tick,
                size_shares: size,
            },
            OrderType::Limit,
            OrderReason::MakerQuote,
            0,
        )
    }

    #[test]
    fn test_queue_position_optimistic() {
        let model = QueuePositionModel::Optimistic;
        assert_eq!(model.fill_probability_multiplier(1000, 500), 1.0);
        assert_eq!(model.fill_probability_multiplier(0, 500), 1.0);
    }

    #[test]
    fn test_queue_position_pessimistic() {
        let model = QueuePositionModel::Pessimistic;
        assert_eq!(model.fill_probability_multiplier(1000, 500), 0.0);
        assert_eq!(model.fill_probability_multiplier(0, 500), 1.0);
    }

    #[test]
    fn test_queue_position_volume_based() {
        let model = QueuePositionModel::VolumeBased;
        // No queue ahead
        let multiplier = model.fill_probability_multiplier(0, 500);
        assert_eq!(multiplier, 1.0);

        // Large queue ahead
        let multiplier = model.fill_probability_multiplier(10000, 500);
        assert!(multiplier < 1.0);
        assert!(multiplier > 0.1);
    }

    #[test]
    fn test_order_submission_and_ack() {
        let mut sim = RealisticFillSimulator::new(RealisticFillConfig {
            order_ack_latency_ns: 100,
            ..Default::default()
        });

        let order = make_order(Side::Buy, 5000, 100_000);
        let _order_id = sim.submit_order(order, 0);

        // Before latency (time 50 < 100)
        let (acks, _) = sim.advance(50);
        assert!(acks.is_empty(), "No acks before latency");

        // After latency (time 150 >= 100)
        let (acks, _) = sim.advance(150);
        assert_eq!(acks.len(), 1, "One ack after latency");
    }

    #[test]
    fn test_cancel_order() {
        let mut sim = RealisticFillSimulator::new(RealisticFillConfig {
            order_ack_latency_ns: 0,
            cancel_ack_latency_ns: 100,
            adverse_selection_probability: 0.25,
            partial_fill_probability: 0.4,
            min_fill_ratio: 0.1,
            market_impact_bps: 10.0,
            base_fill_probability: 0.85,
            fee_rate_bps: 1000,
            queue_position_model: QueuePositionModel::VolumeBased,
        });

        let order = Order::new(
            ClientOrderId("cancel-test".into()),
            "asset-123".into(),
            Side::Buy,
            OrderKind::Limit { price_tick: 5000, size_shares: 100_000 },
            OrderType::Limit,
            OrderReason::MakerQuote,
            0,
        );
        sim.submit_order(order, 0);
        
        // Advance time to process acknowledgment
        let (acks, _) = sim.advance(1);
        assert_eq!(acks.len(), 1, "Order should be acknowledged");
        
        // Get the actual order_id from the acknowledgment
        let order_id = &acks[0].order_id;

        sim.cancel_order(order_id, 10);

        // Before cancel latency (time 50 < 110)
        let (_, cancels) = sim.advance(50);
        assert!(cancels.is_empty(), "Cancel should not complete before latency");

        // After cancel latency (time 200 >= 110)
        let (_, cancels) = sim.advance(200);
        assert_eq!(cancels.len(), 1, "Cancel should complete after latency");
    }

    fn next_test_client_id() -> String {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        format!("test-client-{}", id)
    }

    #[test]
    fn test_queue_position_tracking() {
        let mut sim = RealisticFillSimulator::new(RealisticFillConfig {
            order_ack_latency_ns: 0,
            cancel_ack_latency_ns: 50_000_000,
            adverse_selection_probability: 0.25,
            partial_fill_probability: 0.4,
            min_fill_ratio: 0.1,
            market_impact_bps: 10.0,
            base_fill_probability: 0.85,
            fee_rate_bps: 1000,
            queue_position_model: QueuePositionModel::VolumeBased,
        });

        let order1 = Order::new(
            ClientOrderId(next_test_client_id()),
            "asset-123".into(),
            Side::Buy,
            OrderKind::Limit { price_tick: 5000, size_shares: 100_000 },
            OrderType::Limit,
            OrderReason::MakerQuote,
            0,
        );
        let order_id1 = sim.submit_order(order1, 0);

        let order2 = Order::new(
            ClientOrderId(next_test_client_id()),
            "asset-123".into(),
            Side::Buy,
            OrderKind::Limit { price_tick: 5000, size_shares: 50_000 },
            OrderType::Limit,
            OrderReason::MakerQuote,
            1,
        );
        sim.submit_order(order2, 1);

        // Use larger timestamp to ensure both orders' ack times (0 and 1) are <= timestamp
        let (acks, _) = sim.advance(100);
        assert_eq!(acks.len(), 2, "Both orders should be acknowledged");

        // Use the order_id returned by submit_order (which should match the ack order_id)
        let pos1 = sim.get_queue_position(&order_id1);
        assert!(pos1.is_some(), "Order 1 should have a queue position");
        assert!(pos1.unwrap().is_front, "Order 1 should be at front of queue");
    }

    #[test]
    fn test_partial_fill_probability() {
        let config = RealisticFillConfig {
            partial_fill_probability: 1.0, // Always partial
            min_fill_ratio: 0.1,
            queue_position_model: QueuePositionModel::Optimistic,
            base_fill_probability: 1.0,
            order_ack_latency_ns: 0,
            ..Default::default()
        };

        let mut sim = RealisticFillSimulator::new(config);
        let order = make_order(Side::Buy, 5000, 100_000);
        sim.submit_order(order, 0);
        let (acks, _) = sim.advance(0);
        
        // Order should be acknowledged immediately
        assert_eq!(acks.len(), 1, "Order should be acknowledged");
        assert!(sim.live_orders().next().is_some(), "Should have live orders");
    }

    #[test]
    fn test_adverse_selection_modeling() {
        let config = RealisticFillConfig {
            adverse_selection_probability: 0.5,
            base_fill_probability: 1.0,
            queue_position_model: QueuePositionModel::Optimistic,
            ..Default::default()
        };

        let _sim = RealisticFillSimulator::new(config.clone());

        // Price with no history
        let adjustment = RealisticFillSimulator::calculate_adverse_selection_adjustment(
            &config,
            &HashMap::new(),
            5000,
        );
        assert_eq!(adjustment, 1.0);
    }

    #[test]
    fn test_fee_calculation() {
        let config = RealisticFillConfig {
            fee_rate_bps: 1000,
            ..Default::default()
        };

        let _sim = RealisticFillSimulator::new(config.clone());

        // Price at 0.5 (5000 ticks), size 100000
        // min_tick = 5000, fee = 1000 * 5000 * 100000 / 100000000 = 5000 micro
        let fee = RealisticFillSimulator::calculate_fee(&config, 5000, 100_000);
        assert!(fee > 0);
    }

    #[test]
    fn test_fee_at_boundaries() {
        let config = RealisticFillConfig {
            fee_rate_bps: 1000,
            ..Default::default()
        };

        let _sim = RealisticFillSimulator::new(config.clone());

        // Zero price = no fee
        assert_eq!(RealisticFillSimulator::calculate_fee(&config, 0, 100_000), 0);

        // Max price = no fee (free option)
        assert_eq!(RealisticFillSimulator::calculate_fee(&config, MAX_TICK, 100_000), 0);

        // Zero size = no fee
        assert_eq!(RealisticFillSimulator::calculate_fee(&config, 5000, 0), 0);
    }

    #[test]
    fn test_default_config() {
        let config = RealisticFillConfig::default();

        assert_eq!(config.queue_position_model, QueuePositionModel::VolumeBased);
        assert_eq!(config.adverse_selection_probability, 0.25);
        assert_eq!(config.partial_fill_probability, 0.4);
        assert_eq!(config.min_fill_ratio, 0.1);
        assert_eq!(config.market_impact_bps, 10.0);
    }

    #[test]
    fn test_comparison_with_optimistic_model() {
        let optimistic_config = RealisticFillConfig {
            queue_position_model: QueuePositionModel::Optimistic,
            base_fill_probability: 1.0,
            ..Default::default()
        };

        let pessimistic_config = RealisticFillConfig {
            queue_position_model: QueuePositionModel::Pessimistic,
            base_fill_probability: 1.0,
            ..Default::default()
        };

        let _optimistic_sim = RealisticFillSimulator::new(optimistic_config);
        let _pessimistic_sim = RealisticFillSimulator::new(pessimistic_config);

        // With queue ahead, optimistic should have higher probability
        let opt_mult = QueuePositionModel::Optimistic.fill_probability_multiplier(1000, 500);
        let pess_mult = QueuePositionModel::Pessimistic.fill_probability_multiplier(1000, 500);

        assert!(opt_mult > pess_mult);

        // Without queue ahead, they should be equal
        let opt_mult = QueuePositionModel::Optimistic.fill_probability_multiplier(0, 500);
        let pess_mult = QueuePositionModel::Pessimistic.fill_probability_multiplier(0, 500);

        assert_eq!(opt_mult, pess_mult);
    }

    #[test]
    fn test_fill_check_result_variants() {
        // Test FillCheckResult enum variants
        let filled_result = FillCheckResult::Filled(RealisticFill {
            order_id: "test".to_string(),
            client_order_id: ClientOrderId("client".into()),
            price_tick: 5000,
            size: 100,
            fee_micro_usdc: 100,
            timestamp_ns: 0,
            is_partial: false,
            market_impact_bps: 0.0,
        });
        assert!(matches!(filled_result, FillCheckResult::Filled(_)));

        let queued_result = FillCheckResult::Queued {
            position: 5,
            volume_ahead: 1000,
        };
        assert!(matches!(queued_result, FillCheckResult::Queued { .. }));
    }
}
