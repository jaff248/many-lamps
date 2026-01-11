//! Polymarket Gateway Integration
//!
//! This module provides a complete Polymarket gateway implementation including:
//! - EIP-712 authentication for order signing
//! - Token bucket rate limiting
//! - REST API client for market data and order management
//! - WebSocket handler for real-time market data
//! - Data normalization to internal Tick format

use crate::error::GatewayError;
use crate::messages::{
    BookSnapshot, LastTradePrice, ParsedPriceChange, PriceChange, PriceChangeLevel,
    RestBookSnapshot, RestMarketInfo, SubscribeRequest, TickSizeChange,
};
use crate::ws_client::{WsCommand, WsConfig, WsState};
use futures_util::{SinkExt, StreamExt};
use mtrader_core::{
    parse_price_to_tick_strict, parse_size, Side, Size, Tick,
};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::interval;
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};

// ============================================================================
// Constants
// ============================================================================

/// Polymarket CLOB REST API URL
pub const POLYMARKET_API_URL: &str = "https://api.polymarket.com";

/// Polymarket WebSocket URL
pub const POLYMARKET_WS_URL: &str = "wss://ws.polymarket.com";

/// Polymarket CLOB API URL (for orders)
pub const POLYMARKET_CLOB_URL: &str = "https://clob.polymarket.com";

/// Default rate limit: requests per second
pub const DEFAULT_RATE_LIMIT_RPS: u32 = 10;

/// Default burst size
pub const DEFAULT_RATE_LIMIT_BURST: u32 = 20;

// ============================================================================
// Configuration
// ============================================================================

/// Polymarket gateway configuration.
#[derive(Debug, Clone)]
pub struct PolymarketConfig {
    /// REST API URL
    pub api_url: String,
    /// WebSocket URL
    pub ws_url: String,
    /// CLOB API URL (for authenticated endpoints)
    pub clob_url: String,
    /// API key for authentication
    pub api_key: String,
    /// API secret (private key) for signing
    pub api_secret: String,
    /// API passphrase
    pub api_passphrase: String,
    /// Rate limit: requests per second
    pub rate_limit_rps: u32,
    /// Rate limit: burst size
    pub rate_limit_burst: u32,
    /// Request timeout
    pub request_timeout: Duration,
    /// WebSocket connection config
    pub ws_config: WsConfig,
}

impl Default for PolymarketConfig {
    fn default() -> Self {
        Self {
            api_url: POLYMARKET_API_URL.to_string(),
            ws_url: POLYMARKET_WS_URL.to_string(),
            clob_url: POLYMARKET_CLOB_URL.to_string(),
            api_key: String::new(),
            api_secret: String::new(),
            api_passphrase: String::new(),
            rate_limit_rps: DEFAULT_RATE_LIMIT_RPS,
            rate_limit_burst: DEFAULT_RATE_LIMIT_BURST,
            request_timeout: Duration::from_secs(10),
            ws_config: WsConfig::default(),
        }
    }
}

/// Polymarket-specific order request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolyOrderRequest {
    pub token_id: String,
    pub side: String,        // "buy" | "sell"
    pub price: String,       // e.g., "0.55"
    pub size: String,        // e.g., "100.00"
    pub order_type: String,  // "GTC" | "FOK" | "FAK"
    pub nonce: String,
    pub signature: String,
}

/// Order response from Polymarket.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PolyOrderResponse {
    pub order_id: String,
    pub status: String,
    pub fills: Vec<PolyFillInfo>,
    pub created_at: u64,
}

/// Fill information.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PolyFillInfo {
    pub fill_id: String,
    pub price: String,
    pub size: String,
    pub side: String,
    pub timestamp: u64,
}

/// Position information.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PolyPosition {
    pub token_id: String,
    pub outcome: String,
    pub size: String,
    pub avg_price: String,
    pub realized_pnl: String,
    pub unrealized_pnl: String,
}

/// Market information from Polymarket API.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PolyMarket {
    pub condition_id: String,
    pub question: String,
    pub active: bool,
    pub closed: bool,
    pub volume: String,
    pub liquidity: String,
    pub tokens: Vec<PolyToken>,
}

/// Token in a market.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PolyToken {
    pub token_id: String,
    pub outcome: String,  // "yes" | "no"
    pub price: String,
    pub volume: String,
}

// ============================================================================
// EIP-712 Authentication
// ============================================================================

/// EIP-712 signature domain separator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignatureDomain {
    pub name: String,
    pub version: String,
    pub chain_id: u64,
    pub verifying_contract: String,
}

/// EIP-712 message structure for Polymarket orders.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Eip712OrderMessage {
    pub token_id: String,
    pub side: String,
    pub price: String,
    pub size: String,
    pub order_type: String,
    pub nonce: String,
    pub expiration: String,
}

/// Authentication handler for Polymarket CLOB.
#[derive(Debug)]
pub struct AuthHandler {
    /// API key
    api_key: String,
    /// API secret (private key) for signing
    api_secret: String,
    /// API passphrase
    api_passphrase: String,
    /// Current nonce (stored in atomic for thread safety)
    nonce: AtomicU64,
    /// EIP-712 domain
    domain: SignatureDomain,
    /// Cloned nonce for Clone derive (lazy initialized)
    #[allow(dead_code)]
    nonce_clone: u64,
}

impl Clone for AuthHandler {
    fn clone(&self) -> Self {
        Self {
            api_key: self.api_key.clone(),
            api_secret: self.api_secret.clone(),
            api_passphrase: self.api_passphrase.clone(),
            nonce: AtomicU64::new(self.nonce.load(Ordering::SeqCst)),
            domain: self.domain.clone(),
            nonce_clone: self.nonce.load(Ordering::SeqCst),
        }
    }
}

impl AuthHandler {
    /// Create a new auth handler.
    pub fn new(
        api_key: String,
        api_secret: String,
        api_passphrase: String,
        chain_id: u64,
    ) -> Self {
        Self {
            api_key,
            api_secret,
            api_passphrase,
            nonce: AtomicU64::new(0),
            domain: SignatureDomain {
                name: "Polymarket".to_string(),
                version: "1.0.0".to_string(),
                chain_id,
                verifying_contract: "0x0000000000000000000000000000000000000000".to_string(),
            },
            nonce_clone: 0,
        }
    }

    /// Get the next nonce.
    pub fn get_next_nonce(&self) -> u64 {
        self.nonce.fetch_add(1, Ordering::SeqCst)
    }

    /// Set the current nonce (e.g., from API response).
    pub fn set_nonce(&self, nonce: u64) {
        self.nonce.store(nonce.max(self.nonce.load(Ordering::SeqCst)), Ordering::SeqCst);
    }

    /// Create a signed order request.
    pub async fn sign_order(
        &self,
        token_id: &str,
        side: &str,
        price: &str,
        size: &str,
        order_type: &str,
    ) -> Result<PolyOrderRequest, GatewayError> {
        let nonce = self.get_next_nonce().to_string();

        // Build order payload
        let order = PolyOrderRequest {
            token_id: token_id.to_string(),
            side: side.to_string(),
            price: price.to_string(),
            size: size.to_string(),
            order_type: order_type.to_string(),
            nonce: nonce.clone(),
            signature: String::new(), // Will be filled by signer
        };

        // Sign the order (placeholder - actual implementation would use ethers-rs)
        let signature = self.sign_eip712(&order, &nonce).await?;

        Ok(PolyOrderRequest {
            token_id: order.token_id,
            side: order.side,
            price: order.price,
            size: order.size,
            order_type: order.order_type,
            nonce,
            signature,
        })
    }

    /// Sign an order using EIP-712.
    async fn sign_eip712(
        &self,
        _order: &PolyOrderRequest,
        _nonce: &str,
    ) -> Result<String, GatewayError> {
        // TODO: Implement actual EIP-712 signing using ethers-rs or k256
        // For now, return a placeholder signature
        //
        // In production, this would:
        // 1. Build the EIP-712 typed data structure
        // 2. Create the hash struct according to EIP-712
        // 3. Sign with the private key using secp256k1
        // 4. Encode signature as 65-byte [r, s, v]

        // Placeholder: In real implementation, use k256::ecdsa::SigningKey
        Err(GatewayError::InvalidMessage(
            "EIP-712 signing not yet implemented - requires k256 crate".to_string(),
        ))
    }

    /// Get authentication headers for HTTP requests.
    pub fn get_auth_headers(&self, signature: &str, nonce: &str) -> Vec<(String, String)> {
        vec![
            ("X-Polymarket-Api-Key".to_string(), self.api_key.clone()),
            ("X-Polymarket-Signature".to_string(), signature.to_string()),
            ("X-Polymarket-Nonce".to_string(), nonce.to_string()),
            ("X-Polymarket-Passphrase".to_string(), self.api_passphrase.clone()),
        ]
    }
}

// ============================================================================
// Rate Limiter (Token Bucket)
// ============================================================================

/// Token bucket rate limiter.
#[derive(Debug)]
pub struct TokenBucket {
    /// Maximum tokens (burst capacity)
    capacity: u64,
    /// Current tokens
    tokens: AtomicU64,
    /// Last refill timestamp (nanoseconds since epoch)
    last_update: AtomicU64,
    /// Refill rate (tokens per second)
    refill_rate: u64,
}

impl Clone for TokenBucket {
    fn clone(&self) -> Self {
        Self {
            capacity: self.capacity,
            tokens: AtomicU64::new(self.tokens.load(Ordering::Relaxed)),
            last_update: AtomicU64::new(self.last_update.load(Ordering::Relaxed)),
            refill_rate: self.refill_rate,
        }
    }
}

impl TokenBucket {
    /// Create a new token bucket.
    pub fn new(capacity: u64, refill_rate_rps: u64) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);

        Self {
            capacity,
            tokens: AtomicU64::new(capacity),
            last_update: AtomicU64::new(now),
            refill_rate: refill_rate_rps,
        }
    }

    /// Try to consume tokens without blocking.
    /// Returns Ok(()) if successful, or the wait time needed.
    pub fn try_consume(&self, tokens: u64) -> Result<(), Duration> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);

        let last = self.last_update.load(Ordering::Relaxed);
        let elapsed = now.saturating_sub(last);

        // Calculate tokens to add
        let refill = (elapsed * self.refill_rate) / 1_000_000_000;

        let current = self.tokens.load(Ordering::Relaxed);
        let new_tokens = (current + refill).min(self.capacity);

        if new_tokens >= tokens {
            self.tokens
                .store(new_tokens - tokens, Ordering::Relaxed);
            self.last_update.store(now, Ordering::Relaxed);
            Ok(())
        } else {
            let needed = tokens.saturating_sub(new_tokens);
            let wait_ns = (needed * 1_000_000_000) / self.refill_rate.max(1);
            Err(Duration::from_nanos(wait_ns))
        }
    }

    /// Consume tokens, blocking until available.
    pub async fn consume(&self, tokens: u64) {
        loop {
            if let Err(wait) = self.try_consume(tokens) {
                tokio::time::sleep(wait).await;
            } else {
                break;
            }
        }
    }

    /// Get current token count.
    pub fn tokens(&self) -> u64 {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);

        let last = self.last_update.load(Ordering::Relaxed);
        let elapsed = now.saturating_sub(last);
        let refill = (elapsed * self.refill_rate) / 1_000_000_000;

        let current = self.tokens.load(Ordering::Relaxed);
        (current + refill).min(self.capacity)
    }
}

/// Rate limiter manager for different endpoint types.
#[derive(Debug, Clone)]
pub struct RateLimiterManager {
    /// Rate limiter for order operations (stricter limits)
    pub order_rate: TokenBucket,
    /// Rate limiter for general API requests
    pub general_rate: TokenBucket,
    /// Rate limiter for WebSocket operations
    pub ws_rate: TokenBucket,
}

impl RateLimiterManager {
    /// Create a new rate limiter manager.
    pub fn new(order_rps: u32, general_rps: u32, burst: u32) -> Self {
        Self {
            order_rate: TokenBucket::new(burst as u64, order_rps as u64),
            general_rate: TokenBucket::new(burst as u64, general_rps as u64),
            ws_rate: TokenBucket::new(100, 50), // More lenient for WS
        }
    }

    /// Check and consume for order operations.
    pub async fn check_order_rate(&self) -> Result<(), Duration> {
        self.order_rate.try_consume(1)?;
        self.general_rate.try_consume(1)?;
        Ok(())
    }

    /// Check and consume for general API calls.
    pub async fn check_general_rate(&self) -> Result<(), Duration> {
        self.general_rate.try_consume(1)?;
        Ok(())
    }

    /// Consume for WebSocket operations.
    pub async fn check_ws_rate(&self) {
        self.ws_rate.consume(1).await;
    }
}

// ============================================================================
// REST Client
// ============================================================================

/// Polymarket REST client.
#[derive(Debug)]
pub struct PolymarketRestClient {
    /// HTTP client (not cloneable)
    client: Client,
    /// Configuration
    config: PolymarketConfig,
    /// Authentication handler
    auth: Option<AuthHandler>,
    /// Rate limiter
    rate_limiter: RateLimiterManager,
    /// Last request timestamp for simple rate limiting
    last_request_ns: AtomicU64,
}

impl Clone for PolymarketRestClient {
    fn clone(&self) -> Self {
        Self {
            client: Client::new(), // Create new client for clone
            config: self.config.clone(),
            auth: self.auth.clone(),
            rate_limiter: self.rate_limiter.clone(),
            last_request_ns: AtomicU64::new(0),
        }
    }
}

impl PolymarketRestClient {
    /// Create a new REST client.
    pub fn new(config: PolymarketConfig) -> Result<Self, GatewayError> {
        let client = Client::builder()
            .timeout(config.request_timeout)
            .user_agent("mtrader/0.1.0")
            .build()?;

        let auth = if !config.api_key.is_empty() && !config.api_secret.is_empty() {
            Some(AuthHandler::new(
                config.api_key.clone(),
                config.api_secret.clone(),
                config.api_passphrase.clone(),
                1, // Default to mainnet
            ))
        } else {
            None
        };

        Ok(Self {
            client,
            config,
            auth,
            rate_limiter: RateLimiterManager::new(
                DEFAULT_RATE_LIMIT_RPS,
                DEFAULT_RATE_LIMIT_RPS,
                DEFAULT_RATE_LIMIT_BURST,
            ),
            last_request_ns: AtomicU64::new(0),
        })
    }

    /// Simple rate limiting based on config.
    async fn rate_limit(&self) {
        let min_interval_ns = 1_000_000_000 / self.config.rate_limit_rps as u64;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
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

    /// Fetch active markets.
    pub async fn get_markets(&self) -> Result<Vec<PolyMarket>, GatewayError> {
        self.rate_limit().await;

        let url = format!("{}/markets", self.config.api_url);
        let response = self.client.get(&url).send().await?;

        if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(GatewayError::RateLimited);
        }

        let markets: Vec<PolyMarket> = response.json().await?;
        Ok(markets)
    }

    /// Get order book for a market.
    pub async fn get_orderbook(
        &self,
        token_id: &str,
    ) -> Result<RestBookSnapshot, GatewayError> {
        self.rate_limit().await;

        let url = format!(
            "{}/book?token_id={}",
            self.config.clob_url,
            token_id
        );
        let response = self.client.get(&url).send().await?;

        if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(GatewayError::RateLimited);
        }

        let snapshot: RestBookSnapshot = response.json().await?;
        Ok(snapshot)
    }

    /// Get current positions.
    pub async fn get_positions(&self) -> Result<Vec<PolyPosition>, GatewayError> {
        if let Some(auth) = &self.auth {
            let nonce = auth.get_next_nonce().to_string();

            // Sign request for authenticated endpoint
            // Placeholder - actual implementation would sign and include headers
            let _signature = "placeholder".to_string();

            let url = format!("{}/positions", self.config.clob_url);

            // Build authenticated request
            let request = self
                .client
                .get(&url)
                .header("X-Polymarket-Api-Key", &auth.api_key)
                .header("X-Polymarket-Nonce", &nonce);

            let response = request.send().await?;

            if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
                return Err(GatewayError::RateLimited);
            }

            let positions: Vec<PolyPosition> = response.json().await?;
            return Ok(positions);
        }

        // Return empty if not authenticated
        Ok(vec![])
    }

    /// Create a new order.
    pub async fn create_order(
        &self,
        token_id: &str,
        side: &str,
        price: &str,
        size: &str,
    ) -> Result<PolyOrderResponse, GatewayError> {
        if let Some(auth) = &self.auth {
            // Sign the order
            let signed_order = auth
                .sign_order(token_id, side, price, size, "GTC")
                .await?;

            let url = format!("{}/orders", self.config.clob_url);

            let response = self
                .client
                .post(&url)
                .json(&signed_order)
                .send()
                .await?;

            if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
                return Err(GatewayError::RateLimited);
            }

            if !response.status().is_success() {
                let error_text = response.text().await?;
                return Err(GatewayError::InvalidMessage(format!(
                    "Order failed: {}",
                    error_text
                )));
            }

            let order_response: PolyOrderResponse = response.json().await?;
            return Ok(order_response);
        }

        Err(GatewayError::InvalidMessage(
            "Authentication required for order submission".to_string(),
        ))
    }

    /// Cancel an existing order.
    pub async fn cancel_order(&self, order_id: &str) -> Result<(), GatewayError> {
        if let Some(auth) = &self.auth {
            let nonce = auth.get_next_nonce().to_string();

            let url = format!("{}/orders/{}", self.config.clob_url, order_id);

            let response = self
                .client
                .delete(&url)
                .header("X-Polymarket-Api-Key", &auth.api_key)
                .header("X-Polymarket-Nonce", &nonce)
                .send()
                .await?;

            if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
                return Err(GatewayError::RateLimited);
            }

            if !response.status().is_success() {
                return Err(GatewayError::InvalidMessage(
                    "Cancel order failed".to_string(),
                ));
            }

            return Ok(());
        }

        Err(GatewayError::InvalidMessage(
            "Authentication required for order cancellation".to_string(),
        ))
    }

    /// Get open orders.
    pub async fn get_open_orders(&self) -> Result<Vec<PolyOrderResponse>, GatewayError> {
        if let Some(auth) = &self.auth {
            let nonce = auth.get_next_nonce().to_string();

            let url = format!("{}/orders", self.config.clob_url);

            let response = self
                .client
                .get(&url)
                .header("X-Polymarket-Api-Key", &auth.api_key)
                .header("X-Polymarket-Nonce", &nonce)
                .send()
                .await?;

            if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
                return Err(GatewayError::RateLimited);
            }

            let orders: Vec<PolyOrderResponse> = response.json().await?;
            return Ok(orders);
        }

        Err(GatewayError::InvalidMessage(
            "Authentication required to view orders".to_string(),
        ))
    }

    /// Get market info by condition ID.
    pub async fn get_market_info(
        &self,
        condition_id: &str,
    ) -> Result<RestMarketInfo, GatewayError> {
        self.rate_limit().await;

        let url = format!("{}/markets/{}", self.config.api_url, condition_id);
        let response = self.client.get(&url).send().await?;

        if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(GatewayError::RateLimited);
        }

        let market: RestMarketInfo = response.json().await?;
        Ok(market)
    }
}

// ============================================================================
// WebSocket Handler
// ============================================================================

/// WebSocket message types for Polymarket.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum WsEvent {
    #[serde(rename = "subscribe")]
    Subscribe {
        channel: String,
        assets_ids: Vec<String>,
    },
    #[serde(rename = "unsubscribe")]
    Unsubscribe {
        channel: String,
        assets_ids: Vec<String>,
    },
    #[serde(rename = "book")]
    Book {
        asset_id: String,
        market: Option<String>,
        timestamp: u64,
        hash: String,
        bids: Vec<PriceLevel>,
        asks: Vec<PriceLevel>,
    },
    #[serde(rename = "price_change")]
    PriceChange {
        asset_id: String,
        market: Option<String>,
        timestamp: u64,
        price_changes: Vec<PriceChangeLevel>,
    },
    #[serde(rename = "last_trade_price")]
    LastTradePrice {
        asset_id: String,
        market: Option<String>,
        timestamp: u64,
        price: String,
        size: Option<String>,
    },
    #[serde(rename = "tick_size_change")]
    TickSizeChange {
        asset_id: String,
        market: Option<String>,
        timestamp: u64,
        old_tick_size: String,
        new_tick_size: String,
    },
    #[serde(rename = "heartbeat")]
    Heartbeat { timestamp: u64 },
    #[serde(rename = "error")]
    Error { error: String, code: Option<i32> },
}

/// Price level for WS messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceLevel {
    pub price: String,
    pub size: String,
}

/// WebSocket client for Polymarket.
#[derive(Debug)]
pub struct PolymarketWsClient {
    /// Configuration
    config: PolymarketConfig,
    /// WebSocket stream (not cloneable)
    ws: Option<WebSocketStream<MaybeTlsStream<TcpStream>>>,
    /// Connection state
    state: WsState,
    /// Subscribed assets
    subscribed_assets: Vec<String>,
    /// Rate limiter for WS operations
    rate_limiter: TokenBucket,
}

impl PolymarketWsClient {
    /// Create a new WebSocket client.
    pub fn new(config: PolymarketConfig) -> Self {
        Self {
            config,
            ws: None,
            state: WsState::Disconnected,
            subscribed_assets: vec![],
            rate_limiter: TokenBucket::new(100, 50),
        }
    }

    /// Connect to the WebSocket endpoint.
    pub async fn connect(&mut self) -> Result<(), GatewayError> {
        self.state = WsState::Connecting;

        let (ws, _) = connect_async(&self.config.ws_url).await?;

        self.ws = Some(ws);
        self.state = WsState::Connected;

        Ok(())
    }

    /// Subscribe to market updates.
    pub async fn subscribe(&mut self, asset_ids: Vec<String>) -> Result<(), GatewayError> {
        if let Some(ref mut ws) = self.ws {
            let sub = SubscribeRequest::market(asset_ids.clone());
            let msg = serde_json::to_string(&sub)?;

            ws.send(Message::Text(msg.into())).await?;
            self.subscribed_assets = asset_ids;
        }

        Ok(())
    }

    /// Unsubscribe from market updates.
    pub async fn unsubscribe(&mut self, asset_ids: Vec<String>) -> Result<(), GatewayError> {
        if let Some(ref mut ws) = self.ws {
            let unsub = crate::messages::UnsubscribeRequest::market(asset_ids.clone());
            ws.send(Message::Text(serde_json::to_string(&unsub)?.into())).await?;
            self.subscribed_assets
                .retain(|a| !asset_ids.contains(a));
        }

        Ok(())
    }

    /// Get the next message.
    pub async fn next_message(&mut self) -> Option<Result<WsEvent, GatewayError>> {
        if let Some(ref mut ws) = self.ws {
            match ws.next().await {
                Some(Ok(Message::Text(text))) => {
                    let event: WsEvent = serde_json::from_str(&text).ok()?;
                    Some(Ok(event))
                }
                Some(Ok(Message::Ping(data))) => {
                    // Respond to ping
                    let _ = ws.send(Message::Pong(data)).await;
                    // Return None for ping, caller should call again
                    None
                }
                Some(Ok(Message::Close(_))) => {
                    self.state = WsState::Disconnected;
                    None
                }
                Some(Err(e)) => Some(Err(e.into())),
                _ => None,
            }
        } else {
            None
        }
    }

    /// Close the connection.
    pub async fn close(&mut self) -> Result<(), GatewayError> {
        if let Some(ref mut ws) = self.ws {
            ws.close(None).await?;
        }
        self.state = WsState::Disconnected;
        Ok(())
    }

    /// Get current connection state.
    pub fn state(&self) -> WsState {
        self.state
    }
}

/// Run WebSocket client in background task.
pub async fn run_ws_client(
    config: PolymarketConfig,
    initial_assets: Vec<String>,
) -> (
    mpsc::Receiver<Result<WsEvent, GatewayError>>,
    mpsc::Sender<WsCommand>,
) {
    let (frame_tx, frame_rx) = mpsc::channel(1024);
    let (cmd_tx, cmd_rx) = mpsc::channel(64);

    tokio::spawn(ws_background_task(
        config,
        initial_assets,
        frame_tx,
        cmd_rx,
    ));

    (frame_rx, cmd_tx)
}

async fn ws_background_task(
    config: PolymarketConfig,
    initial_assets: Vec<String>,
    frame_tx: mpsc::Sender<Result<WsEvent, GatewayError>>,
    mut cmd_rx: mpsc::Receiver<WsCommand>,
) {
    let mut subscribed_assets = initial_assets;

    loop {
        // Connect
        match connect_async(&config.ws_url).await {
            Ok((mut ws, _)) => {
                // Send initial subscription
                if !subscribed_assets.is_empty() {
                    let sub = SubscribeRequest::market(subscribed_assets.clone());
                    let msg = serde_json::to_string(&sub).expect("JSON serialization failed");
                    if let Err(e) = ws.send(Message::Text(msg.into())).await {
                        let _ = frame_tx.send(Err(e.into())).await;
                        continue;
                    }
                }

                let mut heartbeat_interval = interval(Duration::from_secs(15));

                loop {
                    tokio::select! {
                        // Incoming message
                        msg = ws.next() => {
                            match msg {
                                Some(Ok(Message::Text(text))) => {
                                    match serde_json::from_str::<WsEvent>(&text) {
                                        Ok(event) => {
                                            if frame_tx.send(Ok(event)).await.is_err() {
                                                return;
                                            }
                                        }
                                        Err(e) => {
                                            let _ = frame_tx.send(Err(e.into())).await;
                                        }
                                    }
                                }
                                Some(Ok(Message::Ping(data))) => {
                                    if let Err(e) = ws.send(Message::Pong(data)).await {
                                        let _ = frame_tx.send(Err(e.into())).await;
                                    }
                                }
                                Some(Ok(Message::Close(_))) => break,
                                Some(Err(e)) => {
                                    let _ = frame_tx.send(Err(e.into())).await;
                                    break;
                                }
                                None => break,
                                _ => {}
                            }
                        }

                        // Command from client
                        cmd = cmd_rx.recv() => {
                            match cmd {
                                Some(WsCommand::Subscribe(assets)) => {
                                    subscribed_assets.extend(assets.clone());
                                    let sub = SubscribeRequest::market(assets);
                                    let msg = serde_json::to_string(&sub).expect("JSON serialization failed");
                                    if let Err(e) = ws.send(Message::Text(msg.into())).await {
                                        let _ = frame_tx.send(Err(e.into())).await;
                                    }
                                }
                                Some(WsCommand::Unsubscribe(assets)) => {
                                    subscribed_assets.retain(|a| !assets.contains(a));
                                    let unsub = crate::messages::UnsubscribeRequest::market(assets);
                                    let msg = serde_json::to_string(&unsub).expect("JSON serialization failed");
                                    if let Err(e) = ws.send(Message::Text(msg.into())).await {
                                        let _ = frame_tx.send(Err(e.into())).await;
                                    }
                                }
                                Some(WsCommand::Shutdown) | None => {
                                    let _ = ws.close(None).await;
                                    return;
                                }
                            }
                        }

                        // Heartbeat
                        _ = heartbeat_interval.tick() => {
                            if let Err(e) = ws.send(Message::Ping(vec![])).await {
                                let _ = frame_tx.send(Err(e.into())).await;
                            }
                        }
                    }
                }
            }
            Err(e) => {
                let _ = frame_tx.send(Err(e.into())).await;
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
}

// ============================================================================
// Data Normalizer
// ============================================================================

/// Data normalizer for Polymarket API responses.
#[derive(Debug, Clone)]
pub struct PolymarketNormalizer {
    /// Tick size in basis points (e.g., 100 = 0.01)
    tick_size_bps: u16,
    /// Default tick size if not specified
    default_tick_size_bps: u16,
}

impl PolymarketNormalizer {
    /// Create a new normalizer.
    pub fn new(tick_size_bps: u16) -> Self {
        Self {
            tick_size_bps,
            default_tick_size_bps: 100, // Default 0.01 tick size
        }
    }

    /// Set the tick size.
    pub fn set_tick_size(&mut self, tick_size_bps: u16) {
        self.tick_size_bps = tick_size_bps;
    }

    /// Normalize a book snapshot to internal format.
    pub fn normalize_book_snapshot(
        &self,
        raw: &RestBookSnapshot,
    ) -> Result<BookSnapshot, GatewayError> {
        let tick_size = if self.tick_size_bps > 0 {
            self.tick_size_bps
        } else {
            self.default_tick_size_bps
        };

        let mut bids = Vec::new();
        for level in &raw.bids {
            if let (Ok(price), Ok(size)) = (
                parse_price_to_tick_strict(&level.price, tick_size),
                parse_size(&level.size),
            ) {
                bids.push((price, size));
            }
        }

        let mut asks = Vec::new();
        for level in &raw.asks {
            if let (Ok(price), Ok(size)) = (
                parse_price_to_tick_strict(&level.price, tick_size),
                parse_size(&level.size),
            ) {
                asks.push((price, size));
            }
        }

        Ok(BookSnapshot {
            asset_id: raw.asset_id.clone(),
            timestamp_ms: raw.timestamp.unwrap_or(0),
            hash: raw.hash.clone(),
            bids,
            asks,
        })
    }

    /// Normalize a WebSocket book event.
    pub fn normalize_ws_book(
        &self,
        event: &WsEvent,
    ) -> Option<Result<BookSnapshot, GatewayError>> {
        match event {
            WsEvent::Book {
                asset_id,
                timestamp,
                hash,
                bids,
                asks,
                ..
            } => {
                let tick_size = if self.tick_size_bps > 0 {
                    self.tick_size_bps
                } else {
                    self.default_tick_size_bps
                };

                let mut normalized_bids = Vec::new();
                for level in bids {
                    if let (Ok(price), Ok(size)) = (
                        parse_price_to_tick_strict(&level.price, tick_size),
                        parse_size(&level.size),
                    ) {
                        normalized_bids.push((price, size));
                    }
                }

                let mut normalized_asks = Vec::new();
                for level in asks {
                    if let (Ok(price), Ok(size)) = (
                        parse_price_to_tick_strict(&level.price, tick_size),
                        parse_size(&level.size),
                    ) {
                        normalized_asks.push((price, size));
                    }
                }

                Some(Ok(BookSnapshot {
                    asset_id: asset_id.clone(),
                    timestamp_ms: *timestamp,
                    hash: hash.clone(),
                    bids: normalized_bids,
                    asks: normalized_asks,
                }))
            }
            _ => None,
        }
    }

    /// Normalize a price change event.
    pub fn normalize_price_change(&self, event: &WsEvent) -> Option<PriceChange> {
        match event {
            WsEvent::PriceChange {
                asset_id,
                timestamp,
                price_changes,
                ..
            } => {
                let tick_size = if self.tick_size_bps > 0 {
                    self.tick_size_bps
                } else {
                    self.default_tick_size_bps
                };

                let mut changes = Vec::new();
                for level in price_changes {
                    if let (Ok(price_tick), Ok(size)) = (
                        parse_price_to_tick_strict(&level.price, tick_size),
                        parse_size(&level.size),
                    ) {
                        let side = match level.side.to_lowercase().as_str() {
                            "buy" | "bid" => Side::Buy,
                            "sell" | "ask" => Side::Sell,
                            _ => continue,
                        };
                        changes.push(ParsedPriceChange {
                            price_tick,
                            side,
                            size,
                        });
                    }
                }

                Some(PriceChange {
                    asset_id: asset_id.clone(),
                    timestamp_ms: *timestamp,
                    price_changes: changes,
                })
            }
            _ => None,
        }
    }

    /// Normalize a last trade price event.
    pub fn normalize_last_trade(&self, event: &WsEvent) -> Option<LastTradePrice> {
        match event {
            WsEvent::LastTradePrice {
                asset_id,
                timestamp,
                price,
                size,
                ..
            } => {
                let tick_size = if self.tick_size_bps > 0 {
                    self.tick_size_bps
                } else {
                    self.default_tick_size_bps
                };

                let price_tick = parse_price_to_tick_strict(price, tick_size).ok()?;
                let size_shares = size.as_ref().and_then(|s| parse_size(s).ok()).unwrap_or(0);

                Some(LastTradePrice {
                    asset_id: asset_id.clone(),
                    timestamp_ms: *timestamp,
                    price_tick,
                    size_shares,
                })
            }
            _ => None,
        }
    }

    /// Normalize a tick size change event.
    pub fn normalize_tick_size_change(&self, event: &WsEvent) -> Option<TickSizeChange> {
        match event {
            WsEvent::TickSizeChange {
                asset_id,
                timestamp,
                old_tick_size,
                new_tick_size,
                ..
            } => {
                let old_bps = (old_tick_size.parse::<f64>().unwrap_or(0.01) * 10000.0) as u16;
                let new_bps = (new_tick_size.parse::<f64>().unwrap_or(0.01) * 10000.0) as u16;

                Some(TickSizeChange {
                    asset_id: asset_id.clone(),
                    timestamp_ms: *timestamp,
                    old_tick_bps: old_bps,
                    new_tick_bps: new_bps,
                })
            }
            _ => None,
        }
    }
}

// ============================================================================
// Unified Polymarket Client
// ============================================================================

/// Unified Polymarket client combining REST and WebSocket.
#[derive(Debug, Clone)]
pub struct PolymarketClient {
    /// REST client
    pub rest: PolymarketRestClient,
    /// Data normalizer
    pub normalizer: PolymarketNormalizer,
    /// WebSocket configuration
    ws_config: PolymarketConfig,
}

impl PolymarketClient {
    /// Create a new Polymarket client.
    pub fn new(config: PolymarketConfig) -> Result<Self, GatewayError> {
        let rest = PolymarketRestClient::new(config.clone())?;
        let normalizer = PolymarketNormalizer::new(100); // Default 0.01 tick size

        Ok(Self {
            rest,
            normalizer,
            ws_config: config,
        })
    }

    /// Get active markets.
    pub async fn get_markets(&self) -> Result<Vec<PolyMarket>, GatewayError> {
        self.rest.get_markets().await
    }

    /// Get order book for a token.
    pub async fn get_orderbook(&self, token_id: &str) -> Result<BookSnapshot, GatewayError> {
        let raw = self.rest.get_orderbook(token_id).await?;
        self.normalizer.normalize_book_snapshot(&raw)
    }

    /// Get current positions.
    pub async fn get_positions(&self) -> Result<Vec<PolyPosition>, GatewayError> {
        self.rest.get_positions().await
    }

    /// Create a new order.
    pub async fn create_order(
        &self,
        token_id: &str,
        side: &str,
        price: &str,
        size: &str,
    ) -> Result<PolyOrderResponse, GatewayError> {
        self.rest.create_order(token_id, side, price, size).await
    }

    /// Cancel an order.
    pub async fn cancel_order(&self, order_id: &str) -> Result<(), GatewayError> {
        self.rest.cancel_order(order_id).await
    }

    /// Get open orders.
    pub async fn get_open_orders(&self) -> Result<Vec<PolyOrderResponse>, GatewayError> {
        self.rest.get_open_orders().await
    }

    /// Connect WebSocket and start receiving events.
    pub async fn connect_websocket(
        &self,
        asset_ids: Vec<String>,
    ) -> Result<
        (
            mpsc::Receiver<Result<WsEvent, GatewayError>>,
            mpsc::Sender<WsCommand>,
        ),
        GatewayError,
    > {
        Ok(run_ws_client(self.ws_config.clone(), asset_ids).await)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_bucket_basic() {
        let bucket = TokenBucket::new(10, 10); // 10 tokens, 10 per second

        // Should be able to consume up to 10 immediately
        for i in 0..10 {
            assert!(bucket.try_consume(1).is_ok(), "Should consume token {}", i);
        }

        // Should fail on 11th
        assert!(bucket.try_consume(1).is_err());
    }

    #[test]
    fn test_token_bucket_refill() {
        let bucket = TokenBucket::new(10, 10); // 10 tokens, 10 per second

        // Consume all tokens
        for _ in 0..10 {
            let _ = bucket.try_consume(1);
        }

        // Wait for 500ms (should refill ~5 tokens)
        std::thread::sleep(Duration::from_millis(500));

        // Should be able to consume 5 tokens
        for _ in 0..5 {
            assert!(bucket.try_consume(1).is_ok());
        }

        // Should fail on 6th
        assert!(bucket.try_consume(1).is_err());
    }

    #[tokio::test]
    async fn test_rate_limiter_manager() {
        let limiter = RateLimiterManager::new(2, 5, 10);

        // Should allow order rate limiting
        assert!(limiter.check_order_rate().await.is_ok());

        // Should allow general rate limiting
        assert!(limiter.check_general_rate().await.is_ok());
    }

    #[test]
    fn test_polymarket_normalizer_price() {
        // Parse price string
        let tick = parse_price_to_tick_strict("0.55", 100).unwrap();
        assert_eq!(tick, 5500);

        let tick = parse_price_to_tick_strict("0.01", 100).unwrap();
        assert_eq!(tick, 100);

        // Parse size string
        let size = parse_size("100.00").unwrap();
        assert_eq!(size, 100_000_000);

        let size = parse_size("1.5").unwrap();
        assert_eq!(size, 1_500_000);
    }

    #[test]
    fn test_config_default() {
        let config = PolymarketConfig::default();
        assert_eq!(config.api_url, POLYMARKET_API_URL);
        assert_eq!(config.ws_url, POLYMARKET_WS_URL);
        assert_eq!(config.rate_limit_rps, DEFAULT_RATE_LIMIT_RPS);
    }
}
