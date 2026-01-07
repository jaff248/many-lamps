//! Market information command.

use crate::config::Config;
use anyhow::Result;
use mtrader_gateway::RestClient;
use tracing::info;

/// Show market information.
pub async fn show(config: &Config, market: &str) -> Result<()> {
    info!(market = market, "Fetching market information");

    let client = RestClient::new(&config.gateway.rest_url)?;

    // Fetch market info
    match client.get_market_info(market).await {
        Ok(info) => {
            println!("\n=== MARKET INFO ===");
            println!("Token ID: {}", market);
            println!("Question: {}", info.question);
            println!("End Date: {}", info.end_date);
            println!("Min Tick: {}", info.min_tick_size);
            println!("Fee Rate: {}%", info.fee_rate_bps as f64 / 100.0);
            println!("Active: {}", info.active);
            println!("===================\n");
        }
        Err(e) => {
            println!("Failed to fetch market info: {}", e);
        }
    }

    // Fetch order book snapshot
    match client.get_book_snapshot(market).await {
        Ok(book) => {
            println!("=== ORDER BOOK ===");
            println!("Timestamp: {}", book.timestamp);
            println!("Hash: {}", book.hash);

            println!("\nBids:");
            for (price, size) in book.bids.iter().take(5) {
                println!("  {:.2}  {:>12}", price, size);
            }

            println!("\nAsks:");
            for (price, size) in book.asks.iter().take(5) {
                println!("  {:.2}  {:>12}", price, size);
            }

            // Calculate spread
            if let (Some((best_bid, _)), Some((best_ask, _))) =
                (book.bids.first(), book.asks.first())
            {
                let spread = best_ask - best_bid;
                let mid = (best_ask + best_bid) / 2.0;
                let spread_bps = (spread / mid) * 10000.0;
                println!("\nSpread: {:.4} ({:.1} bps)", spread, spread_bps);
            }
            println!("==================\n");
        }
        Err(e) => {
            println!("Failed to fetch order book: {}", e);
        }
    }

    Ok(())
}
