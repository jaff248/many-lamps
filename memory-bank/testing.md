# Testing & Validation

## Required baseline checks

Run the full workspace tests:

```bash
cargo test ./...
```

Build the CLI binary:

```bash
cargo build --bin mtrader
```

## Live market data validation

Paper trading uses the live WebSocket feed and must be exercised when possible.

```bash
# Substitute a real Polymarket token/asset id
cargo run --bin mtrader -- paper --market <TOKEN_ID> --strategy maker_mm
```

Notes:
- This is safe mode only; no real orders are sent.
- If the environment blocks outbound WebSocket access, record the failure and
  proceed with other tests.

## Replay/backtest

If you have recordings:

```bash
cargo run --bin mtrader -- replay --input data/recordings --strategy maker_mm --report backtest.json
```
