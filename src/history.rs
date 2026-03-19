use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use tracing::{debug, error};

use crate::config::Config;

const RETENTION_DAYS: i64 = 30;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageDataPoint {
    pub timestamp: DateTime<Utc>,
    pub pct_5h: f64,
    pub pct_7d: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UsageHistory {
    pub data_points: Vec<UsageDataPoint>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeRange {
    Hour1,
    Hour6,
    Day1,
    Day7,
    Day30,
}

impl TimeRange {
    pub const ALL: &[TimeRange] = &[
        TimeRange::Hour1,
        TimeRange::Hour6,
        TimeRange::Day1,
        TimeRange::Day7,
        TimeRange::Day30,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            TimeRange::Hour1 => "1h",
            TimeRange::Hour6 => "6h",
            TimeRange::Day1 => "1d",
            TimeRange::Day7 => "7d",
            TimeRange::Day30 => "30d",
        }
    }

    pub fn seconds(&self) -> i64 {
        match self {
            TimeRange::Hour1 => 3600,
            TimeRange::Hour6 => 6 * 3600,
            TimeRange::Day1 => 86400,
            TimeRange::Day7 => 7 * 86400,
            TimeRange::Day30 => 30 * 86400,
        }
    }

}

impl UsageHistory {
    fn history_path() -> Option<PathBuf> {
        Config::config_dir().ok().map(|d| d.join("history.json"))
    }

    pub fn load() -> Self {
        let Some(path) = Self::history_path() else {
            return Self::default();
        };
        if !path.exists() {
            return Self::default();
        }
        match fs::read_to_string(&path) {
            Ok(contents) => match serde_json::from_str::<UsageHistory>(&contents) {
                Ok(mut h) => {
                    h.prune();
                    debug!("Loaded {} history points", h.data_points.len());
                    h
                }
                Err(e) => {
                    error!("Corrupt history file, starting fresh: {}", e);
                    Self::default()
                }
            },
            Err(e) => {
                error!("Failed to read history: {}", e);
                Self::default()
            }
        }
    }

    pub fn record(&mut self, pct_5h: f64, pct_7d: f64) {
        self.data_points.push(UsageDataPoint {
            timestamp: Utc::now(),
            pct_5h,
            pct_7d,
        });
    }

    pub fn save(&mut self) {
        self.prune();
        let Some(path) = Self::history_path() else {
            return;
        };
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        match serde_json::to_string(&self) {
            Ok(json) => {
                if let Err(e) = fs::write(&path, json) {
                    error!("Failed to write history: {}", e);
                }
            }
            Err(e) => error!("Failed to serialize history: {}", e),
        }
    }

    fn prune(&mut self) {
        let cutoff = Utc::now() - chrono::Duration::days(RETENTION_DAYS);
        self.data_points.retain(|p| p.timestamp >= cutoff);
    }

    pub fn points_for_range(&self, range: TimeRange) -> Vec<UsageDataPoint> {
        let now = Utc::now();
        let start = now - chrono::Duration::seconds(range.seconds());
        self.data_points
            .iter()
            .filter(|p| p.timestamp >= start)
            .cloned()
            .collect()
    }
}
