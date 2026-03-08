mod claude;
mod config;
mod history;
mod models;
mod polling;
mod provider;
mod icon;
mod tray;
#[cfg(feature = "cosmic")]
mod cosmic_app;

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

fn is_cosmic() -> bool {
    std::env::var("XDG_CURRENT_DESKTOP")
        .map(|v| v.to_uppercase().contains("COSMIC"))
        .unwrap_or(false)
}

fn main() -> anyhow::Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    info!("TokenTrkr starting");

    if is_cosmic() {
        #[cfg(feature = "cosmic")]
        {
            info!("COSMIC desktop detected, using native applet");
            return cosmic_app::run();
        }
        #[cfg(not(feature = "cosmic"))]
        {
            info!("COSMIC desktop detected but cosmic feature not compiled, using SNI tray");
        }
    } else {
        info!("Using SNI tray (compatible with GNOME, KDE, COSMIC, etc.)");
    }

    // SNI tray path — build a tokio runtime
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(run_sni())
}

async fn run_sni() -> anyhow::Result<()> {
    let config = Config::load()?;
    info!(
        "Config loaded, poll interval: {}m",
        config.general.poll_interval_minutes
    );

    let provider: Arc<dyn Provider> = Arc::new(ClaudeProvider::new(&config)?);
    let (cmd_tx, cmd_rx) = mpsc::channel::<PollCommand>(16);

    let tray = TrkrTray::new(cmd_tx);
    let tray_handle = tray.spawn().await.expect("Failed to create system tray");

    info!("System tray started");

    let poll_interval = config.poll_interval();
    run_poll_loop(provider, cmd_rx, poll_interval, tray_handle).await;

    Ok(())
}
