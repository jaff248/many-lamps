//! WebSocket client for Polymarket CLOB.
//!
//! Handles connection, reconnection, and message routing.
//! Records raw frames before parsing.

use crate::error::GatewayError;
use crate::messages::{SubscribeRequest, UnsubscribeRequest};
use crate::parser::{ParsedFrame, Parser};
use futures_util::{SinkExt, StreamExt};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::{interval, timeout};
use tokio_tungstenite::{
    connect_async,
    tungstenite::Message,
    MaybeTlsStream, WebSocketStream,
};

/// WebSocket endpoint for Polymarket CLOB.
pub const WS_URL: &str = "wss://ws-subscriptions-clob.polymarket.com/ws/market";

/// Connection configuration.
#[derive(Debug, Clone)]
pub struct WsConfig {
    /// WebSocket URL (defaults to Polymarket production)
    pub url: String,
    /// Connection timeout
    pub connect_timeout: Duration,
    /// Read timeout (triggers reconnect if no messages)
    pub read_timeout: Duration,
    /// Time between heartbeat checks
    pub heartbeat_interval: Duration,
    /// Maximum reconnection attempts before giving up
    pub max_reconnect_attempts: u32,
    /// Delay between reconnection attempts
    pub reconnect_delay: Duration,
}

impl Default for WsConfig {
    fn default() -> Self {
        Self {
            url: WS_URL.to_string(),
            connect_timeout: Duration::from_secs(10),
            read_timeout: Duration::from_secs(30),
            heartbeat_interval: Duration::from_secs(15),
            max_reconnect_attempts: 10,
            reconnect_delay: Duration::from_secs(1),
        }
    }
}

/// State of the WebSocket connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WsState {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
    Failed,
}

/// WebSocket client handle.
///
/// Use this to control subscriptions and monitor connection state.
pub struct WsClient {
    config: WsConfig,
    state: WsState,
    parser: Parser,
    subscribed_assets: Vec<String>,
    /// Monotonic clock for timestamping
    clock: fn() -> u64,
}

/// Messages sent to the WebSocket task.
#[derive(Debug)]
pub enum WsCommand {
    Subscribe(Vec<String>),
    Unsubscribe(Vec<String>),
    Shutdown,
}

impl WsClient {
    pub fn new(config: WsConfig) -> Self {
        Self {
            config,
            state: WsState::Disconnected,
            parser: Parser::new(),
            subscribed_assets: Vec::new(),
            clock: default_clock,
        }
    }

    /// Set a custom clock function for timestamping.
    pub fn with_clock(mut self, clock: fn() -> u64) -> Self {
        self.clock = clock;
        self
    }

    /// Set the tick size for parsing.
    pub fn set_tick_size_bps(&mut self, tick_size_bps: u16) {
        self.parser.set_tick_size_bps(tick_size_bps);
    }

    /// Get current connection state.
    pub fn state(&self) -> WsState {
        self.state
    }

    /// Run the WebSocket client, returning a channel of parsed frames.
    ///
    /// This spawns a background task that maintains the connection
    /// and handles reconnection. Returns channels for receiving frames
    /// and sending commands.
    pub fn run(
        self,
        initial_assets: Vec<String>,
    ) -> (
        mpsc::Receiver<Result<ParsedFrame, GatewayError>>,
        mpsc::Sender<WsCommand>,
    ) {
        let (frame_tx, frame_rx) = mpsc::channel(1024);
        let (cmd_tx, cmd_rx) = mpsc::channel(64);

        tokio::spawn(ws_task(self, initial_assets, frame_tx, cmd_rx));

        (frame_rx, cmd_tx)
    }
}

/// Background task that manages the WebSocket connection.
async fn ws_task(
    mut client: WsClient,
    initial_assets: Vec<String>,
    frame_tx: mpsc::Sender<Result<ParsedFrame, GatewayError>>,
    mut cmd_rx: mpsc::Receiver<WsCommand>,
) {
    client.subscribed_assets = initial_assets;
    let mut reconnect_attempts = 0u32;

    loop {
        // Connect
        client.state = WsState::Connecting;
        let ws = match timeout(client.config.connect_timeout, connect(&client.config.url)).await {
            Ok(Ok(ws)) => {
                client.state = WsState::Connected;
                reconnect_attempts = 0;
                ws
            }
            Ok(Err(e)) => {
                if let Err(_) = frame_tx.send(Err(e)).await {
                    return; // Receiver dropped
                }
                handle_reconnect(&mut client, &mut reconnect_attempts);
                continue;
            }
            Err(_) => {
                if let Err(_) = frame_tx.send(Err(GatewayError::ConnectionTimeout)).await {
                    return;
                }
                handle_reconnect(&mut client, &mut reconnect_attempts);
                continue;
            }
        };

        // Run connection loop
        let result = connection_loop(
            &mut client,
            ws,
            &frame_tx,
            &mut cmd_rx,
        )
        .await;

        match result {
            ConnectionResult::Shutdown => return,
            ConnectionResult::Error(e) => {
                let _ = frame_tx.send(Err(e)).await;
                handle_reconnect(&mut client, &mut reconnect_attempts);
            }
            ConnectionResult::Disconnected => {
                handle_reconnect(&mut client, &mut reconnect_attempts);
            }
        }

        if client.state == WsState::Failed {
            let _ = frame_tx
                .send(Err(GatewayError::ConnectionClosed))
                .await;
            return;
        }
    }
}

fn handle_reconnect(client: &mut WsClient, attempts: &mut u32) {
    *attempts += 1;
    if *attempts >= client.config.max_reconnect_attempts {
        client.state = WsState::Failed;
    } else {
        client.state = WsState::Reconnecting;
        // Sleep happens at start of next loop iteration via tokio::time::sleep
    }
}

enum ConnectionResult {
    Shutdown,
    Error(GatewayError),
    Disconnected,
}

async fn connect(
    url: &str,
) -> Result<WebSocketStream<MaybeTlsStream<TcpStream>>, GatewayError> {
    let (ws, _) = connect_async(url).await?;
    Ok(ws)
}

async fn connection_loop(
    client: &mut WsClient,
    mut ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
    frame_tx: &mpsc::Sender<Result<ParsedFrame, GatewayError>>,
    cmd_rx: &mut mpsc::Receiver<WsCommand>,
) -> ConnectionResult {
    // Send initial subscription
    if !client.subscribed_assets.is_empty() {
        let sub = SubscribeRequest::market(client.subscribed_assets.clone());
        let msg = match serde_json::to_string(&sub) {
            Ok(s) => s,
            Err(e) => return ConnectionResult::Error(GatewayError::Json(e)),
        };
        if let Err(e) = ws.send(Message::Text(msg.into())).await {
            return ConnectionResult::Error(e.into());
        }
    }

    let mut heartbeat_interval = interval(client.config.heartbeat_interval);

    loop {
        tokio::select! {
            // Incoming message
            msg = timeout(client.config.read_timeout, ws.next()) => {
                match msg {
                    Ok(Some(Ok(Message::Text(text)))) => {
                        let ts = (client.clock)();
                        let frame = client.parser.parse_frame(text.as_bytes(), ts);
                        if frame_tx.send(frame).await.is_err() {
                            return ConnectionResult::Shutdown;
                        }
                    }
                    Ok(Some(Ok(Message::Binary(data)))) => {
                        let ts = (client.clock)();
                        let frame = client.parser.parse_frame(&data, ts);
                        if frame_tx.send(frame).await.is_err() {
                            return ConnectionResult::Shutdown;
                        }
                    }
                    Ok(Some(Ok(Message::Ping(data)))) => {
                        if let Err(e) = ws.send(Message::Pong(data)).await {
                            return ConnectionResult::Error(e.into());
                        }
                    }
                    Ok(Some(Ok(Message::Pong(_)))) => {
                        // Heartbeat response, connection is alive
                    }
                    Ok(Some(Ok(Message::Close(_)))) => {
                        return ConnectionResult::Disconnected;
                    }
                    Ok(Some(Ok(Message::Frame(_)))) => {
                        // Raw frame, ignore
                    }
                    Ok(Some(Err(e))) => {
                        return ConnectionResult::Error(e.into());
                    }
                    Ok(None) => {
                        return ConnectionResult::Disconnected;
                    }
                    Err(_) => {
                        // Read timeout - connection may be stale
                        return ConnectionResult::Disconnected;
                    }
                }
            }

            // Command from client
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(WsCommand::Subscribe(assets)) => {
                        for asset in &assets {
                            if !client.subscribed_assets.contains(asset) {
                                client.subscribed_assets.push(asset.clone());
                            }
                        }
                        let sub = SubscribeRequest::market(assets);
                        let msg = match serde_json::to_string(&sub) {
                            Ok(s) => s,
                            Err(e) => return ConnectionResult::Error(GatewayError::Json(e)),
                        };
                        if let Err(e) = ws.send(Message::Text(msg.into())).await {
                            return ConnectionResult::Error(e.into());
                        }
                    }
                    Some(WsCommand::Unsubscribe(assets)) => {
                        client.subscribed_assets.retain(|a| !assets.contains(a));
                        let unsub = UnsubscribeRequest::market(assets);
                        let msg = match serde_json::to_string(&unsub) {
                            Ok(s) => s,
                            Err(e) => return ConnectionResult::Error(GatewayError::Json(e)),
                        };
                        if let Err(e) = ws.send(Message::Text(msg.into())).await {
                            return ConnectionResult::Error(e.into());
                        }
                    }
                    Some(WsCommand::Shutdown) | None => {
                        let _ = ws.close(None).await;
                        return ConnectionResult::Shutdown;
                    }
                }
            }

            // Heartbeat tick
            _ = heartbeat_interval.tick() => {
                if let Err(e) = ws.send(Message::Ping(vec![])).await {
                    return ConnectionResult::Error(e.into());
                }
            }
        }
    }
}

/// Default clock using std::time.
fn default_clock() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default() {
        let config = WsConfig::default();
        assert_eq!(config.url, WS_URL);
        assert_eq!(config.max_reconnect_attempts, 10);
    }

    #[test]
    fn test_ws_state() {
        let client = WsClient::new(WsConfig::default());
        assert_eq!(client.state(), WsState::Disconnected);
    }
}
