//! Alpha signals for Polymarket trading strategies.
//!
//! Based on research from:
//! - On-chain event analysis (OrderFilled, PositionsConverted)
//! - @yzc's decoding-polymarket methodology
//! - Liquidity gap detection
//! - Smart money following
//!
//! Key insights:
//! 1. NO→YES conversion arbitrage in multi-outcome markets
//! 2. Wide spread markets = maker opportunities
//! 3. Large trades signal smart money direction
//! 4. Low HHI = less crowded, better for makers

use super::*;
use std::collections::HashMap;

/// Smart money detection thresholds
const LARGE_TRADE_THRESHOLD_USD: f64 = 500.0;
const MEDIUM_TRADE_THRESHOLD_USD: f64 = 100.0;

/// Alpha signal from a single trade
#[derive(Debug, Clone)]
pub struct AlphaSignal {
    pub signal_type: SignalType,
    pub market_slug: String,
    pub confidence: f64,  // 0.0 to 1.0
    pub direction: Option<TradeDirection>,
    pub size_usd: f64,
    pub timestamp: DateTime<Utc>,
    pub reasoning: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SignalType {
    LargeTrade,           // Large order detected - follow or fade
    LiquidityGap,         // Wide spread, provide liquidity
    SmartMoney,           // Known profitable trader
    ArbitrageOpportunity, // NO→YES conversion
    SentimentShift,       // Rapid price movement
    VolumeImbalance,      // Buying/selling pressure
}

#[derive(Debug, Clone, PartialEq)]
pub enum TradeDirection {
    BuyYes,
    SellYes,
    BuyNo,
    SellNo,
}

/// Detect large trades (smart money signals)
pub fn detect_large_trades(trades: &[TradeEvent], market_slug: &str) -> Vec<AlphaSignal> {
    let mut signals = Vec::new();
    let mut trader_stats: HashMap<String, (f64, usize)> = HashMap::new();

    for trade in trades {
        let size: f64 = trade.maker_amount_filled.parse().unwrap_or(0.0)
            + trade.taker_amount_filled.parse().unwrap_or(0.0);

        // Track trader activity
        let (total_size, count) = trader_stats.entry(trade.maker.clone()).or_insert((0.0, 0));
        *total_size += size / 2.0;
        *count += 1;
        let (total_size_t, count_t) = trader_stats.entry(trade.taker.clone()).or_insert((0.0, 0));
        *total_size_t += size / 2.0;
        *count_t += 1;

        if size >= LARGE_TRADE_THRESHOLD_USD {
            let direction = if trade.maker_asset_id == "0" {
                TradeDirection::BuyYes
            } else {
                TradeDirection::SellYes
            };

            signals.push(AlphaSignal {
                signal_type: SignalType::LargeTrade,
                market_slug: market_slug.to_string(),
                confidence: (size / LARGE_TRADE_THRESHOLD_USD).min(1.0),
                direction: Some(direction),
                size_usd: size,
                timestamp: trade.timestamp,
                reasoning: format!("Large trade detected: ${:.0f}", size),
            });
        }
    }

    // Identify potential smart money (consistent large traders)
    for (trader, (total_size, count)) in &trader_stats {
        if *count >= 3 && *total_size >= 1000.0 {
            // This trader appears frequently with large sizes
            signals.push(AlphaSignal {
                signal_type: SignalType::SmartMoney,
                market_slug: market_slug.to_string(),
                confidence: 0.7,
                direction: None,
                size_usd: *total_size,
                timestamp: Utc::now(),
                reasoning: format!("Smart money detected: {} with ${:.0f} total volume", &trader[..8], total_size),
            });
        }
    }

    signals
}

/// Detect liquidity gaps (wide spread opportunities)
pub fn detect_liquidity_gaps(markets: &[Market]) -> Vec<AlphaSignal> {
    let mut signals = Vec::new();

    for market in markets {
        if market.closed || market.tokens.len() < 2 {
            continue;
        }

        let yes_price = market.tokens.iter()
            .find(|t| t.outcome.to_lowercase() == "yes")
            .map(|t| t.price)
            .unwrap_or(0.5);

        let no_price = market.tokens.iter()
            .find(|t| t.outcome.to_lowercase() == "no")
            .map(|t| t.price)
            .unwrap_or(0.5);

        let spread = (yes_price + no_price - 1.0).abs();

        // Wide spread = opportunity for market making
        if spread > 0.05 {  // > 5% spread
            signals.push(AlphaSignal {
                signal_type: SignalType::LiquidityGap,
                market_slug: market.market_slug.clone(),
                confidence: (spread / 0.2).min(1.0),  // Higher confidence for wider spreads
                direction: None,
                size_usd: market.minimum_order_size.parse().unwrap_or(15.0),
                timestamp: Utc::now(),
                reasoning: format!(
                    "Wide spread {:.1}% - good for maker-MM, expected capture: {:.2}%",
                    spread * 100.0,
                    spread * 100.0 * 0.8  // Assume 80% capture rate
                ),
            });
        }
    }

    signals
}

/// Detect volume imbalances (sentiment shifts)
pub fn detect_volume_imbalance(trades: &[TradeEvent], market_slug: &str) -> Vec<AlphaSignal> {
    let mut yes_volume: f64 = 0.0;
    let mut no_volume: f64 = 0.0;

    for trade in trades {
        let size: f64 = trade.maker_amount_filled.parse().unwrap_or(0.0)
            + trade.taker_amount_filled.parse().unwrap_or(0.0);

        // Assume asset_id 0 = USDC (buying), position_id = selling
        if trade.maker_asset_id == "0" {
            yes_volume += size / 2.0;
        } else {
            no_volume += size / 2.0;
        }
    }

    let total = yes_volume + no_volume;
    if total < 10.0 {
        return Vec::new();  // Not enough volume
    }

    let imbalance = (yes_volume - no_volume) / total;
    let abs_imbalance = imbalance.abs();

    if abs_imbalance > 0.3 {  // Significant imbalance
        let direction = if imbalance > 0.0 {
            TradeDirection::BuyYes
        } else {
            TradeDirection::SellYes
        };

        return vec![AlphaSignal {
            signal_type: SignalType::VolumeImbalance,
            market_slug: market_slug.to_string(),
            confidence: abs_imbalance,
            direction: Some(direction),
            size_usd: total,
            timestamp: Utc::now(),
            reasoning: format!(
                "Volume imbalance: {:.0}% {} pressure",
                abs_imbalance * 100.0,
                if imbalance > 0.0 { "buying" } else { "selling" }
            ),
        }];
    }

    Vec::new()
}

/// Calculate maker rebate opportunity score
pub fn calculate_maker_score(market: &Market, trades: &[TradeEvent]) -> f64 {
    let fee_rate: f64 = market.fee_rate_bps
        .as_ref()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0);

    if fee_rate > 0.0 {
        // Has fees - can collect rebates as maker
        return fee_rate / 100.0;  // Return as percentage
    }

    // Fee-free market - can still earn from spreads
    let yes_price = market.tokens.iter()
        .find(|t| t.outcome.to_lowercase() == "yes")
        .map(|t| t.price)
        .unwrap_or(0.5);

    let no_price = market.tokens.iter()
        .find(|t| t.outcome.to_lowercase() == "no")
        .map(|t| t.price)
        .unwrap_or(0.5);

    let spread = (yes_price + no_price - 1.0).abs();

    // Spread capture potential (assuming 50% capture)
    let spread_capture = spread * 0.5;

    // Volume bonus (more volume = more opportunities)
    let total_volume: f64 = trades.iter()
        .map(|t| {
            t.maker_amount_filled.parse::<f64>().unwrap_or(0.0)
                + t.taker_amount_filled.parse::<f64>().unwrap_or(0.0)
        })
        .sum();

    let volume_bonus = (total_volume / 1000.0).min(0.1);  // Up to 10% bonus

    spread_capture + volume_bonus
}

/// Composite alpha signal combining multiple indicators
#[derive(Debug, Clone)]
pub struct CompositeAlpha {
    pub market_slug: String,
    pub overall_score: f64,  // -1.0 to 1.0
    pub signals: Vec<AlphaSignal>,
    pub recommendation: AlphaRecommendation,
    pub expected_return_bps: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AlphaRecommendation {
    StrongBuy,
    Buy,
    Hold,
    Sell,
    StrongSell,
    Watch,
}

impl From<f64> for AlphaRecommendation {
    fn from(score: f64) -> Self {
        match score {
            s if s > 0.6 => AlphaRecommendation::StrongBuy,
            s if s > 0.2 => AlphaRecommendation::Buy,
            s if s > -0.2 => AlphaRecommendation::Hold,
            s if s > -0.6 => AlphaRecommendation::Sell,
            _ => AlphaRecommendation::StrongSell,
        }
    }
}

/// Generate composite alpha for a market
pub fn generate_composite_alpha(
    market: &Market,
    trades: &[TradeEvent],
    liquidity_signals: &[AlphaSignal],
) -> CompositeAlpha {
    let mut signals = Vec::new();
    let mut score = 0.0;

    // Add large trade signals
    let large_trades = detect_large_trades(trades, &market.market_slug);
    signals.extend(large_trades.clone());

    for signal in large_trades {
        match signal.direction {
            Some(TradeDirection::BuyYes) => score += 0.1 * signal.confidence,
            Some(TradeDirection::SellYes) => score -= 0.1 * signal.confidence,
            _ => {}
        }
    }

    // Add liquidity gap signals
    signals.extend(liquidity_signals.iter().cloned());
    for signal in liquidity_signals {
        score += 0.05 * signal.confidence;  // Liquidity gaps are positive
    }

    // Add volume imbalance
    let imbalances = detect_volume_imbalance(trades, &market.market_slug);
    signals.extend(imbalances.clone());

    for signal in imbalances {
        match signal.direction {
            Some(TradeDirection::BuyYes) => score += 0.15 * signal.confidence,
            Some(TradeDirection::SellYes) => score -= 0.15 * signal.confidence,
            _ => {}
        }
    }

    // Add arbitrage opportunity
    if let Some(arb) = identify_no_to_yes_arbitrage(market) {
        signals.push(AlphaSignal {
            signal_type: SignalType::ArbitrageOpportunity,
            market_slug: market.market_slug.clone(),
            confidence: 0.9,
            direction: None,
            size_usd: arb.min_size,
            timestamp: Utc::now(),
            reasoning: format!("NO→YES arbitrage: {:.1} bps", arb.estimated_profit_bps),
        });
        score += 0.3;  // Arbitrage is strong signal
    }

    // Fee check (negative for fee markets)
    let fee_rate: f64 = market.fee_rate_bps
        .as_ref()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0);
    if fee_rate > 0.0 {
        score -= 0.1;
    }

    // Spread adjustment
    let yes_price = market.tokens.iter()
        .find(|t| t.outcome.to_lowercase() == "yes")
        .map(|t| t.price)
        .unwrap_or(0.5);
    let no_price = market.tokens.iter()
        .find(|t| t.outcome.to_lowercase() == "no")
        .map(|t| t.price)
        .unwrap_or(0.5);
    let spread = (yes_price + no_price - 1.0).abs();

    // Mid-range spreads are best (0.03-0.10)
    if spread > 0.03 && spread < 0.10 {
        score += 0.1;
    } else if spread < 0.01 {
        score -= 0.1;  // Too tight, hard to capture
    }

    // Normalize score to -1.0 to 1.0
    score = score.clamp(-1.0, 1.0);

    // Calculate expected return
    let maker_score = calculate_maker_score(market, trades);
    let expected_return = if score > 0.0 {
        maker_score * (1.0 + score)
    } else {
        maker_score * (1.0 + score * 0.5)
    };

    CompositeAlpha {
        market_slug: market.market_slug.clone(),
        overall_score: score,
        signals,
        recommendation: score.into(),
        expected_return_bps: expected_return * 10000.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_large_trade_detection() {
        let now = Utc::now();
        let trades = vec![
            TradeEvent {
                block_number: 100,
                transaction_hash: "0x123".to_string(),
                timestamp: now,
                maker: "0xAAA".to_string(),
                taker: "0xBBB".to_string(),
                maker_asset_id: "0".to_string(),  // Buying YES
                taker_asset_id: "123".to_string(),
                maker_amount_filled: "1000".to_string(),
                taker_amount_filled: "1000".to_string(),
                market_slug: Some("test".to_string()),
            },
            TradeEvent {
                block_number: 101,
                transaction_hash: "0x124".to_string(),
                timestamp: now,
                maker: "0xAAA".to_string(),
                taker: "0xCCC".to_string(),
                maker_asset_id: "0".to_string(),
                taker_asset_id: "124".to_string(),
                maker_amount_filled: "500".to_string(),
                taker_amount_filled: "500".to_string(),
                market_slug: Some("test".to_string()),
            },
        ];

        let signals = detect_large_trades(&trades, "test");
        assert!(signals.len() >= 1);  // At least one large trade

        // Check for smart money
        let smart_money: Vec<_> = signals.iter()
            .filter(|s| s.signal_type == SignalType::SmartMoney)
            .collect();
        assert!(!smart_money.is_empty());  // 0xAAA appears twice with large trades
    }

    #[test]
    fn test_liquidity_gap_detection() {
        let markets = vec![
            Market {
                condition_id: "1".to_string(),
                question_id: "1".to_string(),
                question: "Wide Spread Market".to_string(),
                market_slug: "wide-spread".to_string(),
                active: true,
                closed: false,
                tokens: vec![
                    MarketToken { token_id: "1".to_string(), outcome: "Yes".to_string(), price: 0.6, winner: false },
                    MarketToken { token_id: "2".to_string(), outcome: "No".to_string(), price: 0.5, winner: false },
                ],
                minimum_order_size: "15".to_string(),
                minimum_tick_size: "0.01".to_string(),
                fee_rate_bps: Some("0".to_string()),
            },
            Market {
                condition_id: "2".to_string(),
                question_id: "2".to_string(),
                question: "Tight Spread Market".to_string(),
                market_slug: "tight-spread".to_string(),
                active: true,
                closed: false,
                tokens: vec![
                    MarketToken { token_id: "3".to_string(), outcome: "Yes".to_string(), price: 0.51, winner: false },
                    MarketToken { token_id: "4".to_string(), outcome: "No".to_string(), price: 0.49, winner: false },
                ],
                minimum_order_size: "15".to_string(),
                minimum_tick_size: "0.01".to_string(),
                fee_rate_bps: Some("0".to_string()),
            },
        ];

        let signals = detect_liquidity_gaps(&markets);
        assert_eq!(signals.len(), 1);  // Only wide spread should trigger
        assert_eq!(signals[0].market_slug, "wide-spread");
    }

    #[test]
    fn test_composite_alpha() {
        let market = Market {
            condition_id: "1".to_string(),
            question_id: "1".to_string(),
            question: "Test Market".to_string(),
            market_slug: "test".to_string(),
            active: true,
            closed: false,
            tokens: vec![
                MarketToken { token_id: "1".to_string(), outcome: "Yes".to_string(), price: 0.55, winner: false },
                MarketToken { token_id: "2".to_string(), outcome: "No".to_string(), price: 0.45, winner: false },
            ],
            minimum_order_size: "15".to_string(),
            minimum_tick_size: "0.01".to_string(),
            fee_rate_bps: Some("0".to_string()),
        };

        let alpha = generate