# MTrader Code Map & Implementation Plan

## Project Overview

MTrader is a Rust-based automated trading system for Polymarket CLOB. Key priorities:
1. **Latency** - Sync core loop, zero-alloc hot paths
2. **Safety** - SAFE_MODE gates, circuit breakers
3. **Determinism** - Event sourcing, deterministic replay
4. **Observability** - Three-timestamp events, Parquet recording

---

## Architecture Diagram

```
┌─────────────────────────────────────────────────────────────┐
│                      CLI (mtrader)                           │
├─────────────────────────────────────────────────────────────┤
│  ┌─────────────┐ ┌─────────────┐ ┌─────────────┐ ┌─────────┐│
│  │  Strategy   │ │ Execution   │ │    Risk     │ │Recorder │
│  └──────┬──────┘ └──────┬──────┘ └──────┬──────┘ └────┬────┘│
│         │               │               │              │    │
│  ┌──────┴───────────────┴───────────────┴──────────────┴────┐
│  │              Core + Book + Gateway                       │
│  └─────────────────────────┬───────────────────────────────┘
│                            │
│  ┌─────────────────────────┴───────────────────────────────┐
│  │                    Sim (Paper Trading)                   │
│  └─────────────────────────────────────────────────────────────┘
```

---

## Crate Structure

| Crate | Path | Purpose | Key Files |
|-------|------|---------|-----------|
| `core` | `crates/core/src/` | Types, events, fees, health, clock | `types.rs`, `events.rs`, `fees.rs`, `health.rs` |
| `book` | `crates/book/src/` | ArrayBook order book | `array_book.rs`, `sync.rs`, `error.rs` |
| `gateway` | `crates/gateway/src/` | WS/REST clients | `ws_client.rs`, `rest_client.rs`, `parser.rs` |
| `execution` | `crates/execution/src/` | Order lifecycle | `order.rs`, `state_manager.rs` |
| `risk` | `crates/risk/src/` | Position, limits, PnL, circuit breaker | `limits.rs`, `circuit_breaker.rs`, `pnl.rs` |
| `strategy` | `crates/strategy/src/` | Strategy traits + implementations | `traits.rs`, `maker_mm.rs`, `combinatorial_arb.rs` |
| `sim` | `crates/sim/src/` | Paper trading + replay | `paper_book.rs`, `recorded_backtest.rs`, `replay.rs` |
| `recorder` | `crates/recorder/src/` | Parquet/raw recording | `parquet_writer.rs`, `event_recorder.rs` |
| `dashboard` | `crates/dashboard/src/` | TUI application | `app.rs`, `screens.rs` |
| `cli` | `crates/cli/src/` | CLI entrypoint | `main.rs`, `commands/paper.rs` |

---

## Data Flow (Happy Path)

```
Gateway WS/REST
    ↓
Book updates → Core events
    ↓
Strategy context (StrategyContext)
    ↓
Strategy.on_update() → Vec<StrategyAction>
    ↓
Risk checks (PositionLimits, CircuitBreaker)
    ↓
Execution (Order lifecycle)
    ↓
Recorder (optional, Parquet)
```

---

## Critical Integration Points

### 1. TUI → Strategy Wiring
- **File**: `crates/dashboard/src/app.rs`
- **Key struct**: `AppState` with `auto_trading: AutoTradingState`
- **Missing**: Actual strategy instantiation and `on_update()` call in event loop

### 2. Gateway Connection [c]
- **File**: `crates/dashboard/src/app.rs` (line 596-604)
- **Current**: Mock data (bid/ask derived from market.price)
- **Needed**: Wire to `crates/gateway/src/ws_client.rs`

### 3. RiskGuard Middleware
- **File**: `crates/risk/src/lib.rs`
- **Current exports**: `CircuitBreaker`, `PositionLimits`, `PnLSnapshot`
- **Needed**: New `RiskGuard` struct for centralized signal→order pipeline

---

## Strategy Trait & Actions

```rust
// crates/strategy/src/traits.rs
pub trait Strategy {
    fn name(&self) -> &str;
    fn on_update(&mut self, ctx: &StrategyContext) -> Vec<StrategyAction>;
    fn on_fill(&mut self, ctx: &StrategyContext, side: Side, tick: Tick, size: Size);
    fn on_halt(&mut self);
    fn on_resume(&mut self);
    fn is_active(&self) -> bool;
    fn activate(&mut self);
    fn deactivate(&mut self);
}

pub enum StrategyAction {
    PlaceOrder { side: Side, kind: OrderKind, order_type: OrderType, reason: OrderReason },
    CancelOrder { client_order_id: ClientOrderId, reason: String },
    AmendOrder { client_order_id: ClientOrderId, new_kind: OrderKind, reason: String },
    NoOp,
}
```

---

## TUI Event Loop (Current - Missing Strategy Execution)

```rust
// crates/dashboard/src/app.rs - App::run()
loop {
    terminal.draw(|f| f.render_widget(&TuiApp { state: &self.state }, f.size()))?;
    if event::poll(Duration::from_millis(50))? {
        if let Event::Key(key) = event::read()? {
            self.handle_input(key);
        }
    }
    // MISSING: Strategy execution when auto_trading == Running
    if self.state.current_menu == MenuItem::Quit {
        break;
    }
}
```

---

## What's Left (from progress.md)

| Item | Status | Priority |
|------|--------|----------|
| Wire actual strategy execution to TUI event loop | ❌ PENDING | HIGH |
| Connect [c] to real Polymarket WebSocket | ❌ PENDING | HIGH |
| Implement RiskGuard middleware | ❌ PENDING | MEDIUM |
| Add unit tests (100% coverage goal) | ❌ PENDING | MEDIUM |

---

## Implementation Plan

### Step 1: Wire Strategy Execution Loop
1. Add `active_strategy: Option<Box<dyn Strategy>>` to `AppState`
2. Initialize strategy when entering PaperTrading screen
3. Call `strategy.on_update()` in the main loop when `auto_trading == Running`
4. Execute returned `StrategyAction`s through risk/execution

### Step 2: Connect [c] to Real WebSocket
1. Create `GatewayClient` in `AppState`
2. On [c] press, spawn async WebSocket connection
3. Update `best_bid`, `best_ask` from real messages
4. Pass book updates to strategy context

### Step 3: Implement RiskGuard
1. Create `crates/risk/src/guard.rs`
2. Implement centralized checks:
   - Position limits
   - Exposure caps
   - Max order size
   - Cooldown management
3. Wire into action→execution pipeline

### Step 4: Add Unit Tests
1. Add tests for RiskGuard checks
2. Add tests for strategy actions
3. Add tests for TUI state transitions
4. Target 100% coverage on dashboard crate

---

## Key Files to Modify

| File | Changes |
|------|---------|
| `crates/dashboard/src/app.rs` | Add strategy instance, wire execution loop, connect gateway |
| `crates/risk/src/lib.rs` | Export new RiskGuard module |
| `crates/risk/src/guard.rs` | NEW - Centralized risk pipeline |
| `crates/gateway/src/ws_client.rs` | Ensure TUI-compatible API |
| `tests/integration_test.rs` | NEW - Integration tests |

---

## Commands Reference

```bash
# Build workspace
cargo build --bin mtrader

# Run tests
cargo test ./...

# Launch TUI
cargo run -p mtrader-cli -- tui

# Paper trading (CLI)
cargo run --bin mtrader -- paper -m <TOKEN_ID> -s maker_mm
```

---

## Conventions Summary

| Convention | Details |
|------------|---------|
| **Tick** | 0-10000 (1 tick = 0.0001), u16 |
| **Size** | Micro shares (100_000_000 = 100 shares) |
| **Time** | Nanoseconds (mono) or MS (wall) |
| **Fee** | Micro shares (BUY) or micro USDC (SELL) |
| **3 Timestamps** | ts_exchange_ms, ts_recv_mono_ns, ts_process_mono_ns |
| **Parabolic Fee** | `fee = (fee_rate_bps / 16000) × price × (1 - price) × size` |
| **SAFE_MODE** | Check `health.can_trade()` before orders |
