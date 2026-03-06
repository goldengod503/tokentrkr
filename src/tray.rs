use chrono::Utc;
use ksni;
use ksni::menu::{StandardItem, MenuItem};
use tokio::sync::mpsc;
use tracing::debug;

use crate::icon::build_icon;
use crate::models::UsageSnapshot;
use crate::polling::PollCommand;

pub struct TrkrTray {
    pub snapshot: Option<UsageSnapshot>,
    pub error: Option<String>,
    cmd_tx: mpsc::Sender<PollCommand>,
}

impl TrkrTray {
    pub fn new(cmd_tx: mpsc::Sender<PollCommand>) -> Self {
        Self {
            snapshot: None,
            error: None,
            cmd_tx,
        }
    }

    fn tooltip_text(&self) -> String {
        if let Some(ref s) = self.snapshot {
            if let Some(ref p) = s.primary {
                return format!("Session: {:.0}% used", p.used_percent);
            }
        }
        if let Some(ref e) = self.error {
            return format!("Error: {}", e);
        }
        "TokenTrkr — Loading...".to_string()
    }
}

impl ksni::Tray for TrkrTray {
    const MENU_ON_ACTIVATE: bool = true;

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        let pct = self
            .snapshot
            .as_ref()
            .and_then(|s| s.primary.as_ref())
            .map(|w| w.used_percent);
        vec![build_icon(pct)]
    }

    fn title(&self) -> String {
        "TokenTrkr".to_string()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: "TokenTrkr".to_string(),
            description: self.tooltip_text(),
            icon_name: String::new(),
            icon_pixmap: Vec::new(),
        }
    }

    fn id(&self) -> String {
        "tokentrkr".to_string()
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let mut items: Vec<MenuItem<Self>> = Vec::new();

        // Title
        items.push(
            StandardItem {
                label: "TokenTrkr".to_string(),
                enabled: false,
                ..Default::default()
            }
            .into(),
        );

        items.push(MenuItem::Separator);

        if let Some(ref snapshot) = self.snapshot {
            // Primary (Session/5h)
            if let Some(ref w) = snapshot.primary {
                items.push(
                    StandardItem {
                        label: w.label.clone(),
                        enabled: false,
                        ..Default::default()
                    }
                    .into(),
                );
                items.push(
                    StandardItem {
                        label: format!("  {}", w.format_bar(12)),
                        enabled: false,
                        ..Default::default()
                    }
                    .into(),
                );
                items.push(
                    StandardItem {
                        label: format!("  {}", w.format_reset_time()),
                        enabled: false,
                        ..Default::default()
                    }
                    .into(),
                );
                items.push(MenuItem::Separator);
            }

            // Secondary (Weekly/7d)
            if let Some(ref w) = snapshot.secondary {
                items.push(
                    StandardItem {
                        label: w.label.clone(),
                        enabled: false,
                        ..Default::default()
                    }
                    .into(),
                );
                items.push(
                    StandardItem {
                        label: format!("  {}", w.format_bar(12)),
                        enabled: false,
                        ..Default::default()
                    }
                    .into(),
                );
                items.push(
                    StandardItem {
                        label: format!("  {}", w.format_reset_time()),
                        enabled: false,
                        ..Default::default()
                    }
                    .into(),
                );
                items.push(MenuItem::Separator);
            }

            // Tertiary (Sonnet/7d)
            if let Some(ref w) = snapshot.tertiary {
                items.push(
                    StandardItem {
                        label: w.label.clone(),
                        enabled: false,
                        ..Default::default()
                    }
                    .into(),
                );
                items.push(
                    StandardItem {
                        label: format!("  {}", w.format_bar(12)),
                        enabled: false,
                        ..Default::default()
                    }
                    .into(),
                );
                items.push(
                    StandardItem {
                        label: format!("  {}", w.format_reset_time()),
                        enabled: false,
                        ..Default::default()
                    }
                    .into(),
                );
                items.push(MenuItem::Separator);
            }

            // Extra usage
            if let Some(ref extra) = snapshot.extra_usage {
                if extra.is_enabled {
                    items.push(
                        StandardItem {
                            label: format!(
                                "Extra: ${:.2} / ${:.2} {}",
                                extra.used_credits, extra.monthly_limit, extra.currency
                            ),
                            enabled: false,
                            ..Default::default()
                        }
                        .into(),
                    );
                    items.push(MenuItem::Separator);
                }
            }

            // Account info
            if let Some(ref identity) = snapshot.identity {
                if let Some(ref email) = identity.email {
                    items.push(
                        StandardItem {
                            label: format!("Account: {}", email),
                            enabled: false,
                            ..Default::default()
                        }
                        .into(),
                    );
                }
                if let Some(ref plan) = identity.plan {
                    items.push(
                        StandardItem {
                            label: format!("Plan: {}", plan),
                            enabled: false,
                            ..Default::default()
                        }
                        .into(),
                    );
                }
            }

            // Updated time
            let ago = Utc::now()
                .signed_duration_since(snapshot.updated_at)
                .num_seconds();
            let updated_text = if ago < 60 {
                "Updated: just now".to_string()
            } else {
                format!("Updated: {} min ago", ago / 60)
            };
            items.push(
                StandardItem {
                    label: updated_text,
                    enabled: false,
                    ..Default::default()
                }
                .into(),
            );
        } else if let Some(ref e) = self.error {
            items.push(
                StandardItem {
                    label: format!("Error: {}", e),
                    enabled: false,
                    ..Default::default()
                }
                .into(),
            );
        } else {
            items.push(
                StandardItem {
                    label: "Loading...".to_string(),
                    enabled: false,
                    ..Default::default()
                }
                .into(),
            );
        }

        items.push(MenuItem::Separator);

        // Refresh Now
        let cmd_tx = self.cmd_tx.clone();
        items.push(
            StandardItem {
                label: "Refresh Now".to_string(),
                activate: Box::new(move |_tray: &mut Self| {
                    debug!("Refresh requested from menu");
                    let _ = cmd_tx.try_send(PollCommand::RefreshNow);
                }),
                ..Default::default()
            }
            .into(),
        );

        // Open Dashboard
        items.push(
            StandardItem {
                label: "Open Dashboard".to_string(),
                activate: Box::new(|_tray: &mut Self| {
                    let _ = std::process::Command::new("xdg-open")
                        .arg("https://claude.ai/settings/usage")
                        .spawn();
                }),
                ..Default::default()
            }
            .into(),
        );

        // Quit
        items.push(
            StandardItem {
                label: "Quit".to_string(),
                activate: Box::new(move |_tray: &mut Self| {
                    std::process::exit(0);
                }),
                ..Default::default()
            }
            .into(),
        );

        items
    }
}
