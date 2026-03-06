use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use crate::provider::Provider;
use crate::tray::TrkrTray;
use crate::RateLimited;

#[derive(Debug)]
#[allow(dead_code)]
pub enum PollCommand {
    RefreshNow,
    SetInterval(Duration),
    Quit,
}

const RETRY_DELAYS: &[Duration] = &[
    Duration::from_secs(10),
    Duration::from_secs(30),
    Duration::from_secs(60),
];

pub async fn run_poll_loop(
    provider: Arc<dyn Provider>,
    mut cmd_rx: mpsc::Receiver<PollCommand>,
    initial_interval: Duration,
    tray_handle: ksni::Handle<TrkrTray>,
) {
    let mut interval = tokio::time::interval(initial_interval);
    // Fetch immediately on startup
    interval.tick().await;

    loop {
        // Do a fetch, with retry on 429
        do_fetch_with_retry(&provider, &tray_handle).await;

        // Wait for next tick or command
        tokio::select! {
            _ = interval.tick() => {
                // Time for a regular fetch
            }
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(PollCommand::RefreshNow) => {
                        info!("Manual refresh requested");
                        interval.reset();
                        continue;
                    }
                    Some(PollCommand::SetInterval(dur)) => {
                        info!("Poll interval changed to {:?}", dur);
                        interval = tokio::time::interval(dur);
                        interval.tick().await; // consume immediate tick
                    }
                    Some(PollCommand::Quit) | None => {
                        info!("Quit command received, shutting down");
                        tray_handle.shutdown().await;
                        std::process::exit(0);
                    }
                }
            }
        }
    }
}

async fn do_fetch_with_retry(
    provider: &Arc<dyn Provider>,
    tray_handle: &ksni::Handle<TrkrTray>,
) {
    for (attempt, delay) in std::iter::once(&Duration::ZERO)
        .chain(RETRY_DELAYS.iter())
        .enumerate()
    {
        if attempt > 0 {
            warn!("Rate limited, retrying in {}s (attempt {}/{})",
                delay.as_secs(), attempt + 1, RETRY_DELAYS.len() + 1);
            tokio::time::sleep(*delay).await;
        }

        match provider.fetch_usage().await {
            Ok(snapshot) => {
                tray_handle
                    .update(|tray| {
                        tray.snapshot = Some(snapshot);
                        tray.error = None;
                    })
                    .await;
                return;
            }
            Err(e) if e.downcast_ref::<RateLimited>().is_some() => {
                // Update tray to show we're retrying (only on first hit)
                if attempt == 0 {
                    tray_handle
                        .update(|tray| {
                            tray.error = Some("Rate limited, retrying...".to_string());
                        })
                        .await;
                }
                continue;
            }
            Err(e) => {
                error!("Fetch failed: {:#}", e);
                let err_msg = format!("{:#}", e);
                tray_handle
                    .update(|tray| {
                        tray.error = Some(err_msg);
                    })
                    .await;
                return;
            }
        }
    }

    // All retries exhausted
    error!("All retry attempts exhausted after rate limiting");
    tray_handle
        .update(|tray| {
            tray.error = Some("Rate limited — will retry next poll".to_string());
        })
        .await;
}
