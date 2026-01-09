# Project Brief

## Overview
`many-lamps` is a Rust workspace that provides an end-to-end trading system focused on Polymarket markets. It includes gateway connectivity, order book management, strategy logic, execution, risk controls, simulation, recording, and a CLI entrypoint.

## Goals
- Provide reliable live and paper trading workflows powered by Polymarket WebSocket/REST data.
- Offer modular strategy implementations (e.g., maker market making, bundle arb) with clean integration points.
- Maintain safe-by-default execution with risk limits and safe mode protections.
- Support replay/backtest workflows from recorded market data.

## Non-Goals
- No GUI frontend is included; the CLI is the primary interface.
- Live trading is not the default; safety gating is prioritized.

## Scope of Work
- Rust workspace spanning multiple crates (core, gateway, book, strategy, execution, risk, sim, recorder, cli).
- Documentation and test workflows captured in the memory bank files.
