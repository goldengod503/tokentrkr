use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

use cosmic::iced::{window::Id, Alignment, Length, Limits, Subscription};
use cosmic::iced_winit::commands::popup::{destroy_popup, get_popup};
use cosmic::prelude::*;
use cosmic::widget::{self, container};
use cosmic::Theme;
use futures_util::SinkExt;
use tracing::{error, info};

use crate::claude::ClaudeProvider;
use crate::config::Config;
use crate::models::UsageSnapshot;
use crate::provider::Provider;

pub fn run() -> anyhow::Result<()> {
    cosmic::applet::run::<TokenTrkrApplet>(())
        .map_err(|e| anyhow::anyhow!("COSMIC applet error: {}", e))
}

pub struct TokenTrkrApplet {
    core: cosmic::Core,
    popup: Option<Id>,
    snapshot: Option<UsageSnapshot>,
    error: Option<String>,
    config: Config,
    provider: Option<Arc<dyn Provider>>,
    refreshing: bool,
    spin_phase: f32,
    refresh_tx: Option<mpsc::Sender<()>>,
}

impl Default for TokenTrkrApplet {
    fn default() -> Self {
        Self {
            core: cosmic::Core::default(),
            popup: None,
            snapshot: None,
            error: None,
            config: Config::default(),
            provider: None,
            refreshing: false,
            spin_phase: 0.0,
            refresh_tx: None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    TogglePopup,
    PopupClosed(Id),
    UsageUpdate(Result<UsageSnapshot, String>),
    RefreshNow,
    OpenDashboard,
    Tick,
    FetchStarted,
    SetRefreshChannel(mpsc::Sender<()>),
}

fn bucket_color(pct: f64) -> cosmic::iced::Color {
    if pct <= 25.0 {
        cosmic::iced::Color::from_rgb8(45, 212, 191)
    } else if pct <= 50.0 {
        cosmic::iced::Color::from_rgb8(245, 158, 11)
    } else if pct <= 75.0 {
        cosmic::iced::Color::from_rgb8(249, 115, 22)
    } else if pct <= 90.0 {
        cosmic::iced::Color::from_rgb8(239, 68, 68)
    } else {
        cosmic::iced::Color::from_rgb8(185, 28, 28)
    }
}

fn format_plan_name(raw: &str) -> String {
    let lower = raw.to_lowercase();
    if lower.contains("max") {
        "Max".to_string()
    } else if lower.contains("pro") {
        "Pro".to_string()
    } else if lower.contains("team") {
        "Team".to_string()
    } else {
        raw.to_string()
    }
}

fn progress_bar_bg(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(cosmic::iced::Color::from_rgba(1.0, 1.0, 1.0, 0.08).into()),
        border: cosmic::iced::Border {
            radius: 4.0.into(),
            ..Default::default()
        },
        ..container::Style::default()
    }
}

fn progress_bar_fill(color: cosmic::iced::Color) -> impl Fn(&Theme) -> container::Style {
    move |_theme: &Theme| container::Style {
        background: Some(color.into()),
        border: cosmic::iced::Border {
            radius: 4.0.into(),
            ..Default::default()
        },
        ..container::Style::default()
    }
}

impl cosmic::Application for TokenTrkrApplet {
    type Executor = cosmic::executor::Default;
    type Flags = ();
    type Message = Message;

    const APP_ID: &'static str = "com.github.goldengod503.TokenTrkr";

    fn core(&self) -> &cosmic::Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut cosmic::Core {
        &mut self.core
    }

    fn init(
        core: cosmic::Core,
        _flags: Self::Flags,
    ) -> (Self, Task<cosmic::Action<Self::Message>>) {
        let config = Config::load().unwrap_or_default();
        let provider: Option<Arc<dyn Provider>> = match ClaudeProvider::new(&config) {
            Ok(p) => Some(Arc::new(p)),
            Err(e) => {
                error!("Failed to create provider: {}", e);
                None
            }
        };

        let app = TokenTrkrApplet {
            core,
            config,
            provider,
            ..Default::default()
        };

        (app, Task::none())
    }

    fn on_close_requested(&self, id: Id) -> Option<Message> {
        Some(Message::PopupClosed(id))
    }

    fn view(&self) -> Element<'_, Self::Message> {
        let pct = self
            .snapshot
            .as_ref()
            .and_then(|s| s.primary.as_ref())
            .map(|w| w.used_percent)
            .unwrap_or(0.0);

        let color = bucket_color(pct);

        // During refresh: show a spinning arc; otherwise solid dot
        let refreshing = self.refreshing;
        let spin = self.spin_phase;

        let dot = widget::container(widget::horizontal_space())
            .width(12)
            .height(12)
            .style(move |_theme: &Theme| {
                if refreshing {
                    // Spinning effect: partial arc via gradient-like opacity shift
                    let alpha = 0.3 + 0.7 * ((spin.sin() + 1.0) / 2.0);
                    container::Style {
                        background: Some(
                            cosmic::iced::Color::from_rgba(color.r, color.g, color.b, alpha)
                                .into(),
                        ),
                        border: cosmic::iced::Border {
                            radius: 6.0.into(),
                            ..Default::default()
                        },
                        ..container::Style::default()
                    }
                } else {
                    container::Style {
                        background: Some(color.into()),
                        border: cosmic::iced::Border {
                            radius: 6.0.into(),
                            ..Default::default()
                        },
                        ..container::Style::default()
                    }
                }
            });

        let label_text = if self.refreshing {
            // Spinning braille characters for a nice text spinner
            const SPINNER: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
            let idx = ((self.spin_phase / std::f32::consts::TAU * SPINNER.len() as f32) as usize)
                % SPINNER.len();
            format!("{}", SPINNER[idx])
        } else if self.error.is_some() {
            "ERR".to_string()
        } else if self.snapshot.is_some() {
            format!("{:.0}%", pct)
        } else {
            "...".to_string()
        };

        let label = widget::text(label_text).size(14.0);

        let content = widget::container(
            widget::row()
                .push(dot)
                .push(label)
                .spacing(6)
                .align_y(Alignment::Center),
        )
        .padding([4, 8]);

        let btn = widget::button::custom(self.core.applet.autosize_window(content))
            .on_press(Message::TogglePopup)
            .class(cosmic::theme::Button::AppletIcon);

        btn.into()
    }

    fn view_window(&self, _id: Id) -> Element<'_, Self::Message> {
        let mut col = widget::column().spacing(8).padding(12);

        // Title
        let title = if let Some(ref snap) = self.snapshot {
            if let Some(ref id) = snap.identity {
                if let Some(ref plan) = id.plan {
                    format!("TokenTrkr — {}", format_plan_name(plan))
                } else {
                    "TokenTrkr".to_string()
                }
            } else {
                "TokenTrkr".to_string()
            }
        } else {
            "TokenTrkr".to_string()
        };
        col = col.push(widget::text::heading(title));
        col = col.push(widget::divider::horizontal::default());

        if let Some(ref snapshot) = self.snapshot {
            // Rate windows
            for w in [&snapshot.primary, &snapshot.secondary, &snapshot.tertiary]
                .into_iter()
                .flatten()
            {
                let pct = w.used_percent;
                let color = bucket_color(pct);
                let bar_width = (240.0 * (pct / 100.0).min(1.0)) as u16;

                let progress = widget::container(
                    widget::container(widget::horizontal_space())
                        .width(Length::Fixed(f32::from(bar_width)))
                        .height(6)
                        .style(progress_bar_fill(color)),
                )
                .width(240)
                .height(6)
                .style(progress_bar_bg);

                col = col
                    .push(
                        widget::row()
                            .push(widget::text(&w.label).width(Length::Fill))
                            .push(widget::text(format!("{:.0}%", pct)))
                            .align_y(Alignment::Center),
                    )
                    .push(progress)
                    .push(widget::text(w.format_reset_time()).size(12.0));
            }

            // Extra usage
            if let Some(ref extra) = snapshot.extra_usage {
                if extra.is_enabled && extra.monthly_limit > 0.0 {
                    let pct = (extra.used_credits / extra.monthly_limit * 100.0).min(100.0);
                    let bar_width = (240.0 * (pct / 100.0)) as u16;

                    col = col
                        .push(widget::divider::horizontal::default())
                        .push(
                            widget::row()
                                .push(widget::text("Extra Usage").width(Length::Fill))
                                .push(widget::text(format!(
                                    "${:.2} / ${:.2}",
                                    extra.used_credits, extra.monthly_limit
                                ))),
                        )
                        .push(
                            widget::container(
                                widget::container(widget::horizontal_space())
                                    .width(Length::Fixed(f32::from(bar_width)))
                                    .height(6)
                                    .style(progress_bar_fill(bucket_color(pct))),
                            )
                            .width(240)
                            .height(6)
                            .style(progress_bar_bg),
                        );
                }
            }

            // Updated time
            col = col.push(widget::divider::horizontal::default());
            let ago = chrono::Utc::now()
                .signed_duration_since(snapshot.updated_at)
                .num_seconds();
            let updated_text = if ago < 60 {
                "Updated just now".to_string()
            } else {
                format!("Updated {} min ago", ago / 60)
            };
            col = col.push(widget::text(updated_text).size(12.0));
        } else if let Some(ref e) = self.error {
            col = col.push(widget::text(format!("Error: {}", e)));
        } else {
            col = col.push(widget::text("Loading..."));
        }

        // Actions
        col = col.push(widget::divider::horizontal::default());
        col = col.push(
            widget::row()
                .push(
                    widget::button::standard("Refresh")
                        .on_press(Message::RefreshNow),
                )
                .push(
                    widget::button::standard("Dashboard")
                        .on_press(Message::OpenDashboard),
                )
                .spacing(8),
        );

        self.core.applet.popup_container(col).into()
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        let poll_secs = self.config.general.poll_interval_minutes * 60;
        let provider = self.provider.clone();

        struct UsagePoll;
        let usage_sub = Subscription::run_with_id(
            std::any::TypeId::of::<UsagePoll>(),
            cosmic::iced::stream::channel(1, move |mut channel| async move {
                let Some(provider) = provider else {
                    _ = channel
                        .send(Message::UsageUpdate(Err(
                            "No provider configured".to_string(),
                        )))
                        .await;
                    loop {
                        tokio::time::sleep(Duration::from_secs(86400)).await;
                    }
                };

                // Create the refresh channel and send the tx back
                let (tx, mut rx) = mpsc::channel::<()>(4);
                _ = channel.send(Message::SetRefreshChannel(tx)).await;

                loop {
                    _ = channel.send(Message::FetchStarted).await;
                    let result = match provider.fetch_usage().await {
                        Ok(snap) => Ok(snap),
                        Err(e) => Err(format!("{:#}", e)),
                    };
                    _ = channel.send(Message::UsageUpdate(result)).await;

                    // Wait for either the poll interval or a manual refresh signal
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_secs(poll_secs)) => {}
                        _ = rx.recv() => {}
                    }
                }
            }),
        );

        let mut subs = vec![usage_sub];

        // Spin animation tick — only active during refresh
        if self.refreshing {
            struct SpinTick;
            subs.push(Subscription::run_with_id(
                std::any::TypeId::of::<SpinTick>(),
                cosmic::iced::stream::channel(1, move |mut channel| async move {
                    loop {
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        _ = channel.send(Message::Tick).await;
                    }
                }),
            ));
        }

        Subscription::batch(subs)
    }

    fn update(&mut self, message: Self::Message) -> Task<cosmic::Action<Self::Message>> {
        match message {
            Message::UsageUpdate(Ok(snapshot)) => {
                info!(
                    "Usage updated: session={:.0}%",
                    snapshot
                        .primary
                        .as_ref()
                        .map(|w| w.used_percent)
                        .unwrap_or(0.0)
                );
                self.snapshot = Some(snapshot);
                self.error = None;
                self.refreshing = false;
            }
            Message::UsageUpdate(Err(e)) => {
                error!("Usage fetch failed: {}", e);
                self.error = Some(e);
                self.refreshing = false;
            }
            Message::FetchStarted => {
                self.refreshing = true;
                self.spin_phase = 0.0;
            }
            Message::Tick => {
                // Advance spin animation — ~1 second full rotation
                self.spin_phase += std::f32::consts::TAU / 20.0; // 20 ticks @ 20fps = 1s
                if self.spin_phase > std::f32::consts::TAU {
                    self.spin_phase -= std::f32::consts::TAU;
                }
            }
            Message::SetRefreshChannel(tx) => {
                self.refresh_tx = Some(tx);
            }
            Message::RefreshNow => {
                info!("Manual refresh requested");
                if let Some(ref tx) = self.refresh_tx {
                    let _ = tx.try_send(());
                }
            }
            Message::OpenDashboard => {
                let _ = std::process::Command::new("xdg-open")
                    .arg("https://claude.ai/settings/usage")
                    .spawn();
            }
            Message::TogglePopup => {
                return if let Some(p) = self.popup.take() {
                    destroy_popup(p)
                } else {
                    let new_id = Id::unique();
                    self.popup.replace(new_id);
                    let mut popup_settings = self.core.applet.get_popup_settings(
                        self.core.main_window_id().unwrap(),
                        new_id,
                        None,
                        None,
                        None,
                    );
                    popup_settings.positioner.size_limits = Limits::NONE
                        .max_width(360.0)
                        .min_width(280.0)
                        .min_height(100.0)
                        .max_height(500.0);
                    get_popup(popup_settings)
                };
            }
            Message::PopupClosed(id) => {
                if self.popup.as_ref() == Some(&id) {
                    self.popup = None;
                }
            }
        }
        Task::none()
    }

    fn style(&self) -> Option<cosmic::iced_runtime::Appearance> {
        Some(cosmic::applet::style())
    }
}
