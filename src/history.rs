use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
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
    // Captured once at load(); None (Default) makes persistence a no-op,
    // which keeps unit tests off the real history file.
    #[serde(skip)]
    path: Option<PathBuf>,
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
        let mut loaded = if !path.exists() {
            Self::default()
        } else {
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
        };
        loaded.path = Some(path);
        loaded
    }

    pub fn record(&mut self, pct_5h: f64, pct_7d: f64) {
        self.data_points.push(UsageDataPoint {
            timestamp: Utc::now(),
            pct_5h,
            pct_7d,
        });
    }

    /// Prunes and serializes on the caller's thread so write ordering follows
    /// record() ordering, returning the bytes and destination so the caller
    /// can move the blocking disk write (fsync) off the UI thread.
    pub fn serialize_pruned(&mut self) -> Option<(PathBuf, Vec<u8>)> {
        self.prune();
        let path = self.path.clone()?;
        match serde_json::to_string(self) {
            Ok(json) => Some((path, json.into_bytes())),
            Err(e) => {
                error!("Failed to serialize history: {}", e);
                None
            }
        }
    }

    pub fn write_bytes(path: &Path, contents: &[u8]) {
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        if let Err(e) = atomic_write(path, contents) {
            error!("Failed to write history: {}", e);
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

fn atomic_write(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};
    // Writes may now run concurrently on blocking threads; a per-call tmp
    // name keeps one write from truncating another's in-flight tmp file.
    static TMP_SEQ: AtomicU64 = AtomicU64::new(0);
    let tmp = path.with_extension(format!(
        "json.tmp{}",
        TMP_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp)?;
        f.write_all(contents)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_replaces_existing_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("history.json");
        fs::write(&path, b"stale").expect("seed");

        atomic_write(&path, b"fresh").expect("write");

        assert_eq!(fs::read(&path).expect("read"), b"fresh");
    }

    #[test]
    fn atomic_write_leaves_no_tmp_artifact_on_success() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("history.json");
        atomic_write(&path, b"{}").expect("write");

        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .expect("read_dir")
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name() != "history.json")
            .collect();
        assert!(leftovers.is_empty(), "tmp artifact should be renamed away");
    }

    #[test]
    fn serialize_pruned_then_write_bytes_round_trips_through_the_injected_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("history.json");
        let mut h = UsageHistory {
            data_points: vec![],
            path: Some(path.clone()),
        };
        h.record(13.0, 27.5);
        h.record(0.0, 100.0);

        let (dest, bytes) = h.serialize_pruned().expect("serialize");
        UsageHistory::write_bytes(&dest, &bytes);

        let contents = fs::read_to_string(&path).expect("read");
        let back: UsageHistory = serde_json::from_str(&contents).expect("de");
        assert_eq!(back.data_points.len(), 2);
        assert_eq!(back.data_points[0].pct_5h, 13.0);
        assert_eq!(back.data_points[1].pct_7d, 100.0);
    }

    #[test]
    fn serialize_pruned_without_a_backing_path_returns_none() {
        let mut h = UsageHistory::default();
        h.record(1.0, 2.0);

        assert!(h.serialize_pruned().is_none());
    }
}
