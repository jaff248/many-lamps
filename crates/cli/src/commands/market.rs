//! Market information command.

use crate::config::Config;
use crate::fee_profile::classify_fee_profile;
use anyhow::Result;
use mtrader_gateway::{RestClient, RestConfig};
use tracing::info;

/// Show market information.
pub async fn show(config: &Config, market: &str) -> Result<()> {
    info!(market = market, "Fetching market information");

    let client = RestClient::new(RestConfig {
        base_url: config.gateway.rest_url.clone(),
        ..Default::default()
    })?;

    match client.get_market(market).await {
        Ok(info) => {
            let fee_profile = classify_fee_profile(&info, config.risk.fee_rate_bps as u16);
            println!("\n=== MARKET INFO ===");
            println!("Condition ID: {}", info.condition_id);
            println!("Active: {}", info.active);
            println!("Closed: {}", info.closed);
            println!("Min Tick: {:?}", info.minimum_tick_size);
            println!("Fee Profile: {}", fee_profile.label);
            println!("Fee Schedule: {:?}", fee_profile.schedule);
            println!("===================\n");
        }
        Err(e) => {
            println!("Failed to fetch market info: {}", e);
        }
    }

    match client.get_book(market).await {
        Ok(book) => {
            println!("=== ORDER BOOK ===");
            println!("Hash: {}", book.hash);
            println!("Timestamp: {:?}", book.timestamp);
            println!("==================\n");
        }
        Err(e) => {
            println!("Failed to fetch order book: {}", e);
        }
    }

    Ok(())
}
