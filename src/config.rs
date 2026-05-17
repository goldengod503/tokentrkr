use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_general")]
    pub general: GeneralConfig,
    #[serde(default = "default_claude")]
    pub claude: ClaudeConfig,
    #[serde(default = "default_display")]
    pub display: DisplayConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralConfig {
    #[serde(default = "default_poll_interval")]
    pub poll_interval_minutes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaudeConfig {
    #[serde(default = "default_source")]
    pub source: String,
    pub credentials_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplayConfig {
    #[serde(default = "default_tray_mode")]
    pub tray_mode: String,
}

fn default_general() -> GeneralConfig {
    GeneralConfig {
        poll_interval_minutes: default_poll_interval(),
    }
}

fn default_claude() -> ClaudeConfig {
    ClaudeConfig {
        source: default_source(),
        credentials_path: None,
    }
}

fn default_display() -> DisplayConfig {
    DisplayConfig {
        tray_mode: default_tray_mode(),
    }
}

fn default_poll_interval() -> u64 {
    5
}

fn default_source() -> String {
    "oauth".to_string()
}

fn default_tray_mode() -> String {
    "session".to_string()
}

impl Default for Config {
    fn default() -> Self {
        Config {
            general: default_general(),
            claude: default_claude(),
            display: default_display(),
        }
    }
}

impl Config {
    pub fn config_dir() -> Result<PathBuf> {
        let config_dir = dirs::config_dir()
            .context("Could not determine config directory")?
            .join("tokentrkr");
        Ok(config_dir)
    }

    pub fn config_path() -> Result<PathBuf> {
        Ok(Self::config_dir()?.join("config.toml"))
    }

    pub fn load() -> Result<Self> {
        let path = Self::config_path()?;

        if !path.exists() {
            let config = Config::default();
            config.save()?;
            return Ok(config);
        }

        let contents = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read config from {}", path.display()))?;

        let config: Config = toml::from_str(&contents)
            .with_context(|| format!("Failed to parse config from {}", path.display()))?;

        Ok(config)
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::config_path()?;
        let dir = Self::config_dir()?;

        if !dir.exists() {
            fs::create_dir_all(&dir)
                .with_context(|| format!("Failed to create config dir {}", dir.display()))?;
        }

        let contents = toml::to_string_pretty(self)?;
        fs::write(&path, contents)
            .with_context(|| format!("Failed to write config to {}", path.display()))?;

        Ok(())
    }

    pub fn credentials_path(&self) -> PathBuf {
        if let Some(ref custom) = self.claude.credentials_path {
            expand_tilde(custom, dirs::home_dir().as_deref())
        } else {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("/tmp"))
                .join(".claude")
                .join(".credentials.json")
        }
    }

    pub fn poll_interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.general.poll_interval_minutes * 60)
    }
}

fn expand_tilde(custom: &str, home: Option<&Path>) -> PathBuf {
    if custom == "~" {
        return home.map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from(custom));
    }
    if let Some(rest) = custom.strip_prefix("~/") {
        if let Some(home) = home {
            return home.join(rest);
        }
    }
    // "~user/..." and absolute/relative paths pass through unchanged.
    PathBuf::from(custom)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_config_defaults_tray_mode_to_session_when_missing() {
        let cfg: DisplayConfig = toml::from_str("").expect("parse empty");
        assert_eq!(cfg.tray_mode, "session");
    }

    #[test]
    fn display_config_round_trips_custom_tray_mode() {
        let cfg = DisplayConfig {
            tray_mode: "both".to_string(),
        };

        let serialized = toml::to_string(&cfg).expect("serialize");
        let parsed: DisplayConfig = toml::from_str(&serialized).expect("parse");

        assert_eq!(parsed.tray_mode, "both");
    }

    #[test]
    fn display_config_ignores_legacy_show_percent_and_show_tertiary() {
        let legacy = r#"
            show_percent = "used"
            show_tertiary = true
            tray_mode = "weekly"
        "#;
        let cfg: DisplayConfig = toml::from_str(legacy).expect("parse legacy");
        assert_eq!(cfg.tray_mode, "weekly");
    }

    #[test]
    fn expand_tilde_handles_bare_tilde() {
        let home = PathBuf::from("/home/me");
        assert_eq!(expand_tilde("~", Some(&home)), PathBuf::from("/home/me"));
    }

    #[test]
    fn expand_tilde_replaces_tilde_slash_prefix() {
        let home = PathBuf::from("/home/me");
        assert_eq!(
            expand_tilde("~/.claude/.credentials.json", Some(&home)),
            PathBuf::from("/home/me/.claude/.credentials.json")
        );
    }

    #[test]
    fn expand_tilde_leaves_other_user_path_unchanged() {
        let home = PathBuf::from("/home/me");
        // We do not try to resolve ~other_user/... — best-effort by leaving it
        // literal so a downstream "file not found" error is unambiguous.
        assert_eq!(
            expand_tilde("~other/foo", Some(&home)),
            PathBuf::from("~other/foo")
        );
    }

    #[test]
    fn expand_tilde_passes_absolute_path_through() {
        let home = PathBuf::from("/home/me");
        assert_eq!(
            expand_tilde("/etc/creds.json", Some(&home)),
            PathBuf::from("/etc/creds.json")
        );
    }

    #[test]
    fn expand_tilde_when_home_unavailable_returns_literal() {
        assert_eq!(expand_tilde("~/foo", None), PathBuf::from("~/foo"));
        assert_eq!(expand_tilde("~", None), PathBuf::from("~"));
    }
}
