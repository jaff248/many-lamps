//! On-chain research and data collection for Polymarket analysis.
//!
//! This module provides infrastructure for:
//! - Fetching market data from Polymarket CLOB API
//! - Tracking on-chain events (OrderFilled, PositionsConverted, etc.)
//! - Analyzing trader behavior and market sentiment
//! - Identifying arbitrage opportunities
//!
//! Key contracts:
//! - CTF Exchange (0x4bFb41d5B3570DeFd03C39a9A4D8dE6Bd8B8982E) - binary markets
//! - NegRisk_CTFExchange (0xC5d563A36AE78145C45a50134d48A1215220f80a) - multi-outcome
//! - NegRiskAdapter (0xd91E80cF2E7be2e162c6513ceD06f1dD0dA35296) - NO→YES conversion

use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::error::Error;

pub const POLYMARKET_CLOB_URL: &str = "https://clob.polymarket.com";
pub const POLYGON_RPC_URL: &str = "https://polygon-rpc.com";

/// Key Polymarket contract addresses
pub mod contracts {
    pub const CTF_EXCHANGE: &str = "0x4bFb41d5B3570DeFd03C39a9A4D8dE6Bd8B8982E";
    pub const NEG_RISK_EXCHANGE: &str = "0xC5d563A36AE78145C45a50134d48A1215220f80a";
    pub const NEG_RISK_ADAPTER: &str = "0xd91E80cF2E7be2e162c6513ceD06f1dD0dA35296";
    pub const CTF: &str = "0x4CaE3f36b1edC3d7b7d4d87c0fC6c7c0cDc8A8d3c";
}

/// Market information from Polymarket API
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Market {
    pub condition_id: String,
    pub question_id: String,
    pub question: String,
    pub market_slug: String,
    pub active: bool,
    pub closed: bool,
    pub tokens: Vec<MarketToken>,
    pub minimum_order_size: String,
    pub minimum_tick_size: String,
    pub fee_rate_bps: Option<String>,
}

/// Token for a market outcome
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketToken {
    pub token_id: String,
    pub outcome: String,
    pub price: f64,
    pub winner: bool,
}

/// On-chain trade event (from OrderFilled)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradeEvent {
    pub block_number: u64,
    pub transaction_hash: String,
    pub timestamp: DateTime<Utc>,
    pub maker: String,
    pub taker: String,
    pub maker_asset_id: String,
    pub taker_asset_id: String,
    pub maker_amount_filled: String,
    pub taker_amount_filled: String,
    pub market_slug: Option<String>,
}

/// Position conversion event (from PositionsConverted - NO→YES arbitrage)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PositionConversionEvent {
    pub block_number: u64,
    pub transaction_hash: String,
    pub timestamp: DateTime<Utc>,
    pub user: String,
    pub index_set: String,  // Bitmask of NO tokens converted
    pub amount: String,
    pub market_slug: Option<String>,
}

/// Market sentiment analysis
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketSentiment {
    pub market_slug: String,
    pub yes_price: f64,
    pub no_price: f64,
    pub spread: f64,
    pub volume_24h: f64,
    pub last_trade_time: Option<DateTime<Utc>>,
    pub trader_concentration: f64,  // Herfindahl index (0-1)
    pub active_traders: usize,
}

/// Arbitrage opportunity
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArbitrageOpportunity {
    pub market_slug: String,
    pub conversion_type: String,  // "NO_TO_YES"
    pub estimated_profit_bps: f64,
    pub requires_collateral_release: bool,
    pub min_size: f64,
    pub risk_level: RiskLevel,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RiskLevel {
    Low,
    Medium,
    High,
}

/// Fetch active markets from Polymarket
pub async fn fetch_active_markets() -> Result<Vec<Market>, Box<dyn Error>> {
    let client = Client::new();
    let url = format!("{}/markets?active=true", POLYMARKET_CLOB_URL);

    let response = client.get(&url)
        .header("User-Agent", "mtrader-research/0.1")
        .send()
        .await?
        .json::<serde_json::Value>()
        .await?;

    let markets = response["data"]
        .as_array()
        .ok_or("No markets array in response")?
        .iter()
        .filter_map(|m| serde_json::from_value(m.clone()).ok())
        .collect();

    Ok(markets)
}

/// Fetch market details by slug
pub async fn fetch_market_by_slug(slug: &str) -> Result<Option<Market>, Box<dyn Error>> {
    let client = Client::new();
    let url = format!("{}/markets?slug={}", POLYMARKET_CLOB_URL, slug);

    let response = client.get(&url)
        .header("User-Agent", "mtrader-research/0.1")
        .send()
        .await?
        .json::<serde_json::Value>()
        .await?;

    if let Some(markets) = response["data"].as_array() {
        if let Some(market) = markets.first() {
            return Ok(serde_json::from_value(market.clone()).ok());
        }
    }

    Ok(None)
}

/// Analyze market sentiment from recent trades
pub async fn analyze_market_sentiment(
    market: &Market,
    recent_trades: &[TradeEvent],
) -> MarketSentiment {
    let yes_token = market.tokens.iter()
        .find(|t| t.outcome.to_lowercase() == "yes")
        .map(|t| t.price)
        .unwrap_or(0.5);

    let no_token = market.tokens.iter()
        .find(|t| t.outcome.to_lowercase() == "no")
        .map(|t| t.price)
        .unwrap_or(0.5);

    // Calculate trader concentration (Herfindahl index)
    let trader_volumes: std::collections::HashMap<String, f64> = recent_trades
        .iter()
        .fold(std::collections::HashMap::new(), |mut acc, trade| {
            let vol = trade.maker_amount_filled.parse::<f64>().unwrap_or(0.0)
                + trade.taker_amount_filled.parse::<f64>().unwrap_or(0.0);
            *acc.entry(trade.maker.clone()).or_insert(0.0) += vol / 2.0;
            *acc.entry(trade.taker.clone()).or_insert(0.0) += vol / 2.0;
            acc
        });

    let total_volume: f64 = trader_volumes.values().sum();
    let hhi: f64 = if total_volume > 0.0 {
        trader_volumes.values()
            .map(|v| (v / total_volume).powi(2))
            .sum()
    } else {
        0.0
    };

    MarketSentiment {
        market_slug: market.market_slug.clone(),
        yes_price: yes_token,
        no_price: no_token,
        spread: (yes_token + no_token - 1.0).abs(),
        volume_24h: total_volume,
        last_trade_time: recent_trades.first().map(|t| t.timestamp),
        trader_concentration: hhi,
        active_traders: trader_volumes.len(),
    }
}

/// Identify NO→YES conversion arbitrage opportunities
pub fn identify_no_to_yes_arbitrage(market: &Market) -> Option<ArbitrageOpportunity> {
    // In multi-outcome markets, holding NO tokens for all but one outcome
    // can be converted to YES for the remaining outcome + USDC collateral
    if market.tokens.len() < 3 {
        return None;  // Not a multi-outcome market
    }

    // Calculate potential arbitrage
    let yes_price = market.tokens.iter()
        .find(|t| t.outcome.to_lowercase() == "yes")
        .map(|t| t.price)
        .unwrap_or(0.5);

    let no_prices: Vec<f64> = market.tokens.iter()
        .filter(|t| t.outcome.to_lowercase() == "no")
        .map(|t| t.price)
        .collect();

    if no_prices.is_empty() {
        return None;
    }

    // If sum of NO prices > YES price, there's potential arbitrage
    let no_sum: f64 = no_prices.iter().sum();
    let spread_bps = ((1.0 - yes_price) - no_sum) * 10000.0;

    if spread_bps > 10.0 {  // > 0.1% spread
        Some(ArbitrageOpportunity {
            market_slug: market.market_slug.clone(),
            conversion_type: "NO_TO_YES".to_string(),
            estimated_profit_bps: spread_bps,
            requires_collateral_release: true,
            min_size: market.minimum_order_size.parse().unwrap_or(15.0),
            risk_level: if spread_bps > 50.0 { RiskLevel::Low } else { RiskLevel::Medium },
        })
    } else {
        None
    }
}

/// Research summary for a market
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketResearch {
    pub market: Market,
    pub sentiment: Option<MarketSentiment>,
    pub arbitrage_opportunity: Option<ArbitrageOpportunity>,
    pub recommendation: StrategyRecommendation,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StrategyRecommendation {
    MakerMM,           // Provide liquidity, collect spreads
    Taker,             // Take positions on direction
    Arbitrage,         // Exploit conversion opportunities
    Avoid,             // Fees or conditions unfavorable
    Observe,           // Not enough data yet
}

/// Research a single market
pub async fn research_market(slug: &str) -> Result<MarketResearch, Box<dyn Error>> {
    let market = fetch_market_by_slug(slug)
        .await?
        .ok_or("Market not found")?;

    let sentiment = analyze_market_sentiment(&market, &[]).await;  // No trades yet
    let arbitrage = identify_no_to_yes_arbitrage(&market);

    let mut notes: Vec<String> = Vec::new();
    let mut recommendation = StrategyRecommendation::Observe;

    // Fee analysis
    if market.fee_rate_bps.as_ref().map(|s| s.parse::<f64>().unwrap_or(0.0)).unwrap_or(0.0) > 0.0 {
        notes.push("Market has trading fees - consider impact on strategy".to_string());
    }

    // Recommendation logic
    let spread = (market.tokens[0].price - market.tokens[1].price).abs();
    if let Some(arb) = &arbitrage {
        recommendation = StrategyRecommendation::Arbitrage;
        notes.push(format!("Arbitrage opportunity: ~{:.1} bps profit", arb.estimated_profit_bps));
    } else if spread > 0.1 {
        recommendation = StrategyRecommendation::MakerMM;
        notes.push(format!("Wide spread {:.1}% - good for market making", spread * 100.0));
    } else if spread < 0.02 {
        recommendation = StrategyRecommendation::Taker;
        notes.push("Tight spread - consider directional bets".to_string());
    }

    if sentiment.trader_concentration > 0.5 {
        notes.push("High trader concentration - careful with large positions".to_string());
    }

    Ok(MarketResearch {
        market,
        sentiment: Some(sentiment),
        arbitrage_opportunity: arbitrage,
        recommendation,
        notes,
    })
}

/// Backtest result structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BacktestResult {
    pub strategy_name: String,
    pub market_slug: String,
    pub total_trades: u32,
    pub win_rate: f64,
    pub profit_factor: f64,
    pub max_drawdown: f64,
    pub sharpe_ratio: f64,
    pub expected_value_bps: f64,
    pub conclusion: String,
}

/// Simple backtest summary (placeholder for more complex implementation)
impl BacktestResult {
    pub fn new(strategy: &str, market: &str) -> Self {
        Self {
            strategy_name: strategy.to_string(),
            market_slug: market.to_string(),
            total_trades: 0,
            win_rate: 0.0,
            profit_factor: 0.0,
            max_drawdown: 0.0,
            sharpe_ratio: 0.0,
            expected_value_bps: 0.0,
            conclusion: "Requires historical data to backtest".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_fetch_active_markets() {
        let markets = fetch_active_markets().await;
        assert!(markets.is_ok());
        let markets = markets.unwrap();
        println!("Found {} active markets", markets.len());
        if let Some(first) = markets.first() {
            println!("First market: {}", first.question);
        }
    }

    #[test]
    fn test_arbitrage_detection() {
        let market = Market {
            condition_id: "test".to_string(),
            question_id: "test".to_string(),
            question: "Test Market".to_string(),
            market_slug: "test-market".to_string(),
            active: true,
            closed: false,
            tokens: vec![
                MarketToken { token_id: "1".to_string(), outcome: "Yes".to_string(), price: 0.6, winner: false },
                MarketToken { token_id: "2".to_string(), outcome: "No".to_string(), price: 0.4, winner: false },
                MarketToken { token_id: "3".to_string(), outcome: "Maybe".to_string(), price: 0.3, winner: false },
            ],
            minimum_order_size: "15".to_string(),
            minimum_tick_size: "0.01".to_string(),
            fee_rate_bps: Some("0".to_string()),
        };

        let arb = identify_no_to_yes_arbitrage(&market);
        println!("Arbitrage: {:?}", arb);
        assert!(arb.is_some());
    }
}
