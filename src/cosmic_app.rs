use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

use cosmic::iced::widget::canvas::{self, Path, Stroke};
use cosmic::iced::{mouse, window::Id, Alignment, Length, Limits, Subscription};
use cosmic::iced_winit::commands::popup::{destroy_popup, get_popup};
use cosmic::prelude::*;
use cosmic::widget::{self, container};
use cosmic::Theme;
use futures_util::SinkExt;
use tracing::{error, info};

use crate::claude::ClaudeProvider;
use crate::config::Config;
use crate::history::{TimeRange, UsageDataPoint, UsageHistory};
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
    fetch_done: bool,
    pending_snapshot: Option<Result<UsageSnapshot, String>>,
    history: UsageHistory,
    selected_range: TimeRange,
}

const MIN_SPIN_CYCLES: f32 = 3.0;
const MIN_SPIN_PHASE: f32 = MIN_SPIN_CYCLES * std::f32::consts::TAU;

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
            fetch_done: false,
            pending_snapshot: None,
            history: UsageHistory::default(),
            selected_range: TimeRange::Day1,
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
    SelectTimeRange(TimeRange),
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

// Chart colors
const COLOR_5H: cosmic::iced::Color = cosmic::iced::Color {
    r: 0.235,
    g: 0.533,
    b: 0.988,
    a: 1.0,
};
const COLOR_7D: cosmic::iced::Color = cosmic::iced::Color {
    r: 0.961,
    g: 0.620,
    b: 0.043,
    a: 1.0,
};

struct UsageChart {
    points: Vec<UsageDataPoint>,
    range: TimeRange,
}

impl canvas::Program<Message, Theme> for UsageChart {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &cosmic::Renderer,
        _theme: &Theme,
        bounds: cosmic::iced::Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry<cosmic::Renderer>> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let w = bounds.width;
        let h = bounds.height;
        let padding_left = 30.0_f32;
        let padding_right = 8.0_f32;
        let padding_top = 4.0_f32;
        let padding_bottom = 16.0_f32;
        let chart_w = w - padding_left - padding_right;
        let chart_h = h - padding_top - padding_bottom;

        // Grid lines and Y-axis labels
        let grid_color = cosmic::iced::Color::from_rgba(1.0, 1.0, 1.0, 0.1);
        let label_color = cosmic::iced::Color::from_rgba(1.0, 1.0, 1.0, 0.4);
        for &pct in &[0, 25, 50, 75, 100] {
            let y = padding_top + chart_h * (1.0 - pct as f32 / 100.0);
            frame.stroke(
                &Path::line(
                    cosmic::iced::Point::new(padding_left, y),
                    cosmic::iced::Point::new(w - padding_right, y),
                ),
                Stroke::default()
                    .with_color(grid_color)
                    .with_width(1.0),
            );
            frame.fill_text(canvas::Text {
                content: format!("{}%", pct),
                position: cosmic::iced::Point::new(0.0, y - 5.0),
                color: label_color,
                size: cosmic::iced::Pixels(9.0),
                ..canvas::Text::default()
            });
        }

        if self.points.len() < 2 {
            // "No data" message
            frame.fill_text(canvas::Text {
                content: "No history data yet".to_string(),
                position: cosmic::iced::Point::new(w / 2.0 - 40.0, h / 2.0 - 5.0),
                color: label_color,
                size: cosmic::iced::Pixels(11.0),
                ..canvas::Text::default()
            });
            return vec![frame.into_geometry()];
        }

        let now = chrono::Utc::now();
        let range_start = now - chrono::Duration::seconds(self.range.seconds());
        let total_secs = self.range.seconds() as f32;

        let to_x = |ts: chrono::DateTime<chrono::Utc>| -> f32 {
            let offset = ts.signed_duration_since(range_start).num_seconds() as f32;
            padding_left + (offset / total_secs).clamp(0.0, 1.0) * chart_w
        };
        let to_y = |pct: f64| -> f32 {
            padding_top + chart_h * (1.0 - (pct / 100.0).clamp(0.0, 1.0) as f32)
        };

        // Draw 5h line
        let path_5h = Path::new(|builder| {
            for (i, p) in self.points.iter().enumerate() {
                let x = to_x(p.timestamp);
                let y = to_y(p.pct_5h);
                if i == 0 {
                    builder.move_to(cosmic::iced::Point::new(x, y));
                } else {
                    builder.line_to(cosmic::iced::Point::new(x, y));
                }
            }
        });
        frame.stroke(
            &path_5h,
            Stroke::default().with_color(COLOR_5H).with_width(1.5),
        );

        // Draw 7d line
        let path_7d = Path::new(|builder| {
            for (i, p) in self.points.iter().enumerate() {
                let x = to_x(p.timestamp);
                let y = to_y(p.pct_7d);
                if i == 0 {
                    builder.move_to(cosmic::iced::Point::new(x, y));
                } else {
                    builder.line_to(cosmic::iced::Point::new(x, y));
                }
            }
        });
        frame.stroke(
            &path_7d,
            Stroke::default().with_color(COLOR_7D).with_width(1.5),
        );

        // Legend
        let legend_y = h - 4.0;
        frame.fill_text(canvas::Text {
            content: "● 5h".to_string(),
            position: cosmic::iced::Point::new(padding_left, legend_y),
            color: COLOR_5H,
            size: cosmic::iced::Pixels(9.0),
            ..canvas::Text::default()
        });
        frame.fill_text(canvas::Text {
            content: "● 7d".to_string(),
            position: cosmic::iced::Point::new(padding_left + 35.0, legend_y),
            color: COLOR_7D,
            size: cosmic::iced::Pixels(9.0),
            ..canvas::Text::default()
        });

        vec![frame.into_geometry()]
    }
}

impl TokenTrkrApplet {
    fn apply_usage_result(&mut self, result: Result<UsageSnapshot, String>) {
        match result {
            Ok(snapshot) => {
                let pct_5h = snapshot
                    .primary
                    .as_ref()
                    .map(|w| w.used_percent)
                    .unwrap_or(0.0);
                let pct_7d = snapshot
                    .secondary
                    .as_ref()
                    .map(|w| w.used_percent)
                    .unwrap_or(0.0);
                info!("Usage updated: session={:.0}%", pct_5h);

                self.history.record(pct_5h, pct_7d);
                self.history.save();

                self.snapshot = Some(snapshot);
                self.error = None;
            }
            Err(e) => {
                error!("Usage fetch failed: {}", e);
                self.error = Some(e);
            }
        }
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

        let history = UsageHistory::load();

        let app = TokenTrkrApplet {
            core,
            config,
            provider,
            history,
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

        let refreshing = self.refreshing;
        let spin = self.spin_phase;

        let dot = widget::container(widget::horizontal_space())
            .width(12)
            .height(12)
            .style(move |_theme: &Theme| {
                if refreshing {
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

            // Per-model breakdown
            if !snapshot.model_windows.is_empty() {
                col = col.push(widget::divider::horizontal::default());
                col = col.push(widget::text("Per-Model Usage").size(13.0));

                for w in &snapshot.model_windows {
                    let pct = w.used_percent;
                    let color = bucket_color(pct);
                    let bar_width = (240.0 * (pct / 100.0).min(1.0)) as u16;

                    let progress = widget::container(
                        widget::container(widget::horizontal_space())
                            .width(Length::Fixed(f32::from(bar_width)))
                            .height(4)
                            .style(progress_bar_fill(color)),
                    )
                    .width(240)
                    .height(4)
                    .style(progress_bar_bg);

                    col = col
                        .push(
                            widget::row()
                                .push(widget::text(&w.label).size(12.0).width(Length::Fill))
                                .push(widget::text(format!("{:.0}%", pct)).size(12.0))
                                .align_y(Alignment::Center),
                        )
                        .push(progress);
                }
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

            // Usage chart
            col = col.push(widget::divider::horizontal::default());

            // Time range picker
            let mut range_row = widget::row().spacing(4);
            for &range in TimeRange::ALL {
                let is_selected = range == self.selected_range;
                let btn = if is_selected {
                    widget::button::suggested(range.label())
                } else {
                    widget::button::standard(range.label())
                };
                range_row = range_row.push(btn.on_press(Message::SelectTimeRange(range)));
            }
            col = col.push(range_row);

            // Canvas chart
            let points = self.history.points_for_range(self.selected_range);
            let chart = UsageChart {
                points,
                range: self.selected_range,
            };
            col = col.push(
                widget::Canvas::new(chart)
                    .width(Length::Fixed(280.0))
                    .height(Length::Fixed(120.0)),
            );

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

                let (tx, mut rx) = mpsc::channel::<()>(4);
                _ = channel.send(Message::SetRefreshChannel(tx)).await;

                loop {
                    _ = channel.send(Message::FetchStarted).await;
                    let result = match provider.fetch_usage().await {
                        Ok(snap) => Ok(snap),
                        Err(e) => Err(format!("{:#}", e)),
                    };
                    _ = channel.send(Message::UsageUpdate(result)).await;

                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_secs(poll_secs)) => {}
                        _ = rx.recv() => {}
                    }
                }
            }),
        );

        let mut subs = vec![usage_sub];

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
            Message::UsageUpdate(result) => {
                if self.refreshing && self.spin_phase < MIN_SPIN_PHASE {
                    self.fetch_done = true;
                    self.pending_snapshot = Some(result);
                } else {
                    self.apply_usage_result(result);
                    self.refreshing = false;
                    self.fetch_done = false;
                }
            }
            Message::FetchStarted => {
                self.refreshing = true;
                self.spin_phase = 0.0;
            }
            Message::Tick => {
                let prev = self.spin_phase;
                self.spin_phase += std::f32::consts::TAU / 20.0;

                if self.fetch_done && self.spin_phase >= MIN_SPIN_PHASE {
                    let prev_cycle = (prev / std::f32::consts::TAU) as u32;
                    let curr_cycle = (self.spin_phase / std::f32::consts::TAU) as u32;
                    if curr_cycle > prev_cycle {
                        if let Some(result) = self.pending_snapshot.take() {
                            self.apply_usage_result(result);
                        }
                        self.refreshing = false;
                        self.fetch_done = false;
                    }
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
            Message::SelectTimeRange(range) => {
                self.selected_range = range;
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
                        .max_height(600.0);
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
