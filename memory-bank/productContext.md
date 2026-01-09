# Product Context

## Why this project exists
Traders and researchers need a reliable, modular system to connect to Polymarket markets, process live order book data, and run strategies safely. `many-lamps` centralizes these capabilities in a single Rust workspace with strong typing and performance.

## Problems it solves
- Integrates live market data ingestion (WebSocket + REST metadata) with book updates and event flows.
- Provides reusable strategy modules and signals with clear entry points.
- Enforces execution safety (risk limits, safe mode) for paper trading and controlled live testing.
- Enables replay/backtest workflows with recording support.

## How it should work
1. CLI selects an operational mode (paper, replay, etc.).
2. Gateway streams live data into the order book and core events.
3. Strategies evaluate context, emit actions, and flow through risk + execution layers.
4. Optional recording stores raw data for offline replay.

## User experience goals
- Clear CLI workflows for paper trading and replay.
- Predictable, safe-by-default execution.
- Easy onboarding via structured documentation and memory bank references.
