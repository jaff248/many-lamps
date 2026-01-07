//! REST client for Polymarket CLOB API.
//!
//! Used for:
//! - Fetching order book snapshots (for sync verification)
//! - Getting market info and tick sizes
//! - Order submission (requires API key)

use crate::error::GatewayError;
use crate::messages::{RestBookSnapshot, RestMarketInfo};
use reqwest::Client;
use std::time::Duration;

/// REST API endpoint for Polymarket CLOB.
pub const REST_URL: &str = "https://clob.polymarket.com";

/// REST client configuration.
#[derive(Debug, Clone)]
pub struct RestConfig {
    /// Base URL for REST API
    pub base_url: String,
    /// Request timeout
    pub timeout: Duration,
    /// Rate limit: requests per second
    pub rate_limit_rps: u32,
}

impl Default for RestConfig {
    fn default() -> Self {
        Self {
            base_url: REST_URL.to_string(),
            timeout: Duration::from_secs(10),
            rate_limit_rps: 10,
        }
    }
}

/// REST client for Polymarket CLOB.
pub struct RestClient {
    config: RestConfig,
    client: Client,
    /// Last request timestamp for rate limiting
    last_request_ns: std::sync::atomic::AtomicU64,
}

impl RestClient {
    pub fn new(config: RestConfig) -> Result<Self, GatewayError> {
        let client = Client::builder()
            .timeout(config.timeout)
            .user_agent("mtrader/0.1.0")
            .build()?;

        Ok(Self {
            config,
            client,
            last_request_ns: std::sync::atomic::AtomicU64::new(0),
        })
    }

    /// Get order book snapshot for an asset.
    pub async fn get_book(&self, asset_id: &str) -> Result<RestBookSnapshot, GatewayError> {
        self.rate_limit().await;

        let url = format!("{}/book?token_id={}", self.config.base_url, asset_id);
        let response = self.client.get(&url).send().await?;

        if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(GatewayError::RateLimited);
        }

        let snapshot: RestBookSnapshot = response.json().await?;
        Ok(snapshot)
    }

    /// Get market info by condition ID.
    pub async fn get_market(&self, condition_id: &str) -> Result<RestMarketInfo, GatewayError> {
        self.rate_limit().await;

        let url = format!("{}/markets/{}", self.config.base_url, condition_id);
        let response = self.client.get(&url).send().await?;

        if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(GatewayError::RateLimited);
        }

        let market: RestMarketInfo = response.json().await?;
        Ok(market)
    }

    /// Get tick size for a market.
    pub async fn get_tick_size(&self, condition_id: &str) -> Result<u16, GatewayError> {
        let market = self.get_market(condition_id).await?;

        let tick_str = market
            .minimum_tick_size
            .ok_or_else(|| GatewayError::InvalidMessage("Market missing tick size".into()))?;

        // Parse tick size string (e.g., "0.01" -> 100 bps)
        let tick: f64 = tick_str.parse().map_err(|_| {
            GatewayError::InvalidMessage(format!("Invalid tick size: {}", tick_str))
        })?;

        let bps = (tick * 10000.0).round() as u16;
        Ok(bps)
    }

    /// Verify order book hash against REST snapshot.
    ///
    /// Returns true if the hash matches.
    pub async fn verify_book_hash(
        &self,
        asset_id: &str,
        expected_hash: &str,
    ) -> Result<bool, GatewayError> {
        let snapshot = self.get_book(asset_id).await?;
        Ok(snapshot.hash == expected_hash)
    }

    /// Simple rate limiting.
    async fn rate_limit(&self) {
        use std::sync::atomic::Ordering;

        let min_interval_ns = 1_000_000_000 / self.config.rate_limit_rps as u64;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);

        let last = self.last_request_ns.load(Ordering::Relaxed);
        let elapsed = now.saturating_sub(last);

        if elapsed < min_interval_ns {
            let sleep_ns = min_interval_ns - elapsed;
            tokio::time::sleep(Duration::from_nanos(sleep_ns)).await;
        }

        self.last_request_ns.store(now, Ordering::Relaxed);
    }
}

/// Builder for authenticated REST client (for order submission).
#[derive(Debug, Clone)]
pub struct AuthenticatedRestConfig {
    pub base_config: RestConfig,
    /// API key for authentication
    pub api_key: String,
    /// API secret for signing
    pub api_secret: String,
    /// API passphrase
    pub api_passphrase: String,
}

/// Authenticated REST client for order operations.
pub struct AuthenticatedRestClient {
    config: AuthenticatedRestConfig,
    client: Client,
}

impl AuthenticatedRestClient {
    pub fn new(config: AuthenticatedRestConfig) -> Result<Self, GatewayError> {
        let client = Client::builder()
            .timeout(config.base_config.timeout)
            .user_agent("mtrader/0.1.0")
            .build()?;

        Ok(Self { config, client })
    }

    /// Place a limit order.
    ///
    /// Returns the order ID on success.
    pub async fn place_order(
        &self,
        asset_id: &str,
        side: &str,
        price: &str,
        size: &str,
    ) -> Result<String, GatewayError> {
        // TODO: Implement proper CLOB order signing
        // This requires:
        // 1. Create order payload
        // 2. Sign with API secret
        // 3. Include signature in headers

        let _url = format!("{}/orders", self.config.base_config.base_url);
        let _payload = serde_json::json!({
            "token_id": asset_id,
            "side": side,
            "price": price,
            "size": size,
            "type": "GTC"
        });

        // Placeholder - actual implementation requires EIP-712 signing
        Err(GatewayError::InvalidMessage(
            "Order submission not yet implemented - requires EIP-712 signing".into(),
        ))
    }

    /// Cancel an order.
    pub async fn cancel_order(&self, order_id: &str) -> Result<(), GatewayError> {
        let _url = format!("{}/orders/{}", self.config.base_config.base_url, order_id);

        // Placeholder - actual implementation requires signing
        Err(GatewayError::InvalidMessage(
            "Order cancellation not yet implemented - requires signing".into(),
        ))
    }

    /// Get open orders.
    pub async fn get_open_orders(&self) -> Result<Vec<serde_json::Value>, GatewayError> {
        let _url = format!("{}/orders", self.config.base_config.base_url);

        // Placeholder
        Err(GatewayError::InvalidMessage(
            "Get orders not yet implemented - requires signing".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default() {
        let config = RestConfig::default();
        assert_eq!(config.base_url, REST_URL);
        assert_eq!(config.rate_limit_rps, 10);
    }

    #[tokio::test]
    async fn test_client_creation() {
        let client = RestClient::new(RestConfig::default());
        assert!(client.is_ok());
    }
}
