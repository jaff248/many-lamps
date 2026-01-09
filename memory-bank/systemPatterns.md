# System Patterns

## Architecture
- Multi-crate Rust workspace with separated concerns (core types, book, gateway, strategy, execution, risk, sim, recorder, cli).
- Event-driven flow: gateway → book/core events → strategy context → strategy actions → risk/execution → recorder.

## Key technical decisions
- Use Rust 2024 edition with tokio async runtime.
- Maintain safe mode gating for paper trading/live feeds.
- Favor modular strategy implementations wired through CLI selection.

## Design patterns
- Strategy trait + context objects for pluggable strategy behavior.
- Separation of simulation/replay from live execution.
- Recording hooks for deterministic replay.

## Component relationships
- `crates/cli` orchestrates modes and wires strategies.
- `crates/gateway` provides live data to `crates/book` and `crates/core` events.
- `crates/strategy` emits actions consumed by `crates/execution` with `crates/risk` checks.
- `crates/recorder` stores raw data for `crates/sim` replay.

## Critical paths
- Live paper trading loop in CLI → gateway feed → strategy → execution/risk.
- Replay flow from recordings into simulation with strategy evaluation.
