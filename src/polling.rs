use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{error, info};

use crate::provider::Provider;
use crate::tray::TrkrTray;

#[derive(Debug)]
#[allow(dead_code)]
pub enum PollCommand {
    RefreshNow,
    SetInterval(Duration),
    Quit,
}

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
        // Do a fetch
        do_fetch(&provider, &tray_handle).await;

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

async fn do_fetch(
    provider: &Arc<dyn Provider>,
    tray_handle: &ksni::Handle<TrkrTray>,
) {
    match provider.fetch_usage().await {
        Ok(snapshot) => {
            tray_handle
                .update(|tray| {
                    tray.snapshot = Some(snapshot);
                    tray.error = None;
                })
                .await;
        }
        Err(e) => {
            error!("Fetch failed: {:#}", e);
            let err_msg = format!("{:#}", e);
            tray_handle
                .update(|tray| {
                    tray.error = Some(err_msg);
                })
                .await;
        }
    }
}
