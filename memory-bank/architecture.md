# Architecture

## Crate map

| Crate | Purpose | Notes |
| --- | --- | --- |
| `crates/core` | Types, events, fees, health, clock | Core event types + units live here. |
| `crates/book` | ArrayBook order book | O(1) updates, tick validation. |
| `crates/gateway` | Polymarket WS/REST clients | WS parse + REST metadata. |
| `crates/execution` | Order lifecycle | Cancel-before-cross flow. |
| `crates/risk` | Limits + PnL | SAFE_MODE gates. |
| `crates/strategy` | Strategy logic | MakerMM + bundle strategies + signals. |
| `crates/sim` | Paper trading + replay | Fill simulator + paper book. |
| `crates/recorder` | Raw/parquet recording | Recording hooks for replay. |
| `crates/cli` | CLI entrypoint | `mtrader` binary. |

## Data flow (happy path)

```
Gateway (WS/REST)
  → Book updates
  → Core events
  → Strategy context
  → Strategy actions
  → Execution + Risk
  → Recorder (optional)
```

## Key entry points

- CLI routing: `crates/cli/src/main.rs`
- Paper trading loop: `crates/cli/src/commands/paper.rs`
- Strategy trait + context: `crates/strategy/src/traits.rs`
- Maker MM strategy: `crates/strategy/src/maker_mm.rs`
- Core events: `crates/core/src/events.rs`
