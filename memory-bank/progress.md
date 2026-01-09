# Progress

## What Works
- ✅ Workspace compiles via Cargo
- ✅ CLI with multiple commands (paper, record, replay, backtest, tui)
- ✅ TUI launches with main menu navigation
- ✅ Strategy modules implemented (maker_mm, bundle_maker, unaffected_arb, rebalancing_arb)
- ✅ Risk crate with PositionLimits and CircuitBreaker
- ✅ Recorder crate with Parquet support
- ✅ Gateway crate with WebSocket and REST clients
- ✅ TUI v2.0 builds and runs (fixed bugs)

## Bugs Fixed (v2.0)
- ✅ **Input editing**: Backspace now deletes char, Esc cancels
- ✅ **Position display**: Shows persistent state, not random values
- ✅ **Trade button**: Removed auto-increment; [a] toggles auto-trading
- ✅ **edit_field matching**: Added PartialEq for EditField and ConfirmAction enums
- ✅ **AutoTradingState**: Removed Copy to allow String in Halted variant

## Phase 1 Complete: Bug Fixes ✅
- InputMode::Editing works correctly
- Position/PnL displays persistent values
- Auto-trading toggle with [a] key

## Phase 2 Complete: Auto-Trading Mode ✅
- [a] key toggles: Disabled → Running → Paused → Halted
- Visual status in header: "DISABLED", "🤖 RUNNING", "⏸ PAUSED", "HALTED: <reason>"
- Circuit breaker integration

## Phase 3 Complete: Risk Settings UI ✅
- Configurable: max position, max order size, max drawdown, consecutive loss limit
- Adjustable via arrow keys
- [r] reset to defaults

## Phase 4 Complete: Strategy Parameters UI ✅
- 8 parameters: spread_bps, order_size, num_levels, inventory_skew, sum_target, edge_threshold, dip_threshold, window_minutes
- Adjustable via arrow keys
- [s] switch strategy type

## Phase 5 Complete: Market Management ✅
- 12 built-in crypto markets (BTC/ETH/SOL/DOGE/etc. up/down 15m-4h)
- [a] add custom market via condition_id
- Browse and select markets

## Phase 6 Complete: Backtest Integration ✅
- Select strategy + parameters
- Choose recording directory
- Set starting balance
- Run simulated backtest

## Phase 7 Complete: Live Trading Parity ✅
- Same UI as paper trading
- Wallet address input
- Risk settings displayed
- Safety warning banner

## Current Status
- **Overall**: 90% complete
- **Documentation**: 100% (tui-requirements.md complete)
- **Core Trading**: 90% (TUI integrated, strategy wiring pending)

## What's Left
- Wire actual strategy execution to TUI event loop
- Connect [c] to real Polymarket WebSocket
- Add unit tests (100% coverage goal)

## Commands
```bash
cargo run -p mtrader-cli -- tui  # Launch TUI
```

## Keyboard Shortcuts
- [↑/↓] Navigate menus
- [Enter] Select / Confirm
- [Esc/q] Back / Quit
- [c] Connect to market
- [a] Toggle auto-trading
- [r] Toggle recording
- [s] Switch strategy
- [p] Show position/PnL
- [e] Edit market ID
