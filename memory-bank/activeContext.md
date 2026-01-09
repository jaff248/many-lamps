# Active Context

## Current focus
- Implementing research-backed arbitrage strategies
- Single-condition rebalancing arbitrage deployed
- Multi-condition and combinatorial arbitrage in progress

## Research Summary (arXiv:2508.03474)

### Key Findings
- **$40M total arbitrage profit** extracted during measurement period (Apr 2024 - Apr 2025)
- **Single-condition arbitrage**: $5.9M (long) + $4.7M (short)
- **Market arbitrage**: $28M+ across NegRisk markets
- **Top arbitrageur**: $2M+ profit, 4,049 transactions

### Two Arbitrage Types Identified

**1. Market Rebalancing Arbitrage (Intra-market)**
- When YES + NO prices ≠ $1 (should sum to 1)
- Long: sum < $1 → buy both, profit = 1 - sum
- Short: sum > $1 → sell both, profit = sum - 1
- Most opportunities in Politics/Sports during elections

**2. Combinatorial Arbitrage (Inter-market)**
- Between dependent market pairs (e.g., "Who wins state" + "Winning margin")
- Requires LLM-based dependency detection
- 11 dependent pairs found in US election alone

## Implementation Status

### ✅ Completed
- [x] Memory Bank structure initialized
- [x] TUI dashboard working
- [x] Research crate with market data collection
- [x] Alpha signals module (large trade detection, liquidity gaps)
- [x] Single-condition rebalancing arbitrage strategy
- [x] Strategy exported from crate

### 🚧 In Progress
- [ ] Multi-condition market arbitrage (NegRisk markets)
- [ ] Combinatorial arbitrage (LLM-based dependency detection)
- [ ] Smart money follower strategy
- [ ] Backtesting framework

## New Strategy: RebalancingArbStrategy

**Configuration:**
- Trigger threshold: 2% deviation from $1
- Min profit per dollar: 1%
- Max position: $100 (conservative)
- Dry-run mode: enabled by default

**Usage:**
```rust
use mtrader_strategy::{RebalancingArbStrategy, RebalancingArbConfig};

let config = RebalancingArbConfig {
    trigger_threshold: 0.02,
    max_position_usd: 100.0,
    dry_run: true,
    ..Default::default()
};

let strategy = RebalancingArbStrategy::new(gateway, Some(config));
let opportunities = strategy.scan_for_opportunities().await;
```

## Key Thresholds from Research
- Large trade: > $500 USD
- Smart money: >3 trades, >$1000 total volume
- Minimum profit threshold: $0.02 per dollar
- Time window for execution: ~1 hour

## Research Contract Addresses
- CTF Exchange: 0x4bFb41d5B3570DeFd03C39a9A4D8dE6Bd8B8982E
- NegRisk Exchange: 0xC5d563A36AE78145C45a50134d48A1215220f80a
- NegRisk Adapter: 0xd91E80cF2E7be2e162c6513ceD06f1dD0dA35296
- CTF Token: 0x4D97DCd97eC945f40cF65F87097ACe5EA0476045

## Next Steps
1. Wire rebalancing_arb into CLI paper trading
2. Implement multi-condition arbitrage (sum of all YES ≠ 1)
3. Add LLM-based market dependency detection
4. Build backtest pipeline
5. Test on fee-free markets only (avoid 15-min crypto)
