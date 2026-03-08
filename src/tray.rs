use chrono::Utc;
use ksni;
use ksni::menu::{StandardItem, MenuItem};
use tokio::sync::mpsc;

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

    fn format_plan_name(raw: &str) -> String {
        let lower = raw.to_lowercase();
        if lower.contains("max") {
            "Max Plan".to_string()
        } else if lower.contains("pro") {
            "Pro Plan".to_string()
        } else if lower.contains("team") {
            "Team Plan".to_string()
        } else {
            let mut c = raw.chars();
            match c.next() {
                None => raw.to_string(),
                Some(f) => f.to_uppercase().to_string() + c.as_str(),
            }
        }
    }

    fn tooltip_text(&self) -> String {
        if let Some(ref s) = self.snapshot {
            let mut parts = Vec::new();
            if let Some(ref p) = s.primary {
                parts.push(format!("Session: {:.0}%", p.used_percent));
            }
            if let Some(ref w) = s.secondary {
                parts.push(format!("Weekly: {:.0}%", w.used_percent));
            }
            if !parts.is_empty() {
                return parts.join(" · ");
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

        // Title with plan name
        let title = if let Some(ref snap) = self.snapshot {
            if let Some(ref id) = snap.identity {
                if let Some(ref plan) = id.plan {
                    format!("TokenTrkr — {}", Self::format_plan_name(plan))
                } else {
                    "TokenTrkr".to_string()
                }
            } else {
                "TokenTrkr".to_string()
            }
        } else {
            "TokenTrkr".to_string()
        };
        items.push(
            StandardItem {
                label: title,
                enabled: false,
                ..Default::default()
            }
            .into(),
        );

        items.push(MenuItem::Separator);

        if let Some(ref snapshot) = self.snapshot {
            // Rate windows — 3-line format: label, bar + %, reset time
            for w in [&snapshot.primary, &snapshot.secondary, &snapshot.tertiary]
                .into_iter()
                .flatten()
            {
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
                        label: format!("  {}  {:.0}%", w.format_bar(12), w.used_percent),
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

            // Per-model breakdown
            if !snapshot.model_windows.is_empty() {
                items.push(
                    StandardItem {
                        label: "Per-Model Usage".to_string(),
                        enabled: false,
                        ..Default::default()
                    }
                    .into(),
                );
                for w in &snapshot.model_windows {
                    items.push(
                        StandardItem {
                            label: format!(
                                "  {} {}  {:.0}%",
                                w.label,
                                w.format_bar(8),
                                w.used_percent
                            ),
                            enabled: false,
                            ..Default::default()
                        }
                        .into(),
                    );
                }
                items.push(MenuItem::Separator);
            }

            // Extra usage
            if let Some(ref extra) = snapshot.extra_usage {
                if extra.is_enabled && extra.monthly_limit > 0.0 {
                    let pct = (extra.used_credits / extra.monthly_limit * 100.0).min(100.0);
                    let filled = ((pct / 100.0) * 12.0).round() as usize;
                    let bar: String = "▓".repeat(filled.min(12))
                        + &"░".repeat(12 - filled.min(12));
                    items.push(
                        StandardItem {
                            label: format!(
                                "Extra Usage          ${:.2} / ${:.2}",
                                extra.used_credits, extra.monthly_limit
                            ),
                            enabled: false,
                            ..Default::default()
                        }
                        .into(),
                    );
                    items.push(
                        StandardItem {
                            label: format!("  {}", bar),
                            enabled: false,
                            ..Default::default()
                        }
                        .into(),
                    );
                    items.push(MenuItem::Separator);
                }
            }

            // Footer: account + updated time
            if let Some(ref identity) = snapshot.identity {
                if let Some(ref email) = identity.email {
                    items.push(
                        StandardItem {
                            label: email.clone(),
                            enabled: false,
                            ..Default::default()
                        }
                        .into(),
                    );
                }
            }

            let ago = Utc::now()
                .signed_duration_since(snapshot.updated_at)
                .num_seconds();
            let updated_text = if ago < 60 {
                "Updated just now".to_string()
            } else {
                format!("Updated {} min ago", ago / 60)
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

        // Actions
        let cmd_tx = self.cmd_tx.clone();
        items.push(
            StandardItem {
                label: "Refresh Now".to_string(),
                activate: Box::new(move |_tray: &mut Self| {
                    let _ = cmd_tx.try_send(PollCommand::RefreshNow);
                }),
                ..Default::default()
            }
            .into(),
        );
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
