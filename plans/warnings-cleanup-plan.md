# Production Readiness - Warnings Cleanup Plan (M-001)

## Status Update

### C-001: Tracing Subscriber Panic ✅ RESOLVED
- The `OnceLock` implementation in [`crates/cli/src/logging.rs`](many-lamps/crates/cli/src/logging.rs:8) is working correctly
- TUI launches without panic
- No code changes needed for C-001

### M-001: Compiler Warnings Cleanup 🔴 IN PROGRESS
- **100+ warnings** across multiple crates
- Need to clean up before production release

---

## Warnings by Crate

| Crate | Warning Count | Types |
|-------|---------------|-------|
| `many-lamps-core` | 9 | unused variables, dead code, ambiguous glob re-exports |
| `mtrader-ml` | 4 | unused fields, unused functions, unused constants |
| `mtrader-strategy` | 19 | unused imports, unused variables, dead code |
| `many-lamps-gateway` | 3 | unused fields, ambiguous glob re-exports |
| `mtrader-sim` | 18 | unused imports, deprecated methods, unused variables, dead code |
| `mtrader-dashboard` | 4 | unused constants, unused functions |
| `mtrader-cli` | 5 | unused imports, unused variables, unused function |

---

## Cleanup Strategy

### Phase 1: Auto-fix with `cargo fix`

```bash
# Apply auto-fixable suggestions
cargo fix --lib -p mtrader-core
cargo fix --lib -p mtrader-strategy
cargo fix --lib -p mtrader-sim
cargo fix --bin "mtrader" -p mtrader-cli
```

### Phase 2: Manual Fixes Required

#### 1. Ambiguous Glob Re-exports (Core)
**File**: [`crates/core/src/lib.rs`](many-lamps/crates/core/src/lib.rs:19)
```rust
pub use data_contracts::*;
pub use events::*;
```
Both export `SignalType` - need to explicitly re-export or rename one.

#### 2. Ambiguous Glob Re-exports (Gateway)
**File**: [`crates/gateway/src/lib.rs`](many-lamps/crates/gateway/src/lib.rs:18)
```rust
pub use messages::*;
pub use polymarket::*;
```
Both export `PriceLevel` - need resolution.

#### 3. Unused Fields in Core Health Monitor
**File**: [`crates/core/src/health_monitor.rs`](many-lamps/crates/core/src/health_monitor.rs)
- `consecutive_latency_violations` - marked #[allow(dead_code)] or remove
- `expected_fill_by` - same
- `new_health` variable - prefix with `_` or use

#### 4. Unused Fields in ML Attention
**File**: [`crates/ml/src/attention.rs`](many-lamps/crates/ml/src/attention.rs)
- `d_model` in `MultiHeadAttention`
- `feature_dim` in `CrossAssetAttention`
- `dot_product` function

#### 5. Unused Fields in Strategy
**File**: [`crates/strategy/src/bundle_maker.rs`](many-lamps/crates/strategy/src/bundle_maker.rs)
- `yes_asset_id`, `no_asset_id` fields
- `calculate_fee`, `check_arb_opportunity`, `enter_arb` methods

#### 6. Unused Fields in ML Strategy
**File**: [`crates/strategy/src/ml_strategy.rs`](many-lamps/crates/strategy/src/ml_strategy.rs)
- `TradeRecord` fields - currently dead code for performance tracking
- Add `#[allow(dead_code)]` or implement the pending trades tracking

#### 7. Unused Methods in Sim
**File**: [`crates/sim/src/realistic_fills.rs`](many-lamps/crates/sim/src/realistic_fills.rs)
- Deprecated `remove` method - use `swap_remove` or `shift_remove`

#### 8. Unused Function in Logging
**File**: [`crates/cli/src/logging.rs`](many-lamps/crates/cli/src/logging.rs)
- `is_initialized()` function not used - remove or add usage

---

## Implementation Order

| Step | Action | Files |
|------|--------|-------|
| 1 | Run `cargo fix` for auto-fixable warnings | All crates |
| 2 | Fix ambiguous glob re-exports in core | `crates/core/src/lib.rs` |
| 3 | Fix ambiguous glob re-exports in gateway | `crates/gateway/src/lib.rs` |
| 4 | Add `#[allow(dead_code)]` to intentionally unused code | Multiple files |
| 5 | Remove unused imports and variables | Multiple files |
| 6 | Verify with `cargo build --workspace` | All |

---

## ML Module Architecture (Future Enhancement)

The ML crate is already structured for pluggability:

```
crates/ml/src/
├── lib.rs              # Main exports, TkanModel, FeatureVector
├── attention.rs        # CrossAssetAttention - pluggable attention
├── margin_softmax.rs   # DirectionalSignal - pluggable classifier  
├── uncertainty.rs      # UncertaintyEstimator - pluggable uncertainty
├── meta_learning.rs    # MAML adapter - pluggable meta-learning
├── feature_pipeline.rs # FeatureExtractor - pluggable features
└── inference.rs        # InferenceEngine - pluggable inference
```

### Adding Stock Data Support (Future)
1. Create `crates/data_sources/src/stock/csv.rs`
2. Create trait `DataSource` in `crates/ml/src/traits.rs`
3. Implement `StockDataSource` that converts CSV to `FeatureVector`
4. Add to `crates/ml/src/lib.rs` re-exports

---

## ML Tests Verification

After warnings cleanup, verify ML tests pass:

```bash
# Run ML crate tests
cargo test -p mtrader-ml

# Run all tests
cargo test

# Verify test count
cargo test --quiet | grep "test result:" | tail -1
```

### ML Tests to Verify
1. `test_feature_buffer` - Feature extraction works
2. `test_tkan_signal` - Signal creation and validation
3. `test_ml_model_inference` - Model inference with dummy weights
4. `test_ml_model_dummy` - Dummy model fallback
5. `test_silu_activation` - Activation function correctness
6. `test_linear_layer` - Linear layer computation

### ML Strategy Tests
1. `test_ml_strategy_creation` - Strategy initializes correctly
2. `test_ml_strategy_without_ml_model` - Falls back gracefully
3. `test_strategy_performance_tracking` - Performance metrics work
4. `test_confidence_multiplier` - Confidence scaling logic

---

## Verification

After cleanup:
```bash
cargo build --workspace 2>&1 | grep -E "^warning" | wc -l
# Should return 0
```
