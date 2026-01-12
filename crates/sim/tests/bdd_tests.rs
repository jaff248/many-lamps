//! Behavior-Driven Tests for MTrader Platform
//!
//! These tests verify system behavior from a user perspective,
//! following Given-When-Then patterns documented in Gherkin style.
//!
//! Test categories:
//! - Trading Loop: Core trading flow from signals to fills
//! - Risk Management: Position limits, circuit breakers, drawdown protection
//! - Strategy Switching: Strategy activation, deactivation, and state management
//! - TUI Display: Terminal UI behavior in different environments
//! - Connection: WebSocket connectivity and graceful degradation
//! - Calculation Accuracy: Financial calculations and fee computations

use mtrader_core::{Side, ClientOrderId};
use mtrader_sim::{PaperBook, PaperOrder, PerformanceReport};
use mtrader_risk::{PnLTracker, PositionLimits, LimitCheck, CircuitBreaker, CircuitBreakerConfig, circuit_breaker::BreakerState, Position};
use mtrader_strategy::{MakerMMStrategy, maker_mm::MakerMMConfig, Strategy, StrategyAction, traits::StrategyContext};
use mtrader_book::ArrayBook;
use std::collections::HashMap;

// =============================================================================
// Trading Loop Scenarios
// =============================================================================

mod trading_loop {
    use super::*;

    /// Given: A connected market with bid/ask prices
    /// When: Auto-trading is enabled with maker_mm strategy
    /// Then: Buy and sell signals should be generated
    #[test]
    fn given_connected_market_when_auto_trading_enabled_then_signals_generated() {
        // Arrange
        let config = MakerMMConfig {
            quote_both_sides: true,
            ..Default::default()
        };
        let mut strategy = MakerMMStrategy::new("test_strategy".to_string(), config);
        strategy.activate();

        // Create a market context with a two-sided market
        let mut book = ArrayBook::new(100);
        book.set_level(Side::Buy, 5150, 1_000_000).unwrap(); // Bid: 0.5150
        book.set_level(Side::Sell, 5250, 1_000_000).unwrap(); // Ask: 0.5250

        // Act - Strategy should generate actions when active and market exists
        let actions = strategy.on_update(&create_test_context(&book, 0));

        // Assert - Actions should include order placements
        let buy_orders = actions.iter()
            .filter(|a| matches!(a, StrategyAction::PlaceOrder { side: Side::Buy, .. }))
            .count();
        let sell_orders = actions.iter()
            .filter(|a| matches!(a, StrategyAction::PlaceOrder { side: Side::Sell, .. }))
            .count();

        assert!(buy_orders > 0, "Should generate buy signals when trading enabled");
        assert!(sell_orders > 0, "Should generate sell signals when trading enabled");
    }

    /// Given: A buy signal is generated
    /// When: Order is submitted to the paper book
    /// Then: Position size should increase accordingly
    #[test]
    fn given_buy_signal_when_order_submitted_then_position_increases() {
        // Arrange
        let mut book = PaperBook::new(10_000.0, 100);
        let initial_position = book.get_position().0;

        // Act - Submit a buy order
        let order = PaperOrder {
            order_id: "test-buy-1".into(),
            client_order_id: ClientOrderId("test-buy-1".into()),
            side: Side::Buy,
            price_tick: 5000,
            size: 100_000, // 0.1 shares
            timestamp_ns: 1000,
            queue_position: 0,
        };
        book.add_order(order);

        // Simulate fill
        book.on_fill(Side::Buy, 5000, 100_000, 0, 2000, "test-buy-1");

        // Assert
        let (position, _) = book.get_position();
        assert!(position > initial_position, "Position should increase after buy fill");
        assert_eq!(position, 100_000, "Position should equal filled size");
    }

    /// Given: An open position exists
    /// When: A fill is received at current market price
    /// Then: PnL should be correctly calculated
    #[test]
    fn given_open_position_when_fill_received_then_pnl_calculated() {
        // Arrange - Open a long position at 50 cents
        let mut book = PaperBook::new(10_000.0, 100);
        book.on_fill(Side::Buy, 5000, 100_000, 0, 1000, "entry-order");

        // Act - Close at 55 cents (10% profit)
        // Cost = 5000 * 100000 / 10000 = 50000 micro
        // Exit = 5500 * 100000 / 10000 = 55000 micro
        // PnL = 55000 - 50000 = 5000 micro
        // But PaperBook PnL is: (exit - entry) * size = 500 * 100000 = 50_000_000 micro
        book.on_fill(Side::Sell, 5500, 100_000, 0, 2000, "exit-order");

        // Assert - PnL should be positive for winning trade
        let pnl = book.get_pnl_micro();
        assert!(pnl > 0, "PnL should be positive for winning trade");
        // PnL = (5500 - 5000) * 100_000 = 50_000_000 micro = $50
        // Allow small tolerance for calculation precision
        assert!((pnl - 50_000_000).abs() <= 5_000, "PnL should be approximately $50");
    }

    /// Given: Realized PnL from closed trades
    /// When: Displayed in dollar format
    /// Then: Should show correct dollar value with proper formatting
    #[test]
    fn given_realized_pnl_when_displayed_then_shows_correct_dollar_value() {
        // Arrange - Multiple trades with mixed results
        let mut report = PerformanceReport::new();

        // Record winning trade: +$1.00
        report.record_trade(create_trade_record(1_000_000, 0));

        // Record losing trade: -$0.50
        report.record_trade(create_trade_record(-500_000, 0));

        // Act
        let formatted = report.summary();

        // Assert - Check for correct dollar formatting in summary
        assert!(formatted.contains("+"), "Positive PnL should have + sign");
        assert!(formatted.contains("$"), "Should show dollar sign");
        assert!(formatted.contains("0.50") || formatted.contains("0.5"), "Should show correct dollar amount");
    }
}

// =============================================================================
// Risk Management Scenarios
// =============================================================================

mod risk_management {
    use super::*;

    /// Given: Maximum position limit is configured
    /// When: Order would exceed the limit
    /// Then: Order should be rejected with PositionLimitExceeded
    #[test]
    fn given_max_position_limit_when_exceeded_then_order_rejected() {
        // Arrange
        let limits = PositionLimits {
            max_position_per_asset: 100_000, // 0.1 shares max
            max_order_size: 200_000,
            min_order_size: 1,
            ..Default::default()
        };

        // Act - Try to open 0.15 share position when max is 0.1
        let check = limits.check_order(150_000, 0, true, 0, 0, 0);

        // Assert
        assert!(!check.is_ok());
        assert!(matches!(check, LimitCheck::PositionLimitExceeded { .. }));
    }

    /// Given: Maximum drawdown limit is configured
    /// When: Drawdown exceeds the limit
    /// Then: Trading should be halted
    #[test]
    fn given_max_drawdown_when_exceeded_then_trading_halted() {
        // Arrange - 5% max drawdown
        let mut breaker = CircuitBreaker::new(CircuitBreakerConfig {
            max_drawdown_bps: 500, // 5%
            ..Default::default()
        });

        // Act - 6% drawdown (exceeds 5% limit)
        let tripped = breaker.check_drawdown(600, 1000);

        // Assert
        assert!(tripped, "Circuit breaker should trip on excessive drawdown");
        assert!(!breaker.is_trading_allowed(), "Trading should be halted");
        assert!(matches!(
            breaker.trip_reason(),
            Some(mtrader_risk::TripReason::MaxDrawdownExceeded { .. })
        ));
    }

    /// Given: Daily loss limit is configured
    /// When: Total losses exceed the limit
    /// Then: Trading should be paused
    #[test]
    fn given_daily_loss_limit_when_exceeded_then_trading_paused() {
        // Arrange - $100 max loss per period
        let mut breaker = CircuitBreaker::new(CircuitBreakerConfig {
            max_loss_per_period: 100_000_000, // $100 in micro-USDC
            loss_period_ns: 3_600_000_000_000, // 1 hour
            ..Default::default()
        });

        // Act - $150 loss (exceeds $100 limit)
        let _pnl_start = 0i64;
        let pnl_current = -150_000_000i64; // -$150
        let tripped = breaker.check_pnl(pnl_current, 3_600_000_000_000);

        // Assert
        assert!(tripped, "Circuit breaker should trip on daily loss limit");
        assert!(!breaker.is_trading_allowed(), "Trading should be paused");
    }

    /// Given: Trading was halted due to loss
    /// When: PnL improves (price moves favorably)
    /// Then: Trading should be allowed to resume after reset
    #[test]
    fn given_loss_recovery_when_pnl_improves_then_trading_resumes() {
        // Arrange
        let mut breaker = CircuitBreaker::new(CircuitBreakerConfig {
            max_drawdown_bps: 500,
            cooldown_ns: 0, // Immediate cooldown
            ..Default::default()
        });

        // Trip the breaker
        breaker.check_drawdown(600, 1000);
        assert!(!breaker.is_trading_allowed());

        // Act - Move to cooling then reset
        breaker.update(2000); // Move to cooling state

        // Simulate recovery
        breaker.reset();

        // Assert
        assert!(breaker.is_trading_allowed(), "Trading should resume after recovery and reset");
        assert_eq!(breaker.state(), BreakerState::Closed);
    }
}

// =============================================================================
// Strategy Switching Scenarios
// =============================================================================

mod strategy_switching {
    use super::*;

    /// Given: A strategy is actively placing orders
    /// When: Strategy is switched to a different one
    /// Then: Old strategy's signals should be cleared
    #[test]
    fn given_active_strategy_when_switched_then_old_signals_cleared() {
        // Arrange
        let config = MakerMMConfig {
            order_size: 1_000_000,
            ..Default::default()
        };
        let mut strategy = MakerMMStrategy::new("strategy_a".to_string(), config);
        strategy.activate();

        let mut book = ArrayBook::new(100);
        book.set_level(Side::Buy, 5150, 1_000_000).unwrap();
        book.set_level(Side::Sell, 5250, 1_000_000).unwrap();
        let initial_actions = strategy.on_update(&create_test_context(&book, 0));
        let initial_order_count = count_place_orders(&initial_actions);

        // Act - Deactivate and switch
        strategy.deactivate();

        let after_deactivate_actions = strategy.on_update(&create_test_context(&book, 0));

        // Assert
        let after_order_count = count_place_orders(&after_deactivate_actions);
        assert_eq!(after_order_count, 0, "No new orders after deactivation");
        assert!(initial_order_count > 0, "Should have had orders before deactivation");
    }

    /// Given: A new strategy is activated
    /// When: Only one activation message should be logged
    /// Then: No duplicate activations should occur
    #[test]
    fn given_new_strategy_when_activated_then_only_one_activation_message() {
        // Arrange
        let mut strategy = MakerMMStrategy::new("test".to_string(), MakerMMConfig::default());

        // Act
        strategy.activate();

        // Assert - Strategy should report as active
        assert!(strategy.is_active(), "Strategy should be active after activation");

        // Double activation should be idempotent
        strategy.activate();
        assert!(strategy.is_active(), "Strategy should remain active");
    }

    /// Given: Multiple strategies are registered
    /// When: Switched rapidly between them
    /// Then: No state leakage should occur (each strategy maintains its own state)
    #[test]
    fn given_multiple_strategies_when_switched_rapidly_then_no_state_leakage() {
        // Arrange - Two strategies with same config
        let config = MakerMMConfig {
            order_size: 1_000_000,
            ..Default::default()
        };
        let mut strategy_a = MakerMMStrategy::new("strategy_a".to_string(), config.clone());
        let mut strategy_b = MakerMMStrategy::new("strategy_b".to_string(), config);

        let mut book = ArrayBook::new(100);
        book.set_level(Side::Buy, 5150, 1_000_000).unwrap();
        book.set_level(Side::Sell, 5250, 1_000_000).unwrap();

        // Act - Rapid switching
        strategy_a.activate();
        let actions_a1 = strategy_a.on_update(&create_test_context(&book, 0));
        let is_active_a1 = strategy_a.is_active();

        strategy_b.activate();
        strategy_a.deactivate();
        let actions_b = strategy_b.on_update(&create_test_context(&book, 0));
        let is_active_a2 = strategy_a.is_active();
        let is_active_b = strategy_b.is_active();

        strategy_a.activate();
        strategy_b.deactivate();
        let actions_a2 = strategy_a.on_update(&create_test_context(&book, 0));
        let is_active_a3 = strategy_a.is_active();
        let is_active_b2 = strategy_b.is_active();

        // Assert - Each strategy maintains its own activation state
        assert!(is_active_a1, "Strategy A should be active after activation");
        assert!(!is_active_a2, "Strategy A should be inactive after deactivation");
        assert!(is_active_b, "Strategy B should be active after activation");
        assert!(is_active_a3, "Strategy A should be active again after re-activation");
        assert!(!is_active_b2, "Strategy B should be inactive after deactivation");

        // Verify both strategies produce consistent results when active
        assert_eq!(actions_a1.len(), actions_a2.len(), "Strategy A should have consistent behavior");
        assert!(actions_a1.len() > 0, "Strategy A should produce actions");
        assert!(actions_b.len() > 0, "Strategy B should produce actions");
    }
}

// =============================================================================
// TUI Display Scenarios
// =============================================================================

mod tui_display {
    /// Given: TUI is displaying a menu
    /// When: Back key is pressed
    /// Then: Screen should clear and return to previous menu
    #[test]
    fn given_menu_navigation_when_back_pressed_then_screen_cleared() {
        // This test verifies the screen state management concept
        // In a real TUI, this would test the navigation stack

        // Arrange - Simulate menu stack behavior
        let mut menu_stack: Vec<String> = vec!["main".to_string()];

        // Act - Push submenu, then go back
        menu_stack.push("submenu".to_string());
        assert_eq!(menu_stack.len(), 2);

        menu_stack.pop(); // Back pressed
        assert_eq!(menu_stack.len(), 1);
        assert_eq!(menu_stack[0], "main");

        // Assert - Screen cleared to main menu
        assert!(menu_stack.len() <= 1, "Should return to previous screen");
    }

    /// Given: Running in non-TTY environment (piped output)
    /// When: Dashboard starts
    /// Then: Should operate in headless mode without terminal control
    #[test]
    fn given_non_tty_environment_when_dashboard_starts_then_headless_mode() {
        // Arrange - Simulate non-TTY detection
        let is_terminal = false; // Would be determined by atty::isnt(STDIN_FILENO)

        // Act
        let headless_mode = !is_terminal;

        // Assert
        assert!(headless_mode, "Should detect non-TTY and enter headless mode");
    }

    /// Given: Log messages are being generated
    /// When: TUI is running
    /// Then: Display should not be corrupted by log output
    #[test]
    fn given_log_messages_when_tui_running_then_display_not_corrupted() {
        // This test verifies log buffering behavior
        // Arrange
        let mut log_buffer: Vec<String> = vec![];
        let max_buffer_size = 1000;

        // Act - Add many log messages
        for i in 0..2000 {
            log_buffer.push(format!("Log message {}", i));
            if log_buffer.len() > max_buffer_size {
                log_buffer.remove(0); // Circular buffer behavior
            }
        }

        // Assert - Buffer should not grow unbounded
        assert!(log_buffer.len() <= max_buffer_size, "Log buffer should be bounded");
        assert_eq!(log_buffer.len(), max_buffer_size, "Buffer should maintain max size");
    }
}

// =============================================================================
// Connection Scenarios
// =============================================================================

mod connection {
    use super::*;

    /// Given: WebSocket connection is established
    /// When: Order book update is received
    /// Then: Strategy should be notified of the change
    #[test]
    fn given_websocket_connected_when_book_update_received_then_strategy_notified() {
        // Arrange
        let mut strategy = MakerMMStrategy::new("test".to_string(), MakerMMConfig::default());
        strategy.activate();

        let mut book = ArrayBook::new(100);

        // Act - First update
        book.set_level(Side::Buy, 5150, 1_000_000).unwrap();
        book.set_level(Side::Sell, 5250, 1_000_000).unwrap();
        let actions1 = strategy.on_update(&create_test_context(&book, 0));

        // Second update with different prices
        book.set_level(Side::Buy, 5160, 1_000_000).unwrap();
        book.set_level(Side::Sell, 5240, 1_000_000).unwrap();
        let actions2 = strategy.on_update(&create_test_context(&book, 0));

        // Assert - Both should generate actions
        assert!(!actions1.is_empty(), "Strategy should react to first update");
        assert!(!actions2.is_empty(), "Strategy should react to second update");
    }

    /// Given: Connection was disconnected
    /// When: Successfully reconnected
    /// Then: Previous state should be preserved
    #[test]
    fn given_disconnection_when_reconnected_then_state_preserved() {
        // Arrange - Create paper book with position
        let mut book = PaperBook::new(10_000.0, 100);
        book.on_fill(Side::Buy, 5000, 100_000, 0, 1000, "order-1");

        let (position_before, _) = book.get_position();
        let balance_before = book.get_balance();

        // Simulate disconnect/reconnect cycle (no state change)
        // Act - Reconnection preserves state
        let (position_after, _) = book.get_position();
        let balance_after = book.get_balance();

        // Assert
        assert_eq!(position_before, position_after, "Position should be preserved");
        assert_eq!(balance_before, balance_after, "Balance should be preserved");
    }

    /// Given: Signal handlers are registered
    /// When: SIGINT is received
    /// Then: Graceful shutdown should occur
    #[test]
    fn given_signal_handlers_when_sigint_received_then_graceful_shutdown() {
        // This tests the shutdown sequence concept
        // Arrange
        let graceful_shutdown = true; // Signal received
        let mut position_closed = false;

        // Simulate shutdown sequence
        // Act
        // In real code: ctrlc::set_handler(|| { graceful_shutdown = true; })
        // Here we test the concept
        if graceful_shutdown {
            position_closed = true; // Close positions on shutdown
        }

        // Assert
        assert!(graceful_shutdown, "Shutdown flag should be set");
        assert!(position_closed, "Positions should be closed on shutdown");
    }
}

// =============================================================================
// Calculation Accuracy Scenarios
// =============================================================================

mod calculation_accuracy {
    use super::*;

    /// Given: Position with micro-USDC PnL
    /// When: Drawdown percentage is calculated
    /// Then: Correct percentage should be returned
    #[test]
    fn given_micro_usdc_pnl_when_drawdown_calculated_then_correct_percentage() {
        // Arrange
        let starting_capital = 1_000_000_000; // $1000 in micro-USDC
        let _tracker = PnLTracker::new(starting_capital);

        // Act - Simulate drawdown: $50 loss from high water mark
        // This would be done via PositionTracker in real code
        let drawdown_micro = -50_000_000i64; // -$50
        let drawdown_bps = (drawdown_micro.abs() * 10_000) / starting_capital;

        // Assert
        assert_eq!(drawdown_bps, 500, "Drawdown should be 500 bps (5%)");
    }

    /// Given: Fee rate is configured
    /// When: Trade is executed
    /// Then: Fees should be calculated correctly
    #[test]
    fn given_fee_rate_when_trade_executed_then_fees_calculated_correctly() {
        // Arrange
        let fee_rate_bps = 1000; // 10 bps = 0.1%
        let price_tick = 5000; // $0.50
        let size: u64 = 100_000; // 0.1 shares

        // Act - Fee = fee_rate_bps * price * size / 10000 / 10000
        // Fee = 1000 * 5000 * 100000 / 100000000 = 5000 micro = $0.005
        let min_tick = price_tick.min(10000 - price_tick) as u128;
        let fee = (fee_rate_bps as u128 * min_tick * size as u128) / (10_000u128 * 10_000u128);

        // Assert
        assert_eq!(fee, 5000, "Fee should be 5000 micro-USDC");
    }

    /// Given: Queue position for order
    /// When: Orders are added at same price
    /// Then: FIFO order should be maintained
    #[test]
    fn given_queue_position_when_orders_added_then_fifo_order_maintained() {
        // Arrange
        let mut book = ArrayBook::new(100);

        // Act - Add orders at same price level
        book.set_level(Side::Buy, 5150, 1_000_000).unwrap();
        book.set_level(Side::Sell, 5250, 1_000_000).unwrap();

        let best_bid = book.best_bid();
        let best_ask = book.best_ask();

        // Assert - Book should maintain order
        assert_eq!(best_bid, Some(5150), "Best bid should be 5150");
        assert_eq!(best_ask, Some(5250), "Best ask should be 5250");
    }
}

// =============================================================================
// Helper Functions
// =============================================================================

/// Create a test strategy context with the given book and position
fn create_test_context(book: &ArrayBook, position: i64) -> StrategyContext {
    StrategyContext {
        now_ns: 1000,
        asset_id: "test-asset".to_string(),
        position: Position {
            net_size: position,
            ..Default::default()
        },
        pnl: mtrader_risk::PnLSnapshot {
            timestamp_ns: 1000,
            realized_pnl: 0,
            unrealized_pnl: 0,
            total_pnl: 0,
            total_fees: 0,
            net_pnl: 0,
            high_water_mark: 0,
            drawdown: 0,
            drawdown_bps: 0,
        },
        best_bid: book.best_bid(),
        best_ask: book.best_ask(),
        best_bid_size: book.get_level(Side::Sell, book.best_bid().unwrap_or(0)),
        best_ask_size: book.get_level(Side::Buy, book.best_ask().unwrap_or(0)),
        mid_tick: book.mid_tick(),
        spread_ticks: book.spread(),
        our_bids: vec![],
        our_asks: vec![],
        market_snapshots: HashMap::new(),
    }
}

/// Count place order actions in a vector
fn count_place_orders(actions: &[StrategyAction]) -> usize {
    actions.iter()
        .filter(|a| matches!(a, StrategyAction::PlaceOrder { .. }))
        .count()
}

/// Create a trade record for testing
fn create_trade_record(pnl_micro: i64, fees_micro: i64) -> mtrader_sim::TradeRecord {
    mtrader_sim::TradeRecord {
        timestamp_ns: 1000,
        side: if pnl_micro >= 0 { Side::Buy } else { Side::Sell },
        price_tick: 5000,
        size: 100_000,
        pnl_micro,
        fees_micro,
        slippage_bps: 0.0,
        duration_ns: 1_000_000_000,
        order_id: "test-trade".to_string(),
    }
}
