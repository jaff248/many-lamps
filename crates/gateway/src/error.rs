//! Gateway error types.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum GatewayError {
    #[error("WebSocket error: {0}")]
    WebSocket(#[from] tokio_tungstenite::tungstenite::Error),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Connection closed")]
    ConnectionClosed,

    #[error("Connection timeout")]
    ConnectionTimeout,

    #[error("Invalid message: {0}")]
    InvalidMessage(String),

    #[error("Unknown event type: {0}")]
    UnknownEventType(String),

    #[error("Channel send error")]
    ChannelSend,

    #[error("Rate limited")]
    RateLimited,
}
