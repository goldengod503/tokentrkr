mod claude;
mod config;
mod models;
mod polling;
mod provider;
mod icon;
mod tray;

use std::fmt;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::info;
use tracing_subscriber::EnvFilter;

use claude::ClaudeProvider;
use config::Config;
use ksni::TrayMethods;
use polling::{run_poll_loop, PollCommand};
use provider::Provider;
use tray::TrkrTray;

/// Sentinel error for 429 responses so the polling loop can retry with backoff.
#[derive(Debug)]
pub struct RateLimited;

impl fmt::Display for RateLimited {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Rate limited (429)")
    }
}

impl std::error::Error for RateLimited {}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    info!("TokenTrkr starting");

    // Load config
    let config = Config::load()?;
    info!(
        "Config loaded, poll interval: {}m",
        config.general.poll_interval_minutes
    );

    // Create provider
    let provider: Arc<dyn Provider> = Arc::new(ClaudeProvider::new(&config)?);

    // Command channel
    let (cmd_tx, cmd_rx) = mpsc::channel::<PollCommand>(16);

    // Create and spawn tray
    let tray = TrkrTray::new(cmd_tx);
    let tray_handle = tray.spawn().await.expect("Failed to create system tray");

    info!("System tray started");

    // Run polling loop (blocks forever)
    let poll_interval = config.poll_interval();
    run_poll_loop(provider, cmd_rx, poll_interval, tray_handle).await;

    Ok(())
}
