# Active Context

## Current Focus
- **TUI Overhaul**: Transform from prototype to production-grade trading terminal
- **Auto-Trading Mode**: Implement strategy-driven autonomous trading
- **Risk Integration**: Wire existing risk crate into TUI

## Recent Changes
- Created comprehensive `tui-requirements.md` with all feature specifications
- Identified bugs: input editing, random position display, manual trade button
- Documented auto-trading flow, risk settings, strategy parameters

## Immediate Next Steps
1. **Fix Input Editing Bug** - Backspace should delete char, not cancel
2. **Fix Position Display** - Show persistent state, not random values
3. **Add Auto-Trading Toggle** - [a] key to enable strategy loop
4. **Wire Gateway Connect** - [c] should connect to real Polymarket WebSocket
5. **Add Risk Settings Screen** - User-configurable limits and drawdown protection

## Critical Requirements (NEW)

### Auto-Trading Behavior
- User selects strategy + parameters
- User enables auto-trading
- Bot autonomously executes trades when conditions are met
- NO manual trade button - strategies do the trading
- Circuit breaker halts on drawdown/consecutive losses

### Risk Parameters to Expose
- Max position per asset
- Max order size
- Max bet percentage (% of balance)
- Max drawdown (%)
- Max consecutive losses
- Max loss per hour

### Live Trading = Paper Trading UI
- Identical interface
- Same strategy selection
- Same parameter configuration
- Difference is backend execution path only

## Decisions & Considerations
- **Data Storage**: Use existing Parquet support for trades, add JSON for configs
- **Markets**: 10-15 built-in crypto markets + ability to query/add custom
- **Backtest**: Must use recorded data + selected strategy with params

## Files to Modify
See `tui-requirements.md` appendix for complete file list.

## Learnings
- Codebase already has robust `PositionLimits` and `CircuitBreaker` in risk crate
- Strategy trait exists with `on_update()` → `Vec<StrategyAction>` pattern
- Need to wire TUI to actual trading loop, not just mock display

