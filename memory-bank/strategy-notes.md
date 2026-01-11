# Strategy Notes

## Strategy entry points

- Trait + context: `crates/strategy/src/traits.rs`
- Maker market making: `crates/strategy/src/maker_mm.rs`
- Bundle arb: `crates/strategy/src/bundle_maker.rs`
- Unaffected arb: `crates/strategy/src/unaffected_arb.rs`
- Combinatorial arb: `crates/strategy/src/combinatorial_arb.rs` ⭐ NEW
- Auto-hedge: `crates/strategy/src/auto_hedge.rs` ⭐ NEW

## Signals

- EMA signal helpers: `crates/strategy/src/signals.rs`
- Flow normalization (Q/M and Q/V): `crates/strategy/src/flow.rs`
- Alpha signals: `crates/research/src/signals.rs` ⭐ NEW

## Strategy Comparison

| Strategy | Timeframe | Type | Alpha Source |
|----------|-----------|------|--------------|
| **maker_mm** | Any | Market making | Spread capture, inventory skew |
| **bundle_maker** | Any | Arb | YES+NO sum != $1 |
| **unaffected_arb** | Any | Arb | Cross-market price divergence |
| **rebalancing_arb** | 15-min | Rebalance | Price dip + mean reversion |
| **combinatorial_arb** | Any | Arb | Inter-market dependency logic |
| **auto_hedge** | 15-min | Dip-buy | Early-round dump capture |

## New Strategy: Combinatorial Arb

Based on arXiv:2508.03474 "Unravelling the Probabilistic Forest"

### Dependency Types
```rust
enum DependencyType {
    /// A implies B - arb when Price(A) > Price(B)
    Implication,
    /// A and B mutually exclusive - arb when Price(A)+Price(B) > 1
    MutuallyExclusive,
    /// A and B identical - arb when Price(A) != Price(B)
    Identical,
}
```

### Usage
```rust
let dep = Dependency {
    source_market_id: "team-a-wins".to_string(),
    source_token_id: "yes".to_string(),
    target_market_id: "team-a-wins-by-2".to_string(),
    target_token_id: "yes".to_string(),
    relation: DependencyType::Implication,
    min_profit_bps: 10,
};

let config = CombinatorialArbConfig {
    dependencies: vec![dep],
    ..Default::default()
};

let strategy = CombinatorialArbStrategy::new(config);
```

## New Strategy: Auto-Hedge

15-min UP/DOWN market dip-buying strategy.

### Flow
1. **Leg1**: Detect sharp price drop (e.g., >15%) in early round
2. **Leg2**: Hedge by buying opposite side when sum < target (e.g., 0.95)
3. **Stop Loss**: Timeout forces exit if Leg2 not reached

### Usage
```rust
let config = AutoHedgeConfig {
    leg_size: 10_000_000,      // 10 shares
    sum_target: 0.95,          // Buy opposite when sum < 0.95
    dip_threshold: 0.15,       // 15% drop triggers Leg1
    dip_window_ms: 3_000,      // Rolling window for dip detection
    window_minutes: 2,         // Only allow Leg1 in first 2 min
    leg2_timeout_seconds: 100, // Force exit if no Leg2
    ..Default::default()
};

let strategy = AutoHedgeStrategy::new(
    "btc-15m-up".to_string(),
    config,
    "btc-15m-up".to_string(),   // up_asset_id
    "btc-15m-down".to_string(), // down_asset_id
);
```

## Alpha Signal Usage

```rust
use mtrader_research::{
    detect_large_trades,
    detect_liquidity_gaps,
    detect_volume_imbalance,
    generate_composite_alpha,
};

// Detect smart money activity
let large_trades = detect_large_trades(&trades, "btc-15m");

// Find wide-spread opportunities
let liquidity_gaps = detect_liquidity_gaps(&markets);

// Get composite signal
let alpha = generate_composite_alpha(&market, &trades, &liquidity_gaps);

if alpha.overall_score > 0.2 {
    println!("Strong buy signal: {:.1}% confidence", alpha.overall_score * 100.0);
}
```

## Integration checklist

When adding a new strategy or signal:
1. Implement in `crates/strategy/src/`
2. Export from `crates/strategy/src/lib.rs`
3. Wire into CLI strategy selection (`crates/cli/src/commands/paper.rs`)
4. Add unit tests in the same module
