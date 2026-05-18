mod claude;
mod config;
mod history;
mod models;
mod provider;
mod icon;
mod tray;
mod usage;
#[cfg(feature = "cosmic")]
mod cosmic_app;

use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::info;
use tracing_subscriber::EnvFilter;

use crate::claude::ClaudeProvider;
use crate::config::Config;
use crate::provider::Provider;
use crate::tray::TrkrTray;
use crate::usage::{UsageEvent, UsageHandle, UsageService};
use ksni::TrayMethods;

fn is_cosmic() -> bool {
    std::env::var("XDG_CURRENT_DESKTOP")
        .map(|v| v.to_uppercase().contains("COSMIC"))
        .unwrap_or(false)
}

fn main() -> anyhow::Result<()> {
    if std::env::var_os("TOKIO_WORKER_THREADS").is_none() {
        std::env::set_var("TOKIO_WORKER_THREADS", "2");
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    info!("TokenTrkr starting");

    let config = Config::load()?;
    info!("Config loaded, poll interval: {}m", config.general.poll_interval_minutes);

    let provider: Arc<dyn Provider> = Arc::new(ClaudeProvider::new(&config)?);
    let poll_interval = Duration::from_secs(config.general.poll_interval_minutes * 60);

    if is_cosmic() {
        #[cfg(feature = "cosmic")]
        {
            info!("COSMIC desktop detected, using native applet");
            return cosmic_app::run(provider, poll_interval);
        }
        #[cfg(not(feature = "cosmic"))]
        {
            info!("COSMIC desktop detected but cosmic feature not compiled, using SNI tray");
        }
    } else {
        info!("Using SNI tray (compatible with GNOME, KDE, COSMIC, etc.)");
    }

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(run_sni(provider, poll_interval))
}

async fn run_sni(provider: Arc<dyn Provider>, poll_interval: Duration) -> anyhow::Result<()> {
    let UsageHandle { events, refresh, .. } = UsageService::new(provider, poll_interval).spawn();

    let tray = TrkrTray::new(refresh);
    let tray_handle: ksni::Handle<TrkrTray> = tray.spawn().await.expect("Failed to create system tray");

    sni_event_loop(events, tray_handle).await;
    Ok(())
}

async fn sni_event_loop(
    mut events: mpsc::Receiver<UsageEvent>,
    tray_handle: ksni::Handle<TrkrTray>,
) {
    while let Some(event) = events.recv().await {
        tray_handle
            .update(|tray| tray.apply_event(&event))
            .await;
    }
    // events channel closed: service was dropped. Shutdown the tray.
    tray_handle.shutdown().await;
    std::process::exit(0);
}
