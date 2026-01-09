# TUI Requirements & Feature Specifications

## Executive Summary
This document defines the requirements for the MTrader TUI to transform from a prototype into a production-grade algorithmic trading terminal.

---

## 1. Core Bugs to Fix

### 1.1 Input Mode Editing Bug
**Issue**: Pressing backspace in edit mode cancels input instead of deleting character
**Fix**: 
- `Backspace` should delete last char: `self.state.text_input.pop()`
- `Esc` should cancel and clear
- `Enter` should confirm

### 1.2 Random Position Display
**Issue**: `[p]` key shows random position via `rand_f64()`
**Fix**: Display actual `AppState.position` which should be persistent and updated only from fills

### 1.3 Auto-increment Trade
**Issue**: `[t]` key just increments counter with random side
**Fix**: In auto-trading mode, the user does NOT manually trade - the strategy does

---

## 2. Auto-Trading Mode (NEW)

### 2.1 Concept
When enabled, the selected strategy autonomously executes trades when conditions are met.

### 2.2 Requirements
```rust
pub struct AutoTradingConfig {
    pub enabled: bool,
    pub strategy: StrategyType,
    pub strategy_params: StrategyParams,
    pub risk_limits: PositionLimits,
    pub circuit_breaker: CircuitBreakerConfig,
}
```

### 2.3 Flow
1. User selects market
2. User selects strategy (maker_mm, bundle_maker, etc.)
3. User configures strategy parameters
4. User enables auto-trading
5. Bot runs strategy loop:
   - Gateway streams data → Book updates
   - Strategy evaluates context → Emits actions
   - Risk checks actions → Approves/rejects
   - Execution places orders (paper or live)
   - Circuit breaker monitors for halts

### 2.4 TUI Display
```
╔══════════════════════════════════════════╗
║ 🤖 AUTO-TRADING: ACTIVE                  ║
║ Strategy: maker_mm | Market: BTC-15m     ║
╠══════════════════════════════════════════╣
║ Position: 50 | PnL: $12.34               ║
║ Active Bids: 3 | Active Asks: 2          ║
║ Drawdown: 2.3% (max 5%)                  ║
╠══════════════════════════════════════════╣
║ [a] Toggle Auto | [s] Strategy Settings  ║
║ [r] Risk Settings | [q] Stop & Exit      ║
╚══════════════════════════════════════════╝
```

---

## 3. Risk Management Integration

### 3.1 Existing Components (Already in `crates/risk/`)
- `PositionLimits`: max position, max order size, daily volume limits
- `CircuitBreaker`: drawdown detection, consecutive loss detection, cooldown

### 3.2 Required TUI Parameters (User Configurable)
```rust
pub struct TuiRiskConfig {
    // Position limits
    pub max_position_per_asset: i64,    // Max shares per market
    pub max_order_size: u64,            // Max single order
    pub max_bet_percentage: f64,        // Max % of balance per trade (NEW)
    
    // Drawdown protection
    pub max_drawdown_bps: i64,          // e.g., 500 = 5%
    pub max_loss_per_hour: i64,         // Micro-USDC
    
    // Stop conditions
    pub stop_on_consecutive_losses: u32, // e.g., 10
    pub stop_on_total_loss: i64,        // Absolute loss threshold
    
    // Daily limits
    pub max_daily_trades: u64,          // Optional trade count limit
    pub trading_hours: Option<(u8, u8)>, // Optional time window
}
```

### 3.3 TUI Risk Settings Screen
```
╔═══════════════════════════════════════╗
║ ⚙️  RISK SETTINGS                      ║
╠═══════════════════════════════════════╣
║ Max Position:        200 shares       ║
║ Max Order Size:      20 shares        ║
║ Max Bet %:           2%               ║
║ Max Drawdown:        5%               ║
║ Max Loss/Hour:       $100             ║
║ Stop After Losses:   10 consecutive   ║
║ Daily Trade Limit:   Unlimited        ║
╠═══════════════════════════════════════╣
║ [↑/↓] Select  [←/→] Adjust  [Enter]   ║
╚═══════════════════════════════════════╝
```

---

## 4. Strategy Parameters

### 4.1 maker_mm (Market Making)
```rust
pub struct MakerMMParams {
    pub spread_bps: u16,           // Target spread in basis points
    pub order_size_micro: u64,     // Order size
    pub num_levels: u8,            // Number of price levels
    pub level_spacing_ticks: u16,  // Ticks between levels
    pub inventory_skew: bool,      // Skew quotes based on position
    pub max_position: i64,         // Strategy-level position limit
}
```

### 4.2 bundle_maker (Bundle Arbitrage)
```rust
pub struct BundleMakerParams {
    pub sum_target: f64,           // Target sum (e.g., 1.0 for YES+NO)
    pub edge_threshold_bps: u16,   // Minimum edge to trade
    pub shares_per_leg: u64,       // Size per leg
    pub leg2_timeout_ms: u64,      // Timeout for second leg
}
```

### 4.3 rebalancing_arb (YES/NO Rebalancing)
```rust
pub struct RebalancingArbParams {
    pub dip_threshold: f64,        // Price deviation threshold
    pub window_minutes: u64,       // Lookback window
    pub shares_per_trade: u64,     // Trade size
}
```

---

## 5. Market Management

### 5.1 Built-in Markets (10-15 Default)
```rust
pub const DEFAULT_MARKETS: &[(&str, &str)] = &[
    ("btc-updown-15m", "BTC Up/Down 15min"),
    ("btc-updown-1h", "BTC Up/Down 1hour"),
    ("btc-updown-4h", "BTC Up/Down 4hour"),
    ("eth-updown-15m", "ETH Up/Down 15min"),
    ("eth-updown-1h", "ETH Up/Down 1hour"),
    ("sol-updown-15m", "SOL Up/Down 15min"),
    ("doge-updown-5m", "DOGE Up/Down 5min"),
    ("avax-updown-15m", "AVAX Up/Down 15min"),
    ("matic-updown-15m", "MATIC Up/Down 15min"),
    ("link-updown-15m", "LINK Up/Down 15min"),
];
```

### 5.2 Custom Market Query
- `[a]` Add market → Enter Polymarket condition_id
- Fetch metadata via `gateway/rest_client.rs`
- Validate market is active
- Add to user's market list

### 5.3 Market Browser Behavior
- Option to STAY in browser after selection (not auto-navigate)
- Option to GO TO paper trading with selected market
- Search/filter by asset (BTC, ETH, etc.)

---

## 6. Backtest Integration

### 6.1 Flow
1. Select strategy from list
2. Configure strategy parameters
3. Select recording file(s) as input
4. Set starting balance
5. Run backtest via `sim/recorded_backtest.rs`
6. Display results

### 6.2 Results Display
```
══════════════════════════════════════════
           BACKTEST RESULTS
══════════════════════════════════════════
Strategy:      maker_mm (spread=50bps)
Input:         data/recordings/btc-15m.parquet
Period:        2026-01-01 to 2026-01-08

Starting:      $1,000.00
Ending:        $1,087.50
Net PnL:       $87.50
ROI:           8.75%

Total Trades:  156
Win Rate:      62.3%
Sharpe:        1.82
Max Drawdown:  3.2%
══════════════════════════════════════════
```

### 6.3 Parameter Optimization (Future)
- Grid search over parameter ranges
- Display best performing config

---

## 7. Live Trading Parity

### 7.1 Requirements
- Live trading UI MUST be identical to paper trading
- Same strategy selection
- Same parameter configuration
- Same risk settings display
- Same statistics view

### 7.2 Key Differences (Backend Only)
- Paper: Orders go to `sim/paper_book.rs`
- Live: Orders go to `execution/` → Polymarket API

### 7.3 Safety Gates
1. Wallet connection required
2. Explicit confirmation dialog
3. Real-money warning banner
4. Circuit breaker ENABLED by default
5. SAFE_MODE toggle visible

---

## 8. Data Storage

### 8.1 Parquet Files (Already supported via `recorder/parquet_writer.rs`)
- `trades.parquet` - Executed trades
- `book_snapshots.parquet` - Order book snapshots
- `events.parquet` - Raw market events

### 8.2 JSON Config Files
- `markets.json` - User's saved markets
- `strategies.json` - Strategy parameter presets
- `risk_profiles.json` - Risk configuration presets

### 8.3 State Persistence
- On exit: Save current session state
- On start: Offer to restore previous session

---

## 9. Implementation Priority

### Phase 1: Bug Fixes & Core (1 day)
- [ ] Fix input mode editing (backspace)
- [ ] Fix position display (persistent state)
- [ ] Remove manual trade button (trade = strategy action)

### Phase 2: Auto-Trading Mode (2 days)
- [ ] Add AutoTradingConfig struct
- [ ] Wire strategy loop to TUI
- [ ] Add toggle for auto-trading
- [ ] Display strategy state in real-time

### Phase 3: Risk Settings UI (1 day)
- [ ] Create risk settings screen
- [ ] Wire to existing PositionLimits
- [ ] Wire to existing CircuitBreaker
- [ ] Add user-configurable params

### Phase 4: Strategy Parameters UI (2 days)
- [ ] Create strategy selection screen
- [ ] Create parameter editing screen per strategy
- [ ] Save/load parameter presets

### Phase 5: Market Management (1 day)
- [ ] Add 10-15 default markets
- [ ] Implement custom market query
- [ ] Fix browser navigation behavior

### Phase 6: Backtest Integration (2 days)
- [ ] Wire to recorded_backtest.rs
- [ ] Strategy/param selection
- [ ] Recording file selection
- [ ] Results display

### Phase 7: Live Trading Parity (1 day)
- [ ] Mirror paper trading UI
- [ ] Add safety gates
- [ ] Wallet connection flow

---

## 10. Technical Architecture

### 10.1 State Machine
```
TuiState {
    mode: TradingMode, // Paper | Live
    trading: TradingState, // Idle | AutoRunning | Paused | Halted
    market: Option<Market>,
    strategy: Option<StrategyConfig>,
    risk: RiskConfig,
    position: Position,
    pnl: PnLSnapshot,
    circuit_breaker: CircuitBreaker,
}
```

### 10.2 Event Loop
```rust
loop {
    // 1. Handle user input
    if let Some(key) = poll_key() {
        handle_input(key, &mut state);
    }
    
    // 2. If auto-trading, run strategy tick
    if state.trading == TradingState::AutoRunning {
        let ctx = build_context(&book, &state);
        let actions = strategy.on_update(&ctx);
        
        for action in actions {
            if risk_check(&action, &state.risk).is_ok() {
                execute(action, &mut state);
            }
        }
        
        // Check circuit breaker
        if circuit_breaker.check_drawdown(state.pnl.drawdown_bps) {
            state.trading = TradingState::Halted;
        }
    }
    
    // 3. Render UI
    terminal.draw(|f| render(&state, f))?;
}
```

---

## Appendix: Key Files to Modify

| File | Changes |
|------|---------|
| `dashboard/src/app.rs` | Fix bugs, add auto-trading loop |
| `dashboard/src/screens.rs` | Add strategy params, risk settings screens |
| `risk/src/limits.rs` | Add max_bet_percentage field |
| `strategy/src/traits.rs` | Add strategy params trait |
| `strategy/src/*.rs` | Add configurable params to each |
| `cli/src/commands/paper.rs` | Wire auto-trading mode |
