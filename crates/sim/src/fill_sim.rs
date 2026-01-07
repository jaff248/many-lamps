//! Print-driven fill simulator.
//!
//! Simulates fills based on market prints (trades).
//! Key principle: we only get filled when the market actually trades
//! at or through our price level.

use mtrader_core::{Side, Size, Tick};
use mtrader_execution::{Order, OrderState};
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Configuration for fill simulation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FillSimConfig {
    /// Latency for order acknowledgment (nanoseconds)
    pub order_ack_latency_ns: u64,
    /// Latency for cancel acknowledgment (nanoseconds)
    pub cancel_ack_latency_ns: u64,
    /// Probability of getting filled when price touches our level (0.0 to 1.0)
    /// Lower = more conservative, models queue position
    pub fill_probability_at_touch: f64,
    /// Whether to always fill when price trades through our level
    pub fill_on_through: bool,
    /// Maximum partial fill ratio per trade (0.0 to 1.0)
    pub max_partial_fill_ratio: f64,
    /// Simulated slippage in ticks (random up to this amount)
    pub max_slippage_ticks: u16,
    /// Fee rate in basis points (matches live environment)
    pub fee_rate_bps: u32,
}

impl Default for FillSimConfig {
    fn default() -> Self {
        Self {
            // 50ms order latency
            order_ack_latency_ns: 50_000_000,
            // 50ms cancel latency
            cancel_ack_latency_ns: 50_000_000,
            // 30% chance to fill at touch (models queue)
            fill_probability_at_touch: 0.3,
            // Always fill if price trades through
            fill_on_through: true,
            // Up to 50% of order filled per trade
            max_partial_fill_ratio: 0.5,
            // No slippage for limit orders
            max_slippage_ticks: 0,
            // 15-min market fee rate
            fee_rate_bps: 1000,
        }
    }
}

/// A simulated fill.
#[derive(Debug, Clone)]
pub struct SimulatedFill {
    /// Order that was filled
    pub order_id: String,
    /// Fill price (tick)
    pub price_tick: Tick,
    /// Fill size (centishares)
    pub size: Size,
    /// Fee for this fill (micro-USDC)
    pub fee_micro_usdc: i64,
    /// Timestamp of fill (mono ns)
    pub timestamp_ns: u64,
}

/// Pending order in the simulator.
#[derive(Debug, Clone)]
struct PendingOrder {
    order: Order,
    /// When the order will be acknowledged
    ack_at_ns: u64,
    /// When a cancel request will be processed (if any)
    cancel_at_ns: Option<u64>,
}

/// Fill simulator for paper trading.
pub struct FillSimulator {
    config: FillSimConfig,
    /// Orders pending acknowledgment
    pending_orders: HashMap<String, PendingOrder>,
    /// Live orders that can be filled
    live_orders: HashMap<String, Order>,
    /// Orders pending cancel
    pending_cancels: HashMap<String, u64>,
    /// RNG for probabilistic fills
    rng: rand::rngs::ThreadRng,
    /// Next simulated order ID
    next_order_id: u64,
}

impl FillSimulator {
    pub fn new(config: FillSimConfig) -> Self {
        Self {
            config,
            pending_orders: HashMap::new(),
            live_orders: HashMap::new(),
            pending_cancels: HashMap::new(),
            rng: rand::thread_rng(),
            next_order_id: 1,
        }
    }

    /// Submit a new order.
    pub fn submit_order(&mut self, mut order: Order, now_ns: u64) -> String {
        let order_id = format!("sim-{}", self.next_order_id);
        self.next_order_id += 1;

        order.order_id = order_id.clone();

        self.pending_orders.insert(
            order.client_order_id.clone(),
            PendingOrder {
                order,
                ack_at_ns: now_ns + self.config.order_ack_latency_ns,
                cancel_at_ns: None,
            },
        );

        order_id
    }

    /// Request to cancel an order.
    pub fn cancel_order(&mut self, order_id: &str, now_ns: u64) {
        // Check if order is live
        if self.live_orders.contains_key(order_id) {
            self.pending_cancels
                .insert(order_id.to_string(), now_ns + self.config.cancel_ack_latency_ns);
        }
    }

    /// Process time advancement and return events.
    pub fn advance(&mut self, now_ns: u64) -> Vec<SimEvent> {
        let mut events = Vec::new();

        // Process order acknowledgments
        let ready_acks: Vec<_> = self
            .pending_orders
            .iter()
            .filter(|(_, p)| p.ack_at_ns <= now_ns)
            .map(|(k, _)| k.clone())
            .collect();

        for client_id in ready_acks {
            if let Some(pending) = self.pending_orders.remove(&client_id) {
                let order_id = pending.order.order_id.clone();
                events.push(SimEvent::OrderAcked {
                    client_order_id: client_id,
                    order_id: order_id.clone(),
                    timestamp_ns: pending.ack_at_ns,
                });
                self.live_orders.insert(order_id, pending.order);
            }
        }

        // Process cancellations
        let ready_cancels: Vec<_> = self
            .pending_cancels
            .iter()
            .filter(|(_, &cancel_at)| cancel_at <= now_ns)
            .map(|(k, _)| k.clone())
            .collect();

        for order_id in ready_cancels {
            if let Some(cancel_at) = self.pending_cancels.remove(&order_id) {
                if let Some(order) = self.live_orders.remove(&order_id) {
                    events.push(SimEvent::OrderCancelled {
                        order_id: order_id.clone(),
                        remaining_size: order.remaining_size,
                        timestamp_ns: cancel_at,
                    });
                }
            }
        }

        events
    }

    /// Process a market trade (print) and check for fills.
    pub fn on_trade(
        &mut self,
        trade_price_tick: Tick,
        trade_size: Size,
        trade_side: Side,
        timestamp_ns: u64,
    ) -> Vec<SimulatedFill> {
        let mut fills = Vec::new();

        // Check each live order for potential fill
        let order_ids: Vec<_> = self.live_orders.keys().cloned().collect();

        for order_id in order_ids {
            if let Some(order) = self.live_orders.get_mut(&order_id) {
                // Skip if pending cancel
                if self.pending_cancels.contains_key(&order_id) {
                    continue;
                }

                if let Some(fill) =
                    self.try_fill(order, trade_price_tick, trade_size, trade_side, timestamp_ns)
                {
                    fills.push(fill);

                    // Remove if fully filled
                    if order.remaining_size == 0 {
                        self.live_orders.remove(&order_id);
                    }
                }
            }
        }

        fills
    }

    fn try_fill(
        &mut self,
        order: &mut Order,
        trade_price: Tick,
        trade_size: Size,
        trade_side: Side,
        timestamp_ns: u64,
    ) -> Option<SimulatedFill> {
        // Check if trade could fill our order
        let could_fill = match order.side {
            Side::Buy => {
                // Buy orders fill when market sells at or below our price
                trade_side == Side::Sell && trade_price <= order.price_tick
            }
            Side::Sell => {
                // Sell orders fill when market buys at or above our price
                trade_side == Side::Buy && trade_price >= order.price_tick
            }
        };

        if !could_fill {
            return None;
        }

        // Determine if we get filled based on price level
        let price_through = match order.side {
            Side::Buy => trade_price < order.price_tick,
            Side::Sell => trade_price > order.price_tick,
        };

        let should_fill = if price_through && self.config.fill_on_through {
            true
        } else {
            // At touch - probabilistic based on queue position
            self.rng.gen::<f64>() < self.config.fill_probability_at_touch
        };

        if !should_fill {
            return None;
        }

        // Calculate fill size
        let max_fill_from_trade =
            (trade_size as f64 * self.config.max_partial_fill_ratio) as Size;
        let fill_size = order.remaining_size.min(max_fill_from_trade).max(1);

        // Update order
        order.remaining_size = order.remaining_size.saturating_sub(fill_size);
        order.filled_size += fill_size;
        if order.remaining_size == 0 {
            order.state = OrderState::Filled;
        } else {
            order.state = OrderState::PartiallyFilled;
        }

        // Calculate fee (parabolic model)
        let fee = self.calculate_fee(order.price_tick, fill_size);

        Some(SimulatedFill {
            order_id: order.order_id.clone(),
            price_tick: order.price_tick,
            size: fill_size,
            fee_micro_usdc: fee,
            timestamp_ns,
        })
    }

    /// Calculate fee using parabolic model.
    fn calculate_fee(&self, price_tick: Tick, size: Size) -> i64 {
        let price_pct = price_tick as u64;
        let complement_pct = 100 - price_pct;

        (self.config.fee_rate_bps as u64 * price_pct * complement_pct * size / 100_000_000) as i64
    }

    /// Get all live orders.
    pub fn live_orders(&self) -> impl Iterator<Item = &Order> {
        self.live_orders.values()
    }

    /// Get a specific order.
    pub fn get_order(&self, order_id: &str) -> Option<&Order> {
        self.live_orders.get(order_id)
    }
}

/// Events from the simulator.
#[derive(Debug, Clone)]
pub enum SimEvent {
    OrderAcked {
        client_order_id: String,
        order_id: String,
        timestamp_ns: u64,
    },
    OrderCancelled {
        order_id: String,
        remaining_size: Size,
        timestamp_ns: u64,
    },
    OrderRejected {
        client_order_id: String,
        reason: String,
        timestamp_ns: u64,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use mtrader_core::OrderReason;
    use mtrader_execution::OrderType;

    fn make_order(side: Side, tick: Tick, size: Size) -> Order {
        Order::new(
            "client-1".into(),
            "asset-123".into(),
            side,
            tick,
            size,
            OrderType::Limit,
            OrderReason::MakerQuote,
            0,
        )
    }

    #[test]
    fn test_order_submission_and_ack() {
        let mut sim = FillSimulator::new(FillSimConfig {
            order_ack_latency_ns: 100,
            ..Default::default()
        });

        let order = make_order(Side::Buy, 50, 100_000);
        let _order_id = sim.submit_order(order, 0);

        // Before latency
        let events = sim.advance(50);
        assert!(events.is_empty());

        // After latency
        let events = sim.advance(150);
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], SimEvent::OrderAcked { .. }));
    }

    #[test]
    fn test_fill_on_touch() {
        let mut sim = FillSimulator::new(FillSimConfig {
            order_ack_latency_ns: 0,
            fill_probability_at_touch: 1.0, // Always fill at touch for test
            ..Default::default()
        });

        let order = make_order(Side::Buy, 50, 100_000);
        sim.submit_order(order, 0);
        sim.advance(0);

        // Trade at our price (touch)
        let fills = sim.on_trade(50, 200_000, Side::Sell, 100);
        assert!(!fills.is_empty());
    }

    #[test]
    fn test_fill_on_through() {
        let mut sim = FillSimulator::new(FillSimConfig {
            order_ack_latency_ns: 0,
            fill_probability_at_touch: 0.0, // Never fill at touch
            fill_on_through: true,
            ..Default::default()
        });

        let order = make_order(Side::Buy, 50, 100_000);
        sim.submit_order(order, 0);
        sim.advance(0);

        // Trade through our price
        let fills = sim.on_trade(48, 200_000, Side::Sell, 100);
        assert!(!fills.is_empty());
    }

    #[test]
    fn test_no_fill_wrong_side() {
        let mut sim = FillSimulator::new(FillSimConfig {
            order_ack_latency_ns: 0,
            fill_probability_at_touch: 1.0,
            ..Default::default()
        });

        let order = make_order(Side::Buy, 50, 100_000);
        sim.submit_order(order, 0);
        sim.advance(0);

        // Trade is a buy (same side) - shouldn't fill us
        let fills = sim.on_trade(50, 200_000, Side::Buy, 100);
        assert!(fills.is_empty());
    }

    #[test]
    fn test_cancel_order() {
        let mut sim = FillSimulator::new(FillSimConfig {
            order_ack_latency_ns: 0,
            cancel_ack_latency_ns: 100,
            ..Default::default()
        });

        let order = make_order(Side::Buy, 50, 100_000);
        let _order_id = sim.submit_order(order.clone(), 0);
        let events = sim.advance(0);
        let order_id = match &events[0] {
            SimEvent::OrderAcked { order_id, .. } => order_id.clone(),
            _ => panic!("Expected ack"),
        };

        sim.cancel_order(&order_id, 50);

        // Before cancel latency
        let events = sim.advance(100);
        assert!(events.is_empty());

        // After cancel latency
        let events = sim.advance(200);
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], SimEvent::OrderCancelled { .. }));
    }
}
