# MTrader Priority Fix List - Production Readiness

## Priority Classification Summary

| Priority | Count | Go/No-Go |
|----------|-------|----------|
| 🔴 Critical | 1 | NO-GO until fixed |
| 🟠 High | 0 | - |
| 🟡 Medium | 2 | Should fix before release |
| 🟢 Nice to Have | 3 | Can defer |

---

## 🔴 CRITICAL PRIORITY

### C-001: TUI Runtime Panic - Duplicate Tracing Subscriber

| Property | Value |
|----------|-------|
| **Issue** | `failed to set global default subscriber: SetGlobalDefaultError("a global default trace dispatcher has already been set")` |
| **Severity** | Prevents application from launching |
| **Affected Command** | `cargo run --bin mtrader tui` |
| **Root Cause** | Tracing subscriber initialized in both CLI and Dashboard modules |
| **Files Involved** | crates/cli/src/logging.rs, crates/cli/src/main.rs, crates/dashboard/src/lib.rs |

### Remediation Steps

1. **Add subscriber guard** in logging initialization:
```rust
// Check if subscriber already set before initializing
use std::sync::OnceLock;
static TRACING_INIT: OnceLock<()> = OnceLock::new();

pub fn init(level: &str, json: bool) -> Result<()> {
    TRACING_INIT.get_or_try_init(|| {
        // existing init code
        Ok(())
    })?;
    Ok(())
}
```

2. **Alternative**: Make Dashboard skip logging init if already initialized

3. **Test**: Run `cargo run --bin mtrader tui` and verify no panic

---

## 🟡 MEDIUM PRIORITY

### M-001: Compiler Warnings Cleanup (100+ warnings)

| Property | Value |
|----------|-------|
| **Issue** | 100+ warnings across multiple crates |
| **Types** | Unused imports, unused variables, dead code, ambiguous glob re-exports, unused mut |

### Remediation Steps

1. **Run compilation with warnings**:
```bash
cargo build --workspace 2>&1 | grep -E "^warning"
```

2. **Fix by crate**:
   - data_pipeline: Remove unused imports
   - strategy: Remove dead code or add `#[allow(dead_code)]`
   - risk: Fix unused variables
   - Other crates: Address specific warnings

3. **Test**: `cargo build --workspace` should produce zero warnings

---

### M-002: NATS JetStream Integration Verification

| Property | Value |
|----------|-------|
| **Issue** | Documentation claims NATS JetStream is integrated but may not be fully functional |
| **Severity** | Documentation drift, potential feature gap |
| **Files** | crates/data_pipeline/src/lib.rs |

### Remediation Steps

1. **Verify NATS usage**:
   - Check if NATS is actually used in the streaming pipeline
   - If not implemented, either implement it or remove from docs

2. **Options**:
   - **Option A**: Implement NATS JetStream integration
   - **Option B**: Remove NATS claims from documentation

3. **Update docs** if NATS is not being used:
   - Edit `memory-bank/architecture.md` - Remove NATS section
   - Edit `memory-bank/techContext.md` - Remove NATS section
   - Edit `memory-bank/progress.md` - Update streaming claims

---

## 🟢 NICE TO HAVE

### N-001: ML Model File Documentation

| Property | Value |
|----------|-------|
| **Issue** | ML strategy uses dummy model when `models/tkan_model.ot` is missing |
| **Recommendation** | Document this behavior or provide default model |

### N-002: Prometheus Metrics Endpoint Verification

| Property | Value |
|----------|-------|
| **Issue** | Metrics structs exist but endpoint exposure not verified |
| **Recommendation** | Verify metrics are properly exposed for Prometheus scraping |

### N-003: Test Count Documentation Update

| Property | Value |
|----------|-------|
| **Issue** | Documentation claims "161+ tests" but actual is 172+ |
| **Recommendation** | Update progress.md to reflect 172+ tests |

---

## Timeline Estimate

| Phase | Tasks | Order |
|-------|-------|-------|
| **Phase 1** | Fix C-001 (Critical) | First |
| **Phase 2** | Fix M-001 (Warnings) | After C-001 |
| **Phase 3** | Fix M-002 (NATS) or remove claims | After M-001 |
| **Phase 4** | Apply N-001, N-002, N-003 | Last |

---

## Go/No-Go Decision

### Current Status: **NO-GO**

**Reason**: Critical issue C-001 prevents TUI from launching. All other issues are acceptable for production but should be addressed.

### Requirements for Go
- [ ] C-001: Fix tracing subscriber panic
- [ ] M-001: Clean up compiler warnings (optional but recommended)
- [ ] M-002: Clarify NATS status (optional but recommended)

### After C-001 Fix: **LIKELY GO**

Once the critical issue is fixed, the codebase is production-ready with 35/38 (92%) features verified as working correctly.

---

## Audit Summary Statistics

| Category | Count | Percentage |
|----------|-------|------------|
| Verified Working | 35 | 78% |
| Partially Working | 3 | 7% |
| Fixed During Audit | 5 | 11% |
| Critical Issues | 1 | 2% |
| Medium Issues | 2 | 4% |

**Document Accuracy**: 89% (35 verified + 5 fixed = 40 accurate out of 45 claims)
