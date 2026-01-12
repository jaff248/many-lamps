# Tech Context

## Technologies

### Core Stack
- **Rust workspace** (edition 2024)
- **Tokio async runtime** - Async I/O and task scheduling
- **WebSocket** via tokio-tungstenite - Real-time market data
- **HTTP** via reqwest - REST API for metadata and orders
- **Serialization** via serde - JSON/MessagePack support
- **Data storage** via parquet/arrow - Columnar time-series storage

### ML/AI Stack
- **tch-rs** - PyTorch bindings for T-KAN model
- **ndarray** - N-dimensional array operations
- **rayon** - Parallel data processing

### Streaming & Messaging
- **NATS JetStream** - Selected for streaming pipeline
  - At-least-once delivery guarantees
  - Built-in persistence and WAL
  - Low-latency pub/sub (<1ms)
  - Consumer groups for parallel processing

### Data Processing
- **Polars** - DataFrame operations (for backtesting)
- **Arrow** - In-memory columnar format

### TUI
- **ratatui** - Terminal UI framework
- **crossterm** - Cross-platform terminal handling

## Development Setup

### Workspace Structure
- **Root**: `/Users/hadijaffery/Development/mtrader/many-lamps`
- **Build/Test**: `cargo build --workspace`, `cargo test --workspace`
- **Run CLI**: `cargo run --bin mtrader -- <command>`

### Build Commands
```bash
# Build all crates
cargo build --workspace

# Build specific binary
cargo build --bin mtrader

# Run tests
cargo test --workspace

# Run tests with output
cargo test --workspace -- --nocapture

# Run specific test
cargo test --package <crate> <test_name>
```

## Technical Constraints

### Environment
- Live market validation depends on WebSocket connectivity
- May be blocked in restricted environments
- Safe mode prevents real order placement during paper trading

### Performance Targets
- **Total pipeline latency**: <10ms P99
- **Feature extraction**: <500μs P99
- **ML inference**: <1ms P99
- **Risk check**: <1ms P99
- **Throughput**: >100 messages/second

## Crate Dependencies

### Workspace-Level Dependencies
```toml
[workspace.dependencies]
tokio = { version = "1.40", features = ["full"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
anyhow = "1.0"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
```

### Core Crate Dependencies
```toml
[dependencies]
tokio = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
anyhow = { workspace = true }
tracing = { workspace = true }
```

## Crate Module Structure

### `crates/core` - Core Types and Events
- **[`data_contracts.rs`](../crates/core/src/data_contracts.rs)**: NormalizedMarketData, RiskCheckRequest/Response, ExecutionRequest/Response
- **[`events.rs`](../crates/core/src/events.rs)**: Core event types (BookUpdate, Fill, Trade, etc.)
- **[`health_monitor.rs`](../crates/core/src/health_monitor.rs)**: HealthMonitor, LatencyTracker, ErrorTracker
- **[`health.rs`](../crates/core/src/health.rs)**: Health status types and metrics
- **[`fees.rs`](../crates/core/src/fees.rs)**: Fee models and transaction costs
- **[`clock.rs`](../crates/core/src/clock.rs)**: Monotonic clock implementation
- **[`types.rs`](../crates/core/src/types.rs)**: Core type definitions (MarketId, TokenId, Tick, Size, etc.)
- **[`lib.rs`](../crates/core/src/lib.rs)**: Public exports

### `crates/gateway` - Exchange Connectivity
- **[`polymarket.rs`](../crates/gateway/src/polymarket.rs)**: Polymarket-specific implementation
- **[`ws_client.rs`](../crates/gateway/src/ws_client.rs)**: WebSocket client
- **[`rest_client.rs`](../crates/gateway/src/rest_client.rs)**: REST API client
- **[`parser.rs`](../crates/gateway/src/parser.rs)**: Message parsing
- **[`messages.rs`](../crates/gateway/src/messages.rs)**: Message types
- **[`error.rs`](../crates/gateway/src/error.rs)**: Gateway errors

### `crates/execution` - Order Execution
- **[`order.rs`](../crates/execution/src/order.rs)**: Order lifecycle management
- **[`state_manager.rs`](../crates/execution/src/state_manager.rs)**: Order state tracking
- **[`self_trade_guard.rs`](../crates/execution/src/self_trade_guard.rs)**: Self-trade prevention
- **[`error.rs`](../crates/execution/src/error.rs)**: Execution errors
- **[`lib.rs`](../crates/execution/src/lib.rs)**: Public exports

### `crates/risk` - Risk Management
- **[`limits.rs`](../crates/risk/src/limits.rs)**: Position limits and risk checks
- **[`circuit_breaker.rs`](../crates/risk/src/circuit_breaker.rs)**: Circuit breaker implementation
- **[`pnl.rs`](../crates/risk/src/pnl.rs)**: P&L calculation and tracking
- **[`position.rs`](../crates/risk/src/position.rs)**: Position management
- **[`reconciliation.rs`](../crates/risk/src/reconciliation.rs)**: Position reconciliation with exchange
- **[`lib.rs`](../crates/risk/src/lib.rs)**: Public exports

### `crates/strategy` - Trading Strategies
- **[`maker_mm.rs`](../crates/strategy/src/maker_mm.rs)**: Market maker strategy
- **[`bundle_maker.rs`](../crates/strategy/src/bundle_maker.rs)**: Bundle arbitrage
- **[`unaffected_arb.rs`](../crates/strategy/src/unaffected_arb.rs)**: Unaffected outcome arbitrage
- **[`rebalancing_arb.rs`](../crates/strategy/src/rebalancing_arb.rs)**: Rebalancing arbitrage
- **[`auto_hedge.rs`](../crates/strategy/src/auto_hedge.rs)**: Auto-hedge strategy
- **[`ensemble.rs`](../crates/strategy/src/ensemble.rs)**: Strategy ensemble
- **[`regime_detector.rs`](../crates/strategy/src/regime_detector.rs)**: Market regime detection
- **[`signals.rs`](../crates/strategy/src/signals.rs)**: Signal generation
- **[`ml_strategy.rs`](../crates/strategy/src/ml_strategy.rs)**: ML-based strategy
- **[`traits.rs`](../crates/strategy/src/traits.rs)**: Strategy trait definitions
- **[`flow.rs`](../crates/strategy/src/flow.rs)**: Strategy execution flow
- **[`lib.rs`](../crates/strategy/src/lib.rs)**: Public exports
- **[`combinatorial_arb/`](../crates/strategy/src/combinatorial_arb/)**: Combinatorial arbitrage module
  - **[`mod.rs`](../crates/strategy/src/combinatorial_arb/mod.rs)**: Module exports
  - **[`config.rs`](../crates/strategy/src/combinatorial_arb/config.rs)**: Configuration
  - **[`dependency.rs`](../crates/strategy/src/combinatorial_arb/dependency.rs)**: Dependency graph
  - **[`price.rs`](../crates/strategy/src/combinatorial_arb/price.rs)**: Price calculations
  - **[`strategy.rs`](../crates/strategy/src/combinatorial_arb/strategy.rs)**: Strategy implementation
  - **[`tests.rs`](../crates/strategy/src/combinatorial_arb/tests.rs)**: Tests

### `crates/sim` - Simulation & Backtesting
- **[`event_driven_backtest.rs`](../crates/sim/src/event_driven_backtest.rs)**: Event-driven backtesting
- **[`recorded_backtest.rs`](../crates/sim/src/recorded_backtest.rs)**: Recorded data backtesting
- **[`fill_sim.rs`](../crates/sim/src/fill_sim.rs)**: Fill simulation
- **[`realistic_fills.rs`](../crates/sim/src/realistic_fills.rs)**: Realistic fill modeling
- **[`paper_book.rs`](../crates/sim/src/paper_book.rs)**: Paper trading book
- **[`walk_forward.rs`](../crates/sim/src/walk_forward.rs)**: Walk-forward optimization
- **[`replay.rs`](../crates/sim/src/replay.rs)**: Data replay
- **[`lib.rs`](../crates/sim/src/lib.rs)**: Public exports

### `crates/data_pipeline` - Data Infrastructure
- **[`storage.rs`](../crates/data_pipeline/src/storage.rs)**: Time-series storage with Parquet
- **[`feature_store.rs`](../crates/data_pipeline/src/feature_store.rs)**: Feature store with caching
- **[`quality.rs`](../crates/data_pipeline/src/quality.rs)**: Data quality monitoring
- **[`lib.rs`](../crates/data_pipeline/src/lib.rs)**: Public exports

### `crates/ml` - Machine Learning
- **[`inference.rs`](../crates/ml/src/inference.rs)**: ML inference engine
- **[`attention.rs`](../crates/ml/src/attention.rs)**: Multi-head attention and regime classification
- **[`margin_softmax.rs`](../crates/ml/src/margin_softmax.rs)**: Large-margin softmax
- **[`uncertainty.rs`](../crates/ml/src/uncertainty.rs)**: Uncertainty quantification
- **[`meta_learning.rs`](../crates/ml/src/meta_learning.rs)**: Meta-learning for rapid adaptation
- **[`feature_pipeline.rs`](../crates/ml/src/feature_pipeline.rs)**: Feature extraction pipeline
- **[`lib.rs`](../crates/ml/src/lib.rs)**: Public exports

### `crates/recorder` - Data Recording
- **[`event_recorder.rs`](../crates/recorder/src/event_recorder.rs)**: Event recording
- **[`frame_recorder.rs`](../crates/recorder/src/frame_recorder.rs)**: Frame recording
- **[`parquet_writer.rs`](../crates/recorder/src/parquet_writer.rs)**: Parquet writing
- **[`error.rs`](../crates/recorder/src/error.rs)**: Recording errors
- **[`lib.rs`](../crates/recorder/src/lib.rs)**: Public exports

### `crates/dashboard` - Terminal UI
- **[`app.rs`](../crates/dashboard/src/app.rs)**: Main TUI application
- **[`screens.rs`](../crates/dashboard/src/screens.rs)**: Screen implementations
- **[`lib.rs`](../crates/dashboard/src/lib.rs)**: Public exports

### `crates/cli` - Command Line Interface
- **[`main.rs`](../crates/cli/src/main.rs)**: CLI entry point
- **[`config.rs`](../crates/cli/src/config.rs)**: Configuration management
- **[`logging.rs`](../crates/cli/src/logging.rs)**: Logging setup
- **[`fee_profile.rs`](../crates/cli/src/fee_profile.rs)**: Fee profiles
- **[`commands/`](../crates/cli/src/commands/)**: CLI commands
  - **[`mod.rs`](../crates/cli/src/commands/mod.rs)**: Command exports
  - **[`paper.rs`](../crates/cli/src/commands/paper.rs)**: Paper trading
  - **[`record.rs`](../crates/cli/src/commands/record.rs)**: Data recording
  - **[`replay.rs`](../crates/cli/src/commands/replay.rs)**: Data replay
  - **[`backtest.rs`](../crates/cli/src/commands/backtest.rs)**: Backtesting
  - **[`market.rs`](../crates/cli/src/commands/market.rs)**: Market queries
  - **[`status.rs`](../crates/cli/src/commands/status.rs)**: System status
  - **[`tui.rs`](../crates/cli/src/commands/tui.rs)**: TUI launcher

### `crates/book` - Order Book
- **[`array_book.rs`](../crates/book/src/array_book.rs)**: Array-based order book
- **[`sync.rs`](../crates/book/src/sync.rs)**: Synchronization utilities
- **[`error.rs`](../crates/book/src/error.rs)**: Book errors
- **[`lib.rs`](../crates/book/src/lib.rs)**: Public exports

### `crates/research` - Research Module
- **[`signals.rs`](../crates/research/src/signals.rs)**: Alpha signal research
- **[`lib.rs`](../crates/research/src/lib.rs)**: Public exports

## NATS JetStream Decision

### Selection Rationale
NATS JetStream was selected for the streaming pipeline (Phase 5) based on:

1. **At-least-once delivery**: Guaranteed message delivery with acknowledgments
2. **Built-in persistence**: Write-ahead log (WAL) included, no external storage needed
3. **Low latency**: Sub-millisecond publish/subscribe performance
4. **Consumer groups**: Support for multiple consumers with load balancing
5. **Message replay**: Easy recovery from failures by replaying from WAL
6. **Simple deployment**: Single binary, minimal configuration

### Integration Points
- **Streaming Pipeline** ([`crates/data_pipeline/src/lib.rs`](../crates/data_pipeline/src/lib.rs))
  - Event publishing to JetStream streams
  - Consumer groups for parallel processing
  - Durable subscriptions for reliability

- **Health Monitoring** ([`crates/core/src/health_monitor.rs`](../crates/core/src/health_monitor.rs))
  - JetStream connection health tracking
  - Message backlog monitoring
  - Consumer lag alerts

### Configuration
```toml
[streaming]
nats_url = "nats://localhost:4222"
stream_name = "mtrader_events"
consumer_group = "strategy_engine"
ack_timeout_ms = 5000
max_pending_acks = 1000
```

## Tool Usage Patterns

### Testing
```bash
# Run all tests
cargo test --workspace

# Run specific crate tests
cargo test --package core

# Run tests with output
cargo test --workspace -- --nocapture --test-threads=1
```

### Building
```bash
# Build all crates
cargo build --workspace

# Build release
cargo build --workspace --release

# Build specific binary
cargo build --bin mtrader
```

### Running
```bash
# Launch TUI
cargo run --bin mtrader -- tui

# Paper trading
cargo run --bin mtrader -- paper --market <TOKEN_ID> --strategy maker_mm

# Record market data
cargo run --bin mtrader -- record --market <TOKEN_ID> --output ./data/recordings/

# Replay recorded data
cargo run --bin mtrader -- replay --input ./data/recordings/ --strategy maker_mm

# Backtest
cargo run --bin mtrader -- backtest --strategy maker_mm --data ./data/recordings/
```

## Configuration

### Main Config (`config.toml`)
```toml
[trading]
safe_mode = true
max_position = 1000000
max_order_size = 100000

[risk]
max_drawdown_pct = 10.0
consecutive_loss_limit = 5
daily_loss_limit_usd = 10000

[logging]
level = "info"
```

### Environment Variables
- `RUST_LOG`: Logging level (e.g., `RUST_LOG=debug`)
- `MTRADER_CONFIG`: Path to config file
- `POLYMARKET_API_KEY`: Polymarket API key (for live trading)

## Performance Optimization

### Build Optimizations
```bash
# Release build with LTO
cargo build --workspace --release --lto

# Profile-guided optimization
cargo build --workspace --release --profile pgo
```

### Runtime Optimizations
- Zero-copy message parsing where possible
- LRU caches for hot data (features, inference results)
- Parallel processing with rayon for batch operations
- Async I/O with tokio for network operations

## Monitoring & Observability

### Metrics
- Prometheus-compatible metrics via [`crates/core/src/health.rs`](../crates/core/src/health.rs)
- Latency histograms for each pipeline stage
- Throughput counters for messages, signals, orders
- Quality metrics for signals and fills

### Logging
- Structured logging with tracing
- Log levels: error, warn, info, debug, trace
- Context propagation across async tasks

### Health Checks
- Component health status via [`crates/core/src/health_monitor.rs`](../crates/core/src/health_monitor.rs)
- Automatic degradation on high latency/error rates
- Circuit breaker integration for critical failures
