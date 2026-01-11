# Alpha Research Report: Polymarket Trading Systems

This report summarizes key architectural patterns and "alpha" discovered from analyzing open-source Rust-based trading systems for Polymarket and general HFT.

## Research Status: HIGH-VALUE ITEMS IMPLEMENTED

| Feature | Status | Alpha Value |
|---------|--------|-------------|
| **Combinatorial Arb** | ✅ PARTIAL | **HIGH** - Basic dependency graph, needs price monitoring loop |
| **Auto-Hedge** | ✅ DONE | **HIGH** - Leg1/Leg2 state machine for 15-min markets |
| **Smart Money Detection** | ✅ DONE | **HIGH** - Large trade + volume analysis |
| **Signal Processing** | ✅ DONE | **MEDIUM** - EMA, momentum, spread signals |
| **T-KAN** | ⏳ TODO | **HIGH** - Not started, needs tch-rs integration |

---

## 1. High-Performance Order Book (`polysqueeze`)

**Source:** `polysqueeze/src/book.rs`

The most significant performance optimization found is the use of fixed-point arithmetic for the order book instead of `rust_decimal::Decimal`.

### Key Optimizations:
- **Internal Representation**: Uses `u64` (or similar integer types) for `Price` and `Qty` internally.
- **Fast Path**: Implements `apply_delta_fast` that bypasses Decimal conversion in the hot path.
- **Performance Gain**: ~10-50x speedup for order book updates compared to Decimal-based implementations.
- **hashing**: Hashes `token_id` once to avoid repeated string comparisons during updates.

### Status: ⏭️ SKIPPED
**Reason**: Polymarket tick data (1/10000) fits in u16. Current implementation is adequate for market making scale. Revisit if HFT requirements emerge.

---

## 2. Robust Market Depth Handling (`hftbacktest`)

**Source:** `hftbacktest/src/depth/hashmapmarketdepth.rs`

`hftbacktest` provides a robust `HashMapMarketDepth` implementation that handles L2/L3 data streams effectively.

### Key Features:
- **Robustness**: Handles missing depth delete events gracefully (unlike `BTreeMap` which might retain stale levels).
- **L3 Support**: Explicit support for Level 3 (Market-By-Order) data, tracking individual orders alongside price levels.
- **Snapshot Application**: Clean trait `ApplySnapshot` for initializing state from snapshots.

### Status: ⏭️ SKIPPED
**Reason**: Polymarket provides L2 data only. ArrayBook is sufficient for current requirements.

---

## 3. Modular Strategy Engine (`clobster`)

**Source:** `clobster/src/strategy/engine.rs`, `context.rs`

`clobster` implements a highly modular strategy framework that separates concerns effectively.

### Architecture:
- **Strategy Trait**: Defines a clear interface (`evaluate(ctx) -> Vec<Signal>`).
- **Strategy Context**: Provides a rich, read-only snapshot of market state (`MarketSnapshot`, `PositionSnapshot`, `OrderSnapshot`).
- **Signal Abstraction**: Strategies emit `Signal` objects (Entry, Exit) rather than raw orders.
- **Risk Guard**: Centralized risk checks applied to all signals before execution.
- **Engine**: Manages the lifecycle, execution loop, and signal processing for multiple strategies running concurrently.

### Status: ✅ PARTIALLY IMPLEMENTED
- ✅ Signal abstraction in `strategy/signals.rs`
- ✅ StrategyContext with market_snapshots in `traits.rs`
- ⏳ **RiskGuard** - NOT YET IMPLEMENTED (Phase 8)

---

## 4. Cross-Platform Execution (`Polymarket-Kalshi-Arbitrage-bot`)

**Source:** `Polymarket-Kalshi-Arbitrage-bot/src/execution.rs`

This bot demonstrates advanced execution patterns for arbitrage.

### Patterns:
- **Concurrent Execution**: Uses `tokio::join!` to execute legs on different platforms simultaneously.
- **Auto-Closing**: Background tasks automatically close excess inventory if one leg fails or partially fills.
- **Circuit Breaker**: Integrated circuit breaker to halt trading on consecutive failures or drawdown.

### Status: ✅ PARTIALLY IMPLEMENTED
- ✅ Auto-hedge in `auto_hedge.rs` (our implementation)
- ✅ CircuitBreaker in `risk/circuit_breaker.rs`
- ⏭️ Cross-platform - SKIPPED (not relevant for Polymarket-only)

---

## 5. Dynamic Data Scheduling (`polymarket-hft`)

**Source:** `polymarket-hft/src/scheduler.rs`

Implements a dynamic job scheduler using `tokio-cron-scheduler`.

### Patterns:
- **Runtime Scheduling**: Allows adding/removing data ingestion jobs (e.g., fetching external signals like "Fear & Greed") without restarting the bot.
- **Job Handle**: Uses a `SchedulerHandle` to manage lifecycle safely across threads.

### Status: ⏭️ SKIPPED
**Reason**: Overkill for current scope. WebSocket stream is sufficient.

---

## 6. Smart Wallet Support (`rs-clob-client`)

**Source:** `rs-clob-client/src/lib.rs`

Contains robust logic for deriving smart contract wallet addresses.

### Patterns:
- **CREATE2 Derivation**: correctly derives Proxy (Magic/email) and Safe (Gnosis) wallet addresses from an EOA address.
- **Chain Awareness**: Handles config differences between Polygon Mainnet and Amoy Testnet.

### Status: ⏭️ SKIPPED
**Reason**: Nice-to-have, not core alpha. Users can connect via MetaMask directly.

---

## Summary of Alpha Applied

| Feature | Source Repo | Impact | Status |
|---------|-------------|--------|--------|
| **Combinatorial Arb** | Research | **HIGH** | ⏳ PARTIAL |
| **Auto-Hedge** | Research | **HIGH** | ✅ DONE |
| **Smart Money Signals** | Research | **HIGH** | ✅ DONE |
| **Signal Abstraction** | clobster | **MEDIUM** | ✅ DONE |
| **Risk Guard** | clobster | **HIGH** | ⏳ TODO |
| **T-KAN ML Model** | Research | **HIGH** | ⏳ TODO |
| **Fixed-Point Book** | polysqueeze | **HIGH** | ⏭️ SKIPPED |
| **Robust Depth** | hftbacktest | **MEDIUM** | ⏭️ SKIPPED |
| **Wallet Derivation** | rs-clob-client | **MEDIUM** | ⏭️ SKIPPED |
| **Job Scheduler** | polymarket-hft | **LOW** | ⏭️ SKIPPED |

---

## Implementation Roadmap

### ✅ COMPLETED (Jan 2026)

1. **Combinatorial Arb Strategy** (Phase 6 partial)
   - Dependency graph with Implication/MutuallyExclusive/Identical relations
   - Located: `crates/strategy/src/combinatorial_arb.rs`

2. **Auto-Hedge Strategy** (Phase 6 partial)
   - Leg1/Leg2 state machine for 15-min markets
   - Located: `crates/strategy/src/auto_hedge.rs`

3. **Alpha Signal Processing** (Phase 6 partial)
   - Large trade detection, smart money tracking
   - Located: `crates/research/src/signals.rs`

### ⏳ TODO (Next Sprints)

1. **Phase 6 (Combinatorial Arbitrage - Full Implementation)**
   - Dependency graph with negative log prices for cycle detection
   - Price monitoring loop with debounce
   - Concurrent leg execution with rollback on partial fills

2. **Phase 7 (T-KAN Integration)** ⭐ HIGH PRIORITY
   - Rust ML Stack: `tch-rs` for PyTorch parity
   - MLP-approximated KAN layer (SiLU activation)
   - Feature-gated: `#[cfg(feature = "ml")]`
   - Pipeline: normalization → windowing → model → signal

3. **Phase 8 (Signal + RiskGuard)**
   - Centralized RiskGuard pipeline
   - Signal→Order tracing with IDs
   - Execution flow: Strategy → Signal → RiskGuard → Planner → Order

---

## Phase 6: Combinatorial Arbitrage (Full Implementation)

### Dependency Graph Representation
Model markets and instruments as a directed graph:
- **Nodes**: Outcomes/contracts (e.g., "BTC-15m-YES", "BTC-15m-NO")
- **Edges**: Conversion paths via orders or swaps
- **Edge Weight**: Negative log price (or fee-adjusted spread)
- **Detection**: Shortest-path / negative-cycle checks

### Price Monitoring Loop
- Maintain real-time price cache keyed by market/outcome
- Track best bid/ask and depth at each level
- Update edge weights on each tick or book delta
- Recompute candidate cycles incrementally (BFS from touched nodes)
- Throttle with debounce window (avoid noise triggers)

### Execution Rules
- Minimum net edge weight (profit threshold after fees/slippage)
- Max cycle length enforcement
- Liquidity validation at each hop
- Atomic-ish execution with concurrent leg placement
- Rollback/auto-hedge for partial fills
- Structured telemetry for fill rates and abort reasons

---

## Phase 7: T-KAN Integration Roadmap

### Rust ML Stack Choice
```toml
[dependencies]
tch-rs = { version = "0.12", optional = true }
burn = { version = "0.12", optional = true }
```

**Strategy**: Prototype with `tch-rs` for PyTorch parity, evaluate `burn` later. Feature flag `ml` controls availability.

### Layer Approximation
The Python reference uses a SiLU-based MLP approximation of the KAN layer:

```python
# Python reference (KAN-Linear approximation)
class KANLinear(nn.Module):
    def __init__(self, in_features, out_features):
        self.layers = nn.Sequential(
            nn.Linear(in_features, 64),
            nn.SiLU(),
            nn.Linear(64, out_features),
        )
    
    def forward(self, x):
        return self.layers(x)
```

**Implementation in Rust**: Port this MLP structure to `tch-rs` for inference.

### Data/Feature Pipeline
1. **Normalization**: Scale inputs to [0, 1] or [-1, 1]
2. **Windowing**: Sliding window over price history
3. **Features**: [price, volume, spread, momentum, ...]
4. **Model Registry**: Store weights + metadata as checkpoints

### Integration Steps
1. Port model definition to Rust (`crates/ml/src/tkan.rs`)
2. Load PyTorch weights via `tch-rs` (`load("model.ot")`)
3. Introduce inference in strategy path behind runtime toggle
4. Profile latency and memory (critical for tick processing)
5. Consider swap to `burn` if performance improves

### T-KAN Signal Output
```rust
pub struct TkanSignal {
    pub direction: f64,    // -1.0 (bearish) to 1.0 (bullish)
    pub confidence: f64,   // 0.0 to 1.0
    pub timestamp_ns: u64,
}
```

---

## Phase 8: Strategy Engine Refactor (Signal + RiskGuard)

### Signal Abstraction
```rust
#[derive(Debug, Clone)]
pub enum Signal {
    Entry {
        market_id: String,
        side: Side,
        size_shares: u64,
        price_constraint: Option<Tick>,
        confidence: f64,
        metadata: SignalMetadata,
    },
    Exit {
        market_id: String,
        position_id: String,
        reason: ExitReason,
    },
    Adjust {
        market_id: String,
        current_size: u64,
        target_size: u64,
    },
}
```

### Risk Guard Processing
Centralized checks before execution:

```rust
pub struct RiskGuard {
    position_limits: PositionLimits,
    exposure_caps: ExposureCaps,
    max_order_size: u64,
    cooldowns: CooldownManager,
}

impl RiskGuard {
    pub fn check(&self, signal: &Signal, state: &TradingState) 
        -> Result<(), RiskRejection> 
    {
        // 1. Check position limits
        // 2. Check exposure caps  
        // 3. Check max order size
        // 4. Check cooldown
        
        // Return structured rejection with reason
    }
}
```

### Execution Flow Update
```
Strategy.on_update() 
    → Vec<Signal>
    → RiskGuard.check() [filter/reject with reasons]
    → ExecutionPlanner [translate to orders]
    → Order placement
    → Tracing IDs tie signals→orders→fills
```

---

## RiskGuard Design (Draft)

```rust
pub struct RiskGuard {
    position_limits: PositionLimits,
    exposure_caps: ExposureCaps,
    cooldowns: CooldownManager,
}

impl RiskGuard {
    pub fn check(&self, signal: &Signal, state: &TradingState) -> Result<(), RiskRejection> {
        // 1. Check position limits
        if !self.position_limits.allow(&signal.size)? {
            return Err(RiskRejection::PositionLimitExceeded);
        }
        
        // 2. Check exposure caps
        if state.total_exposure + signal.size > self.exposure_caps.max {
            return Err(RiskRejection::ExposureLimitExceeded);
        }
        
        // 3. Check cooldown
        if self.cooldowns.is_in_cooldown(signal.instrument)? {
            return Err(RiskRejection::InCooldown);
        }
        
        Ok(())
    }
}
```
