//! Logging setup.

use anyhow::Result;
use std::sync::OnceLock;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

/// Global flag to track if subscriber has been initialized.
static SUBSCRIBER_INITIALIZED: OnceLock<bool> = OnceLock::new();

/// Check if tracing subscriber is already initialized.
pub fn is_initialized() -> bool {
    SUBSCRIBER_INITIALIZED.get().is_some()
}

/// Initialize logging.
/// 
/// Logs are written to stderr (not stdout) to prevent log messages from bleeding into TUI display.
/// This function is safe to call multiple times - subsequent calls are no-ops.
pub fn init(level: &str, json: bool) -> Result<()> {
    // Check if already initialized (only initialize once)
    if SUBSCRIBER_INITIALIZED.set(true).is_err() {
        // Already initialized, skip
        return Ok(());
    }

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level));

    if json {
        tracing_subscriber::registry()
            .with(filter)
            .with(fmt::layer().json().with_writer(std::io::stderr))
            .init();
    } else {
        tracing_subscriber::registry()
            .with(filter)
            .with(fmt::layer().with_target(true).with_thread_ids(true).with_writer(std::io::stderr))
            .init();
    }

    Ok(())
}
