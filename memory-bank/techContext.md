# Tech Context

## Technologies
- Rust workspace (edition 2024).
- Tokio async runtime.
- WebSocket via tokio-tungstenite.
- HTTP via reqwest.
- Serialization via serde.
- Data storage via parquet/arrow.

## Development setup
- Workspace root: `/workspace/many-lamps`.
- Build/test with Cargo.

## Technical constraints
- Live market validation depends on WebSocket connectivity; may be blocked in restricted environments.
- Safe mode prevents real order placement during paper trading.

## Dependencies
See `Cargo.toml` workspace dependencies for shared crates and versions.

## Tool usage patterns
- `cargo test ./...` for full workspace tests.
- `cargo build --bin mtrader` for CLI build.
- `cargo run --bin mtrader -- paper --market <TOKEN_ID> --strategy maker_mm` for live feed validation.
