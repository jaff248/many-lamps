use mtrader_core::Tick;

/// Extract the price from a market snapshot.
///
/// Returns the mid-price if available, otherwise falls back to:
/// 1. Average of best bid and ask
/// 2. Best bid only
/// 3. Best ask only
/// 4. None if no price data is available
pub fn snapshot_price(snapshot: &crate::MarketSnapshot) -> Option<f64> {
    if let Some(mid_tick) = snapshot.mid_tick {
        return Some(tick_to_price(mid_tick));
    }

    match (snapshot.best_bid, snapshot.best_ask) {
        (Some(bid), Some(ask)) => Some(tick_to_price((bid + ask) / 2)),
        (Some(bid), None) => Some(tick_to_price(bid)),
        (None, Some(ask)) => Some(tick_to_price(ask)),
        (None, None) => None,
    }
}

/// Convert a tick value to a decimal price.
///
/// Tick values are in basis points (e.g., 5000 = 0.5000 = 50%)
pub fn tick_to_price(tick: Tick) -> f64 {
    tick as f64 / 10000.0
}
