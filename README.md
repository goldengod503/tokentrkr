# TokenTrkr

System tray app that tracks your Claude token usage and displays it in the Pop!_OS/COSMIC top bar.

![Rust](https://img.shields.io/badge/rust-stable-orange) ![Platform](https://img.shields.io/badge/platform-linux-blue)

## What it does

TokenTrkr reads your Claude OAuth credentials, polls the usage API, and shows your current utilization as a circular gauge icon in the system tray. The icon color shifts from **teal → amber → red** as usage increases.

Click the tray icon to see:

```
TokenTrkr — Max Plan
─────────────────
Session (5h)         67% used
  ▓▓▓▓▓▓▓▓░░░░  Resets in 2h 15m
─────────────────
Weekly (7d)          42% used
  ▓▓▓▓▓░░░░░░░  Resets Mar 8, 12:00 AM
─────────────────
Extra Usage          $12.50 / $100.00
  ▓▓░░░░░░░░░░
─────────────────
Updated just now
─────────────────
Refresh Now
Open Dashboard
Quit
```

## Requirements

- Linux with StatusNotifierItem support (COSMIC, KDE Plasma, GNOME with AppIndicator extension)
- Claude CLI installed and authenticated (`~/.claude/.credentials.json` must exist)
- Rust toolchain (to build)

## Install

```bash
git clone https://github.com/goldengod503/tokentrkr.git
cd tokentrkr
cargo build --release
```

The binary is at `target/release/tokentrkr`.

### Autostart

Copy the desktop file to your autostart directory:

```bash
cp tokentrkr.desktop ~/.config/autostart/
```

Edit the `Exec=` line to point to the full path of the binary.

## Configuration

Config lives at `~/.config/tokentrkr/config.toml` (created with defaults on first run):

```toml
[general]
poll_interval_minutes = 5

[claude]
source = "oauth"
# credentials_path = "~/.claude/.credentials.json"  # override

[display]
show_percent = "used"     # "used" or "remaining"
show_tertiary = true      # show Sonnet usage window
```

## How it works

1. Reads OAuth tokens from `~/.claude/.credentials.json` (written by Claude CLI)
2. Refreshes the access token if expired
3. Calls the Claude usage API to get session (5h), weekly (7d), and Sonnet utilization
4. Renders a dynamic tray icon — circular gauge with a "T" glyph, color-coded by usage level
5. Repeats on a configurable interval

## License

MIT
