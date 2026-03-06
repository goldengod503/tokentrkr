use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

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
    #[serde(default = "default_show_percent")]
    pub show_percent: String,
    #[serde(default = "default_true")]
    pub show_tertiary: bool,
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
        show_percent: default_show_percent(),
        show_tertiary: true,
    }
}

fn default_poll_interval() -> u64 {
    5
}

fn default_source() -> String {
    "oauth".to_string()
}

fn default_show_percent() -> String {
    "used".to_string()
}

fn default_true() -> bool {
    true
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
            let expanded = if custom.starts_with('~') {
                if let Some(home) = dirs::home_dir() {
                    home.join(&custom[2..])
                } else {
                    PathBuf::from(custom)
                }
            } else {
                PathBuf::from(custom)
            };
            expanded
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
