# Strategy Notes

## Strategy entry points

- Trait + context: `crates/strategy/src/traits.rs`
- Maker market making: `crates/strategy/src/maker_mm.rs`
- Bundle arb: `crates/strategy/src/bundle_maker.rs`
- Unaffected arb: `crates/strategy/src/unaffected_arb.rs`

## Signals

- EMA signal helpers: `crates/strategy/src/signals.rs`
- Flow normalization (Q/M and Q/V): `crates/strategy/src/flow.rs`

## Integration checklist

When adding a new strategy or signal:
1. Implement in `crates/strategy/src/`.
2. Export from `crates/strategy/src/lib.rs`.
3. Wire into CLI strategy selection (`crates/cli/src/commands/paper.rs`).
4. Add unit tests in the same module.
