# MTrader Copilot Instructions

This document provides guidance for AI coding agents working on the MTrader codebase—a latency-first, production-grade Polymarket trading system focused on BTC 15-minute markets.

## Project Overview

MTrader is a Rust-based automated trading system designed for maker strategies on Polymarket's CLOB (Central Limit Order Book). The system prioritizes:

1. **Latency** - Sync core loop, zero-alloc hot paths, monotonic timestamps
2. **Safety** - SAFE_MODE gates, circuit breakers, paper trading first
3. **Determinism** - Event sourcing, deterministic replay, reproducible results
4. **Observability** - Three-timestamp events, Parquet recording, structured logging

## Memory Bank (onboarding index)

Reference the `memory-bank/` directory for a concise map of the repo, data flow,
and testing steps:
- `memory-bank/architecture.md` for crate map + entry points
- `memory-bank/testing.md` for baseline tests + live market validation
- `memory-bank/strategy-notes.md` for strategy/signal integration

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                         CLI (mtrader)                        │
├─────────────────────────────────────────────────────────────┤
│    Strategy     │   Execution   │    Risk     │  Recorder   │
├─────────────────────────────────────────────────────────────┤
│                     Core + Book + Gateway                    │
├─────────────────────────────────────────────────────────────┤
│                     Sim (Paper Trading)                      │
└─────────────────────────────────────────────────────────────┘
```

### Crate Structure

| Crate | Purpose |
|-------|---------|
| `mtrader-core` | Fundamental types, events, fees, clock, health |
| `mtrader-book` | ArrayBook with O(1) updates, tick validation |
| `mtrader-gateway` | WS/REST clients for Polymarket CLOB API |
| `mtrader-execution` | Order state machine, self-trade prevention |
| `mtrader-risk` | Position tracking, limits, PnL, circuit breaker |
| `mtrader-strategy` | Strategy traits, MakerMM, BundleMaker |
| `mtrader-sim` | Fill simulator, paper book, replay engine |
| `mtrader-recorder` | Parquet/raw frame recording |
| `mtrader-cli` | Command-line interface |

## Critical Conventions

### 1. Tick Validation - NEVER SNAP

```rust
// WRONG - Never silently correct invalid ticks
fn process_tick(tick: u16) -> u16 {
    tick.clamp(0, 100)  // NO! This hides bugs
}

// CORRECT - Reject invalid ticks explicitly
fn validate_inbound_tick(&self, tick: Tick) -> Result<(), BookError> {
    if tick > 10000 {
        return Err(BookError::InvalidTick { tick, max: 10000 });
    }
    if tick % self.tick_size_ticks != 0 {
        return Err(BookError::TickNotAligned { tick, tick_size: self.tick_size_ticks });
    }
    Ok(())
}
```

### 2. Three Timestamps Per Event

Every market event must carry three timestamps:
- `ts_exchange_ms` - Exchange/server timestamp (wall clock)
- `ts_recv_mono_ns` - When we received it (monotonic)
- `ts_process_mono_ns` - When we finished processing (monotonic)

```rust
pub struct BookUpdateEvent {
    pub side: Side,
    pub price_tick: Tick,
    pub new_size: Size,
    pub ts_exchange_ms: u64,      // From Polymarket
    pub ts_recv_mono_ns: u64,     // Our receive time
    pub ts_process_mono_ns: u64,  // After processing
}
```

### 3. Parabolic Fee Model

BTC 15-minute markets use parabolic fees. The formula is:

```
fee = (fee_rate_bps / 16000) × price × (1 - price) × size
```

Where `fee_rate_bps = 1000` (~6.25%) for 15-minute markets.

```rust
// Use fixed-point math in hot path
pub fn compute_fee_micro_usdc(
    &self,
    price_tick: Tick,
    size: Size,
) -> i64 {
    let price_frac = price_tick as i64;
    let complement = 10000 - price_frac;
    // fee = fee_rate_bps * price * (1-price) * size / (16000 * 10000 * 10000)
    (self.fee_rate_bps as i64 * price_frac * complement * size) / 1_600_000_000_000
}
```

### 4. Book Sync Strategy

We treat the server's hash as canonical. We do NOT try to reproduce the hash:

```rust
pub fn on_hash_update(&mut self, server_hash: &str, timestamp_ms: u64) {
    self.last_server_hash = server_hash.to_string();
    self.last_hash_time_ms = timestamp_ms;
    
    // Schedule REST verification if drift suspected
    if self.should_verify() {
        self.request_rest_snapshot();
    }
}
```

### 5. SAFE_MODE Pattern

All trading paths must check safe mode:

```rust
impl SystemHealth {
    pub fn can_trade(&self) -> bool {
        matches!(self.state, HealthState::Normal)
    }
    
    pub fn enter_safe_mode(&mut self, reason: SafeModeReason) {
        self.state = HealthState::SafeMode;
        self.safe_mode_reason = Some(reason);
        // Log, alert, cancel all orders
    }
}

// In trading loop:
if !health.can_trade() {
    return; // No orders sent in safe mode
}
```

### 6. Units Convention

| Value | Unit | Example |
|-------|------|---------|
| Price | Tick (0-10000, 1 tick = 0.0001) | `5000` = $0.50 |
| Size | Micro shares | `100_000_000` = 100 shares |
| Time | Nanoseconds (mono) or MS (wall) | `1_000_000_000` = 1 second |
| Fee | Micro shares (BUY) or micro USDC (SELL) | `500_000` = 0.5 shares |

### 7. Error Handling

Use `thiserror` for domain errors, propagate with `?`:

```rust
#[derive(Error, Debug)]
pub enum BookError {
    #[error("Invalid tick {tick}, max is {max}")]
    InvalidTick { tick: Tick, max: Tick },
    
    #[error("Tick {tick} not aligned to tick size {tick_size}")]
    TickNotAligned { tick: Tick, tick_size: u16 },
}
```

### 8. Testing Patterns

- Unit tests in same file as implementation
- Integration tests in `tests/` directory
- Use `#[cfg(test)]` modules
- Test edge cases: empty book, max position, circuit breaker trips

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fee_at_50_cents() {
        let model = FeeModel::new(1000);
        // At 50c: fee = 0.0625 * 0.50 * 0.50 * 100 = 1.5625 shares
        let fee = model.compute_fee_micro_usdc(5000, 100_000_000);
        assert_eq!(fee, 1_562_500);
    }
}
```

## API Reference

### Polymarket CLOB

- **WebSocket**: `wss://ws-subscriptions-clob.polymarket.com/ws/market`
- **REST**: `https://clob.polymarket.com`

Message types:
- `book` - Order book updates (price, size changes)
- `trade` - Trade executions (prints)
- `hash` - Book hash for sync verification

### Internal Events

```rust
pub enum CoreEvent {
    BookUpdate(BookUpdateEvent),
    Trade(TradeEvent),
    OrderAck(OrderAckEvent),
    OrderFill(OrderFillEvent),
    OrderCancel(OrderCancelEvent),
    OrderReject(OrderRejectEvent),
    StrategySignal(StrategySignalEvent),
    RiskEvent(RiskEventRecord),
    SystemHealth(SystemHealthEvent),
}
```

## Common Tasks

### Adding a New Strategy

1. Implement `Strategy` trait in `mtrader-strategy`
2. Add to strategy selection in CLI
3. Write tests with mock book data

```rust
pub trait Strategy: Send + Sync {
    fn name(&self) -> &str;
    fn on_book_update(&mut self, ctx: &StrategyContext) -> Vec<StrategyAction>;
    fn on_fill(&mut self, fill: &OrderFillEvent);
    fn on_trade(&mut self, trade: &TradeEvent);
}
```

### Adding a New Risk Check

1. Add check in `mtrader-risk/src/limits.rs`
2. Integrate into `PositionLimits::check_order()`
3. Add trip reason to circuit breaker

### Recording New Event Types

1. Add variant to `CoreEvent` enum
2. Update `ParquetEventWriter::buffer_event()`
3. Update replay engine's timestamp extraction

## Do's and Don'ts

### DO

- ✅ Use monotonic clock for latency measurements
- ✅ Validate all inbound ticks before processing
- ✅ Check `health.can_trade()` before any order action
- ✅ Record three timestamps on every event
- ✅ Use fixed-point math for fees in hot path
- ✅ Test with paper trading before any live changes

### DON'T

- ❌ Never snap/clamp invalid ticks silently
- ❌ Never skip safe mode checks
- ❌ Never use floats in hot path calculations
- ❌ Never assume book state without verification
- ❌ Never send orders without position limit checks
- ❌ Never ignore circuit breaker state

## File Organization

```
many-lamps/
├── Cargo.toml              # Workspace manifest
├── config.toml             # Default configuration
├── .github/
│   └── copilot-instructions.md
└── crates/
    ├── core/               # Fundamental types
    │   └── src/
    │       ├── lib.rs
    │       ├── types.rs    # Tick, Size, Side
    │       ├── events.rs   # CoreEvent enum
    │       ├── health.rs   # SystemHealth, SafeMode
    │       ├── fees.rs     # Parabolic fee model
    │       └── clock.rs    # MonotonicClock
    ├── book/               # Order book
    │   └── src/
    │       ├── array_book.rs
    │       ├── sync.rs
    │       └── error.rs
    ├── gateway/            # Polymarket connectivity
    │   └── src/
    │       ├── messages.rs
    │       ├── parser.rs
    │       ├── ws_client.rs
    │       └── rest_client.rs
    ├── execution/          # Order management
    │   └── src/
    │       ├── order.rs
    │       ├── self_trade_guard.rs
    │       └── state_manager.rs
    ├── risk/               # Risk management
    │   └── src/
    │       ├── position.rs
    │       ├── limits.rs
    │       ├── pnl.rs
    │       └── circuit_breaker.rs
    ├── strategy/           # Trading strategies
    │   └── src/
    │       ├── traits.rs
    │       ├── signals.rs
    │       ├── maker_mm.rs
    │       └── bundle_maker.rs
    ├── sim/                # Simulation
    │   └── src/
    │       ├── fill_sim.rs
    │       ├── paper_book.rs
    │       └── replay.rs
    ├── recorder/           # Event recording
    │   └── src/
    │       ├── frame_recorder.rs
    │       ├── event_recorder.rs
    │       └── parquet_writer.rs
    └── cli/                # Command line
        └── src/
            ├── main.rs
            ├── config.rs
            └── commands/
```

## Running the System

```bash
# Paper trading (safe - no real orders)
cargo run --bin mtrader -- paper -m <TOKEN_ID> -s maker_mm

# Record market data
cargo run --bin mtrader -- record -m <TOKEN_ID> -o data/recordings -d 1h

# Replay for backtesting
cargo run --bin mtrader -- replay -i data/recordings -s maker_mm --report backtest.json

# Show market info
cargo run --bin mtrader -- market -m <TOKEN_ID>

# Validate config
cargo run --bin mtrader -- validate-config
```

## Live market validation expectation

When making strategy or execution changes, attempt a paper trading run against
live market data (WebSocket feed). If the environment blocks network access,
note the limitation and proceed with other tests.

## Questions?

When in doubt:
1. Check existing patterns in the codebase
2. Prioritize safety over performance
3. Add logging for debugging
4. Write a test first
