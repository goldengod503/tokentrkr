use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::sync::mpsc;

use cosmic::iced::window::Id;
use cosmic::iced::{Alignment, Length, Limits, Rectangle, Subscription};
use cosmic::prelude::*;
use cosmic::surface::action::{app_popup, destroy_popup};
use cosmic::widget::{self, container};
use cosmic::Theme;
use cosmic::iced::futures::SinkExt;
use tracing::{error, info};

static USAGE_HANDLE: OnceLock<tokio::sync::Mutex<Option<crate::usage::UsageHandle>>> =
    OnceLock::new();
static BOOTSTRAP: OnceLock<(Arc<dyn Provider>, Duration)> = OnceLock::new();

use crate::config::Config;
use crate::history::{TimeRange, UsageDataPoint, UsageHistory};
use crate::models::UsageSnapshot;
use crate::provider::Provider;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TrayMode {
    Session,
    Weekly,
    Both,
}

impl TrayMode {
    fn from_config(value: &str) -> Self {
        match value {
            "weekly" => TrayMode::Weekly,
            "both" => TrayMode::Both,
            _ => TrayMode::Session,
        }
    }

    fn as_config(self) -> &'static str {
        match self {
            TrayMode::Session => "session",
            TrayMode::Weekly => "weekly",
            TrayMode::Both => "both",
        }
    }

    fn next(self) -> Self {
        match self {
            TrayMode::Session => TrayMode::Weekly,
            TrayMode::Weekly => TrayMode::Both,
            TrayMode::Both => TrayMode::Session,
        }
    }

    fn tooltip(self) -> &'static str {
        match self {
            TrayMode::Session => "Tray: Session",
            TrayMode::Weekly => "Tray: Weekly",
            TrayMode::Both => "Tray: Both",
        }
    }
}

pub fn run(provider: Arc<dyn Provider>, poll_interval: Duration) -> anyhow::Result<()> {
    // UsageService::spawn must run inside libcosmic's tokio runtime,
    // which isn't created until cosmic::applet::run() is called.
    // Stash inputs in BOOTSTRAP; init() reads and spawns.
    BOOTSTRAP
        .set((provider, poll_interval))
        .map_err(|_| anyhow::anyhow!("BOOTSTRAP already set"))?;

    cosmic::applet::run::<TokenTrkrApplet>(())
        .map_err(|e| anyhow::anyhow!("COSMIC applet error: {}", e))
}

pub struct TokenTrkrApplet {
    core: cosmic::Core,
    popup: Option<Id>,
    snapshot: Option<UsageSnapshot>,
    error: Option<String>,
    config: Config,
    refreshing: bool,
    spin_phase: f32,
    refresh_tx: Option<mpsc::Sender<()>>,
    fetch_done: bool,
    pending_snapshot: Option<Result<UsageSnapshot, String>>,
    history: UsageHistory,
    selected_range: TimeRange,
    latest_fetch_id: u64,
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
            refreshing: false,
            spin_phase: 0.0,
            refresh_tx: None,
            fetch_done: false,
            pending_snapshot: None,
            history: UsageHistory::default(),
            selected_range: TimeRange::Day1,
            latest_fetch_id: 0,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    PopupClosed(Id),
    Usage(crate::usage::UsageEvent),
    RefreshNow,
    OpenDashboard,
    Tick,
    SetRefreshChannel(mpsc::Sender<()>),
    SelectTimeRange(TimeRange),
    CycleTrayMode,
    Surface(cosmic::surface::Action),
    /// libcosmic restarted the usage subscription after the handle was
    /// already consumed. The applet treats this as a terminal failure
    /// for any in-flight fetch — clears spinner state and surfaces an
    /// error — so the user is not stuck staring at a permanent spinner.
    UsageStreamUnavailable,
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

fn build_chart_svg(points: &[UsageDataPoint], range: TimeRange) -> String {
    let w = 280.0_f64;
    let h = 130.0_f64;
    let pl = 30.0_f64; // padding left
    let pr = 8.0_f64;  // padding right
    let pt = 4.0_f64;  // padding top
    let pb = 28.0_f64; // padding bottom (room for x-axis labels + legend)
    let cw = w - pl - pr;
    let ch = h - pt - pb;

    let mut svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}">"#
    );

    // Grid lines + Y labels
    for &pct in &[0u32, 25, 50, 75, 100] {
        let y = pt + ch * (1.0 - pct as f64 / 100.0);
        svg.push_str(&format!(
            r#"<line x1="{pl}" y1="{y:.1}" x2="{x2:.1}" y2="{y:.1}" stroke="rgba(255,255,255,0.1)" stroke-width="1"/>"#,
            x2 = w - pr
        ));
        svg.push_str(&format!(
            r#"<text x="0" y="{:.1}" fill="rgba(255,255,255,0.4)" font-size="9" font-family="sans-serif">{pct}%</text>"#,
            y + 4.0
        ));
    }

    if points.len() < 2 {
        svg.push_str(&format!(
            r#"<text x="{:.1}" y="{:.1}" fill="rgba(255,255,255,0.4)" font-size="11" font-family="sans-serif">No history data yet</text>"#,
            w / 2.0 - 40.0, h / 2.0 + 4.0
        ));
        svg.push_str("</svg>");
        return svg;
    }

    let now = chrono::Utc::now();
    let range_start = now - chrono::Duration::seconds(range.seconds());
    let total_secs = range.seconds() as f64;

    let to_x = |ts: chrono::DateTime<chrono::Utc>| -> f64 {
        let offset = ts.signed_duration_since(range_start).num_seconds() as f64;
        pl + (offset / total_secs).clamp(0.0, 1.0) * cw
    };
    let to_y = |pct: f64| -> f64 {
        pt + ch * (1.0 - (pct / 100.0).clamp(0.0, 1.0))
    };

    // Build polyline points strings
    let pts_5h: String = points
        .iter()
        .map(|p| format!("{:.1},{:.1}", to_x(p.timestamp), to_y(p.pct_5h)))
        .collect::<Vec<_>>()
        .join(" ");
    let pts_7d: String = points
        .iter()
        .map(|p| format!("{:.1},{:.1}", to_x(p.timestamp), to_y(p.pct_7d)))
        .collect::<Vec<_>>()
        .join(" ");

    svg.push_str(&format!(
        "<polyline points=\"{pts_5h}\" fill=\"none\" stroke=\"#3C88FC\" stroke-width=\"1.5\" stroke-linejoin=\"round\"/>"
    ));
    svg.push_str(&format!(
        "<polyline points=\"{pts_7d}\" fill=\"none\" stroke=\"#F59E0B\" stroke-width=\"1.5\" stroke-linejoin=\"round\"/>"
    ));

    // X-axis ticks
    use chrono::{Datelike, Timelike};
    let chart_bottom = pt + ch;
    let tick_y2 = chart_bottom + 4.0;
    let label_y = chart_bottom + 13.0;
    let (tick_interval_secs, tick_count): (i64, usize) = match range {
        TimeRange::Hour1  => (15 * 60, 4),
        TimeRange::Hour6  => (3600, 6),
        TimeRange::Day1   => (4 * 3600, 6),
        TimeRange::Day7   => (86400, 7),
        TimeRange::Day30  => (5 * 86400, 6),
    };
    for i in 0..=tick_count {
        let tick_time = now - chrono::Duration::seconds(range.seconds() - i as i64 * tick_interval_secs);
        let local = tick_time.with_timezone(&chrono::Local);
        let label = match range {
            TimeRange::Hour1 | TimeRange::Hour6 | TimeRange::Day1 =>
                format!("{:02}:{:02}", local.hour(), local.minute()),
            TimeRange::Day7 => {
                const DAYS: [&str; 7] = ["Mon","Tue","Wed","Thu","Fri","Sat","Sun"];
                DAYS[local.weekday().num_days_from_monday() as usize].to_string()
            }
            TimeRange::Day30 => format!("{}/{}", local.month(), local.day()),
        };
        let x = pl + (i as f64 / tick_count as f64) * cw;
        // tick mark
        svg.push_str(&format!(
            r#"<line x1="{x:.1}" y1="{chart_bottom:.1}" x2="{x:.1}" y2="{tick_y2:.1}" stroke="rgba(255,255,255,0.3)" stroke-width="1"/>"#
        ));
        // label — left-align first, right-align last, center others
        let anchor = if i == 0 { "start" } else if i == tick_count { "end" } else { "middle" };
        svg.push_str(&format!(
            r#"<text x="{x:.1}" y="{label_y:.1}" fill="rgba(255,255,255,0.4)" font-size="8" font-family="sans-serif" text-anchor="{anchor}">{label}</text>"#
        ));
    }

    // Legend
    let ly = h - 4.0;
    svg.push_str(&format!(
        "<text x=\"{pl}\" y=\"{ly}\" fill=\"#3C88FC\" font-size=\"9\" font-family=\"sans-serif\">\u{25cf} Session</text>"
    ));
    svg.push_str(&format!(
        "<text x=\"{:.1}\" y=\"{ly}\" fill=\"#F59E0B\" font-size=\"9\" font-family=\"sans-serif\">\u{25cf} Weekly</text>",
        pl + 60.0
    ));

    svg.push_str("</svg>");
    svg
}

impl TokenTrkrApplet {
    /// Translates a UsageEvent into the applet's existing state mutations.
    /// Generation-guards via fetch_id (B3 fix).
    fn handle_event(&mut self, event: crate::usage::UsageEvent) {
        use crate::usage::UsageEvent;
        match event {
            UsageEvent::FetchStarted { fetch_id } => {
                self.latest_fetch_id = fetch_id;
                self.refreshing = true;
                self.spin_phase = 0.0;
                self.fetch_done = false;
                self.pending_snapshot = None;
            }
            UsageEvent::Snapshot { fetch_id, snapshot } => {
                if fetch_id != self.latest_fetch_id {
                    return; // stale — ignore
                }
                if self.refreshing && self.spin_phase < MIN_SPIN_PHASE {
                    self.fetch_done = true;
                    self.pending_snapshot = Some(Ok(snapshot));
                } else {
                    self.apply_usage_result(Ok(snapshot));
                    self.refreshing = false;
                    self.fetch_done = false;
                }
            }
            UsageEvent::TransientError { fetch_id, message, .. }
            | UsageEvent::PermanentError { fetch_id, message } => {
                if fetch_id != self.latest_fetch_id {
                    return;
                }
                if self.refreshing && self.spin_phase < MIN_SPIN_PHASE {
                    self.fetch_done = true;
                    self.pending_snapshot = Some(Err(message));
                } else {
                    self.apply_usage_result(Err(message));
                    self.refreshing = false;
                    self.fetch_done = false;
                }
            }
            UsageEvent::Stalled => {
                // UI was busy; no state change.
            }
        }
    }

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

    fn render_tray_window(
        &self,
        window: Option<&crate::models::RateWindow>,
    ) -> Element<'_, Message> {
        let pct = window.map(|w| w.used_percent).unwrap_or(0.0);
        let color = bucket_color(pct);

        let refreshing = self.refreshing;
        let spin = self.spin_phase;

        let dot = widget::container(widget::Space::new())
            .width(12)
            .height(12)
            .style(move |_theme: &Theme| {
                if refreshing {
                    let alpha = 0.3 + 0.7 * ((spin.sin() + 1.0) / 2.0);
                    container::Style {
                        background: Some(
                            cosmic::iced::Color::from_rgba(color.r, color.g, color.b, alpha).into(),
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

        let label_text: Option<String> = if self.refreshing {
            None
        } else if self.error.is_some() {
            None
        } else if window.is_some() {
            Some(format!("{:.0}%", pct))
        } else {
            Some("...".to_string())
        };

        let mut row = widget::Row::new().push(dot).spacing(6).align_y(Alignment::Center);
        if let Some(text) = label_text {
            row = row.push(widget::text(text).size(14.0));
        }
        row.into()
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
        let history = UsageHistory::load();

        let app = TokenTrkrApplet {
            core,
            config,
            history,
            ..Default::default()
        };

        let (provider, poll_interval) = BOOTSTRAP
            .get()
            .cloned()
            .expect("BOOTSTRAP not initialized — cosmic_app::run must be called first");
        let handle = crate::usage::UsageService::new(provider, poll_interval).spawn();
        let _ = USAGE_HANDLE.set(tokio::sync::Mutex::new(Some(handle)));

        (app, Task::none())
    }

    fn on_close_requested(&self, id: Id) -> Option<Message> {
        Some(Message::PopupClosed(id))
    }

    fn view(&self) -> Element<'_, Self::Message> {
        let mode = TrayMode::from_config(&self.config.display.tray_mode);

        let spinner_char = if self.refreshing {
            const SPINNER: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
            let idx = ((self.spin_phase / std::f32::consts::TAU * SPINNER.len() as f32) as usize)
                % SPINNER.len();
            Some(SPINNER[idx])
        } else {
            None
        };

        let primary = self.snapshot.as_ref().and_then(|s| s.primary.as_ref());
        let secondary = self.snapshot.as_ref().and_then(|s| s.secondary.as_ref());

        let mut row = widget::Row::new().spacing(6).align_y(Alignment::Center);

        if let Some(ch) = spinner_char {
            row = row.push(widget::text(format!("{}", ch)).size(14.0));
        }

        match mode {
            TrayMode::Session => {
                row = row.push(self.render_tray_window(primary));
            }
            TrayMode::Weekly => {
                row = row.push(self.render_tray_window(secondary));
            }
            TrayMode::Both => {
                row = row.push(self.render_tray_window(primary));
                row = row.push(widget::text("|").size(14.0));
                row = row.push(self.render_tray_window(secondary));
            }
        }

        if self.error.is_some() {
            row = row.push(widget::text("ERR").size(14.0));
        }

        let content = widget::container(row).padding([4, 8]);
        let have_popup = self.popup;

        widget::button::custom(self.core.applet.autosize_window(Element::from(content)))
            .class(cosmic::theme::Button::AppletIcon)
            .on_press_with_rectangle(move |offset, bounds| {
                if let Some(id) = have_popup {
                    Message::Surface(destroy_popup(id))
                } else {
                    Message::Surface(app_popup::<TokenTrkrApplet>(
                        move |state: &mut TokenTrkrApplet| {
                            let new_id = Id::unique();
                            state.popup = Some(new_id);
                            let mut popup_settings = state.core.applet.get_popup_settings(
                                state.core.main_window_id().unwrap(),
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
                            popup_settings.positioner.offset = match popup_settings.positioner.offset {
                                (0, y) if y > 0 => (0, y + 8),
                                (0, y) if y < 0 => (0, y - 8),
                                (x, 0) if x > 0 => (x + 8, 0),
                                (x, 0) if x < 0 => (x - 8, 0),
                                other => other,
                            };
                            popup_settings.positioner.anchor_rect = Rectangle {
                                x: (bounds.x - offset.x) as i32,
                                y: (bounds.y - offset.y) as i32,
                                width: bounds.width as i32,
                                height: bounds.height as i32,
                            };
                            popup_settings
                        },
                        Some(Box::new(|state: &TokenTrkrApplet| {
                            state.popup_view().map(cosmic::Action::App)
                        })),
                    ))
                }
            })
            .into()
    }

    fn view_window(&self, _id: Id) -> Element<'_, Self::Message> {
        self.popup_view()
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        struct UsagePoll;
        let usage_sub = Subscription::run_with(
            std::any::TypeId::of::<UsagePoll>(),
            |_| {
                cosmic::iced::stream::channel(8, |mut channel: cosmic::iced::futures::channel::mpsc::Sender<Message>| async move {
                    let handle_opt = USAGE_HANDLE
                        .get()
                        .expect("USAGE_HANDLE not initialized")
                        .lock()
                        .await
                        .take();

                    let Some(mut handle) = handle_opt else {
                        // Already taken (subscription restarted unexpectedly).
                        // Notify the app once so it can clear any in-flight
                        // spinner state, then stay idle. Without this signal
                        // `refreshing` could stay true forever (only Snapshot
                        // or error events clear it, and no events will arrive).
                        _ = channel.send(Message::UsageStreamUnavailable).await;
                        loop {
                            tokio::time::sleep(Duration::from_secs(86400)).await;
                        }
                    };

                    _ = channel
                        .send(Message::SetRefreshChannel(handle.refresh.clone()))
                        .await;

                    while let Some(event) = handle.events.recv().await {
                        _ = channel.send(Message::Usage(event)).await;
                    }
                })
            },
        );

        let mut subs = vec![usage_sub];

        if self.refreshing {
            struct SpinTick;
            subs.push(Subscription::run_with(
                std::any::TypeId::of::<SpinTick>(),
                |_| {
                    cosmic::iced::stream::channel(1, |mut channel: cosmic::iced::futures::channel::mpsc::Sender<Message>| async move {
                        loop {
                            tokio::time::sleep(Duration::from_millis(50)).await;
                            _ = channel.send(Message::Tick).await;
                        }
                    })
                },
            ));
        }

        Subscription::batch(subs)
    }

    fn update(&mut self, message: Self::Message) -> Task<cosmic::Action<Self::Message>> {
        match message {
            Message::Usage(event) => {
                self.handle_event(event);
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
            Message::CycleTrayMode => {
                let current = TrayMode::from_config(&self.config.display.tray_mode);
                let next = current.next();
                self.config.display.tray_mode = next.as_config().to_string();
                if let Err(e) = self.config.save() {
                    tracing::warn!("Failed to persist tray_mode change: {}", e);
                }
            }
            Message::PopupClosed(id) => {
                if self.popup.as_ref() == Some(&id) {
                    self.popup = None;
                }
            }
            Message::Surface(a) => {
                return cosmic::task::message(cosmic::Action::Cosmic(
                    cosmic::app::Action::Surface(a),
                ));
            }
            Message::UsageStreamUnavailable => {
                // Treat as terminal failure for the current cycle: drop the
                // spinner so SpinTick stops firing, discard any pending
                // snapshot, and surface a visible error. Recovery requires
                // restarting the applet — there is no path to re-acquire
                // the handle from inside the running subscription.
                self.refreshing = false;
                self.fetch_done = false;
                self.pending_snapshot = None;
                // The service loop is gone; a retained sender would make the
                // Refresh button a permanent silent no-op. Dropping it lets
                // RefreshNow's Some-guard reflect reality.
                self.refresh_tx = None;
                self.error = Some("Usage stream unavailable (restart applet)".into());
            }
        }
        Task::none()
    }

    fn style(&self) -> Option<cosmic::iced::theme::Style> {
        Some(cosmic::applet::style())
    }
}

impl TokenTrkrApplet {
    fn popup_view(&self) -> Element<'_, Message> {
        let mut col = widget::Column::new().spacing(8).padding(12);

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
                    widget::container(widget::Space::new())
                        .width(Length::Fixed(f32::from(bar_width)))
                        .height(6)
                        .style(progress_bar_fill(color)),
                )
                .width(240)
                .height(6)
                .style(progress_bar_bg);

                col = col
                    .push(
                        widget::Row::new()
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
                        widget::container(widget::Space::new())
                            .width(Length::Fixed(f32::from(bar_width)))
                            .height(4)
                            .style(progress_bar_fill(color)),
                    )
                    .width(240)
                    .height(4)
                    .style(progress_bar_bg);

                    col = col
                        .push(
                            widget::Row::new()
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
                            widget::Row::new()
                                .push(widget::text("Extra Usage").width(Length::Fill))
                                .push(widget::text(format!(
                                    "${:.2} / ${:.2}",
                                    extra.used_credits, extra.monthly_limit
                                ))),
                        )
                        .push(
                            widget::container(
                                widget::container(widget::Space::new())
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
            let mut range_row = widget::Row::new().spacing(4);
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

            // SVG chart — rebuilds fresh every view call so range/data changes always render
            let points = self.history.points_for_range(self.selected_range);
            let svg_data = build_chart_svg(&points, self.selected_range);
            let svg_handle = widget::svg::Handle::from_memory(svg_data.into_bytes());
            col = col.push(
                widget::Svg::new(svg_handle)
                    .width(Length::Fixed(280.0))
                    .height(Length::Fixed(130.0)),
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
        let mode = TrayMode::from_config(&self.config.display.tray_mode);
        let toggle_icon = widget::icon::from_name("view-list-symbolic").size(16);
        col = col.push(
            widget::Row::new()
                .push(
                    widget::button::standard("Refresh")
                        .on_press(Message::RefreshNow),
                )
                .push(
                    widget::button::standard("Dashboard")
                        .on_press(Message::OpenDashboard),
                )
                .push(widget::Space::new().width(Length::Fill))
                .push(
                    widget::button::icon(toggle_icon)
                        .on_press(Message::CycleTrayMode)
                        .tooltip(mode.tooltip()),
                )
                .spacing(8)
                .width(Length::Fill),
        );

        self.core.applet.popup_container(col).into()
    }
}
