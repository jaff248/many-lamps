# Active Context

## Current focus
- Implementing RBI System: Research → Backtest → Implement
- Building on-chain data infrastructure for strategy research

## Research Findings Summary

### Polymarket Fee Structure
- **Most markets are fee-free** (no trading fees)
- **15-min crypto markets** charge taker fees
- Maker rebate program redistributes fees to LPs

### Key On-Chain Contracts
- `CTF Exchange` (0x4bFb41d5B3570DeFd03C39a9A4D8dE6Bd8B8982E) - binary markets
- `NegRisk_CTFExchange` (0xC5d563A36AE78145C45a50134d48A1215220f80a) - multi-outcome markets
- `NegRiskAdapter` (0xd91E80cF2E7be2e162c6513ceD06f1dD0dA35296) - NO→YES conversion

### Key Events to Track
- `OrderFilled` - identifies buyers/sellers
- `OrdersMatched` - confirms trades
- `PositionsSplit` - token minting (new positions)
- `PositionsMerge` - token burning (position closes)
- `PositionsConverted` - NO→YES arbitrage opportunity

### @CRYINGLITTLEBABY Analysis
- $382,876 profit from concentrated positions
- Event-driven approach, not frequent trading
- Capital intensive ($380k+ positions)

## Implementation Roadmap

### Phase 1: Research Infrastructure
- [ ] On-chain data scraper (Polygon RPC)
- [ ] Event tracking for key market signals
- [ ] Market sentiment analyzer

### Phase 2: Backtesting Framework
- [ ] Python backtest scripts
- [ ] EV, drawdown, volatility metrics
- [ ] Strategy validation pipeline

### Phase 3: Strategy Development
- [ ] Fee-free market maker strategy
- [ ] NO→YES conversion arbitrage
- [ ] Ensemble system for multiple conditions

### Phase 4: Safe Implementation
- [ ] Start with $10 position size
- [ ] Circuit breakers and risk limits
- [ ] Gradual scaling after proving EV

## RBI System Philosophy
1. **Research**: Find anomalies, don't assume first idea works
2. **Backtest**: 2000+ tests to find winners, use backtesting.py
3. **Implement**: Ensemble approach, start small, remove emotion

## Key Contracts for Data
- Polygon RPC: https://polygon-rpc.com
- The Graph: polymarket/markets subgraph
- Event logs more reliable than transaction data

## Next Steps
- Build on-chain scraper to collect market data
- Identify fee-free markets for maker-MM
- Research NO→YES arbitrage in multi-outcome markets
