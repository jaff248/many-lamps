# Alpha Research Report: Polymarket Trading Systems

This report summarizes key architectural patterns and "alpha" discovered from analyzing open-source Rust-based trading systems for Polymarket and general HFT.

## 1. High-Performance Order Book (`polysqueeze`)

**Source:** `polysqueeze/src/book.rs`

The most significant performance optimization found is the use of fixed-point arithmetic for the order book instead of `rust_decimal::Decimal`.

### Key Optimizations:
- **Internal Representation**: Uses `u64` (or similar integer types) for `Price` and `Qty` internally.
- **Fast Path**: Implements `apply_delta_fast` that bypasses Decimal conversion in the hot path.
- **Performance Gain**: ~10-50x speedup for order book updates compared to Decimal-based implementations.
- **hashing**: Hashes `token_id` once to avoid repeated string comparisons during updates.

**Recommendation for `many-lamps`:**
- Refactor `mtrader-book` to use integer-based fixed-point representation internally.
- Implement a "fast path" for WebSocket updates that parses directly to integers.

## 2. Robust Market Depth Handling (`hftbacktest`)

**Source:** `hftbacktest/src/depth/hashmapmarketdepth.rs`

`hftbacktest` provides a robust `HashMapMarketDepth` implementation that handles L2/L3 data streams effectively.

### Key Features:
- **Robustness**: Handles missing depth delete events gracefully (unlike `BTreeMap` which might retain stale levels).
- **L3 Support**: Explicit support for Level 3 (Market-By-Order) data, tracking individual orders alongside price levels.
- **Snapshot Application**: Clean trait `ApplySnapshot` for initializing state from snapshots.

**Recommendation for `many-lamps`:**
- Evaluate switching `ArrayBook` to a `HashMap`-based implementation if L2 feed reliability is an issue.
- Adopt the `ApplySnapshot` pattern for cleaner state initialization in backtests.

## 3. Modular Strategy Engine (`clobster`)

**Source:** `clobster/src/strategy/engine.rs`, `context.rs`

`clobster` implements a highly modular strategy framework that separates concerns effectively.

### Architecture:
- **Strategy Trait**: Defines a clear interface (`evaluate(ctx) -> Vec<Signal>`).
- **Strategy Context**: Provides a rich, read-only snapshot of market state (`MarketSnapshot`, `PositionSnapshot`, `OrderSnapshot`).
- **Signal Abstraction**: Strategies emit `Signal` objects (Entry, Exit) rather than raw orders.
- **Risk Guard**: Centralized risk checks applied to all signals before execution.
- **Engine**: Manages the lifecycle, execution loop, and signal processing for multiple strategies running concurrently.

**Recommendation for `many-lamps`:**
- Adopt the `Signal` abstraction to decouple strategy logic from execution details.
- Enhance `StrategyContext` to provide richer data (positions, open orders, history) to strategies.
- Implement a `RiskGuard` middleware layer for centralized safety checks.

## 4. Cross-Platform Execution (`Polymarket-Kalshi-Arbitrage-bot`)

**Source:** `Polymarket-Kalshi-Arbitrage-bot/src/execution.rs`

This bot demonstrates advanced execution patterns for arbitrage.

### Patterns:
- **Concurrent Execution**: Uses `tokio::join!` to execute legs on different platforms simultaneously.
- **Auto-Closing**: Background tasks automatically close excess inventory if one leg fails or partially fills.
- **Circuit Breaker**: Integrated circuit breaker to halt trading on consecutive failures or drawdown.

**Recommendation for `many-lamps`:**
- If multi-venue trading is planned, adopt the `tokio::join!` pattern for atomic-like execution.
- Implement the "auto-close excess" logic for safer arbitrage execution.

## 5. Dynamic Data Scheduling (`polymarket-hft`)

**Source:** `polymarket-hft/src/scheduler.rs`

Implements a dynamic job scheduler using `tokio-cron-scheduler`.

### Patterns:
- **Runtime Scheduling**: Allows adding/removing data ingestion jobs (e.g., fetching external signals like "Fear & Greed") without restarting the bot.
- **Job Handle**: Uses a `SchedulerHandle` to manage lifecycle safely across threads.

**Recommendation for `many-lamps`:**
- Implement a similar scheduler if the strategy requires periodic external data (e.g., hourly funding rates, sentiment analysis) that goes beyond WebSocket streams.

## 6. Smart Wallet Support (`rs-clob-client`)

**Source:** `rs-clob-client/src/lib.rs`

Contains robust logic for deriving smart contract wallet addresses.

### Patterns:
- **CREATE2 Derivation**: correctly derives Proxy (Magic/email) and Safe (Gnosis) wallet addresses from an EOA address.
- **Chain Awareness**: Handles config differences between Polygon Mainnet and Amoy Testnet.

**Recommendation for `many-lamps`:**
- Port the `derive_proxy_wallet` and `derive_safe_wallet` logic to support users who trade via smart contract wallets (common on Polymarket).

## 7. Summary of Alpha

| Feature | Source Repo | Impact |
|---------|-------------|--------|
| **Fixed-Point Book** | `polysqueeze` | **High**: Critical for low-latency tick processing. |
| **Strategy Signals** | `clobster` | **Medium**: Improves code modularity and testing. |
| **Risk Guard** | `clobster` | **High**: Essential for safe automated trading. |
| **Robust Depth** | `hftbacktest` | **Medium**: Improves data integrity handling. |
| **Auto-Hedge** | `Kalshi-Arb` | **High**: Reduces leg risk in arbitrage strategies. |
| **Wallet Derivation** | `rs-clob-client` | **Medium**: Enables support for Proxy/Safe wallets. |
| **Job Scheduler** | `polymarket-hft` | **Low/Medium**: Useful for periodic signal data. |

## Implementation Plan for `many-lamps`

1.  **Phase 1 (Core)**: Refactor `mtrader-book` to use `u64` fixed-point arithmetic.
2.  **Phase 2 (Safety)**: Implement `RiskGuard` and integrate it into the execution path.
3.  **Phase 3 (Strategy)**: Refactor `Strategy` trait to return `Vec<Signal>` and provide richer `StrategyContext`.
4.  **Phase 4 (Execution)**: Add auto-hedging logic for partial fills in arbitrage strategies.
5.  **Phase 5 (Utilities)**: Add wallet derivation tools and optional job scheduler.
