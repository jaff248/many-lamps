//! Paper Trading System
//!
//! Provides realistic paper trading that mirrors the real Polymarket market.
//! Key principle: We use real market data, just with virtual money.

use mtrader_book::ArrayBook;
use mtrader_core::{ClientOrderId, Side, Size, Tick};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;

// ============================================================================
// Paper Order
// ============================================================================

/// Our order in the paper book.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaperOrder {
    pub order_id: String,
    pub client_order_id: ClientOrderId,
    pub side: Side,
    pub price_tick: Tick,
    pub size: Size,
    pub timestamp_ns: u64,
    /// Our position in the queue at this price level
    pub queue_position: Size,
}

/// Performance tracking for a single trade
#[derive(Debug, Clone)]
pub struct TradeRecord {
    pub timestamp_ns: u64,
    pub side: Side,
    pub price_tick: Tick,
    pub size: Size,
    pub pnl_micro: i64,
    pub fees_micro: i64,
    pub slippage_bps: f64,
    pub duration_ns: u64,
    pub order_id: String,
}

/// Performance report for a trading session
#[derive(Debug, Clone, Default)]
pub struct PerformanceReport {
    pub total_pnl_micro: i64,
    pub realized_pnl_micro: i64,
    pub unrealized_pnl_micro: i64,
    pub total_fees_micro: i64,
    pub total_rebates_micro: i64,
    pub net_cost_micro: i64,
    pub total_trades: usize,
    pub winning_trades: usize,
    pub losing_trades: usize,
    pub avg_slippage_bps: f64,
    pub max_slippage_bps: f64,
    pub min_slippage_bps: f64,
    pub avg_trade_duration: Duration,
    pub max_trade_duration: Duration,
    pub min_trade_duration: Duration,
    pub win_rate_pct: f64,
    pub profit_factor: f64,
    pub sharpe_ratio: f64,
    pub peak_equity_micro: i64,
    pub drawdown_micro: i64,
    pub max_drawdown_micro: i64,
    trades: Vec<TradeRecord>,
}

impl PerformanceReport {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_trade(&mut self, trade: TradeRecord) {
        self.trades.push(trade.clone());
        self.total_trades += 1;
        self.total_fees_micro += trade.fees_micro;
        self.total_pnl_micro += trade.pnl_micro;
        
        if trade.pnl_micro > 0 {
            self.winning_trades += 1;
        } else if trade.pnl_micro < 0 {
            self.losing_trades += 1;
        }

        // Update slippage stats
        if self.trades.len() == 1 {
            self.avg_slippage_bps = trade.slippage_bps.abs();
            self.max_slippage_bps = trade.slippage_bps.abs();
            self.min_slippage_bps = trade.slippage_bps.abs();
        } else {
            let sum: f64 = self.trades.iter().map(|t| t.slippage_bps.abs()).sum();
            self.avg_slippage_bps = sum / self.trades.len() as f64;
            self.max_slippage_bps = self.max_slippage_bps.max(trade.slippage_bps.abs());
            self.min_slippage_bps = self.min_slippage_bps.min(trade.slippage_bps.abs());
        }

        // Update duration stats
        if self.trades.len() == 1 {
            self.avg_trade_duration = Duration::from_nanos(trade.duration_ns);
            self.max_trade_duration = Duration::from_nanos(trade.duration_ns);
            self.min_trade_duration = Duration::from_nanos(trade.duration_ns);
        } else {
            let sum: u64 = self.trades.iter().map(|t| t.duration_ns).sum();
            self.avg_trade_duration = Duration::from_nanos(sum / self.trades.len() as u64);
            self.max_trade_duration = self.max_trade_duration.max(Duration::from_nanos(trade.duration_ns));
            self.min_trade_duration = self.min_trade_duration.min(Duration::from_nanos(trade.duration_ns));
        }

        self.realized_pnl_micro += trade.pnl_micro;
        self.win_rate_pct = if self.total_trades > 0 {
            (self.winning_trades as f64 / self.total_trades as f64) * 100.0
        } else {
            0.0
        };

        // Update equity tracking
        if self.total_pnl_micro > self.peak_equity_micro {
            self.peak_equity_micro = self.total_pnl_micro;
        }
        self.drawdown_micro = self.peak_equity_micro - self.total_pnl_micro;
        self.max_drawdown_micro = self.max_drawdown_micro.max(self.drawdown_micro);
        
        self.net_cost_micro = self.total_fees_micro - self.total_rebates_micro;
    }

    pub fn update_unrealized_pnl(&mut self, unrealized: i64) {
        self.unrealized_pnl_micro = unrealized;
    }

    pub fn calculate_sharpe(&mut self, risk_free_rate_bps: f64) {
        if self.trades.len() < 2 {
            self.sharpe_ratio = 0.0;
            return;
        }

        let returns: Vec<f64> = self.trades.iter()
            .map(|t| t.pnl_micro as f64 / 1_000_000.0)
            .collect();

        let mean: f64 = returns.iter().sum::<f64>() / returns.len() as f64;
        let variance: f64 = returns.iter()
            .map(|r| (r - mean).powi(2))
            .sum::<f64>() / returns.len() as f64;
        let std_dev = variance.sqrt();

        if std_dev == 0.0 {
            self.sharpe_ratio = 0.0;
            return;
        }

        let trades_per_day = 60.0 * 24.0;
        let annualized_return = mean * trades_per_day;
        let annualized_std = std_dev * trades_per_day.sqrt();

        let excess_return = annualized_return - (risk_free_rate_bps / 100.0);
        self.sharpe_ratio = excess_return / annualized_std;
    }

    pub fn calculate_profit_factor(&mut self) {
        let gross_profit: i64 = self.trades.iter()
            .filter(|t| t.pnl_micro > 0)
            .map(|t| t.pnl_micro)
            .sum();
            
        let gross_loss: i64 = self.trades.iter()
            .filter(|t| t.pnl_micro < 0)
            .map(|t| -t.pnl_micro)
            .sum();

        self.profit_factor = if gross_loss > 0 {
            gross_profit as f64 / gross_loss as f64
        } else if gross_profit > 0 {
            f64::INFINITY
        } else {
            0.0
        };
    }

    pub fn summary(&self) -> String {
        format!(
            "Performance Summary:
  PnL: {} (realized: {}, unrealized: {})
  Net Cost (fees - rebates): {}
  Trades: {} (wins: {}, losses: {})
  Win Rate: {:.1}%
  Profit Factor: {:.2}
  Avg Slippage: {:.2} bps
  Avg Trade Duration: {:?}",
            self.format_pnl(self.total_pnl_micro),
            self.format_pnl(self.realized_pnl_micro),
            self.format_pnl(self.unrealized_pnl_micro),
            self.format_pnl(self.net_cost_micro),
            self.total_trades,
            self.winning_trades,
            self.losing_trades,
            self.win_rate_pct,
            self.profit_factor,
            self.avg_slippage_bps,
            self.avg_trade_duration.as_secs_f64(),
        )
    }

    fn format_pnl(&self, pnl_micro: i64) -> String {
        let usdc = pnl_micro as f64 / 1_000_000.0;
        if usdc >= 0.0 {
            format!("+${:.2}", usdc)
        } else {
            format!("-${:.2}", usdc.abs())
        }
    }

    pub fn trades(&self) -> &[TradeRecord] {
        &self.trades
    }
}

/// Paper order book that mirrors real market and tracks our virtual orders.
#[derive(Debug, Clone)]
pub struct PaperBook {
    /// The real market book (from Polymarket WebSocket)
    market_book: ArrayBook,
    /// Our buy orders by tick
    our_bids: HashMap<Tick, Vec<PaperOrder>>,
    /// Our sell orders by tick
    our_asks: HashMap<Tick, Vec<PaperOrder>>,
    /// Virtual USD balance in micro-USDC
    balance_micro: i64,
    /// Initial balance for tracking PnL
    initial_balance_micro: i64,
    /// Performance report
    pub performance: PerformanceReport,
    /// Fee rate in bps (0 for Polymarket-like)
    fee_rate_bps: u16,
    /// Position size (positive = YES, negative = NO)
    position_size: i64,
    /// Average entry price
    avg_entry_tick: Option<Tick>,
}

impl PaperBook {
    /// Create a new paper book with initial balance
    pub fn new(initial_balance: f64, tick_size_bps: u16) -> Self {
        let balance_micro = (initial_balance * 1_000_000.0) as i64;
        Self {
            market_book: ArrayBook::new(tick_size_bps),
            our_bids: HashMap::new(),
            our_asks: HashMap::new(),
            balance_micro,
            initial_balance_micro: balance_micro,
            performance: PerformanceReport::new(),
            fee_rate_bps: 0, // Polymarket-like: 0% fees
            position_size: 0,
            avg_entry_tick: None,
        }
    }

    /// Create with custom fee rate
    pub fn with_fees(initial_balance: f64, tick_size_bps: u16, fee_rate_bps: u16) -> Self {
        let balance_micro = (initial_balance * 1_000_000.0) as i64;
        Self {
            market_book: ArrayBook::new(tick_size_bps),
            our_bids: HashMap::new(),
            our_asks: HashMap::new(),
            balance_micro,
            initial_balance_micro: balance_micro,
            performance: PerformanceReport::new(),
            fee_rate_bps,
            position_size: 0,
            avg_entry_tick: None,
        }
    }

    // =========================================================================
    // Balance & PnL Methods
    // =========================================================================

    /// Get current balance
    pub fn get_balance(&self) -> f64 {
        self.balance_micro as f64 / 1_000_000.0
    }

    /// Get balance in micro-USDC
    pub fn get_balance_micro(&self) -> i64 {
        self.balance_micro
    }

    /// Get total PnL
    pub fn get_pnl(&self) -> f64 {
        (self.balance_micro - self.initial_balance_micro) as f64 / 1_000_000.0
    }

    /// Get PnL in micro-USDC
    pub fn get_pnl_micro(&self) -> i64 {
        self.balance_micro - self.initial_balance_micro
    }

    /// Get performance report
    pub fn get_performance_report(&self) -> &PerformanceReport {
        &self.performance
    }

    /// Get mutable performance report
    pub fn get_performance_report_mut(&mut self) -> &mut PerformanceReport {
        &mut self.performance
    }

    // =========================================================================
    // Market Book Access (mirrors real Polymarket data)
    // =========================================================================

    /// Get reference to the market book
    pub fn market_book(&self) -> &ArrayBook {
        &self.market_book
    }

    /// Get mutable reference to the market book
    pub fn market_book_mut(&mut self) -> &mut ArrayBook {
        &mut self.market_book
    }

    // =========================================================================
    // Order Management
    // =========================================================================

    /// Add our order to the paper book
    pub fn add_order(&mut self, order: PaperOrder) {
        let orders = match order.side {
            Side::Buy => self.our_bids.entry(order.price_tick).or_default(),
            Side::Sell => self.our_asks.entry(order.price_tick).or_default(),
        };
        orders.push(order);
    }

    /// Remove our order from the paper book
    pub fn remove_order(&mut self, order_id: &str) -> Option<PaperOrder> {
        // Try bids first
        for orders in self.our_bids.values_mut() {
            if let Some(idx) = orders.iter().position(|o| o.order_id == order_id) {
                return Some(orders.remove(idx));
            }
        }

        // Try asks
        for orders in self.our_asks.values_mut() {
            if let Some(idx) = orders.iter().position(|o| o.order_id == order_id) {
                return Some(orders.remove(idx));
            }
        }

        None
    }

    /// Remove our order by client order ID
    pub fn remove_order_by_client_id(
        &mut self,
        client_order_id: &ClientOrderId,
    ) -> Option<PaperOrder> {
        for orders in self.our_bids.values_mut().chain(self.our_asks.values_mut()) {
            if let Some(idx) = orders
                .iter()
                .position(|order| order.client_order_id == *client_order_id)
            {
                return Some(orders.remove(idx));
            }
        }

        None
    }

    /// Update order size (after partial fill)
    pub fn update_order_size(&mut self, order_id: &str, new_size: Size) {
        for orders in self.our_bids.values_mut().chain(self.our_asks.values_mut()) {
            if let Some(order) = orders.iter_mut().find(|o| o.order_id == order_id) {
                order.size = new_size;
                return;
            }
        }
    }

    /// Get our orders at a specific tick
    pub fn our_orders_at(&self, side: Side, tick: Tick) -> &[PaperOrder] {
        let map = match side {
            Side::Buy => &self.our_bids,
            Side::Sell => &self.our_asks,
        };
        map.get(&tick).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Get all our bid ticks
    pub fn our_bid_ticks(&self) -> Vec<Tick> {
        self.our_bids.keys().copied().collect()
    }

    /// Get all our ask ticks
    pub fn our_ask_ticks(&self) -> Vec<Tick> {
        self.our_asks.keys().copied().collect()
    }

    /// Get total size of our orders on a side
    pub fn our_total_size(&self, side: Side) -> Size {
        let map = match side {
            Side::Buy => &self.our_bids,
            Side::Sell => &self.our_asks,
        };
        map.values().flat_map(|v| v.iter()).map(|o| o.size).sum()
    }

    /// Estimate queue ahead size at a price level
    pub fn estimate_queue_ahead(&self, side: Side, tick: Tick) -> Size {
        let market_size = self.market_book.get_level(side, tick);
        let our_size: Size = self.our_orders_at(side, tick).iter().map(|o| o.size).sum();
        market_size.saturating_sub(our_size)
    }

    /// Get our best bid tick
    pub fn our_best_bid(&self) -> Option<Tick> {
        self.our_bids
            .iter()
            .filter(|(_, orders)| !orders.is_empty())
            .map(|(&tick, _)| tick)
            .max()
    }

    /// Get our best ask tick
    pub fn our_best_ask(&self) -> Option<Tick> {
        self.our_asks
            .iter()
            .filter(|(_, orders)| !orders.is_empty())
            .map(|(&tick, _)| tick)
            .min()
    }

    /// Check if our orders would cross (self-trade potential)
    pub fn would_self_trade(&self) -> bool {
        match (self.our_best_bid(), self.our_best_ask()) {
            (Some(bid), Some(ask)) => bid >= ask,
            _ => false,
        }
    }

    /// Clear all our orders
    pub fn clear_our_orders(&mut self) {
        self.our_bids.clear();
        self.our_asks.clear();
    }

    /// Count total number of our orders
    pub fn our_order_count(&self) -> usize {
        self.our_bids.values().map(|v| v.len()).sum::<usize>()
            + self.our_asks.values().map(|v| v.len()).sum::<usize>()
    }

    // =========================================================================
    // Position Management
    // =========================================================================

    /// Get current position
    pub fn get_position(&self) -> (Size, Option<Tick>) {
        (self.position_size as Size, self.avg_entry_tick)
    }

    /// Process a fill (called when real market trade would fill our order)
    pub fn on_fill(&mut self, side: Side, price_tick: Tick, size: Size, fees_micro: i64, now_ns: u64, order_id: &str) {
        // Calculate PnL if closing position
        let pnl_micro = if self.position_size != 0 && self.avg_entry_tick.is_some() {
            let entry = self.avg_entry_tick.unwrap() as i64;
            let exit = price_tick as i64;
            
            match side {
                Side::Buy => {
                    // Adding to long or closing short
                    if self.position_size < 0 {
                        // Closing short - profit if exit < entry
                        (entry - exit) * size as i64
                    } else {
                        0 // Adding to long, no realized PnL
                    }
                }
                Side::Sell => {
                    // Reducing long or opening short
                    if self.position_size > 0 {
                        // Closing long - profit if exit > entry
                        (exit - entry) * size as i64
                    } else {
                        0 // Opening short, no realized PnL
                    }
                }
            }
        } else {
            0
        };

        // Update balance
        let cost_micro = (price_tick as i128 * size as i128 / 10000) as i64;
        
        match side {
            Side::Buy => {
                self.balance_micro -= cost_micro + fees_micro;
            }
            Side::Sell => {
                self.balance_micro += cost_micro - fees_micro;
            }
        }

        // Add PnL to balance
        self.balance_micro += pnl_micro;

        // Update position
        if self.position_size == 0 {
            // Opening new position
            self.position_size = match side {
                Side::Buy => size as i64,
                Side::Sell => -(size as i64),
            };
            self.avg_entry_tick = Some(price_tick);
        } else {
            let sign = self.position_size.signum();
            let current_size = self.position_size.unsigned_abs() as i64;
            
            match side {
                Side::Buy => {
                    if sign < 0 {
                        // Buying to close short
                        let close_size = size.min(current_size as Size) as i64;
                        self.position_size = (current_size - close_size) * sign;
                        if self.position_size == 0 {
                            self.avg_entry_tick = None;
                        }
                    } else {
                        // Adding to long
                        self.position_size = current_size + size as i64;
                    }
                }
                Side::Sell => {
                    if sign > 0 {
                        // Selling to close long
                        let close_size = size.min(current_size as Size) as i64;
                        self.position_size = (current_size - close_size) * sign;
                        if self.position_size == 0 {
                            self.avg_entry_tick = None;
                        }
                    } else {
                        // Adding to short
                        self.position_size = -(current_size + size as i64);
                    }
                }
            }
        }

        // Update our order size
        self.update_order_size(order_id, size);

        // Record trade in performance
        self.performance.record_trade(TradeRecord {
            timestamp_ns: now_ns,
            side,
            price_tick,
            size,
            pnl_micro,
            fees_micro,
            slippage_bps: 0.0, // TODO: calculate from market data
            duration_ns: 0,    // TODO: track order duration
            order_id: order_id.to_string(),
        });
    }

    /// Calculate fee in micro-USDC
    pub fn calculate_fee(&self, price_tick: Tick, size: Size) -> i64 {
        if self.fee_rate_bps == 0 || size == 0 {
            return 0;
        }

        let price = price_tick as f64 / 10000.0;
        let min_price = price.min(1.0 - price);
        let fee = (self.fee_rate_bps as f64 / 10000.0) * min_price * size as f64;
        fee as i64
    }

    /// Reset the paper book
    pub fn reset(&mut self) {
        self.balance_micro = self.initial_balance_micro;
        self.position_size = 0;
        self.avg_entry_tick = None;
        self.performance = PerformanceReport::new();
        self.clear_our_orders();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_and_remove_order() {
        let mut book = PaperBook::new(10_000.0, 100);

        let order = PaperOrder {
            order_id: "order-1".into(),
            client_order_id: ClientOrderId("order-1".into()),
            side: Side::Buy,
            price_tick: 5000,
            size: 100_000,
            timestamp_ns: 1000,
            queue_position: 0,
        };

        book.add_order(order.clone());
        assert_eq!(book.our_order_count(), 1);
        assert_eq!(book.our_best_bid(), Some(5000));

        let removed = book.remove_order("order-1");
        assert!(removed.is_some());
        assert_eq!(book.our_order_count(), 0);
    }

    #[test]
    fn test_self_trade_detection() {
        let mut book = PaperBook::new(10_000.0, 100);

        // Add bid at 50c
        book.add_order(PaperOrder {
            order_id: "bid-1".into(),
            client_order_id: ClientOrderId("bid-1".into()),
            side: Side::Buy,
            price_tick: 5000,
            size: 100_000,
            timestamp_ns: 1000,
            queue_position: 0,
        });

        // Add ask at 52c - no self trade
        book.add_order(PaperOrder {
            order_id: "ask-1".into(),
            client_order_id: ClientOrderId("ask-1".into()),
            side: Side::Sell,
            price_tick: 5200,
            size: 100_000,
            timestamp_ns: 2000,
            queue_position: 0,
        });

        assert!(!book.would_self_trade());

        // Add crossing ask at 50c - would self trade
        book.add_order(PaperOrder {
            order_id: "ask-2".into(),
            client_order_id: ClientOrderId("ask-2".into()),
            side: Side::Sell,
            price_tick: 5000,
            size: 100_000,
            timestamp_ns: 3000,
            queue_position: 0,
        });

        assert!(book.would_self_trade());
    }

    #[test]
    fn test_update_order_size() {
        let mut book = PaperBook::new(10_000.0, 100);

        book.add_order(PaperOrder {
            order_id: "order-1".into(),
            client_order_id: ClientOrderId("order-1".into()),
            side: Side::Buy,
            price_tick: 5000,
            size: 100_000,
            timestamp_ns: 1000,
            queue_position: 0,
        });

        book.update_order_size("order-1", 50_000);

        let orders = book.our_orders_at(Side::Buy, 5000);
        assert_eq!(orders[0].size, 50_000);
    }

    #[test]
    fn test_paper_book_balance() {
        let book = PaperBook::new(10_000.0, 100);
        assert_eq!(book.get_balance(), 10_000.0);
        assert_eq!(book.get_pnl(), 0.0);
    }

    #[test]
    fn test_performance_report() {
        let mut report = PerformanceReport::new();

        for i in 0..5 {
            report.record_trade(TradeRecord {
                timestamp_ns: i * 1_000_000_000,
                side: if i % 2 == 0 { Side::Buy } else { Side::Sell },
                price_tick: 5000 + i as Tick,
                size: 100_000,
                pnl_micro: if i % 2 == 0 { 100_000 } else { -50_000 },
                fees_micro: 0,
                slippage_bps: 1.0,
                duration_ns: 1_000_000_000,
                order_id: format!("trade-{}", i),
            });
        }

        assert_eq!(report.total_trades, 5);
        assert_eq!(report.winning_trades, 3);
        assert_eq!(report.losing_trades, 2);
        assert_eq!(report.win_rate_pct, 60.0);
    }
}
